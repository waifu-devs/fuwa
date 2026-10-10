//! Keeps one instance in the store in step with its server: loads who you
//! are and which servers you're in, follows all of them over one event
//! stream, and reconnects with backoff, resuming from the last event seen so
//! nothing is missed or applied twice. A port of `web/src/fuwa/sync.ts`.
//!
//! An instance with live connections (docs/live.md) sends that stream, and
//! the direct-message, friends and presence feeds, over one connection
//! (`follow_live`); each feed's responses go to the same code as its own
//! stream's. Messages out of focus then come as heads (`live.rs`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tonic::Code;

use crate::core::api::{Api, Problem};
use crate::core::config::Prefs;
use crate::core::dms::{self, DmEngine, DmStatus};
use crate::core::live;
use crate::core::notifications;
use crate::core::reports;
use crate::core::store::{self, Connection, InstanceState, Outcome};
use crate::core::threads;
use crate::core::{Core, Notice};
use crate::pb;
use crate::rpc;

/// The server sends a heartbeat every 25 seconds; this long without anything means the connection is gone.
const SILENCE: Duration = Duration::from_secs(70);
/// How often the instance's public details (and its announcement) are read again.
const NODE_REFRESH: Duration = Duration::from_secs(60);
/// Events older than this came in while catching up after a reconnect; they stay quiet.
const FRESH_MS: i64 = 30_000;

/// Retry quickly at first, then every 20 seconds at most, a little at random.
struct Backoff(Duration);

impl Backoff {
    fn new() -> Self {
        Self(Duration::from_millis(400))
    }

    async fn wait(&mut self) {
        let mut b = [0u8; 1];
        let _ = getrandom::fill(&mut b);
        let jitter = 0.75 + f64::from(b[0]) / 510.0;
        tokio::time::sleep(self.0.mul_f64(jitter)).await;
        self.0 = (self.0 * 2).min(Duration::from_secs(20));
    }
}

/// Runs a call until it works, showing the instance as offline while it doesn't.
async fn retrying<T, F, Fut>(core: &Core, key: &str, mut f: F) -> Result<T, Problem>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Problem>>,
{
    let mut backoff = Backoff::new();
    loop {
        match f().await {
            Ok(value) => return Ok(value),
            Err(err) if err.retryable() => {
                core.shared.instance(key, |i| {
                    i.connection = Connection::Offline;
                    i.problem = Some(err.message.clone());
                });
                backoff.wait().await;
            }
            Err(err) => return Err(err),
        }
    }
}

pub(super) async fn run(
    core: Arc<Core>,
    key: String,
    api: Api,
    followed: watch::Receiver<Vec<String>>,
    dms: Arc<Mutex<Option<Arc<DmEngine>>>>,
) {
    let result = follow_instance(&core, &key, &api, followed, &dms).await;
    if let Some(engine) = dms.lock().take() {
        engine.stop();
    }
    if let Err(err) = result {
        if err.signed_out() {
            core.signed_out(&key);
        } else {
            core.shared.instance(&key, |i| {
                i.connection = Connection::Offline;
                i.problem = Some(err.message.clone());
            });
        }
    }
}

async fn follow_instance(
    core: &Arc<Core>,
    key: &str,
    api: &Api,
    followed: watch::Receiver<Vec<String>>,
    dms: &Arc<Mutex<Option<Arc<DmEngine>>>>,
) -> Result<(), Problem> {
    let node = retrying(core, key, || rpc!(api.node(), get_node(pb::GetNodeRequest {}))).await?;
    core.shared.instance(key, |i| {
        i.node = node.node;
        i.problem = None;
    });
    let Some(token) = api.token() else {
        core.shared.instance(key, |i| i.connection = Connection::SignedOut);
        return Ok(());
    };
    // Where the instance has them, one live connection carries every feed
    // (docs/live.md); elsewhere each has its own stream, as before.
    let live =
        core.live_connections() && core.shared.read(|s| s.instance(key).is_some_and(|i| i.has("live-connection")));

    let me = retrying(core, key, || rpc!(api.auth(), get_me(pb::GetMeRequest {}))).await?;
    core.shared.instance(key, |i| {
        if let Some(user) = &me.user {
            i.users.insert(user.id.clone(), user.clone());
        }
        i.me = me.user.clone();
        i.admin = me.admin;
        i.recent_sign_ins = me.recent_sign_in_methods.clone();
    });

    // Whose session this is, kept to switch back to (accounts.rs).
    if let Some(user) = &me.user {
        core.remember_account(key, user, &token);
    }
    // Encrypted direct messages run alongside, for as long as this does.
    if let Some(user) = me.user.clone() {
        start_dms(core, key, api, user, &token, dms, live);
    }

    // Notification settings follow the account; an older instance without them just has none.
    core.refresh_notifications(key).await;
    core.refresh_presence(key).await;
    // So is how you arranged your servers on the rail.
    core.refresh_rail(key).await;
    // The instance's profile effects and decorations; an older instance has none.
    core.refresh_instance_items(key).await;

    let servers = retrying(core, key, || rpc!(api.servers(), list_servers(pb::ListServersRequest {}))).await?.servers;
    let ids: Vec<String> = servers.iter().map(|s| s.id.clone()).collect();
    core.shared.instance(key, |i| {
        for server in servers {
            store::add_server(i, server);
        }
        i.connection = if ids.is_empty() { Connection::Live } else { Connection::Connecting };
    });
    if let Some(engine) = core.engines.lock().get(key) {
        engine.followed.send_replace(ids);
    }

    // The instance's name, sign-up options and announcement change without an event.
    let refresh = {
        let (core, key, api) = (core.clone(), key.to_owned(), api.clone());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(NODE_REFRESH).await;
                if let Ok(res) = rpc!(api.node(), get_node(pb::GetNodeRequest {})).await {
                    let moved = core.shared.read(|s| {
                        s.instance(&key).is_some_and(|i| {
                            crate::core::profile_items::items_moved(i.node.as_ref(), res.node.as_ref())
                        })
                    });
                    core.shared.instance(&key, |i| i.node = res.node);
                    // The instance's profile items changed: list them again.
                    if moved {
                        core.refresh_instance_items(&key).await;
                    }
                }
                // Notification settings changed on another device don't send an event either.
                core.refresh_notifications(&key).await;
                core.refresh_presence(&key).await;
                core.refresh_rail(&key).await;
            }
        })
    };
    // What games report goes out with this app's presence.
    let presence = tokio::spawn(crate::core::presence::keep(api.clone(), core.games.clone(), core.idle.away()));
    let result = if live {
        crate::core::friends::loading(core, key);
        follow_live(core, key, api, followed, dms).await
    } else {
        // Who's online and what they're doing, while synced.
        let people = tokio::spawn(crate::core::presence::people::follow(core.clone(), key.to_owned(), api.clone()));
        // Friends follow alongside; having that stream open is what shows you online to them.
        let friends = tokio::spawn(crate::core::friends::follow(core.clone(), key.to_owned(), api.clone()));
        let result = follow_events(core, key, api, followed).await;
        people.abort();
        friends.abort();
        result
    };
    refresh.abort();
    presence.abort();
    result
}

fn start_dms(
    core: &Arc<Core>,
    key: &str,
    api: &Api,
    user: pb::User,
    token: &str,
    slot: &Arc<Mutex<Option<Arc<DmEngine>>>>,
    live: bool,
) {
    core.shared.instance(key, |i| {
        i.dms.status = DmStatus::Starting;
        i.dms.problem = None;
    });
    let (core, key, api, token, slot) = (core.clone(), key.to_owned(), api.clone(), token.to_owned(), slot.clone());
    tokio::spawn(async move {
        match DmEngine::start(&key, api, user, &token, core.paths.vaults.clone(), core.vault_key, core.shared.clone())
            .await
        {
            Ok(engine) => {
                core.shared.instance(&key, |i| {
                    i.dms.status = DmStatus::Ready;
                    i.dms.device_id = engine.device_id().to_owned();
                });
                *slot.lock() = Some(engine.clone());
                tokio::spawn(crate::core::backup::run(engine.clone()));
                // Servers that loaded first: their secure channels' news is read now.
                let servers: Vec<(String, Vec<pb::Channel>)> = core.shared.read(|s| {
                    s.instance(&key)
                        .map(|i| i.channels.iter().map(|(sid, list)| (sid.clone(), list.clone())).collect())
                        .unwrap_or_default()
                });
                for (server_id, channels) in servers {
                    follow_secure(&core, &key, &server_id, &channels);
                }
                // On a live connection its feed comes through the connection
                // (`follow_live`), which may have said it's ready already.
                if live {
                    engine.caught_up().await;
                } else {
                    engine.follow().await;
                }
            }
            Err(err) => {
                tracing::warn!("direct messages didn't start");
                core.shared.instance(&key, |i| {
                    i.dms.status = DmStatus::Failed;
                    i.dms.problem = Some(if err.0.contains("support that yet") {
                        "This instance doesn't have direct messages yet.".into()
                    } else {
                        err.0.clone()
                    });
                });
            }
        }
    });
}

/// Where the event stream is, shared with the snapshot loads it starts.
#[derive(Default)]
struct Follow {
    /// The last event applied (or skipped as already known) per server.
    cursors: HashMap<String, i64>,
    /// Events for servers whose snapshot is still loading, applied once it lands.
    held: HashMap<String, Vec<pb::Event>>,
    /// Where each resumed server's replay started, to tell whether anything happened while away.
    resumed_from: HashMap<String, i64>,
    /// Channel events that arrived while a server's channels were being listed again.
    relisting: HashMap<String, Vec<pb::Event>>,
    /// The servers a live connection follows: those it was opened with, and
    /// those it said it followed since.
    covered: HashSet<String>,
}

async fn follow_events(
    core: &Arc<Core>,
    key: &str,
    api: &Api,
    mut followed: watch::Receiver<Vec<String>>,
) -> Result<(), Problem> {
    let state = Arc::new(Mutex::new(Follow::default()));
    let mut backoff = Backoff::new();
    loop {
        let ids = followed.borrow_and_update().clone();
        if ids.is_empty() {
            core.shared.instance(key, |i| {
                i.connection = Connection::Live;
                i.problem = None;
            });
            if followed.changed().await.is_err() {
                return Ok(());
            }
            continue;
        }
        let request = {
            let mut s = state.lock();
            s.resumed_from = s.cursors.clone();
            pb::SubscribeRequest {
                servers: ids
                    .iter()
                    .map(|id| pb::ServerCursor { server_id: id.clone(), after_sequence: s.cursors.get(id).copied() })
                    .collect(),
                ..Default::default()
            }
        };
        let outcome = tokio::select! {
            outcome = stream_once(core, key, api, request, &state, &mut backoff) => outcome,
            changed = followed.changed() => match changed {
                Ok(()) => continue,
                Err(_) => return Ok(()),
            },
        };
        match outcome {
            // The server stopped (an older one, for a deploy): follow again at once, quietly.
            Ok(()) => {}
            Err(err) if err.signed_out() => return Err(err),
            Err(err) => {
                core.shared.instance(key, |i| {
                    i.connection = Connection::Reconnecting;
                    i.problem = Some(err.message.clone());
                });
            }
        }
        backoff.wait().await;
    }
}

/// Every feed over one live connection (docs/live.md): server events,
/// direct messages, friends and presence, each handed to what its own stream
/// would have fed. It reconnects as `follow_events` does, from the same
/// cursors, and keeps the connection's focus on what's on screen (`live.rs`).
async fn follow_live(
    core: &Arc<Core>,
    key: &str,
    api: &Api,
    mut followed: watch::Receiver<Vec<String>>,
    dms: &Arc<Mutex<Option<Arc<DmEngine>>>>,
) -> Result<(), Problem> {
    let state = Arc::new(Mutex::new(Follow::default()));
    let mut backoff = Backoff::new();
    loop {
        let ids = followed.borrow_and_update().clone();
        let focus = core.shared.read(|s| live::choose_focus(s, key));
        // Only while there's a device here to read them with.
        let direct_messages = core.shared.read(|s| s.instance(key).is_some_and(|i| i.dms.status != DmStatus::Failed));
        let request = {
            let mut s = state.lock();
            s.resumed_from = s.cursors.clone();
            s.covered = ids.iter().cloned().collect();
            pb::OpenRequest {
                servers: ids
                    .iter()
                    .map(|id| pb::ServerCursor { server_id: id.clone(), after_sequence: s.cursors.get(id).copied() })
                    .collect(),
                // Servers joined meanwhile come as `followed`, without opening again.
                follow_new_servers: true,
                direct_messages,
                friends: true,
                presence: true,
                messages: pb::MessageIntent::Unspecified as i32,
                focus: Some(focus),
            }
        };
        let outcome = live_once(core, key, api, request, &state, &mut backoff, &mut followed, dms).await;
        core.shared.instance(key, |i| i.live = None);
        match outcome {
            // The server stopped (an older one, for a deploy), or a server
            // has to be added by hand: open again at once, quietly.
            Ok(()) => {}
            Err(err) if err.signed_out() => return Err(err),
            Err(err) => {
                core.shared.instance(key, |i| {
                    i.connection = Connection::Reconnecting;
                    i.problem = Some(err.message.clone());
                });
            }
        }
        if followed.has_changed().is_err() {
            return Ok(());
        }
        backoff.wait().await;
    }
}

/// How long a server added to the followed list waits for the live
/// connection to say it follows it too (servers joined or made are
/// announced, on a split instance after the directory hears of them),
/// before the connection opens again with it.
const COVER_WAIT: Duration = Duration::from_secs(10);

/// One response from a live connection's direct-message feed. After `ready`
/// the engine catches up in the background (`tasks`), and what follows
/// waits in `queue` until it has, so it's taken in order, as the feed's own
/// stream would.
fn take_dms(
    dms: &Arc<Mutex<Option<Arc<DmEngine>>>>,
    res: pb::WatchResponse,
    queue: &mut Option<VecDeque<pb::WatchResponse>>,
    tasks: &mut JoinSet<live::Done>,
) {
    if let Some(waiting) = queue {
        waiting.push_back(res);
        return;
    }
    // Not started yet: it catches up by itself once it is.
    let Some(engine) = dms.lock().clone() else { return };
    if res.ready {
        let rest = res.event.map(|event| pb::WatchResponse { ready: false, event: Some(event) });
        *queue = Some(rest.into_iter().collect());
        tasks.spawn(async move {
            engine.caught_up().await;
            live::Done::DmsCaughtUp
        });
    } else {
        engine.take_event(res);
    }
}

#[allow(clippy::too_many_arguments)]
async fn live_once(
    core: &Arc<Core>,
    key: &str,
    api: &Api,
    request: pb::OpenRequest,
    state: &Arc<Mutex<Follow>>,
    backoff: &mut Backoff,
    followed: &mut watch::Receiver<Vec<String>>,
    dms: &Arc<Mutex<Option<Arc<DmEngine>>>>,
) -> Result<(), Problem> {
    use pb::open_response::Item;
    let mut catching_up = Some(std::time::Instant::now());
    let mut focus = live::Focuser::new(core, key, request.focus.clone().unwrap_or_default());
    let mut stream = api.live().open(request).await.map_err(Problem::from)?.into_inner();
    let mut presence = crate::core::presence::people::Feed::default();
    // What the connection starts (reads, catching up) ends with it.
    let mut tasks: JoinSet<live::Done> = JoinSet::new();
    let mut notifier = live::Notifier::default();
    let mut dm_queue: Option<VecDeque<pb::WatchResponse>> = None;
    let mut heard = tokio::time::Instant::now();
    // When a server the connection doesn't cover yet was added.
    let mut uncovered: Option<tokio::time::Instant> = None;
    let missing = |followed: &watch::Receiver<Vec<String>>| {
        let s = state.lock();
        followed.borrow().iter().any(|id| !s.covered.contains(id))
    };
    loop {
        let (presence_due, focus_due, cover_due, notify_due) = (presence.due, focus.due, uncovered, notifier.due());
        tokio::select! {
            next = stream.message() => {
                let Some(res) = next.map_err(Problem::from)? else { return Ok(()) };
                heard = tokio::time::Instant::now();
                match res.item {
                    // The heartbeat.
                    None => {}
                    Some(Item::ConnectionId(id)) => focus.connected(api, id),
                    Some(Item::Events(res)) => {
                        if take_events(core, key, api, res, state, &mut catching_up) {
                            *backoff = Backoff::new();
                            focus.ready(api, &mut tasks);
                        }
                    }
                    // Catching up takes a while; the other feeds go on meanwhile.
                    Some(Item::DirectMessages(res)) => take_dms(dms, res, &mut dm_queue, &mut tasks),
                    Some(Item::Friends(res)) => {
                        crate::core::friends::take(core, key, api, res).await?;
                    }
                    Some(Item::Presence(res)) => {
                        presence.take(core, key, res);
                    }
                    Some(Item::Heads(heads)) => {
                        {
                            let mut s = state.lock();
                            for server in &heads.servers {
                                if let Some(cursor) = s.cursors.get_mut(&server.server_id) {
                                    *cursor = (*cursor).max(server.sequence);
                                }
                            }
                        }
                        live::take_heads(core, key, &heads, &mut notifier);
                        notifier.pump(core, key, api, &mut tasks);
                    }
                    Some(Item::Focus(now)) => focus.echoed(api, now, &mut tasks),
                }
            }
            Some(done) = tasks.join_next(), if !tasks.is_empty() => {
                match done {
                    Ok(live::Done::Notified(channel_id)) => {
                        notifier.finished(&channel_id);
                        notifier.pump(core, key, api, &mut tasks);
                    }
                    Ok(live::Done::DmsCaughtUp) => {
                        for res in dm_queue.take().unwrap_or_default() {
                            take_dms(dms, res, &mut dm_queue, &mut tasks);
                        }
                    }
                    Ok(live::Done::Nothing) | Err(_) => {}
                }
            }
            () = tokio::time::sleep_until(notify_due.unwrap_or(heard)), if notify_due.is_some() => {
                notifier.pump(core, key, api, &mut tasks);
            }
            () = tokio::time::sleep_until(heard + SILENCE) => {
                return Err(Problem::new(Code::Unavailable, "Lost the connection."));
            }
            () = tokio::time::sleep_until(presence_due.unwrap_or(heard)), if presence_due.is_some() => {
                presence.flush(core, key);
            }
            changed = focus.changes.changed() => {
                if changed.is_ok() {
                    focus.changed();
                }
            }
            () = tokio::time::sleep_until(focus_due.unwrap_or(heard)), if focus_due.is_some() => {
                focus.settle();
            }
            // Joining or making a server adds it here, and the connection
            // says it follows it too; leaving one ends it there. Only a
            // server it never mentions needs the connection opened again.
            changed = followed.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                if uncovered.is_none() && missing(followed) {
                    uncovered = Some(tokio::time::Instant::now() + COVER_WAIT);
                }
            }
            () = tokio::time::sleep_until(cover_due.unwrap_or(heard)), if cover_due.is_some() => {
                uncovered = None;
                if missing(followed) {
                    return Ok(());
                }
            }
        }
    }
}

async fn stream_once(
    core: &Arc<Core>,
    key: &str,
    api: &Api,
    request: pb::SubscribeRequest,
    state: &Arc<Mutex<Follow>>,
    backoff: &mut Backoff,
) -> Result<(), Problem> {
    // Catching up: from asking to follow until the stream says it's ready.
    let mut catching_up = Some(std::time::Instant::now());
    let mut stream = api.events().subscribe(request).await.map_err(Problem::from)?.into_inner();
    loop {
        let next = tokio::time::timeout(SILENCE, stream.message())
            .await
            .map_err(|_| Problem::new(Code::Unavailable, "Lost the connection."))?
            .map_err(Problem::from)?;
        let Some(res) = next else { return Ok(()) };
        if take_events(core, key, api, res, state, &mut catching_up) {
            *backoff = Backoff::new();
        }
    }
}

/// One response from the server-event feed, from its own stream or a live
/// connection's. True when it's the stream saying it's ready.
fn take_events(
    core: &Arc<Core>,
    key: &str,
    api: &Api,
    res: pb::SubscribeResponse,
    state: &Arc<Mutex<Follow>>,
    catching_up: &mut Option<std::time::Instant>,
) -> bool {
    let ready = res.ready.is_some();
    if let Some(ready) = res.ready {
        if let Some(began) = catching_up.take() {
            reports::timing("catch_up", began.elapsed());
        }
        for head in ready.servers {
            let known = {
                let mut s = state.lock();
                if s.cursors.contains_key(&head.server_id) {
                    let from = s.resumed_from.get(&head.server_id).copied();
                    Some(from.is_some_and(|from| head.sequence > from))
                } else {
                    s.cursors.insert(head.server_id.clone(), head.sequence);
                    s.held.insert(head.server_id.clone(), Vec::new());
                    None
                }
            };
            if known.is_some() {
                tokio::spawn(relist_voice(core.clone(), key.to_owned(), api.clone(), head.server_id.clone()));
                tokio::spawn(reread_shown(core.clone(), key.to_owned(), api.clone(), head.server_id.clone()));
            }
            match known {
                Some(true) => {
                    tokio::spawn(relist(core.clone(), key.to_owned(), api.clone(), head.server_id, state.clone()));
                }
                Some(false) => {}
                None => {
                    tokio::spawn(snapshot(core.clone(), key.to_owned(), api.clone(), head.server_id, state.clone()));
                }
            }
        }
        core.shared.instance(key, |i| {
            i.connection = Connection::Live;
            i.problem = None;
        });
    }
    // A server joined while a live connection was open: loaded like a new
    // one at `ready`, then what follows applies.
    if let Some(head) = res.followed {
        let new = {
            let mut s = state.lock();
            s.covered.insert(head.server_id.clone());
            let new = !s.cursors.contains_key(&head.server_id);
            if new {
                s.cursors.insert(head.server_id.clone(), head.sequence);
                s.held.insert(head.server_id.clone(), Vec::new());
            }
            new
        };
        if new {
            core.follow(key, &head.server_id, true);
            tokio::spawn(snapshot(core.clone(), key.to_owned(), api.clone(), head.server_id, state.clone()));
        }
    }
    if let Some(event) = res.event {
        handle_event(core, key, event, state);
    }
    ready
}

fn handle_event(core: &Arc<Core>, key: &str, event: pb::Event, state: &Arc<Mutex<Follow>>) {
    use pb::event::Payload;
    let sid = event.server_id.clone();
    {
        let mut s = state.lock();
        if event.sequence > 0 {
            if s.cursors.get(&sid).is_some_and(|last| event.sequence <= *last) {
                return;
            }
            s.cursors.insert(sid.clone(), event.sequence);
        }
        if let Some(buffer) = s.held.get_mut(&sid) {
            buffer.push(event);
            return;
        }
        if matches!(
            event.payload,
            Some(Payload::ChannelCreated(_) | Payload::ChannelUpdated(_) | Payload::ChannelDeleted(_))
        ) && let Some(list) = s.relisting.get_mut(&sid)
        {
            list.push(event.clone());
        }
    }
    let prefs = core.prefs();
    let (outcome, name, notice) = core.shared.update(|store| {
        let focus = store.focus_channel(key).map(str::to_owned);
        let focus_thread = store.focus_thread(key).map(str::to_owned);
        let Some(i) = store.instances.get_mut(key) else { return (Outcome::Nothing, None, None) };
        let name = i.server(&sid).map(|s| s.name.clone());
        let outcome = store::apply_event(i, &event, focus.as_deref(), focus_thread.as_deref());
        let notice = match (&outcome, &event.payload) {
            (
                Outcome::Unread { channel_id } | Outcome::ThreadReply { channel_id, .. },
                Some(Payload::MessageCreated(created)),
            ) => {
                // A thread reply reaches the people following the thread and the people it mentions.
                let thread = match &outcome {
                    Outcome::ThreadReply { thread_id, .. } => Some(thread_id.as_str()),
                    _ => None,
                };
                created.message.as_ref().and_then(|m| {
                    message_notice(i, key, &sid, channel_id, thread, m, event.created_at.as_ref(), &prefs)
                })
            }
            _ => None,
        };
        (outcome, name, notice)
    });
    if let Some(notice) = notice {
        core.shared.notice(notice);
    }
    // Secure channels' records, and changes to who can read them, go to the encryption.
    if let Some(payload) = &event.payload
        && let Some(engine) = core.dm_engine(key)
    {
        engine.on_server_event(&sid, payload);
    }
    // A pin changed: the lists it shows in are read again where they're open.
    if let Some(Payload::MessagePinned(p)) = &event.payload {
        core.reload_pins(key, &sid, &p.channel_id, &p.thread_id);
    }
    // A server's shared channels changed: read them again where a manager has them open.
    if matches!(event.payload, Some(Payload::SharedChannelsUpdated(_)))
        && core.shared.read(|s| s.instance(key).is_some_and(|i| i.shared.contains_key(&sid)))
    {
        let (core, key, sid) = (core.clone(), key.to_owned(), sid.clone());
        tokio::spawn(async move {
            let _ = core.list_connections(&key, &sid).await;
        });
    }
    if outcome == Outcome::Gone {
        {
            let mut s = state.lock();
            s.cursors.remove(&sid);
            s.held.remove(&sid);
        }
        if let Some(server) = name {
            core.shared.notice(Notice::Removed { server });
        }
        core.follow(key, &sid, false);
    }
}

/// What a new message from someone else in a channel that isn't on screen
/// says out loud: a notification, by the channel's settings, when it should.
/// A thread reply (`thread`) reaches only the people following the thread
/// and the people it mentions.
#[allow(clippy::too_many_arguments)]
pub(super) fn message_notice(
    i: &InstanceState,
    key: &str,
    sid: &str,
    channel_id: &str,
    thread: Option<&str>,
    m: &pb::Message,
    at: Option<&prost_types::Timestamp>,
    prefs: &Prefs,
) -> Option<Notice> {
    // Join messages and catching up after a reconnect stay quiet.
    let fresh = at.is_none_or(|t| dms::now_ms() - t.seconds * 1000 < FRESH_MS);
    if m.kind != pb::MessageKind::Unspecified as i32 || !fresh {
        return None;
    }
    let settings = i.effective_notifications(sid, channel_id, dms::now_ms());
    let mention = i.pings_me(sid, m, settings.suppress_everyone);
    let following = thread.is_some_and(|t| threads::follows(i, sid, t) == Some(true));
    if thread.is_some() && !mention && !following {
        return None;
    }
    notifications::should_notify(settings, mention || following, prefs).then(|| Notice::Message {
        instance: key.to_owned(),
        server_id: Some(sid.to_owned()),
        channel_id: channel_id.to_owned(),
        title: format!(
            "{}{} in #{}",
            match &m.webhook {
                Some(w) => w.name.clone(),
                None => i.display_name(Some(sid), &m.author_id),
            },
            if thread.is_some() { " replied in a thread" } else { "" },
            i.channel(sid, channel_id).map(|c| c.name.as_str()).unwrap_or("a channel")
        ),
        // An app may post only a card.
        body: [&m.content]
            .into_iter()
            .chain(m.embeds.first().map(|e| &e.title))
            .chain(m.embeds.first().map(|e| &e.description))
            .find(|t| !t.is_empty())
            .map(|t| t.chars().take(160).collect())
            .unwrap_or_default(),
        mention,
        thread: thread.map(str::to_owned),
    })
}

/// A server's state, loaded in one go once the stream says where it stands.
async fn snapshot(core: Arc<Core>, key: String, api: Api, server_id: String, state: Arc<Mutex<Follow>>) {
    let load = || async {
        let id = server_id.clone();
        let (server, channels, members, roles) = tokio::try_join!(
            rpc!(api.servers(), get_server(pb::GetServerRequest { server_id: id.clone() })),
            rpc!(api.channels(), list_channels(pb::ListChannelsRequest { server_id: id.clone() })),
            rpc!(api.servers(), list_members(pb::ListMembersRequest { server_id: id.clone() })),
            rpc!(api.roles(), list_roles(pb::ListRolesRequest { server_id: id.clone() })),
        )?;
        // An instance from before custom emoji has none to list.
        let emojis =
            rpc!(api.emojis(), list_emojis(pb::ListEmojisRequest { server_id: id.clone(), ..Default::default() }))
                .await
                .map(|r| r.emojis)
                .unwrap_or_default();
        // Instances from before calls don't know who's in voice; that's nobody.
        let voice = rpc!(api.calls(), list_voice_states(pb::ListVoiceStatesRequest { server_id: id.clone() }))
            .await
            .map(|r| r.states)
            .unwrap_or_default();
        // Apps' live tiles aren't in the log either: listed with voice.
        let tiles = core.list_live_tiles(&key, &id).await;
        // Nor profile items before those.
        let items = rpc!(
            api.profile_items(),
            list_server_profile_items(pb::ListServerProfileItemsRequest { server_id: id.clone() })
        )
        .await
        .map(|r| r.items)
        .unwrap_or_default();
        Ok::<_, Problem>((server, channels, members, roles, emojis, (voice, tiles, items)))
    };
    match retrying(&core, &key, load).await {
        Ok((server, channels, members, roles, emojis, (voice, tiles, items))) => {
            let held = state.lock().held.remove(&server_id).unwrap_or_default();
            follow_secure(&core, &key, &server_id, &channels.channels);
            core.shared.update(|s| {
                let focus = s.focus_channel(&key).map(str::to_owned);
                let focus_thread = s.focus_thread(&key).map(str::to_owned);
                let Some(i) = s.instances.get_mut(&key) else { return };
                let Some(server) = server.server else { return };
                let sid = server.id.clone();
                store::apply_snapshot(i, server, channels.channels, members.members, roles.roles, emojis);
                i.voice.insert(sid.clone(), voice);
                i.live_tiles.insert(sid.clone(), tiles);
                i.server_items.insert(sid, items);
                for event in &held {
                    store::apply_event(i, event, focus.as_deref(), focus_thread.as_deref());
                }
            });
        }
        Err(_) => {
            // Left or deleted while loading: stop following it.
            {
                let mut s = state.lock();
                s.held.remove(&server_id);
                s.cursors.remove(&server_id);
            }
            core.shared.instance(&key, |i| store::remove_server(i, &server_id));
            core.follow(&key, &server_id, false);
        }
    }
}

/// A replay goes by what you can see now, so channels you gained or lost
/// while away only show up by listing them again.
/// Who's in voice isn't kept in the server's log (those events carry no
/// sequence), so any that came while the stream was away are lost: after a
/// gap, read them again.
async fn relist_voice(core: Arc<Core>, key: String, api: Api, server_id: String) {
    // So are apps' live tiles.
    let tiles = core.list_live_tiles(&key, &server_id).await;
    core.shared.instance(&key, |i| {
        if i.synced.contains(&server_id) {
            i.live_tiles.insert(server_id.clone(), tiles);
        }
    });
    let req = pb::ListVoiceStatesRequest { server_id: server_id.clone() };
    let Ok(res) = rpc!(api.calls(), list_voice_states(req)).await else { return };
    core.shared.instance(&key, |i| {
        if i.synced.contains(&server_id) {
            i.voice.insert(server_id, res.states);
        }
    });
}

/// What's said in a channel shown from another server arrives live but isn't
/// in this server's log, so after a gap the channels open here read their
/// latest messages again from the home.
async fn reread_shown(core: Arc<Core>, key: String, api: Api, server_id: String) {
    let shown: Vec<String> = core.shared.read(|s| {
        let Some(i) = s.instance(&key) else { return Vec::new() };
        i.channels
            .get(&server_id)
            .into_iter()
            .flatten()
            .filter(|c| c.shared.as_ref().is_some_and(|sh| !sh.home) && i.messages.contains_key(&c.id))
            .map(|c| c.id.clone())
            .collect()
    });
    for channel_id in shown {
        let req = pb::ListMessagesRequest {
            server_id: server_id.clone(),
            channel_id: channel_id.clone(),
            limit: 50,
            ..Default::default()
        };
        let Ok(res) = rpc!(api.messages(), list_messages(req)).await else { continue };
        core.shared.instance(&key, |i| {
            for user in res.authors {
                i.users.insert(user.id.clone(), user);
            }
            store::add_shared_authors(&mut i.users, &res.messages);
            if let Some(loaded) = i.messages.get_mut(&channel_id) {
                for m in res.messages {
                    store::put_message(&mut loaded.items, m);
                }
            }
        });
    }
}

async fn relist(core: Arc<Core>, key: String, api: Api, server_id: String, state: Arc<Mutex<Follow>>) {
    state.lock().relisting.insert(server_id.clone(), Vec::new());
    let listed = retrying(&core, &key, || {
        rpc!(api.channels(), list_channels(pb::ListChannelsRequest { server_id: server_id.clone() }))
    })
    .await;
    let events = state.lock().relisting.remove(&server_id).unwrap_or_default();
    if let Ok(listed) = listed {
        follow_secure(&core, &key, &server_id, &listed.channels);
        core.shared.update(|s| {
            let focus = s.focus_channel(&key).map(str::to_owned);
            let focus_thread = s.focus_thread(&key).map(str::to_owned);
            let Some(i) = s.instances.get_mut(&key) else { return };
            if !i.synced.contains(&server_id) {
                return;
            }
            store::set_channels(i, &server_id, listed.channels);
            for event in &events {
                store::apply_event(i, event, focus.as_deref(), focus_thread.as_deref());
            }
        });
    }
}

/// Follows the secure channels this device was already in, once their server loads.
fn follow_secure(core: &Arc<Core>, key: &str, server_id: &str, channels: &[pb::Channel]) {
    let ids: Vec<String> =
        channels.iter().filter(|c| c.r#type == pb::ChannelType::Secure as i32).map(|c| c.id.clone()).collect();
    if ids.is_empty() {
        return;
    }
    if let Some(engine) = core.dm_engine(key) {
        let server_id = server_id.to_owned();
        tokio::spawn(async move { engine.follow_server(&server_id, &ids).await });
    }
}
