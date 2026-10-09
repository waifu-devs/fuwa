//! Agents' endpoints (docs/agent-endpoints.md): agents that don't hold a
//! stream open get their events posted to a URL instead.
//!
//! Each server's own log is the queue. Where servers are kept, one worker
//! per server and agent with an endpoint reads the events after where that
//! agent stands (the server file's `agent_deliveries`), passes them through
//! the agent's [`View`] as `Subscribe` would, and posts them in order, a
//! batch at a time, as proto3 JSON signed the Standard Webhooks way. Only
//! once the endpoint answers 2xx does the agent's place move on, so a
//! failure is tried again (backing off) with nothing lost, until a day of
//! failures turns the endpoint off. An answer may carry replies to the
//! interactions it got, sent as the agent's own `SendMessage`.
//!
//! The endpoints themselves (URL, events, secret) are node.db's, reached
//! through [`App::agent_endpoints`] and remembered here until
//! [`App::agent_endpoint_changed`] says otherwise.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use futures::StreamExt;
use hmac::{Hmac, Mac};
use prost_reflect::{DescriptorPool, DeserializeOptions, DynamicMessage, MessageDescriptor, ReflectMessage, Value};
use sha2::Sha256;
use tokio::sync::{Notify, Semaphore, broadcast, mpsc};
use tokio::task::JoinHandle;
use url::Url;

use super::Api;
use super::events::View;
use crate::app::App;
use crate::cpb;
use crate::db::query_all;
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms};
use crate::node::Account;
use crate::pb;
use crate::servers::{Payload, ServerDb};

/// How long an endpoint may keep failing before it's turned off.
pub const ENDPOINT_GIVE_UP_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
/// How long an endpoint has to answer.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Most events in one delivery.
const MAX_EVENTS: usize = 50;
/// Events read from the log at a time.
const READ_PAGE: i64 = 200;
/// Most bytes of an answer read.
const MAX_ANSWER: usize = 1 << 20;
/// Longest URL an endpoint may have.
const MAX_URL: usize = 2048;
/// Deliveries in flight at once, from this part. Each agent has one at a
/// time, whatever the number of its servers (`Run::lane`), so an endpoint that
/// never answers holds one of these, not all of them.
static IN_FLIGHT: Semaphore = Semaphore::const_new(64);
/// Checks of new endpoints in flight at once, apart from deliveries.
static CHECKS: Semaphore = Semaphore::const_new(8);
/// Checks one account may have sent in a minute.
const CHECKS_PER_MINUTE: i64 = 10;
/// Checks each account sent this minute, kept in memory.
static CHECKED: Mutex<Option<HashMap<String, (i64, i64)>>> = Mutex::new(None);
/// How often servers with events the agents' tap had no room for are woken.
const MISSED_EVERY: Duration = Duration::from_secs(1);
/// How long an endpoint is remembered without word of a change, in case word
/// went missing.
const REMEMBER_FOR: Duration = Duration::from_secs(5 * 60);
/// How often the directory hears that deliveries still go through, or still fail.
const REPORT_EVERY: Duration = Duration::from_secs(10 * 60);
const FIRST_RETRY: Duration = Duration::from_secs(1);
const LAST_RETRY: Duration = Duration::from_secs(5 * 60);
/// How often an idle worker checks it's still wanted.
const IDLE_CHECK: Duration = Duration::from_secs(60);

// ── JSON ────────────────────────────────────────────────────────────────────

static POOL: LazyLock<DescriptorPool> = LazyLock::new(|| {
    DescriptorPool::decode(crate::proto::FILE_DESCRIPTOR_SET).expect("the API's descriptor set reads")
});

fn descriptor(name: &str) -> MessageDescriptor {
    POOL.get_message_by_name(name).expect("a fuwa.v1 message")
}

fn dynamic(name: &str, message: &impl prost::Message) -> DynamicMessage {
    DynamicMessage::decode(descriptor(name), message.encode_to_vec().as_slice()).expect("a message reads as itself")
}

/// The names an endpoint may ask for: the fields of `Event.payload`.
pub fn event_names() -> Vec<String> {
    descriptor("fuwa.v1.Event")
        .oneofs()
        .filter(|oneof| oneof.name() == "payload")
        .flat_map(|oneof| oneof.fields().map(|field| field.name().to_string()).collect::<Vec<_>>())
        .collect()
}

/// An event's payload name, such as "message_created".
fn payload_name(event: &DynamicMessage) -> Option<String> {
    let oneof = event.descriptor().oneofs().find(|oneof| oneof.name() == "payload")?;
    oneof.fields().find(|field| event.has_field(field)).map(|field| field.name().to_string())
}

fn delivery_json(agent_id: &str, challenge: &str, events: Vec<DynamicMessage>) -> Vec<u8> {
    let mut delivery = DynamicMessage::new(descriptor("fuwa.v1.AgentDelivery"));
    delivery.set_field_by_name("agent_id", Value::String(agent_id.to_string()));
    if !challenge.is_empty() {
        delivery.set_field_by_name("challenge", Value::String(challenge.to_string()));
    }
    delivery.set_field_by_name("events", Value::List(events.into_iter().map(Value::Message).collect()));
    serde_json::to_vec(&delivery).expect("a message writes as JSON")
}

/// An endpoint's answer: nothing at all, or an AgentDeliveryAnswer in proto3
/// JSON. Fields this instance doesn't know are skipped.
fn read_answer(body: &[u8]) -> Option<pb::AgentDeliveryAnswer> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Some(pb::AgentDeliveryAnswer::default());
    }
    let options = DeserializeOptions::new().deny_unknown_fields(false);
    let mut json = serde_json::Deserializer::from_slice(body);
    let answer =
        DynamicMessage::deserialize_with_options(descriptor("fuwa.v1.AgentDeliveryAnswer"), &mut json, &options)
            .ok()?;
    answer.transcode_to().ok()
}

// ── Signing ─────────────────────────────────────────────────────────────────

/// A new signing secret, as Standard Webhooks libraries take it.
pub fn new_secret() -> String {
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).expect("the OS random number generator failed");
    format!("whsec_{}", STANDARD.encode(key))
}

/// `webhook-signature` for a delivery: HMAC-SHA256 over "id.timestamp.body"
/// under the secret's key.
fn signature(secret: &str, id: &str, timestamp: i64, body: &[u8]) -> String {
    let key = STANDARD.decode(secret.strip_prefix("whsec_").unwrap_or(secret)).unwrap_or_default();
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).expect("HMAC takes any key");
    mac.update(format!("{id}.{timestamp}.").as_bytes());
    mac.update(body);
    format!("v1,{}", STANDARD.encode(mac.finalize().into_bytes()))
}

// ── Posting ─────────────────────────────────────────────────────────────────

/// Whether `url` may be an endpoint under the instance's setting: https at a
/// public address, or with `Any`, http(s) anywhere. Names are checked again
/// as they're looked up ([`crate::outside::PublicOnly`]).
pub fn check_url(policy: pb::AgentEndpoints, url: &str) -> std::result::Result<Url, String> {
    if policy == pb::AgentEndpoints::Off {
        return Err("this instance doesn't send events to agents' endpoints".into());
    }
    if url.len() > MAX_URL {
        return Err(format!("an endpoint's link is at most {MAX_URL} characters"));
    }
    let url = Url::parse(url).map_err(|_| "that isn't a link".to_string())?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("an endpoint's link can't have a user name or password in it".into());
    }
    let anywhere = policy == pb::AgentEndpoints::Any;
    match url.scheme() {
        "https" => {}
        "http" if anywhere => {}
        _ if anywhere => return Err("an endpoint's link starts with https:// or http://".into()),
        _ => return Err("an endpoint's link starts with https://".into()),
    }
    let internal = match url.host() {
        None => return Err("that link has no host".into()),
        _ if anywhere => false,
        Some(url::Host::Ipv4(ip)) => !crate::outside::is_public(ip.into()),
        Some(url::Host::Ipv6(ip)) => !crate::outside::is_public(ip.into()),
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || [".localhost", ".internal", ".local"].iter().any(|end| name.ends_with(end))
        }
    };
    if internal {
        return Err("this instance only sends to public addresses".into());
    }
    Ok(url)
}

static PUBLIC_ONLY: LazyLock<reqwest::Client> = LazyLock::new(|| client(true));
static ANYWHERE: LazyLock<reqwest::Client> = LazyLock::new(|| client(false));

fn client(public_only: bool) -> reqwest::Client {
    let builder = reqwest::Client::builder()
        .user_agent(format!("fuwa/{} (agent endpoint; +https://github.com/waifu-devs/fuwa)", crate::VERSION))
        // Never through a proxy from the environment: it would resolve
        // names itself, past the check on where they point.
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(TIMEOUT);
    let builder = if public_only { builder.dns_resolver(Arc::new(crate::outside::PublicOnly)) } else { builder };
    builder.build().expect("the endpoint client's settings are valid")
}

/// Posts a signed delivery, once `pool` has room; the answer's body, on a
/// 2xx. Otherwise what went wrong, in a few words for the endpoint's owner.
async fn post(
    pool: &Semaphore,
    policy: pb::AgentEndpoints,
    url: &str,
    secret: &str,
    id: &str,
    body: Vec<u8>,
) -> Result<Bytes, String> {
    let url = check_url(policy, url)?;
    let client = if policy == pb::AgentEndpoints::Any { &*ANYWHERE } else { &*PUBLIC_ONLY };
    let timestamp = now_ms() / 1000;
    let signed = signature(secret, id, timestamp, &body);
    let _turn = pool.acquire().await.map_err(|_| "stopping".to_string())?;
    let response = client
        .post(url)
        .header(http::header::CONTENT_TYPE, "application/json")
        .header("webhook-id", id)
        .header("webhook-timestamp", timestamp.to_string())
        .header("webhook-signature", signed)
        .body(body)
        .send()
        .await
        .map_err(|err| {
            if err.is_timeout() {
                "timed out".to_string()
            } else if err.is_connect() {
                "couldn't be reached".to_string()
            } else {
                "didn't answer".to_string()
            }
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("answered {}", status.as_u16()));
    }
    let mut answer = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "cut its answer off".to_string())?;
        if answer.len() + chunk.len() > MAX_ANSWER {
            return Err("answered with more than 1 MB".into());
        }
        answer.extend_from_slice(&chunk);
    }
    Ok(answer.into())
}

/// Checks an endpoint before it's saved: it must answer a delivery carrying
/// a challenge with that challenge, which only something written for fuwa
/// does, so nobody's events are sent to a site that didn't ask for them.
/// `account_id`, who asked for it, may ask [`CHECKS_PER_MINUTE`] times a minute.
pub async fn check(
    policy: pb::AgentEndpoints,
    account_id: &str,
    agent_id: &str,
    url: &str,
    secret: &str,
) -> Result<()> {
    {
        let mut checked = CHECKED.lock().unwrap_or_else(|p| p.into_inner());
        take_check(checked.get_or_insert_with(HashMap::new), account_id, now_ms())?;
    }
    let challenge = crate::auth::new_token();
    let body = delivery_json(agent_id, &challenge, vec![]);
    let answer = post(&CHECKS, policy, url, secret, &new_id(), body)
        .await
        .map_err(|why| Error::FailedPrecondition(format!("the endpoint didn't pass the check: it {why}")))?;
    match read_answer(&answer) {
        Some(answer) if answer.challenge == challenge => Ok(()),
        _ => Err(Error::FailedPrecondition(
            "the endpoint didn't pass the check: answer it with {\"challenge\": \"...\"} from the delivery".into(),
        )),
    }
}

/// Counts one check by `account_id` against [`CHECKS_PER_MINUTE`], in a
/// table of each account's minute and how many it's used.
fn take_check(counts: &mut HashMap<String, (i64, i64)>, account_id: &str, now: i64) -> Result<()> {
    let minute = now / 60_000;
    if counts.len() > 10_000 {
        counts.retain(|_, (at, _)| *at == minute);
    }
    let entry = counts.entry(account_id.to_string()).or_insert((minute, 0));
    if entry.0 != minute {
        *entry = (minute, 0);
    }
    if entry.1 >= CHECKS_PER_MINUTE {
        return Err(Error::ResourceExhausted("too many endpoint checks; try again in a minute".into()));
    }
    entry.1 += 1;
    Ok(())
}

// ── Delivering ──────────────────────────────────────────────────────────────

/// Starts delivering, for a process that keeps servers.
pub fn spawn_agent_deliveries(app: Arc<App>) {
    let tap = app.hub.agents_tap();
    let changes = app.agent_endpoint_changes();
    tokio::spawn(async move {
        let shutdown = app.shutdown.clone();
        tokio::select! {
            _ = shutdown.cancelled() => {}
            _ = Deliveries::new(app).run(tap, changes) => {}
        }
    });
}

/// An endpoint that's on, as deliveries need it.
struct Endpoint {
    agent: Account,
    url: String,
    /// Empty for every event.
    events: HashSet<String>,
    secret: String,
    epoch: i64,
    /// When the URL was set (unix ms): deliveries start from the events after.
    set_at: i64,
}

impl Endpoint {
    fn from_cluster(found: cpb::DeliveryEndpoint) -> Option<(String, Arc<Self>)> {
        let agent = crate::cluster::account_from_pb(found.agent?);
        let endpoint = Self {
            agent,
            url: found.url,
            events: found.events.into_iter().collect(),
            secret: found.secret,
            epoch: found.epoch,
            set_at: found.set_at,
        };
        Some((endpoint.agent.id.clone(), Arc::new(endpoint)))
    }
}

/// When an endpoint was looked up, and what it was: on (and how), or off.
type Remembered = (Instant, Option<Arc<Endpoint>>);

/// What this part remembers of agents' endpoints.
#[derive(Default)]
struct Known {
    endpoints: Mutex<HashMap<String, Remembered>>,
}

impl Known {
    /// The endpoints of these agents that are on, asking about any not
    /// remembered.
    async fn get(&self, app: &App, agent_ids: &[String]) -> Result<HashMap<String, Arc<Endpoint>>> {
        let mut found = HashMap::new();
        let mut missing = Vec::new();
        {
            let endpoints = self.endpoints.lock().unwrap_or_else(|p| p.into_inner());
            for id in agent_ids {
                match endpoints.get(id) {
                    Some((at, endpoint)) if at.elapsed() < REMEMBER_FOR => {
                        if let Some(endpoint) = endpoint {
                            found.insert(id.clone(), endpoint.clone());
                        }
                    }
                    _ => missing.push(id.clone()),
                }
            }
        }
        if missing.is_empty() {
            return Ok(found);
        }
        let fetched: HashMap<String, Arc<Endpoint>> =
            app.agent_endpoints(&missing).await?.into_iter().filter_map(Endpoint::from_cluster).collect();
        let mut endpoints = self.endpoints.lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();
        for id in missing {
            let endpoint = fetched.get(&id).cloned();
            if let Some(endpoint) = &endpoint {
                found.insert(id.clone(), endpoint.clone());
            }
            endpoints.insert(id, (now, endpoint));
        }
        Ok(found)
    }

    async fn one(&self, app: &App, agent_id: &str) -> Result<Option<Arc<Endpoint>>> {
        Ok(self.get(app, &[agent_id.to_string()]).await?.remove(agent_id))
    }

    /// Forgets an agent's endpoint, or every one for an empty id.
    fn forget(&self, agent_id: &str) {
        let mut endpoints = self.endpoints.lock().unwrap_or_else(|p| p.into_inner());
        if agent_id.is_empty() {
            endpoints.clear();
        } else {
            endpoints.remove(agent_id);
        }
    }
}

struct Worker {
    /// New events in its server.
    wake: Arc<Notify>,
    /// Its endpoint changed: try again now, whatever the backoff.
    kick: Arc<Notify>,
    task: JoinHandle<()>,
}

struct Deliveries {
    app: Arc<App>,
    known: Arc<Known>,
    /// Each server's agents, as last read; dropped when its members change.
    agents: HashMap<String, Vec<String>>,
    /// By (server, agent).
    workers: HashMap<(String, String), Worker>,
    /// Each agent's one delivery in flight, shared by its workers.
    lanes: HashMap<String, Arc<Semaphore>>,
}

impl Deliveries {
    fn new(app: Arc<App>) -> Self {
        Self { app, known: Arc::default(), agents: HashMap::new(), workers: HashMap::new(), lanes: HashMap::new() }
    }

    async fn run(mut self, mut tap: mpsc::Receiver<Arc<pb::Event>>, mut changes: broadcast::Receiver<Arc<str>>) {
        // Agents whose deliveries were behind when the instance stopped.
        for sdb in self.app.servers.all() {
            self.wake(&sdb).await;
        }
        let mut missed = tokio::time::interval(MISSED_EVERY);
        missed.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                event = tap.recv() => match event {
                    Some(event) => self.note(&event).await,
                    None => return,
                },
                _ = missed.tick() => self.wake_missed().await,
                changed = changes.recv() => match changed {
                    Ok(agent_id) => self.changed(&agent_id),
                    Err(broadcast::error::RecvError::Lagged(_)) => self.changed(""),
                    Err(broadcast::error::RecvError::Closed) => return,
                },
            }
        }
    }

    /// Wakes the servers whose events the tap had no room for.
    async fn wake_missed(&mut self) {
        for server_id in self.app.hub.agents_missed() {
            // Who's in it may have changed among what was missed.
            self.agents.remove(&server_id);
            if let Ok(sdb) = self.app.servers.get(&server_id).await {
                self.wake(&sdb).await;
            }
        }
    }

    fn changed(&mut self, agent_id: &str) {
        self.known.forget(agent_id);
        self.workers.retain(|_, worker| !worker.task.is_finished());
        let working: HashSet<&String> = self.workers.keys().map(|(_, agent)| agent).collect();
        self.lanes.retain(|agent, _| working.contains(agent));
        for ((_, agent), worker) in &self.workers {
            if agent_id.is_empty() || agent == agent_id {
                worker.kick.notify_one();
            }
        }
    }

    async fn note(&mut self, event: &pb::Event) {
        if crate::hub::is_moved(event) {
            return;
        }
        if matches!(event.payload, Some(Payload::MemberJoined(_) | Payload::MemberLeft(_) | Payload::ServerDeleted(_)))
        {
            self.agents.remove(&event.server_id);
        }
        let Ok(sdb) = self.app.servers.get(&event.server_id).await else { return };
        self.wake(&sdb).await;
    }

    /// Wakes the workers of a server's agents with endpoints, starting any
    /// that aren't running.
    async fn wake(&mut self, sdb: &Arc<ServerDb>) {
        let agents = match self.agents.get(&sdb.id) {
            Some(agents) => agents.clone(),
            None => {
                let Ok(agents) = agents_in(sdb).await else { return };
                self.agents.insert(sdb.id.clone(), agents.clone());
                agents
            }
        };
        if agents.is_empty() {
            return;
        }
        let Ok(endpoints) = self.known.get(&self.app, &agents).await else {
            tracing::info!("couldn't look agents' endpoints up; trying with the next event");
            return;
        };
        for agent_id in endpoints.into_keys() {
            let key = (sdb.id.clone(), agent_id);
            if let Some(worker) = self.workers.get(&key)
                && !worker.task.is_finished()
            {
                worker.wake.notify_one();
                continue;
            }
            let (wake, kick) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
            let lane = self.lanes.entry(key.1.clone()).or_insert_with(|| Arc::new(Semaphore::new(1))).clone();
            let run = Run {
                lane,
                app: self.app.clone(),
                known: self.known.clone(),
                server_id: key.0.clone(),
                agent_id: key.1.clone(),
                wake: wake.clone(),
                kick: kick.clone(),
            };
            self.workers.insert(key, Worker { wake, kick, task: tokio::spawn(run.run()) });
        }
    }
}

/// The agents among a server's members.
async fn agents_in(sdb: &ServerDb) -> Result<Vec<String>> {
    let conn = sdb.read()?;
    query_all(
        &conn,
        "SELECT members.user_id FROM members JOIN users ON users.id = members.user_id WHERE users.kind = ?1",
        [pb::AccountKind::Agent as i64],
        |r| r.get::<String>(0),
    )
    .await
}

/// One agent's deliveries from one server.
struct Run {
    app: Arc<App>,
    known: Arc<Known>,
    server_id: String,
    agent_id: String,
    wake: Arc<Notify>,
    kick: Arc<Notify>,
    /// The agent's one delivery in flight, shared with its other servers' workers.
    lane: Arc<Semaphore>,
}

/// A batch read from the log, ready to post.
struct Batch {
    events: Vec<DynamicMessage>,
    /// The first event's id, the delivery's `webhook-id`.
    id: String,
    /// The last sequence it takes the agent past.
    through: i64,
    /// Interactions in it, by id, with their channels, for replies.
    interactions: HashMap<String, String>,
    /// The agent left the server (or it was deleted) in it.
    gone: bool,
}

impl Run {
    async fn run(self) {
        let mut backoff = FIRST_RETRY;
        let mut view: Option<(i64, View)> = None;
        // The last report to the directory, and whether it was a failure.
        let mut reported: Option<(Instant, bool)> = None;
        loop {
            if self.app.shutdown.is_cancelled() {
                return;
            }
            let endpoint = match self.known.one(&self.app, &self.agent_id).await {
                Ok(Some(endpoint)) => endpoint,
                Ok(None) => return,
                Err(_) => {
                    self.pause(IDLE_CHECK).await;
                    continue;
                }
            };
            let Ok(sdb) = self.app.servers.get(&self.server_id).await else { return };
            let policy = self.app.settings().agent_endpoints;
            if policy == pb::AgentEndpoints::Off {
                // Waits where it is for the instance to send again.
                self.pause(IDLE_CHECK).await;
                continue;
            }
            let Ok(cursor) = self.cursor(&sdb, &endpoint).await else {
                self.pause(backoff).await;
                continue;
            };
            if view.as_ref().is_none_or(|(epoch, _)| *epoch != endpoint.epoch) {
                view = match sdb.member_access(&self.agent_id).await {
                    Ok(Some((member, access))) => {
                        Some((endpoint.epoch, View::new(sdb.clone(), &self.agent_id, member, access)))
                    }
                    Ok(None) => {
                        self.forget(&sdb).await;
                        return;
                    }
                    Err(_) => {
                        self.pause(backoff).await;
                        continue;
                    }
                };
            }
            let Some((_, current)) = view.as_mut() else { continue };
            let batch = match self.read(&sdb, current, &endpoint, cursor).await {
                Ok(Some(batch)) => batch,
                Ok(None) => {
                    tokio::select! {
                        _ = self.wake.notified() => {}
                        _ = self.kick.notified() => {}
                        _ = tokio::time::sleep(IDLE_CHECK) => {}
                        _ = self.app.shutdown.cancelled() => return,
                    }
                    continue;
                }
                Err(_) => {
                    // What the agent can see couldn't be worked out: start over.
                    view = None;
                    self.pause(backoff).await;
                    continue;
                }
            };
            if batch.events.is_empty() {
                if self.advance(&sdb, endpoint.epoch, batch.through).await.is_err() {
                    self.pause(backoff).await;
                }
                if batch.gone {
                    self.forget(&sdb).await;
                    return;
                }
                continue;
            }
            let body = delivery_json(&self.agent_id, "", batch.events);
            let posted = tokio::select! {
                turn = self.lane.acquire() => match turn {
                    Ok(_turn) => post(&IN_FLIGHT, policy, &endpoint.url, &endpoint.secret, &batch.id, body).await,
                    Err(_) => return,
                },
                _ = self.app.shutdown.cancelled() => return,
            };
            match posted {
                Ok(answer) => {
                    backoff = FIRST_RETRY;
                    if self.advance(&sdb, endpoint.epoch, batch.through).await.is_err() {
                        // Delivered again later: agents skip what they've had.
                        self.pause(FIRST_RETRY).await;
                    }
                    if reported.is_none_or(|(at, failed)| failed || at.elapsed() >= REPORT_EVERY) {
                        reported = Some((Instant::now(), false));
                        let _ = self.app.report_agent_delivery(&self.agent_id, endpoint.epoch, "").await;
                    }
                    self.reply(&endpoint.agent, &batch.interactions, &answer).await;
                    if batch.gone {
                        self.forget(&sdb).await;
                        return;
                    }
                }
                Err(why) => {
                    if reported.is_none_or(|(at, failed)| !failed || at.elapsed() >= REPORT_EVERY) {
                        reported = Some((Instant::now(), true));
                        if let Ok(true) = self.app.report_agent_delivery(&self.agent_id, endpoint.epoch, &why).await {
                            self.known.forget(&self.agent_id);
                            return;
                        }
                    }
                    if self.pause(backoff).await {
                        backoff = FIRST_RETRY;
                    } else {
                        backoff = (backoff * 2).min(LAST_RETRY);
                    }
                }
            }
        }
    }

    /// Waits a while, or until the endpoint changes (true) or the instance stops.
    async fn pause(&self, wait: Duration) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(wait) => false,
            _ = self.kick.notified() => true,
            _ = self.app.shutdown.cancelled() => false,
        }
    }

    /// Where the agent stands in the log for this epoch of its endpoint. A
    /// new place starts after the last event before the endpoint was set or
    /// the agent joined, whichever is later, so it hears of what came since
    /// (its own arrival included) and nothing older.
    async fn cursor(&self, sdb: &ServerDb, endpoint: &Endpoint) -> Result<i64> {
        let epoch = endpoint.epoch;
        let conn = sdb.read()?;
        let stored = crate::db::query_one(
            &conn,
            "SELECT epoch, sequence FROM agent_deliveries WHERE agent_id = ?1",
            [self.agent_id.as_str()],
            |r| Ok((r.get::<i64>(0)?, r.get::<i64>(1)?)),
        )
        .await?;
        if let Some((stored_epoch, sequence)) = stored
            && stored_epoch == epoch
        {
            return Ok(sequence);
        }
        let joined_at = crate::db::query_one(
            &conn,
            "SELECT joined_at FROM members WHERE user_id = ?1",
            [self.agent_id.as_str()],
            |r| r.get::<i64>(0),
        )
        .await?;
        let from = match joined_at {
            Some(joined_at) => crate::db::query_one(
                &conn,
                "SELECT sequence FROM events WHERE created_at < ?1 ORDER BY sequence DESC LIMIT 1",
                [endpoint.set_at.max(joined_at)],
                |r| r.get::<i64>(0),
            )
            .await?
            .unwrap_or(0),
            None => sdb.head_sequence().await?,
        };
        let agent_id = self.agent_id.clone();
        sdb.write_quiet(async move |conn| {
            conn.execute(
                "INSERT INTO agent_deliveries (agent_id, epoch, sequence) VALUES (?1, ?2, ?3)
                 ON CONFLICT (agent_id) DO UPDATE SET epoch = excluded.epoch, sequence = excluded.sequence",
                (agent_id.as_str(), epoch, from),
            )
            .await?;
            Ok(())
        })
        .await?;
        Ok(from)
    }

    async fn advance(&self, sdb: &ServerDb, epoch: i64, through: i64) -> Result<()> {
        let agent_id = self.agent_id.clone();
        sdb.write_quiet(async move |conn| {
            conn.execute(
                "UPDATE agent_deliveries SET sequence = ?3 WHERE agent_id = ?1 AND epoch = ?2 AND sequence < ?3",
                (agent_id.as_str(), epoch, through),
            )
            .await?;
            Ok(())
        })
        .await
    }

    /// The agent left the server: its place goes too.
    async fn forget(&self, sdb: &ServerDb) {
        let agent_id = self.agent_id.clone();
        let forgot = sdb
            .write_quiet(async move |conn| {
                conn.execute("DELETE FROM agent_deliveries WHERE agent_id = ?1", [agent_id.as_str()]).await?;
                Ok(())
            })
            .await;
        if forgot.is_err() {
            tracing::info!("couldn't forget where an agent that left stood");
        }
    }

    /// The next events for the agent after `cursor`, through its view and
    /// its endpoint's choice of events. None when there are none yet.
    async fn read(
        &self,
        sdb: &ServerDb,
        view: &mut View,
        endpoint: &Endpoint,
        cursor: i64,
    ) -> Result<Option<Batch>, tonic::Status> {
        let page = sdb.events_after(cursor, READ_PAGE).await?;
        if page.is_empty() {
            return Ok(None);
        }
        let mut batch =
            Batch { events: vec![], id: String::new(), through: cursor, interactions: HashMap::new(), gone: false };
        for event in &page {
            let passed = view.pass(event).await?;
            batch.through = event.sequence;
            for event in passed {
                if batch.id.is_empty() {
                    batch.id = event.id.clone();
                }
                match &event.payload {
                    Some(Payload::MemberLeft(left)) if left.user_id == self.agent_id => batch.gone = true,
                    Some(Payload::ServerDeleted(_)) => batch.gone = true,
                    Some(Payload::InteractionCreated(pb::InteractionCreated { interaction: Some(i) })) => {
                        batch.interactions.insert(i.id.clone(), i.channel_id.clone());
                    }
                    _ => {}
                }
                let json = dynamic("fuwa.v1.Event", &event);
                let wanted = endpoint.events.is_empty()
                    || payload_name(&json).is_some_and(|name| endpoint.events.contains(&name));
                if wanted {
                    batch.events.push(json);
                }
            }
            if batch.gone || batch.events.len() >= MAX_EVENTS {
                break;
            }
        }
        Ok(Some(batch))
    }

    /// Sends the replies an answer carried, for interactions it was given.
    async fn reply(&self, agent: &Account, interactions: &HashMap<String, String>, answer: &[u8]) {
        let Some(answer) = read_answer(answer) else {
            tracing::info!("an agent's endpoint answered with something that isn't an AgentDeliveryAnswer");
            return;
        };
        let api = Api::new(self.app.clone());
        for reply in answer.replies.into_iter().take(MAX_EVENTS) {
            let Some(channel_id) = interactions.get(&reply.interaction_id) else { continue };
            let request = pb::SendMessageRequest {
                server_id: self.server_id.clone(),
                channel_id: channel_id.clone(),
                content: reply.content,
                embeds: reply.embeds,
                components: reply.components,
                interaction_id: reply.interaction_id,
                ..Default::default()
            };
            if api.send_as(agent.clone(), request).await.is_err() {
                tracing::info!("a reply from an agent's endpoint was refused");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_are_paced_by_the_account_and_the_minute() {
        let mut counts = HashMap::new();
        let minute = 60_000 * 1000;
        for _ in 0..CHECKS_PER_MINUTE {
            take_check(&mut counts, "rin", minute).unwrap();
        }
        assert!(matches!(take_check(&mut counts, "rin", minute + 59_999), Err(Error::ResourceExhausted(_))));
        take_check(&mut counts, "mika", minute).expect("each account has its own");
        take_check(&mut counts, "rin", minute + 60_000).expect("a new minute starts over");
    }

    #[test]
    fn signatures_follow_standard_webhooks() {
        // The example from the Standard Webhooks spec's reference libraries.
        let secret = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
        let body = br#"{"test": 2432232314}"#;
        assert_eq!(
            signature(secret, "msg_p5jXN8AQM9LWM0D4loKWxJek", 1614265330, body),
            "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE="
        );
    }

    #[test]
    fn endpoints_keep_to_the_instances_setting() {
        let public = pb::AgentEndpoints::Public;
        assert!(check_url(public, "https://agent.example.com/fuwa").is_ok());
        assert!(check_url(public, "http://agent.example.com/fuwa").is_err());
        assert!(check_url(public, "https://127.0.0.1/").is_err());
        assert!(check_url(public, "https://10.0.0.8/").is_err());
        assert!(check_url(public, "https://localhost:8080/").is_err());
        assert!(check_url(public, "https://box.internal/").is_err());
        assert!(check_url(public, "https://user:pass@agent.example.com/").is_err());
        assert!(check_url(public, "ftp://agent.example.com/").is_err());
        let any = pb::AgentEndpoints::Any;
        assert!(check_url(any, "http://127.0.0.1:3000/").is_ok());
        assert!(check_url(any, "ftp://127.0.0.1/").is_err());
        assert!(check_url(pb::AgentEndpoints::Off, "https://agent.example.com/").is_err());
    }

    #[test]
    fn events_are_named_and_written_as_proto3_json() {
        let names = event_names();
        assert!(names.contains(&"message_created".to_string()));
        assert!(names.contains(&"interaction_created".to_string()));
        let event = pb::Event {
            id: "e1".into(),
            server_id: "s1".into(),
            sequence: 7,
            payload: Some(Payload::MessageDeleted(pb::MessageDeleted {
                channel_id: "c1".into(),
                message_id: "m1".into(),
            })),
            ..Default::default()
        };
        let json = dynamic("fuwa.v1.Event", &event);
        assert_eq!(payload_name(&json).as_deref(), Some("message_deleted"));
        let body: serde_json::Value = serde_json::from_slice(&delivery_json("a1", "", vec![json])).unwrap();
        assert_eq!(body["agentId"], "a1");
        assert_eq!(body["events"][0]["sequence"], "7");
        assert_eq!(body["events"][0]["messageDeleted"]["messageId"], "m1");
    }

    #[test]
    fn answers_are_read_loosely() {
        assert_eq!(read_answer(b"").unwrap(), pb::AgentDeliveryAnswer::default());
        let answer = read_answer(
            br#"{"challenge":"x","replies":[{"interactionId":"i1","content":"hi","whatever":1}],"later":true}"#,
        )
        .unwrap();
        assert_eq!(answer.challenge, "x");
        assert_eq!(answer.replies[0].interaction_id, "i1");
        assert_eq!(answer.replies[0].content, "hi");
        assert!(read_answer(b"not json").is_none());
    }
}
