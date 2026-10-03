//! Keeps one instance in the store in step with its server: loads who you
//! are and which servers you're in, follows all of them over one event
//! stream, and reconnects with backoff, resuming from the last event seen so
//! nothing is missed or applied twice. A port of `web/src/fuwa/sync.ts`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::watch;
use tonic::Code;

use crate::core::api::{Api, Problem};
use crate::core::dms::{self, DmEngine, DmStatus};
use crate::core::notifications;
use crate::core::store::{self, Connection, Outcome};
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

    let me = retrying(core, key, || rpc!(api.auth(), get_me(pb::GetMeRequest {}))).await?;
    core.shared.instance(key, |i| {
        if let Some(user) = &me.user {
            i.users.insert(user.id.clone(), user.clone());
        }
        i.me = me.user.clone();
        i.admin = me.admin;
    });

    // Encrypted direct messages run alongside, for as long as this does.
    if let Some(user) = me.user.clone() {
        start_dms(core, key, api, user, &token, dms);
    }

    // Notification settings follow the account; an older instance without them just has none.
    core.refresh_notifications(key).await;

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
                    core.shared.instance(&key, |i| i.node = res.node);
                }
                // Notification settings changed on another device don't send an event either.
                core.refresh_notifications(&key).await;
            }
        })
    };
    let result = follow_events(core, key, api, followed).await;
    refresh.abort();
    result
}

fn start_dms(
    core: &Arc<Core>,
    key: &str,
    api: &Api,
    user: pb::User,
    token: &str,
    slot: &Arc<Mutex<Option<Arc<DmEngine>>>>,
) {
    core.shared.instance(key, |i| {
        i.dms.status = DmStatus::Starting;
        i.dms.problem = None;
    });
    let (core, key, api, token, slot) = (core.clone(), key.to_owned(), api.clone(), token.to_owned(), slot.clone());
    tokio::spawn(async move {
        match DmEngine::start(&key, api, user, &token, core.paths.vaults.clone(), core.shared.clone()).await {
            Ok(engine) => {
                core.shared.instance(&key, |i| {
                    i.dms.status = DmStatus::Ready;
                    i.dms.device_id = engine.device_id().to_owned();
                });
                *slot.lock() = Some(engine.clone());
                engine.follow().await;
            }
            Err(err) => {
                tracing::warn!("direct messages didn't start: {err}");
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

async fn stream_once(
    core: &Arc<Core>,
    key: &str,
    api: &Api,
    request: pb::SubscribeRequest,
    state: &Arc<Mutex<Follow>>,
    backoff: &mut Backoff,
) -> Result<(), Problem> {
    let mut stream = api.events().subscribe(request).await.map_err(Problem::from)?.into_inner();
    loop {
        let next = tokio::time::timeout(SILENCE, stream.message())
            .await
            .map_err(|_| Problem::new(Code::Unavailable, "Lost the connection."))?
            .map_err(Problem::from)?;
        let Some(res) = next else { return Ok(()) };
        if let Some(ready) = res.ready {
            *backoff = Backoff::new();
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
                match known {
                    Some(true) => {
                        tokio::spawn(relist(core.clone(), key.to_owned(), api.clone(), head.server_id, state.clone()));
                    }
                    Some(false) => {}
                    None => {
                        tokio::spawn(snapshot(
                            core.clone(),
                            key.to_owned(),
                            api.clone(),
                            head.server_id,
                            state.clone(),
                        ));
                    }
                }
            }
            core.shared.instance(key, |i| {
                i.connection = Connection::Live;
                i.problem = None;
            });
        }
        let Some(event) = res.event else { continue };
        handle_event(core, key, event, state);
    }
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
        let Some(i) = store.instances.get_mut(key) else { return (Outcome::Nothing, None, None) };
        let name = i.server(&sid).map(|s| s.name.clone());
        let outcome = store::apply_event(i, &event, focus.as_deref());
        let notice = match (&outcome, &event.payload) {
            (Outcome::Unread { channel_id }, Some(Payload::MessageCreated(created))) => {
                created.message.as_ref().and_then(|m| {
                    // Join messages and catching up after a reconnect stay quiet.
                    let fresh = event.created_at.as_ref().is_none_or(|t| dms::now_ms() - t.seconds * 1000 < FRESH_MS);
                    if m.kind != pb::MessageKind::Unspecified as i32 || !fresh {
                        return None;
                    }
                    let settings = i.effective_notifications(&sid, channel_id, dms::now_ms());
                    let mention = i.pings_me(&sid, m, settings.suppress_everyone);
                    notifications::should_notify(settings, mention, &prefs).then(|| Notice::Message {
                        instance: key.to_owned(),
                        server_id: Some(sid.clone()),
                        channel_id: channel_id.clone(),
                        title: format!(
                            "{} in #{}",
                            i.display_name(Some(&sid), &m.author_id),
                            i.channel(&sid, channel_id).map(|c| c.name.as_str()).unwrap_or("a channel")
                        ),
                        body: m.content.chars().take(160).collect(),
                        mention,
                    })
                })
            }
            _ => None,
        };
        (outcome, name, notice)
    });
    if let Some(notice) = notice {
        core.shared.notice(notice);
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
        // Instances from before calls don't know who's in voice; that's nobody.
        let voice = rpc!(api.calls(), list_voice_states(pb::ListVoiceStatesRequest { server_id: id.clone() }))
            .await
            .map(|r| r.states)
            .unwrap_or_default();
        Ok::<_, Problem>((server, channels, members, roles, voice))
    };
    match retrying(&core, &key, load).await {
        Ok((server, channels, members, roles, voice)) => {
            let held = state.lock().held.remove(&server_id).unwrap_or_default();
            core.shared.update(|s| {
                let focus = s.focus_channel(&key).map(str::to_owned);
                let Some(i) = s.instances.get_mut(&key) else { return };
                let Some(server) = server.server else { return };
                let sid = server.id.clone();
                store::apply_snapshot(i, server, channels.channels, members.members, roles.roles);
                i.voice.insert(sid, voice);
                for event in &held {
                    store::apply_event(i, event, focus.as_deref());
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
async fn relist(core: Arc<Core>, key: String, api: Api, server_id: String, state: Arc<Mutex<Follow>>) {
    state.lock().relisting.insert(server_id.clone(), Vec::new());
    let listed = retrying(&core, &key, || {
        rpc!(api.channels(), list_channels(pb::ListChannelsRequest { server_id: server_id.clone() }))
    })
    .await;
    let events = state.lock().relisting.remove(&server_id).unwrap_or_default();
    if let Ok(listed) = listed {
        core.shared.update(|s| {
            let focus = s.focus_channel(&key).map(str::to_owned);
            let Some(i) = s.instances.get_mut(&key) else { return };
            if !i.synced.contains(&server_id) {
                return;
            }
            store::set_channels(i, &server_id, listed.channels);
            for event in &events {
                store::apply_event(i, event, focus.as_deref());
            }
        });
    }
}
