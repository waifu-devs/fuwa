//! Encrypted direct messages and secure channels for one signed-in account
//! on one instance: this app's device, kept in the vault, following every
//! conversation and every secure channel it's in. A port of the web app's
//! `web/src/e2ee/engine.ts`, on `fuwa-e2ee` directly (no WebAssembly);
//! `docs/e2ee.md` and `docs/secure-channels.md` have the design.
//!
//! Both are MLS groups whose records the instance keeps the same way, so
//! everything here works on a [`Room`]: a conversation (DirectMessageService)
//! or a secure channel in a server (SecureChannelService).
//!
//! Each conversation is an MLS group of every device of both people. The
//! instance keeps its records in one order and takes one only for the
//! group's current epoch, so every device reads the same history: commits
//! (devices joining or leaving) and messages. This device reads them in order
//! and keeps what they said. Before sending, it makes sure the group holds
//! exactly the devices both people are signed in on, adding new ones with the
//! key packages they published and dropping signed-out ones. A device that
//! wasn't added (a new sign-in) joins by itself from the group's public state.
//!
//! Everything that touches the device runs under one lock, so the device and
//! its vault never disagree.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuwa_e2ee::{Device, Error as E2eeError, Member, Processed};
use prost::Message as _;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tonic::Code;

use crate::core::Shared;
use crate::core::api::{Api, Problem};
use crate::core::calls;
use crate::core::history::{self, Candidate, LogKind, Logged};
use crate::core::secure_threads;
use crate::core::vault::{Change, DeviceRef, Item, ItemKind, Note, Signed, Vault, VoiceFile, sha256_hex};
use crate::pb;
use crate::rpc;

/// Key packages a new device publishes, and when to publish more.
const KEY_PACKAGES: usize = 30;
const LOW_ON_KEY_PACKAGES: i32 = 10;
/// Records read at a time while catching up.
const PAGE: i32 = 200;
/// The server sends a heartbeat every 25 seconds; this long without anything means the stream is gone.
const SILENCE: Duration = Duration::from_secs(70);
/// The longest message, in characters, as the composer allows.
pub const MAX_DM: usize = 4000;
/// The most people or devices one call asks the instance about.
const LOOKUPS: usize = 100;
/// The most shared history one device passes on at once: the newest messages that fit in one encrypted message.
const HISTORY_BYTES: usize = 56_000;
const HISTORY_ENTRIES: usize = 500;
/// Pages of the log a device reads to check shared history against: enough for what one share carries.
const LOG_PAGES: usize = 10;
/// How long a device waits, at most, before taking people who lost access out of a secure channel.
const ACCESS_SETTLE_MS: f64 = 2500.0;
/// How much longer a device that joined a secure channel after it started waits, so one with more of its history goes first.
const LATE_SETTLE_MS: f64 = 3500.0;

/// Why a secure channel can't be read or written: its group can't be followed
/// any more (a change to it no device could read), until someone with Manage
/// Channels starts its encryption over.
pub const SECURE_BROKEN: &str = "This channel's encryption can't be followed any more: a change to its keys couldn't be read. Someone who can manage the channel can start it over.";

/// Where direct messages stand on an instance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DmStatus {
    #[default]
    Off,
    Starting,
    Ready,
    Failed,
}

/// A device in an encrypted conversation, as its group says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmMember {
    pub user_id: String,
    pub device_id: String,
    pub signature_key: Vec<u8>,
}

/// Encrypted direct messages on one instance, as this app's device sees them.
#[derive(Debug, Clone, Default)]
pub struct DmState {
    pub status: DmStatus,
    pub problem: Option<String>,
    /// This app's device for the account.
    pub device_id: String,
    /// The latest first.
    pub conversations: Vec<pb::Conversation>,
    /// What each conversation said, as this device opened it, oldest first.
    pub items: HashMap<String, Vec<Item>>,
    pub unread: HashMap<String, u32>,
    /// Every device in each conversation's group.
    pub members: HashMap<String, Vec<DmMember>>,
    /// The safety number as it is now, per conversation.
    pub safety: HashMap<String, String>,
    /// The safety number you checked with the other person.
    pub verified: HashMap<String, String>,
    /// Why you can't send in a conversation right now, if you can't.
    pub blocked: HashMap<String, String>,
    /// Conversations this device is still joining.
    pub joining: HashSet<String>,
    /// Messages on their way, per conversation, and those that couldn't go.
    pub sending: HashMap<String, Vec<DmPending>>,
    /// The calls going on, per conversation.
    pub calls: HashMap<String, pb::DmCall>,
    /// Per secure channel: whether earlier messages are passed on to people added later.
    pub secure_history: HashMap<String, bool>,
    /// Your reactions on their way, shown before they're read back (`reactions`).
    pub reacting: Vec<crate::core::reactions::PendingReaction>,
    /// Per conversation, once opened: its pins (`pins`).
    pub pins: HashMap<String, crate::core::pins::DmPinList>,
    /// Per secure channel: the threads you follow or stopped following, and
    /// what you've seen in each (`secure_threads.rs`).
    pub thread_notes: HashMap<String, Note>,
}

/// A message being encrypted and sent, or one that couldn't be (the web's `PendingMessage`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmPending {
    pub nonce: u64,
    pub text: String,
    pub created_at: i64,
    /// Why it didn't go, once it didn't.
    pub failed: Option<String>,
    /// In a secure channel: the thread it replies in (0: none), and whether it goes to the channel too.
    pub thread: i64,
    pub in_channel: bool,
}

/// A number for a message on its way, unique while the app runs.
pub fn new_nonce() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Something a person can be told about why sending didn't work.
#[derive(Debug, Clone)]
pub struct DmError(pub String);

impl std::fmt::Display for DmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DmError {}

impl From<Problem> for DmError {
    fn from(p: Problem) -> Self {
        Self(p.message)
    }
}

impl From<E2eeError> for DmError {
    fn from(e: E2eeError) -> Self {
        Self(format!("Encryption problem: {e}"))
    }
}

impl From<std::io::Error> for DmError {
    fn from(e: std::io::Error) -> Self {
        Self(format!("Couldn't keep encrypted messages on this computer: {e}"))
    }
}

type Result<T, E = DmError> = std::result::Result<T, E>;

/// The plaintext of a message: what only the conversation's devices see.
#[derive(Debug, Clone)]
pub enum Content {
    Text {
        text: String,
        reply_to: i64,
    },
    Edit {
        sequence: i64,
        text: String,
    },
    /// A voice message whose sealed file is already uploaded.
    Voice(pb::DirectMessageVoice),
    /// A text with files, sealed and uploaded already (`core/sealed_files.rs`).
    Files {
        text: String,
        files: Vec<pb::SealedFile>,
    },
    /// A secure channel's thread reply (with files, if any), and whether it goes to the channel too.
    Reply {
        text: String,
        thread: i64,
        in_channel: bool,
        files: Vec<pb::SealedFile>,
    },
    /// A moderator locking or unlocking the thread under a secure channel's message.
    Lock {
        parent: i64,
        locked: bool,
    },
    /// Reacting to the message at `sequence` with a standard emoji, or (`removed`) taking it off.
    React {
        sequence: i64,
        emoji: String,
        removed: bool,
    },
}

impl Content {
    /// The uploaded files a message names, which the instance ties to it.
    fn media_ids(&self) -> Vec<String> {
        match self {
            Content::Voice(v) => v.file.iter().map(|f| f.media_id.clone()).collect(),
            Content::Files { files, .. } | Content::Reply { files, .. } => {
                files.iter().map(|f| f.media_id.clone()).collect()
            }
            _ => Vec::new(),
        }
    }
}

fn content_of(content: &Content) -> pb::DirectMessageContent {
    use pb::direct_message_content::Body;
    let body = match content {
        Content::Text { text, reply_to } => Body::Text(pb::DirectMessageText {
            content: text.clone(),
            reply_to_sequence: *reply_to,
            ..Default::default()
        }),
        Content::Edit { sequence, text } => {
            Body::Edit(pb::DirectMessageEdit { sequence: *sequence, content: text.clone() })
        }
        Content::Voice(voice) => Body::Voice(voice.clone()),
        Content::Files { text, files } => {
            Body::Text(pb::DirectMessageText { content: text.clone(), files: files.clone(), ..Default::default() })
        }
        Content::Reply { text, thread, in_channel, files } => Body::Text(pb::DirectMessageText {
            content: text.clone(),
            thread_sequence: *thread,
            in_channel: *in_channel,
            files: files.clone(),
            ..Default::default()
        }),
        Content::Lock { parent, locked } => {
            Body::Thread(pb::ThreadChange { parent_sequence: *parent, locked: *locked })
        }
        Content::React { sequence, emoji, removed } => {
            Body::Reaction(pb::DirectMessageReaction { sequence: *sequence, emoji: emoji.clone(), removed: *removed })
        }
    };
    pb::DirectMessageContent { body: Some(body) }
}

/// A received voice message's file, when it's one that can be fetched and
/// opened: an instance media id, a 32-byte key and digest, a sensible size,
/// and sealed in one piece as voice messages are.
fn voice_file(voice: &pb::DirectMessageVoice) -> Option<VoiceFile> {
    let file = voice.file.as_ref()?;
    let id_ok =
        file.media_id.len() == 26 && file.media_id.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase());
    let sized = file.size > 0 && file.size <= 256 * 1024 * 1024;
    (id_ok && sized && file.chunk_bytes == 0 && file.key.len() == 32 && file.sha256.len() == 32).then(|| VoiceFile {
        media_id: file.media_id.clone(),
        key: file.key.clone(),
        sha256: file.sha256.clone(),
        size: file.size,
        duration_ms: voice.duration_ms.min(24 * 60 * 60 * 1000),
        waveform: voice.waveform.iter().take(crate::core::voice_notes::MAX_BARS).copied().collect(),
    })
}

/// A thread reply's place, from what its sender wrote (nothing for a line that isn't one).
fn thread_fields(item: &mut Item, text: &pb::DirectMessageText) {
    if text.thread_sequence > 0 {
        item.thread = text.thread_sequence;
        item.in_channel = text.in_channel;
    }
}

/// A thread lock change, as an item.
fn lock_item(seq: i64, at: i64, sender: &str, device: &str, change: &pb::ThreadChange) -> Item {
    let mut item = Item::new(seq, ItemKind::Thread, at, sender, device);
    item.thread = change.parent_sequence;
    item.content = if change.locked { "locked" } else { "unlocked" }.into();
    item
}

fn encode(content: &Content) -> Vec<u8> {
    content_of(content).encode_to_vec()
}

fn ts_ms(t: Option<&prost_types::Timestamp>) -> i64 {
    t.map(|t| t.seconds.saturating_mul(1000).saturating_add(i64::from(t.nanos) / 1_000_000)).unwrap_or_else(now_ms)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn clip(text: &str) -> String {
    text.chars().take(MAX_DM).collect()
}

fn is_precondition(p: &Problem) -> bool {
    p.code == Code::FailedPrecondition
}

fn unique(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    ids.into_iter().filter(|id| !id.is_empty() && seen.insert(id.clone())).collect()
}

/// What kind of record it is: a conversation's and a secure channel's share
/// their numbers for commits and messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecKind {
    Commit,
    Message,
    /// A secure channel's encryption started over.
    Reset,
    /// A secure channel's earlier messages, passed on.
    History,
    /// A secure channel's history sharing turned on or off.
    Settings,
    Other,
}

/// One entry in a group's log: a direct message's ConversationRecord, or a secure channel's SecureRecord.
#[derive(Debug, Clone)]
struct Rec {
    sequence: i64,
    kind: RecKind,
    sender_id: String,
    sender_device_id: String,
    data: Vec<u8>,
    at: i64,
    /// For a settings record: whether sharing is on from here.
    share_history: bool,
}

impl From<pb::ConversationRecord> for Rec {
    fn from(r: pb::ConversationRecord) -> Self {
        let kind = match pb::ConversationRecordKind::try_from(r.kind) {
            Ok(pb::ConversationRecordKind::Commit) => RecKind::Commit,
            Ok(pb::ConversationRecordKind::Message) => RecKind::Message,
            _ => RecKind::Other,
        };
        Self {
            sequence: r.sequence,
            kind,
            at: ts_ms(r.created_at.as_ref()),
            sender_id: r.sender_id,
            sender_device_id: r.sender_device_id,
            data: r.data,
            share_history: false,
        }
    }
}

impl From<pb::SecureRecord> for Rec {
    fn from(r: pb::SecureRecord) -> Self {
        use pb::SecureRecordKind as K;
        let kind = match K::try_from(r.kind) {
            Ok(K::Commit) => RecKind::Commit,
            Ok(K::Message) => RecKind::Message,
            Ok(K::Reset) => RecKind::Reset,
            Ok(K::History) => RecKind::History,
            Ok(K::Settings) => RecKind::Settings,
            _ => RecKind::Other,
        };
        Self {
            sequence: r.sequence,
            kind,
            at: ts_ms(r.created_at.as_ref()),
            sender_id: r.sender_id,
            sender_device_id: r.sender_device_id,
            data: r.data,
            share_history: r.share_history,
        }
    }
}

/// A secure channel this device follows.
#[derive(Debug, Clone, Default)]
struct SecureChannel {
    server_id: String,
    member_ids: Vec<String>,
    share_history: bool,
}

/// One MLS group this device takes part in, and how to reach it: a direct
/// message conversation or a secure channel in a community server. Both keep
/// their records the same way, so everything else here is the same for both.
#[derive(Debug, Clone)]
enum Room {
    Dm(pb::Conversation),
    Channel {
        id: String,
        server_id: String,
        /// Who may show up in the group's history: someone added earlier
        /// may have lost access since, and the group still names them.
        allowed: Vec<String>,
    },
}

impl Room {
    fn id(&self) -> &str {
        match self {
            Room::Dm(c) => &c.id,
            Room::Channel { id, .. } => id,
        }
    }

    /// The server, for a secure channel.
    fn server(&self) -> Option<&str> {
        match self {
            Room::Dm(_) => None,
            Room::Channel { server_id, .. } => Some(server_id),
        }
    }

    fn allowed(&self) -> Vec<String> {
        match self {
            Room::Dm(c) => c.users.iter().map(|u| u.id.clone()).collect(),
            Room::Channel { allowed, .. } => allowed.clone(),
        }
    }
}

/// What opening a record found.
enum Opened {
    Fine,
    /// The device lost track of the group and has to join again.
    Rejoin,
    /// A secure channel's encryption started over.
    Reset,
}

/// The device and its vault: only touched under the engine's lock.
pub(crate) struct Inner {
    device: Device,
    pub(crate) vault: Vault,
}

impl Inner {
    fn save(&mut self, mut change: Change) -> Result<()> {
        change.device = Some(self.device.save());
        Ok(self.vault.write(change)?)
    }

    /// An item as this read has it so far: from the change being made, else the vault.
    fn known(&mut self, change: &Change, id: &str, seq: i64) -> Result<Option<Item>> {
        if let Some((_, item)) = change.items.iter().rev().find(|(c, i)| c == id && i.seq == seq) {
            return Ok(Some(item.clone()));
        }
        Ok(self.vault.items(id)?.iter().find(|i| i.seq == seq).cloned())
    }
}

/// What a signed message says, once its signature checks out against the device that signed it.
struct OpenedSigned {
    payload: pb::SignedPayload,
    signed: Signed,
}

pub struct DmEngine {
    pub(crate) key: String,
    pub(crate) api: Api,
    pub(crate) me: pb::User,
    shared: Shared,
    device_id: String,
    pub(crate) inner: Mutex<Inner>,
    conversations: parking_lot::Mutex<HashMap<String, pb::Conversation>>,
    /// Secure channels in servers, by channel id: followed once opened, or once a record arrives.
    secure: parking_lot::Mutex<HashMap<String, SecureChannel>>,
    /// Servers whose secure channels wait to be brought in step with a permission change.
    settling: parking_lot::Mutex<HashSet<String>>,
    /// Catch-ups waiting their turn, so a burst of records reads each room once.
    queued: parking_lot::Mutex<HashSet<String>>,
    stop: CancellationToken,
}

impl DmEngine {
    /// Loads (or makes) this account's device and registers it with the
    /// instance. A new sign-in is a new session, so it gets a new device, and
    /// whatever the old one kept here goes.
    pub async fn start(
        key: &str,
        api: Api,
        me: pb::User,
        token: &str,
        vaults: PathBuf,
        vault_key: [u8; 32],
        shared: Shared,
    ) -> Result<Arc<Self>> {
        let session = sha256_hex(token.as_bytes());
        let dir = Vault::dir_for(&vaults, key, &me.id);
        let mut vault = tokio::task::spawn_blocking(move || Vault::open(dir, &session, vault_key))
            .await
            .map_err(|e| DmError(e.to_string()))??;
        let stored = vault.device();
        let device = match &stored {
            Some(bytes) => Device::restore(bytes)?,
            None => Device::new(&me.id)?,
        };
        let (packages, last_resort) = match stored {
            Some(_) => (Vec::new(), Vec::new()),
            None => (device.key_packages(KEY_PACKAGES)?, device.last_resort_key_package()?),
        };
        // Kept before the instance hands any of them out.
        vault.write(Change { device: Some(device.save()), ..Change::default() })?;
        let registered = rpc!(
            api.dms(),
            register_device(pb::RegisterDeviceRequest {
                signature_key: device.signature_key().to_vec(),
                key_packages: packages,
                last_resort_key_package: last_resort,
            })
        )
        .await?;
        let mut inner = Inner { device, vault };
        if registered.key_packages < LOW_ON_KEY_PACKAGES {
            let more = inner.device.key_packages(KEY_PACKAGES)?;
            inner.save(Change::default())?;
            rpc!(api.dms(), add_key_packages(pb::AddKeyPackagesRequest { key_packages: more })).await?;
        }
        let device_id = inner.device.device_id();
        Ok(Arc::new(Self {
            key: key.to_owned(),
            api,
            me,
            shared,
            device_id,
            inner: Mutex::new(inner),
            conversations: parking_lot::Mutex::new(HashMap::new()),
            secure: parking_lot::Mutex::new(HashMap::new()),
            settling: parking_lot::Mutex::new(HashSet::new()),
            queued: parking_lot::Mutex::new(HashSet::new()),
            stop: CancellationToken::new(),
        }))
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn stop(&self) {
        self.stop.cancel();
    }

    pub(crate) fn stopped(&self) -> bool {
        self.stop.is_cancelled()
    }

    fn update(&self, f: impl FnOnce(&mut DmState)) {
        self.shared.instance(&self.key, |i| f(&mut i.dms));
    }

    /// Follows the account's conversations until stopped, reconnecting with backoff.
    pub async fn follow(self: Arc<Self>) {
        let mut delay = Duration::from_millis(400);
        while !self.stopped() {
            let result = tokio::select! {
                result = self.watch_once() => result,
                () = self.stop.cancelled() => return,
            };
            match result {
                Ok(()) => {}
                Err(problem) if problem.signed_out() => return,
                Err(problem) if problem.code == Code::Unimplemented => {
                    self.update(|d| d.problem = Some("This instance doesn't have direct messages yet.".into()));
                    return;
                }
                Err(problem) => self.update(|d| d.problem = Some(problem.message.clone())),
            }
            let jitter = 0.75 + rand_unit() / 2.0;
            tokio::select! {
                () = tokio::time::sleep(delay.mul_f64(jitter)) => {}
                () = self.stop.cancelled() => return,
            }
            delay = (delay * 2).min(Duration::from_secs(20));
        }
    }

    async fn watch_once(self: &Arc<Self>) -> Result<(), Problem> {
        let mut stream = self.api.dms().watch(pb::WatchRequest {}).await.map_err(Problem::from)?.into_inner();
        loop {
            let next = tokio::time::timeout(SILENCE, stream.message())
                .await
                .map_err(|_| Problem::new(Code::Unavailable, "lost the connection"))?
                .map_err(Problem::from)?;
            let Some(res) = next else { return Ok(()) };
            if res.ready {
                self.update(|d| d.problem = None);
                if self.resync().await.is_err() {
                    tracing::warn!("couldn't list conversations");
                }
            }
            if let Some(event) = res.event.and_then(|e| e.payload) {
                self.on_event(event);
            }
        }
    }

    /// Lists the conversations again and catches up on each.
    pub async fn resync(self: &Arc<Self>) -> Result<()> {
        let list = rpc!(self.api.dms(), list_conversations(pb::ListConversationsRequest {})).await?.conversations;
        self.set_conversations(list.clone());
        for c in list {
            let (note, member) = {
                let inner = self.inner.lock().await;
                (inner.vault.note(&c.id), inner.device.is_member(&c.id))
            };
            if c.last_sequence > note.cursor || !member {
                self.queue(c.id.clone());
            } else {
                self.refresh(&c.id).await;
            }
        }
        // Instances from before calls have none going.
        let going = rpc!(self.api.calls(), list_dm_calls(pb::ListDmCallsRequest {})).await.map(|r| r.calls);
        self.update(|s| {
            s.calls = going.unwrap_or_default().into_iter().map(|c| (c.conversation_id.clone(), c)).collect()
        });
        Ok(())
    }

    fn set_conversations(&self, list: Vec<pb::Conversation>) {
        let sorted = {
            let mut all = self.conversations.lock();
            for c in list {
                all.insert(c.id.clone(), c);
            }
            let mut sorted: Vec<pb::Conversation> = all.values().cloned().collect();
            let at = |c: &pb::Conversation| c.updated_at.as_ref().map(|t| (t.seconds, t.nanos)).unwrap_or_default();
            sorted.sort_by(|a, b| at(b).cmp(&at(a)).then_with(|| b.id.cmp(&a.id)));
            sorted
        };
        self.update(|d| d.conversations = sorted);
    }

    pub fn conversation(&self, id: &str) -> Option<pb::Conversation> {
        self.conversations.lock().get(id).cloned()
    }

    /// A conversation someone just opened, as the instance answered.
    pub fn add(self: &Arc<Self>, conversation: pb::Conversation) {
        let id = conversation.id.clone();
        self.set_conversations(vec![conversation]);
        self.queue(id);
    }

    fn on_event(self: &Arc<Self>, event: pb::direct_message_event::Payload) {
        use pb::direct_message_event::Payload;
        match event {
            Payload::ConversationOpened(c) => self.add(c),
            Payload::RecordAdded(record) => {
                if let Some(mut c) = self.conversation(&record.conversation_id) {
                    c.last_sequence = record.sequence;
                    c.updated_at = record.created_at;
                    self.set_conversations(vec![c]);
                }
                self.queue(record.conversation_id);
            }
            Payload::RecordDeleted(record) => {
                // Its pin goes with it.
                self.update(|s| {
                    if let Some(list) = s.pins.get_mut(&record.conversation_id) {
                        crate::core::pins::change_dm(list, record.sequence, None);
                    }
                });
                let this = self.clone();
                tokio::spawn(async move { this.forget_deleted(&record.conversation_id, record.sequence).await });
            }
            Payload::CallUpdated(call) => self.update(|s| calls::set_dm_call(&mut s.calls, call)),
            Payload::PinUpdated(pb::DmPinUpdated { pin: Some(pin), pinned }) => self.update(|s| {
                if let Some(list) = s.pins.get_mut(&pin.conversation_id) {
                    let sequence = pin.sequence;
                    crate::core::pins::change_dm(list, sequence, pinned.then_some(pin));
                }
            }),
            Payload::PinUpdated(_) => {}
        }
    }

    /// Catches up on a conversation or secure channel in the background,
    /// once for a burst of news about it.
    pub fn queue(self: &Arc<Self>, id: String) {
        if !self.queued.lock().insert(id.clone()) {
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            this.catch_up_now(&id).await;
        });
    }

    /// Catches up on a room now (after whatever holds the lock), and shows what it found.
    async fn catch_up_now(self: &Arc<Self>, id: &str) {
        if self.stopped() {
            self.queued.lock().remove(id);
            return;
        }
        {
            let mut inner = self.inner.lock().await;
            self.queued.lock().remove(id);
            match self.catch_up(&mut inner, id, 0).await {
                Ok(()) => self.set_broken(id, false),
                Err(err) if err.0 == SECURE_BROKEN => self.set_broken(id, true),
                Err(_) => tracing::warn!("couldn't catch up on a conversation"),
            }
        }
        self.refresh(id).await;
    }

    /// Shows (or clears) that a secure channel's encryption can't be followed.
    fn set_broken(&self, id: &str, broken: bool) {
        self.update(|d| {
            let now = d.blocked.get(id).is_some_and(|b| b == SECURE_BROKEN);
            if broken && !now {
                d.blocked.insert(id.to_owned(), SECURE_BROKEN.to_owned());
            } else if !broken && now {
                d.blocked.remove(id);
            }
        });
    }

    /// How to reach a conversation or secure channel's group, if it's one this device knows.
    fn room(&self, id: &str) -> Option<Room> {
        if let Some(c) = self.conversation(id) {
            return Some(Room::Dm(c));
        }
        let sc = self.secure.lock().get(id).cloned()?;
        let members: Vec<String> = self.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.members.get(&sc.server_id))
                .map(|list| list.iter().filter_map(|m| m.user.as_ref().map(|u| u.id.clone())).collect())
                .unwrap_or_default()
        });
        let allowed = unique(std::iter::once(self.me.id.clone()).chain(sc.member_ids).chain(members));
        Some(Room::Channel { id: id.to_owned(), server_id: sc.server_id, allowed })
    }

    // ───────────────────────── Reaching a room ─────────────────────────

    fn at(server_id: &str, id: &str) -> (String, String) {
        (server_id.to_owned(), id.to_owned())
    }

    /// Who belongs in the group now, asked fresh.
    async fn belong(&self, room: &Room) -> Result<Vec<String>> {
        match room {
            Room::Dm(c) => Ok(c.users.iter().map(|u| u.id.clone()).collect()),
            Room::Channel { id, server_id, .. } => {
                let (server_id, channel_id) = Self::at(server_id, id);
                let res =
                    rpc!(self.api.secure(), get_secure_channel(pb::GetSecureChannelRequest { server_id, channel_id }))
                        .await?;
                if let Some(sc) = self.secure.lock().get_mut(id) {
                    sc.member_ids = res.member_ids.clone();
                    sc.share_history = res.share_history;
                }
                self.show_history_setting(id, res.share_history);
                Ok(res.member_ids)
            }
        }
    }

    /// Refuses to write when the group can't be made right yet.
    fn check(&self, room: &Room, devices: &[pb::Device], belong: &[String]) -> Result<()> {
        match room {
            Room::Dm(c) => {
                if let Some(partner) = c.users.iter().find(|u| u.id != self.me.id)
                    && !devices.iter().any(|d| d.user_id == partner.id)
                {
                    let name = crate::core::store::user_name(partner);
                    return Err(DmError(if partner.username == "deleted" {
                        "This account was deleted.".into()
                    } else {
                        format!(
                            "{name} isn't signed in to fuwa anywhere that can receive encrypted messages yet. You can write once they are."
                        )
                    }));
                }
                Ok(())
            }
            Room::Channel { .. } if !belong.contains(&self.me.id) => {
                Err(DmError("You can't see this channel any more.".into()))
            }
            Room::Channel { .. } => Ok(()),
        }
    }

    async fn records(&self, room: &Room, after: i64) -> Result<(Vec<Rec>, bool), Problem> {
        match room {
            Room::Dm(c) => {
                let page = rpc!(
                    self.api.dms(),
                    list_records(pb::ListRecordsRequest {
                        conversation_id: c.id.clone(),
                        after_sequence: after,
                        limit: PAGE
                    })
                )
                .await?;
                Ok((page.records.into_iter().map(Rec::from).collect(), page.has_more))
            }
            Room::Channel { id, server_id, .. } => {
                let (server_id, channel_id) = Self::at(server_id, id);
                let req = pb::ListSecureRecordsRequest { server_id, channel_id, after_sequence: after, limit: PAGE };
                let page = rpc!(self.api.secure(), list_secure_records(req)).await?;
                Ok((page.records.into_iter().map(Rec::from).collect(), page.has_more))
            }
        }
    }

    /// The welcome waiting for this device, if someone added it: the commit's place, and the welcome.
    async fn welcome(&self, room: &Room) -> Result<Option<(i64, Vec<u8>)>, Problem> {
        match room {
            Room::Dm(c) => {
                let list = rpc!(self.api.dms(), list_welcomes(pb::ListWelcomesRequest {})).await?.welcomes;
                Ok(list.into_iter().find(|w| w.conversation_id == c.id).map(|w| (w.sequence, w.data)))
            }
            Room::Channel { id, server_id, .. } => {
                let req = pb::ListSecureWelcomesRequest { server_id: server_id.clone() };
                let list = rpc!(self.api.secure(), list_secure_welcomes(req)).await?.welcomes;
                Ok(list.into_iter().find(|w| &w.channel_id == id).map(|w| (w.sequence, w.data)))
            }
        }
    }

    async fn group_info(&self, room: &Room) -> Result<(i64, Vec<u8>), Problem> {
        match room {
            Room::Dm(c) => {
                let req = pb::GetGroupInfoRequest { conversation_id: c.id.clone() };
                let info = rpc!(self.api.dms(), get_group_info(req)).await?;
                Ok((info.epoch, info.group_info))
            }
            Room::Channel { id, server_id, .. } => {
                let (server_id, channel_id) = Self::at(server_id, id);
                let req = pb::GetSecureGroupInfoRequest { server_id, channel_id };
                let info = rpc!(self.api.secure(), get_secure_group_info(req)).await?;
                Ok((info.epoch, info.group_info))
            }
        }
    }

    /// Posts a commit; `welcome` hands the devices it adds their welcome.
    async fn post_commit(&self, room: &Room, commit: fuwa_e2ee::Commit, welcome: bool) -> Result<Option<Rec>, Problem> {
        let welcome_device_ids = if welcome && commit.welcome.is_some() {
            commit.added.iter().map(|m| m.device_id.clone()).collect()
        } else {
            vec![]
        };
        let welcome = if welcome { commit.welcome.unwrap_or_default() } else { Vec::new() };
        match room {
            Room::Dm(c) => {
                let req = pb::PostCommitRequest {
                    conversation_id: c.id.clone(),
                    commit: commit.commit,
                    group_info: commit.group_info,
                    welcome,
                    welcome_device_ids,
                };
                Ok(rpc!(self.api.dms(), post_commit(req)).await?.record.map(Rec::from))
            }
            Room::Channel { id, server_id, .. } => {
                let (server_id, channel_id) = Self::at(server_id, id);
                let req = pb::PostSecureCommitRequest {
                    server_id,
                    channel_id,
                    commit: commit.commit,
                    group_info: commit.group_info,
                    welcome,
                    welcome_device_ids,
                };
                Ok(rpc!(self.api.secure(), post_secure_commit(req)).await?.record.map(Rec::from))
            }
        }
    }

    async fn post_message(&self, room: &Room, message: Vec<u8>, media_ids: Vec<String>) -> Result<(), Problem> {
        match room {
            Room::Dm(c) => {
                let req = pb::PostMessageRequest { conversation_id: c.id.clone(), message, media_ids };
                rpc!(self.api.dms(), post_message(req)).await?;
            }
            Room::Channel { id, server_id, .. } => {
                let (server_id, channel_id) = Self::at(server_id, id);
                let req = pb::PostSecureMessageRequest { server_id, channel_id, message, ..Default::default() };
                rpc!(self.api.secure(), post_secure_message(req)).await?;
            }
        }
        Ok(())
    }

    /// Whether earlier messages are passed on to devices added later (secure channels only), asked fresh.
    async fn shares(&self, room: &Room) -> Result<bool, Problem> {
        let Room::Channel { id, server_id, .. } = room else { return Ok(false) };
        let (server_id, channel_id) = Self::at(server_id, id);
        let res =
            rpc!(self.api.secure(), get_secure_channel(pb::GetSecureChannelRequest { server_id, channel_id })).await?;
        if let Some(sc) = self.secure.lock().get_mut(id) {
            sc.share_history = res.share_history;
        }
        self.show_history_setting(id, res.share_history);
        Ok(res.share_history)
    }

    /// Passes earlier messages on, right after this device's commit that added devices.
    async fn post_history(&self, room: &Room, message: Vec<u8>) -> Result<(), Problem> {
        let Room::Channel { id, server_id, .. } = room else { return Ok(()) };
        let (server_id, channel_id) = Self::at(server_id, id);
        rpc!(self.api.secure(), post_secure_history(pb::PostSecureHistoryRequest { server_id, channel_id, message }))
            .await?;
        Ok(())
    }

    async fn delete_record(&self, room: &Room, sequence: i64) -> Result<(), Problem> {
        match room {
            Room::Dm(c) => {
                let req = pb::DeleteRecordRequest { conversation_id: c.id.clone(), sequence };
                rpc!(self.api.dms(), delete_record(req)).await?;
            }
            Room::Channel { id, server_id, .. } => {
                let (server_id, channel_id) = Self::at(server_id, id);
                let req = pb::DeleteSecureRecordRequest { server_id, channel_id, sequence };
                rpc!(self.api.secure(), delete_secure_record(req)).await?;
            }
        }
        Ok(())
    }

    /// A message someone else sent, just opened.
    fn notify(&self, room: &Room, item: &Item) {
        match room {
            Room::Dm(c) => self.shared.notify_dm(&self.key, c, item),
            Room::Channel { id, server_id, .. } => self.shared.notify_secure(&self.key, server_id, id, item),
        }
    }

    /// The devices of these people, asked a hundred at a time.
    async fn devices_of(&self, people: &[String]) -> Result<Vec<pb::Device>, Problem> {
        let mut devices = Vec::new();
        for user_ids in people.chunks(LOOKUPS) {
            let req = pb::ListDevicesRequest { user_ids: user_ids.to_vec() };
            devices.extend(rpc!(self.api.dms(), list_devices(req)).await?.devices);
        }
        Ok(devices)
    }

    // ───────────────────────── Under the lock ─────────────────────────

    /// Reads a room's records this device hasn't, joining it first if it isn't in.
    async fn catch_up(self: &Arc<Self>, inner: &mut Inner, id: &str, depth: u8) -> Result<()> {
        let Some(room) = self.room(id) else { return Ok(()) };
        let mut note = inner.vault.note(id);
        if !inner.device.is_member(id) {
            self.update(|d| {
                d.joining.insert(id.to_owned());
            });
            let joined = self.join(inner, &room, note.clone()).await;
            self.update(|d| {
                d.joining.remove(id);
            });
            match joined? {
                Some(next) => note = next,
                None => return Ok(()),
            }
        }
        loop {
            let (records, has_more) = self.records(&room, note.cursor).await?;
            let mut change = Change::default();
            let mut rejoin = false;
            let mut fresh = Vec::new();
            for record in &records {
                let opened = self.open(inner, &room, record, &mut change, &mut fresh).await?;
                note.cursor = record.sequence;
                match opened {
                    Opened::Fine => {}
                    Opened::Rejoin => {
                        rejoin = true;
                        break;
                    }
                    Opened::Reset => {
                        if let Some(server) = room.server() {
                            self.restart(server, id);
                        }
                        rejoin = true;
                        break;
                    }
                }
            }
            let channel = room.server().is_some();
            let mut lines = Vec::new();
            if channel {
                // A thread goes with its message: replies under a deleted one are dropped here (and only here).
                lines = inner.vault.items(id)?.clone();
                for (c, i) in &change.items {
                    if c == id {
                        match lines.iter_mut().find(|l| l.seq == i.seq) {
                            Some(l) => *l = i.clone(),
                            None => lines.push(i.clone()),
                        }
                    }
                }
                let seqs = secure_threads::by_seq(&lines);
                let gone = secure_threads::orphaned(&lines, &seqs);
                for i in &gone {
                    change.items.push((id.to_owned(), i.clone()));
                }
            }
            change.notes.push((id.to_owned(), note.clone()));
            inner.save(change)?;
            let seqs = secure_threads::by_seq(&lines);
            for item in fresh {
                // A reply kept to its thread only reaches you if you follow the thread.
                let parent = if channel { secure_threads::thread_of(&item, &seqs) } else { 0 };
                if parent != 0
                    && !item.in_channel
                    && !secure_threads::following(Some(&note), parent, &lines, &self.me.id)
                {
                    continue;
                }
                self.notify(&room, &item);
            }
            if rejoin {
                if depth < 2 {
                    return Box::pin(self.catch_up(inner, id, depth + 1)).await;
                }
                return Ok(());
            }
            if !has_more || records.is_empty() {
                return Ok(());
            }
        }
    }

    /// Opens one record and notes what it said.
    async fn open(
        &self,
        inner: &mut Inner,
        room: &Room,
        record: &Rec,
        change: &mut Change,
        fresh: &mut Vec<Item>,
    ) -> Result<Opened> {
        let seq = record.sequence;
        let at = record.at;
        let id = room.id().to_owned();
        let channel = room.server().is_some();
        if channel && record.kind == RecKind::Settings {
            let on = record.share_history;
            if let Some(sc) = self.secure.lock().get_mut(&id) {
                sc.share_history = on;
            }
            self.show_history_setting(&id, on);
            let mut item = Item::new(seq, ItemKind::Setting, at, &record.sender_id, &record.sender_device_id);
            item.content = if on { "on" } else { "off" }.into();
            change.items.push((id, item));
            return Ok(Opened::Fine);
        }
        if channel && record.kind == RecKind::Reset {
            // The group before it is gone; the next commit starts a new one.
            inner.device.forget(&id)?;
            let item = Item::new(seq, ItemKind::Reset, at, &record.sender_id, &record.sender_device_id);
            change.items.push((id, item));
            return Ok(Opened::Reset);
        }
        if record.data.is_empty() {
            // Deleted before this device read it.
            if let Some(mut before) = inner.known(change, &id, seq)?
                && !before.deleted
            {
                before.deleted = true;
                before.content.clear();
                before.files.clear();
                before.reactions.clear();
                // The signed copies hold the words too.
                before.signed = None;
                before.edit_signed = None;
                change.items.push((id, before));
            }
            return Ok(Opened::Fine);
        }
        let own = record.sender_device_id == self.device_id;
        let out = match inner.device.process(&id, &record.data, own, &room.allowed()) {
            Ok(out) => out,
            Err(err) => {
                // A commit this device can't follow leaves it out of the group: it joins again.
                if matches!(err, E2eeError::Behind) || record.kind == RecKind::Commit {
                    tracing::warn!("lost track of an encrypted group; joining it again");
                    inner.device.forget(&id)?;
                    return Ok(Opened::Rejoin);
                }
                let item = Item::new(seq, ItemKind::Unreadable, at, &record.sender_id, &record.sender_device_id);
                change.items.push((id, item));
                return Ok(Opened::Fine);
            }
        };
        let history = channel && record.kind == RecKind::History;
        match out {
            Processed::Message { sender, plaintext } => {
                if history {
                    self.take_history(inner, room, &sender.user_id, &plaintext, change).await?;
                } else {
                    self.read(inner, change, fresh, room, seq, at, &sender, &plaintext)?;
                }
            }
            // What this device passed on, the others already have.
            Processed::Own if history => {}
            Processed::Own => {
                let hash = sha256_hex(&record.data);
                match inner.vault.sent(&hash) {
                    Some(plaintext) => {
                        let me = Member {
                            user_id: self.me.id.clone(),
                            device_id: self.device_id.clone(),
                            signature_key: inner.device.signature_key().to_vec(),
                        };
                        self.read(inner, change, fresh, room, seq, at, &me, &plaintext)?;
                        change.forget_sent.push(hash);
                    }
                    None => {
                        let item = Item::new(seq, ItemKind::Unreadable, at, &self.me.id, &self.device_id);
                        change.items.push((id, item));
                    }
                }
            }
            Processed::Commit { by, added, removed, removed_me } => {
                if removed_me {
                    inner.device.forget(&id)?;
                    return Ok(Opened::Rejoin);
                }
                if !added.is_empty() || !removed.is_empty() {
                    let (sender, device) = match by {
                        Some(by) => (by.user_id, by.device_id),
                        None => (record.sender_id.clone(), record.sender_device_id.clone()),
                    };
                    let mut item = Item::new(seq, ItemKind::Devices, at, &sender, &device);
                    item.added = added.iter().map(device_ref).collect();
                    item.removed = removed.iter().map(device_ref).collect();
                    change.items.push((id, item));
                }
            }
            Processed::Stale => {}
        }
        Ok(Opened::Fine)
    }

    /// What a message said: new text, or an edit of the sender's own earlier
    /// message. In a secure channel it comes signed by the device that sent
    /// it, which is kept so it can be passed on to devices added later.
    #[allow(clippy::too_many_arguments)]
    fn read(
        &self,
        inner: &mut Inner,
        change: &mut Change,
        fresh: &mut Vec<Item>,
        room: &Room,
        seq: i64,
        at: i64,
        sender: &Member,
        plaintext: &[u8],
    ) -> Result<()> {
        use pb::direct_message_content::Body;
        let id = room.id().to_owned();
        let (sender_id, device_id) = (sender.user_id.as_str(), sender.device_id.as_str());
        let unreadable = |change: &mut Change| {
            change.items.push((id.clone(), Item::new(seq, ItemKind::Unreadable, at, sender_id, device_id)));
        };
        let Ok(mut content) = pb::DirectMessageContent::decode(plaintext) else {
            unreadable(change);
            return Ok(());
        };
        let mut signed = None;
        if let Some(Body::Signed(s)) = &content.body {
            let opened = open_signed(&id, &s.payload, &s.signature, &sender.signature_key);
            let Some(opened) = opened.filter(|o| o.payload.sender_id == sender_id) else {
                unreadable(change);
                return Ok(());
            };
            let Some(inside) = opened.payload.content else {
                unreadable(change);
                return Ok(());
            };
            content = inside;
            signed = Some(opened.signed);
        }
        match content.body {
            Some(Body::Text(text)) => {
                let mut item = Item::new(seq, ItemKind::Text, at, sender_id, device_id);
                item.content = clip(&text.content);
                item.files = crate::core::sealed_files::files_of(&text.files);
                item.reply_to = text.reply_to_sequence;
                if room.server().is_some() {
                    thread_fields(&mut item, &text);
                }
                item.signed = signed;
                let had = inner.vault.items(&id)?.iter().any(|i| i.seq == seq);
                if !had && sender_id != self.me.id {
                    fresh.push(item.clone());
                }
                change.items.push((id, item));
            }
            Some(Body::Edit(edit)) => {
                if let Some(mut target) = inner.known(change, &id, edit.sequence)?
                    && target.kind == ItemKind::Text
                    && target.voice.is_none()
                    && target.sender_id == sender_id
                    && !target.deleted
                {
                    target.content = clip(&edit.content);
                    target.edited_at = at;
                    target.edit_signed = signed;
                    change.items.push((id, target));
                }
            }
            // Secure channels don't carry voice messages yet, as on the web.
            Some(Body::Voice(_)) if self.secure.lock().contains_key(&id) => {}
            Some(Body::Voice(voice)) => {
                let Some(file) = voice_file(&voice) else { return Ok(()) };
                let mut item = Item::new(seq, ItemKind::Text, at, sender_id, device_id);
                item.voice = Some(file);
                item.reply_to = voice.reply_to_sequence;
                item.signed = signed;
                let had = inner.vault.items(&id)?.iter().any(|i| i.seq == seq);
                if !had && sender_id != self.me.id {
                    fresh.push(item.clone());
                }
                change.items.push((id, item));
            }
            // Whether a lock counts (only from someone with Manage Messages) is worked out where threads are shown.
            Some(Body::Thread(lock)) if room.server().is_some() && signed.is_some() => {
                let mut item = lock_item(seq, at, sender_id, device_id, &lock);
                item.signed = signed;
                change.items.push((id, item));
            }
            // A reaction: kept on the message it's to, never shown as a line of its own,
            // counted as unread or notified. In a secure channel only signed ones count.
            Some(Body::Reaction(r)) if room.server().is_none() || signed.is_some() => {
                self.take_reaction(inner, change, room, seq, at, sender_id, &r, signed)?;
            }
            // Anything else is from a newer app: there's nothing to show for it here.
            Some(Body::Signed(_) | Body::History(_) | Body::Thread(_) | Body::Reaction(_)) | None => {}
        }
        Ok(())
    }

    /// A reaction from `sender_id` in record `seq`, kept on the message it's
    /// to. In a secure channel it counts only from someone this device sees
    /// with Add Reactions there, though anyone may take their own off.
    #[allow(clippy::too_many_arguments)]
    fn take_reaction(
        &self,
        inner: &mut Inner,
        change: &mut Change,
        room: &Room,
        seq: i64,
        at: i64,
        sender_id: &str,
        r: &pb::DirectMessageReaction,
        signed: Option<Signed>,
    ) -> Result<()> {
        if !crate::core::reactions::valid_emoji(&r.emoji) {
            return Ok(());
        }
        if let Some(server) = room.server()
            && !r.removed
            && !self.shared.read(|s| {
                s.instance(&self.key)
                    .is_none_or(|i| crate::core::reactions::may_react(i, server, room.id(), sender_id, at))
            })
        {
            return Ok(());
        }
        let id = room.id().to_owned();
        if let Some(mut target) = inner.known(change, &id, r.sequence)?
            && target.kind == ItemKind::Text
            && !target.deleted
            && crate::core::reactions::mark(&mut target.reactions, sender_id, &r.emoji, seq, r.removed, signed)
        {
            change.items.push((id, target));
        }
        Ok(())
    }

    /// What this device sends in a secure channel: the content, signed by this device.
    fn signed_content(&self, inner: &Inner, id: &str, content: &Content) -> Result<Vec<u8>> {
        use pb::direct_message_content::Body;
        let payload = pb::SignedPayload {
            conversation_id: id.to_owned(),
            sender_id: self.me.id.clone(),
            sent_at_ms: now_ms(),
            content: Some(content_of(content)),
        }
        .encode_to_vec();
        let signature = inner.device.sign(&payload)?;
        Ok(pb::DirectMessageContent { body: Some(Body::Signed(pb::SignedContent { payload, signature })) }
            .encode_to_vec())
    }

    /// Earlier messages someone's device passed on when it added this one.
    /// Each is checked against the signature of the device that sent it,
    /// which must be one of its sender's devices now, and against the
    /// channel's log (see `history.rs`); only messages from before this device
    /// joined, and not already here, are taken. The rest are left out.
    async fn take_history(
        &self,
        inner: &mut Inner,
        room: &Room,
        by: &str,
        plaintext: &[u8],
        change: &mut Change,
    ) -> Result<()> {
        use pb::direct_message_content::Body;
        let id = room.id().to_owned();
        let Ok(pb::DirectMessageContent { body: Some(Body::History(shared)) }) =
            pb::DirectMessageContent::decode(plaintext)
        else {
            return Ok(());
        };
        if !self.shares(room).await? {
            return Ok(());
        }
        let mine = |i: &Item| i.kind == ItemKind::Joined && i.sender_id == self.me.id;
        let joined = inner
            .vault
            .items(&id)?
            .iter()
            .chain(change.items.iter().filter(|(c, _)| *c == id).map(|(_, i)| i))
            .filter(|i| mine(i))
            .map(|i| i.seq)
            .max()
            .unwrap_or(0);
        if joined == 0 {
            return Ok(());
        }
        let mut opened: Vec<(i64, OpenedSigned, String)> = Vec::new();
        for entry in shared.entries.iter().take(HISTORY_ENTRIES) {
            if entry.sequence <= 0 || entry.sequence >= joined {
                continue;
            }
            let Some(o) = open_signed(&id, &entry.payload, &entry.signature, &entry.signature_key) else { continue };
            if matches!(
                o.payload.content.as_ref().and_then(|c| c.body.as_ref()),
                Some(Body::Text(_) | Body::Edit(_) | Body::Thread(_) | Body::Reaction(_))
            ) {
                opened.push((entry.sequence, o, fuwa_e2ee::device_id(&entry.signature_key)));
            }
        }
        if opened.is_empty() {
            return Ok(());
        }
        let allowed = room.allowed();
        let senders = unique(opened.iter().map(|(_, o, _)| o.payload.sender_id.clone()))
            .into_iter()
            .filter(|s| allowed.contains(s))
            .collect::<Vec<_>>();
        let devices: HashSet<String> =
            self.devices_of(&senders).await?.into_iter().map(|d| format!("{}/{}", d.user_id, d.id)).collect();
        let earliest = opened.iter().map(|(seq, _, _)| *seq).min().unwrap_or(1);
        let (log, times) = self.log_between(room, earliest - 1, joined).await?;
        let candidates: Vec<Candidate> = opened
            .iter()
            .map(|(seq, o, device)| Candidate {
                seq: *seq,
                sender_id: o.payload.sender_id.clone(),
                device_id: device.clone(),
                edit_of: match o.payload.content.as_ref().and_then(|c| c.body.as_ref()) {
                    Some(Body::Edit(e)) => Some(e.sequence),
                    _ => None,
                },
            })
            .collect();
        let ctx = history::Context { joined, allowed: &allowed, devices: &devices, log: &log };
        for n in history::accept(&candidates, &ctx) {
            let (seq, o, device) = &opened[n];
            let sender = o.payload.sender_id.as_str();
            let at = times.get(seq).copied().unwrap_or(o.payload.sent_at_ms);
            match o.payload.content.as_ref().and_then(|c| c.body.as_ref()) {
                Some(Body::Text(text)) => {
                    if inner.known(change, &id, *seq)?.is_some() {
                        continue;
                    }
                    let mut item = Item::new(*seq, ItemKind::Text, at, sender, device);
                    item.content = clip(&text.content);
                    item.files = crate::core::sealed_files::files_of(&text.files);
                    item.reply_to = text.reply_to_sequence;
                    thread_fields(&mut item, text);
                    item.signed = Some(o.signed.clone());
                    item.shared_by = by.to_owned();
                    change.items.push((id.clone(), item));
                }
                Some(Body::Thread(lock)) => {
                    if inner.known(change, &id, *seq)?.is_some() {
                        continue;
                    }
                    let mut item = lock_item(*seq, at, sender, device, lock);
                    item.signed = Some(o.signed.clone());
                    item.shared_by = by.to_owned();
                    change.items.push((id.clone(), item));
                }
                // A reaction, at its own record: on a message this device has.
                Some(Body::Reaction(r)) => {
                    self.take_reaction(inner, change, room, *seq, at, sender, r, Some(o.signed.clone()))?;
                }
                Some(Body::Edit(edit)) => {
                    if let Some(mut target) = inner.known(change, &id, *seq)?
                        && !target.shared_by.is_empty()
                        && target.kind == ItemKind::Text
                        && target.sender_id == sender
                        && !target.deleted
                    {
                        target.content = clip(&edit.content);
                        target.edited_at = o.payload.sent_at_ms;
                        target.edit_signed = Some(o.signed.clone());
                        change.items.push((id.clone(), target));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The headers of a channel's records after `after` and before `before`:
    /// who sent what kind, and whether it's gone, with when each came.
    async fn log_between(
        &self,
        room: &Room,
        after: i64,
        before: i64,
    ) -> Result<(BTreeMap<i64, Logged>, HashMap<i64, i64>), Problem> {
        let (mut headers, mut times) = (BTreeMap::new(), HashMap::new());
        let mut cursor = after;
        for _ in 0..LOG_PAGES {
            if cursor >= before - 1 {
                break;
            }
            let (records, has_more) = self.records(room, cursor).await?;
            for r in &records {
                if r.sequence >= before {
                    return Ok((headers, times));
                }
                let kind = match r.kind {
                    RecKind::Message => LogKind::Message,
                    RecKind::Settings => LogKind::Settings,
                    _ => LogKind::Other,
                };
                headers.insert(
                    r.sequence,
                    Logged {
                        kind,
                        sender_id: r.sender_id.clone(),
                        device_id: r.sender_device_id.clone(),
                        deleted: r.data.is_empty(),
                        on: r.share_history,
                    },
                );
                times.insert(r.sequence, r.at);
                cursor = r.sequence;
            }
            if !has_more || records.is_empty() {
                break;
            }
        }
        Ok((headers, times))
    }

    /// Passes the newest earlier messages this device has, as their senders
    /// signed them, on to the devices its commit just added. Only while the
    /// channel shares history; the server takes one right after the commit.
    async fn share_history(&self, inner: &mut Inner, room: &Room) -> Result<(), DmError> {
        use pb::direct_message_content::Body;
        if room.server().is_none() || !self.shares(room).await? {
            return Ok(());
        }
        let id = room.id();
        let all = inner.vault.items(id)?.clone();
        // What was said while sharing was off stays with those who were there.
        let since = all.iter().filter(|i| i.kind == ItemKind::Setting).map(|i| i.seq).max().unwrap_or(0);
        let mut items: Vec<&Item> = all
            .iter()
            .filter(|i| {
                matches!(i.kind, ItemKind::Text | ItemKind::Thread) && !i.deleted && i.signed.is_some() && i.seq > since
            })
            .collect();
        items.sort_by_key(|i| std::cmp::Reverse(i.seq));
        let mut entries: Vec<pb::SharedEntry> = Vec::new();
        let mut size = 0;
        for i in items {
            let entry = |sequence: i64, s: &Signed| pb::SharedEntry {
                sequence,
                payload: s.payload.clone(),
                signature: s.signature.clone(),
                signature_key: s.key.clone(),
            };
            let mut parts: Vec<pb::SharedEntry> =
                i.signed.iter().chain(i.edit_signed.iter()).map(|s| entry(i.seq, s)).collect();
            // Its reactions that are on, each at its own record, after it.
            let mut reactions: Vec<_> = i.reactions.iter().filter(|m| !m.removed && m.seq > since).collect();
            reactions.sort_by_key(|m| m.seq);
            parts.extend(reactions.into_iter().filter_map(|m| Some(entry(m.seq, m.signed.as_ref()?))));
            let bytes: usize =
                parts.iter().map(|p| p.payload.len() + p.signature.len() + p.signature_key.len() + 16).sum();
            if size + bytes > HISTORY_BYTES || entries.len() + parts.len() > HISTORY_ENTRIES {
                break;
            }
            size += bytes;
            // Newest first here; turned around below, so an edit follows its message.
            parts.reverse();
            entries.extend(parts);
        }
        if entries.is_empty() {
            return Ok(());
        }
        entries.reverse();
        let plaintext =
            pb::DirectMessageContent { body: Some(Body::History(pb::SharedHistory { entries })) }.encode_to_vec();
        let ciphertext = inner.device.encrypt(id, &plaintext)?;
        inner.save(Change::default())?;
        self.post_history(room, ciphertext).await?;
        Ok(())
    }

    /// Tells the screens whether a secure channel shares history.
    fn show_history_setting(&self, id: &str, on: bool) {
        if self.shared.read(|s| s.instance(&self.key).and_then(|i| i.dms.secure_history.get(id).copied())) == Some(on) {
            return;
        }
        self.update(|d| {
            d.secure_history.insert(id.to_owned(), on);
        });
    }

    /// Joins a room's group: from a welcome if someone added this device, or
    /// else by itself from the group's public state. None if nobody started
    /// the group yet.
    async fn join(&self, inner: &mut Inner, room: &Room, note: Note) -> Result<Option<Note>> {
        let allowed = room.allowed();
        let id = room.id().to_owned();
        if let Some((sequence, data)) = self.welcome(room).await? {
            match inner.device.join_from_welcome(&id, &data, &allowed) {
                Ok(_) => {
                    let next = Note { cursor: sequence, ..note };
                    let item = Item::new(sequence, ItemKind::Joined, now_ms(), &self.me.id, &self.device_id);
                    inner.save(Change {
                        notes: vec![(id.clone(), next.clone())],
                        items: vec![(id, item)],
                        ..Change::default()
                    })?;
                    return Ok(Some(next));
                }
                // Used up or out of date: join by itself instead.
                Err(_) => tracing::warn!("couldn't join an encrypted group from its welcome"),
            }
        }
        for _ in 0..3 {
            let (epoch, info) = self.group_info(room).await?;
            if epoch == 0 || info.is_empty() {
                return Ok(None);
            }
            let commit = match inner.device.join_by_itself(&id, &info, &allowed) {
                Ok(commit) => commit,
                // Nothing to join from: the group the server holds can't be read.
                Err(_) if room.server().is_some() => return Err(DmError(SECURE_BROKEN.into())),
                Err(err) => return Err(err.into()),
            };
            inner.save(Change::default())?;
            match self.post_commit(room, commit, false).await {
                Ok(record) => {
                    let seq = record.as_ref().map(|r| r.sequence).unwrap_or(0);
                    let at = record.as_ref().map(|r| r.at).unwrap_or_else(now_ms);
                    let next = Note { cursor: seq, ..note };
                    let item = Item::new(seq, ItemKind::Joined, at, &self.me.id, &self.device_id);
                    inner.save(Change {
                        notes: vec![(id.clone(), next.clone())],
                        items: vec![(id, item)],
                        ..Change::default()
                    })?;
                    return Ok(Some(next));
                }
                Err(err) => {
                    inner.device.forget(&id)?;
                    inner.save(Change::default())?;
                    if !is_precondition(&err) {
                        return Err(err.into());
                    }
                }
            }
        }
        Err(DmError("Couldn't join this conversation; try again.".into()))
    }

    /// Makes the group hold exactly the devices of everyone who belongs in
    /// it: starts it if nobody has, adds new devices, drops ones whose
    /// sessions ended or whose people lost access.
    async fn reconcile(self: &Arc<Self>, inner: &mut Inner, room: &Room) -> Result<()> {
        let id = room.id().to_owned();
        let channel = room.server().is_some();
        for attempt in 0.. {
            let belong = self.belong(room).await?;
            let devices = self.devices_of(&belong).await?;
            self.check(room, &devices, &belong)?;
            let allowed = unique(room.allowed().into_iter().chain(belong.iter().cloned()));
            let starting = !inner.device.is_member(&id);
            if starting {
                inner.device.create_group(&id)?;
            }
            let members = inner.device.members(&id)?;
            let present: HashSet<&str> = members.iter().map(|m| m.device_id.as_str()).collect();
            let expected: HashSet<&str> = devices.iter().map(|d| d.id.as_str()).collect();
            let adds: Vec<String> =
                devices.iter().filter(|d| !present.contains(d.id.as_str())).map(|d| d.id.clone()).collect();
            let removes: Vec<String> = members
                .iter()
                .filter(|m| !expected.contains(m.device_id.as_str()) && m.device_id != self.device_id)
                .map(|m| m.device_id.clone())
                .collect();
            let mut claimed = Vec::new();
            for device_ids in adds.chunks(LOOKUPS) {
                let req = pb::ClaimKeyPackagesRequest { device_ids: device_ids.to_vec() };
                claimed.extend(rpc!(self.api.dms(), claim_key_packages(req)).await?.key_packages);
            }
            // A secure channel starts its group even when nobody else is signed in yet, so its first writer isn't stuck.
            if claimed.is_empty() && removes.is_empty() && !(starting && channel) {
                if starting {
                    inner.device.forget(&id)?;
                    if matches!(room, Room::Dm(c) if c.users.iter().any(|u| u.id != self.me.id)) {
                        return Err(DmError("Couldn't reach their devices yet; try again in a moment.".into()));
                    }
                }
                return Ok(());
            }
            let adding = !claimed.is_empty();
            let adds: Vec<(String, Vec<u8>)> = claimed.into_iter().map(|k| (k.device_id, k.key_package)).collect();
            let commit = inner.device.commit(&id, &adds, &removes, &allowed)?;
            inner.save(Change::default())?;
            match self.post_commit(room, commit, true).await {
                Ok(_) => {
                    self.catch_up(inner, &id, 0).await?;
                    if adding && channel {
                        // Best effort: messages keep working whether or not this goes through.
                        if self.share_history(inner, room).await.is_err() {
                            tracing::warn!("couldn't pass history on");
                        }
                        self.catch_up(inner, &id, 0).await?;
                    }
                    return Ok(());
                }
                Err(err) => {
                    if inner.device.epoch(&id).unwrap_or(0) == 0 {
                        inner.device.forget(&id)?;
                    } else {
                        inner.device.discard_pending(&id)?;
                    }
                    inner.save(Change::default())?;
                    if !is_precondition(&err) || attempt >= 2 {
                        return Err(err.into());
                    }
                    // Someone else's commit got there first.
                    self.catch_up(inner, &id, 0).await?;
                }
            }
        }
        unreachable!()
    }

    // ───────────────────────── What people do ─────────────────────────

    /// The secret a call in this conversation seals its sound with, and the
    /// epoch it's from (see [`calls::CALL_LABEL`]). When someone's frames
    /// say they're from a newer epoch than `seen`, this catches up first.
    pub async fn call_secret(self: &Arc<Self>, id: &str, seen: Option<u64>) -> Result<(u64, Vec<u8>)> {
        let mut inner = self.inner.lock().await;
        if !inner.device.is_member(id) {
            self.catch_up(&mut inner, id, 0).await?;
        }
        let (epoch, secret) = inner.device.export_secret(id, calls::CALL_LABEL, calls::CALL_SECRET_LEN)?;
        if seen.is_some_and(|seen| seen > epoch) {
            self.catch_up(&mut inner, id, 0).await?;
            return Ok(inner.device.export_secret(id, calls::CALL_LABEL, calls::CALL_SECRET_LEN)?);
        }
        Ok((epoch, secret))
    }

    /// Shows why a room can't be written in right now, or that it can. A
    /// secure channel shows only that its encryption broke: anything else is
    /// for whoever asked.
    fn show_result(&self, id: &str, result: &Result<()>) {
        if self.secure.lock().contains_key(id) {
            match result {
                Err(err) if err.0 == SECURE_BROKEN => self.set_broken(id, true),
                Ok(()) => self.set_broken(id, false),
                Err(_) => {}
            }
            return;
        }
        self.update(|d| match result {
            Ok(()) => {
                d.blocked.remove(id);
            }
            Err(err) => {
                d.blocked.insert(id.to_owned(), err.0.clone());
            }
        });
    }

    /// Gets a conversation or secure channel ready to write in, saying why it can't be yet.
    pub async fn prepare(self: &Arc<Self>, id: &str) -> Result<()> {
        let result = async {
            let mut inner = self.inner.lock().await;
            self.catch_up(&mut inner, id, 0).await?;
            let room = self.room(id).ok_or_else(|| DmError("That conversation isn't here.".into()))?;
            self.reconcile(&mut inner, &room).await
        }
        .await;
        self.show_result(id, &result);
        self.refresh(id).await;
        result
    }

    /// Encrypts and sends a message (or an edit) to everyone in the room.
    pub async fn send(self: &Arc<Self>, id: &str, content: Content) -> Result<()> {
        let result = async {
            let mut inner = self.inner.lock().await;
            self.catch_up(&mut inner, id, 0).await?;
            let room = self.room(id).ok_or_else(|| DmError("That conversation isn't here.".into()))?;
            if room.server().is_some() && matches!(content, Content::Voice(_)) {
                return Err(DmError("Voice messages can't be sent in secure channels yet.".into()));
            }
            self.reconcile(&mut inner, &room).await?;
            let plaintext =
                if room.server().is_some() { self.signed_content(&inner, id, &content)? } else { encode(&content) };
            for attempt in 0.. {
                let ciphertext = inner.device.encrypt(id, &plaintext)?;
                let hash = sha256_hex(&ciphertext);
                // Kept first: this device can't open what it sent, so this is how it knows what it said.
                inner.save(Change { sent: vec![(hash.clone(), plaintext.clone())], ..Change::default() })?;
                match self.post_message(&room, ciphertext, content.media_ids()).await {
                    Ok(()) => break,
                    Err(err) => {
                        inner.vault.write(Change { forget_sent: vec![hash], ..Change::default() })?;
                        if !is_precondition(&err) || attempt >= 3 {
                            return Err(err.into());
                        }
                        self.catch_up(&mut inner, id, 0).await?;
                    }
                }
            }
            self.catch_up(&mut inner, id, 0).await
        }
        .await;
        self.show_result(id, &result);
        self.refresh(id).await;
        result
    }

    /// Deletes a message: yours, or (with Manage Messages) anyone's in a
    /// secure channel. From the instance, and from every device's copy.
    pub async fn remove(self: &Arc<Self>, id: &str, seq: i64) -> Result<()> {
        let room = self.room(id).ok_or_else(|| DmError("That conversation isn't here.".into()))?;
        self.delete_record(&room, seq).await?;
        self.forget_deleted(id, seq).await;
        Ok(())
    }

    async fn forget_deleted(&self, id: &str, seq: i64) {
        let secure = self.secure.lock().contains_key(id);
        {
            let mut inner = self.inner.lock().await;
            let mut all = inner.vault.items(id).map(|i| i.clone()).unwrap_or_default();
            if let Some(before) = all.iter_mut().find(|i| i.seq == seq)
                && !before.deleted
            {
                // The signed copies hold the words too.
                let gone = secure_threads::emptied(before);
                *before = gone.clone();
                let mut items = vec![(id.to_owned(), gone)];
                // In a secure channel the line's thread goes with it, on this device only.
                if secure {
                    let seqs = secure_threads::by_seq(&all);
                    items.extend(secure_threads::orphaned(&all, &seqs).into_iter().map(|i| (id.to_owned(), i)));
                }
                let _ = inner.vault.write(Change { items, ..Change::default() });
            }
        }
        self.refresh(id).await;
    }

    /// Notes that you've seen everything in a conversation or secure channel so far.
    pub async fn mark_read(&self, id: &str) {
        {
            let mut inner = self.inner.lock().await;
            let last = inner.vault.items(id).ok().and_then(|items| items.last().map(|i| i.seq)).unwrap_or(0);
            let mut note = inner.vault.note(id);
            if note.read < last {
                note.read = last;
                let _ = inner.vault.write(Change { notes: vec![(id.to_owned(), note)], ..Change::default() });
            }
        }
        if self.secure.lock().contains_key(id) {
            self.shared.instance(&self.key, |i| {
                if i.unread.get(id).is_some_and(|n| *n > 0) {
                    i.unread.insert(id.to_owned(), 0);
                }
            });
        } else {
            self.update(|d| {
                d.unread.insert(id.to_owned(), 0);
            });
        }
    }

    /// Remembers that you compared this safety number with the other person (or forgets it, with "").
    pub async fn verify(&self, id: &str, safety: &str) {
        {
            let mut inner = self.inner.lock().await;
            let mut note = inner.vault.note(id);
            note.verified = safety.to_owned();
            let _ = inner.vault.write(Change { notes: vec![(id.to_owned(), note)], ..Change::default() });
        }
        self.refresh(id).await;
    }

    /// Follows a secure channel's thread by hand, or stops (kept on this device only).
    pub async fn follow_thread(&self, id: &str, parent: i64, on: bool) {
        {
            let mut inner = self.inner.lock().await;
            let mut note = inner.vault.note(id);
            note.follows.insert(parent, on);
            let _ = inner.vault.write(Change { notes: vec![(id.to_owned(), note)], ..Change::default() });
        }
        self.refresh(id).await;
    }

    /// Notes that you've seen a secure channel's thread up to `seq`.
    pub async fn mark_thread_read(&self, id: &str, parent: i64, seq: i64) {
        {
            let mut inner = self.inner.lock().await;
            let mut note = inner.vault.note(id);
            if note.thread_read.get(&parent).copied().unwrap_or(0) >= seq {
                return;
            }
            note.thread_read.insert(parent, seq);
            let _ = inner.vault.write(Change { notes: vec![(id.to_owned(), note)], ..Change::default() });
        }
        self.refresh(id).await;
    }

    // ───────────────────────── Showing it ─────────────────────────

    /// Puts what this app knows about a conversation or secure channel in the store.
    pub async fn refresh(&self, id: &str) {
        if self.stopped() {
            return;
        }
        let secure = self.secure.lock().contains_key(id);
        let conversation = self.conversation(id);
        if !secure && conversation.is_none() {
            return;
        }
        // Held until the store has what was read, so a refresh that read
        // earlier can't put its older copy over a newer one.
        let mut inner = self.inner.lock().await;
        let items = inner.vault.items(id).map(|i| i.clone()).unwrap_or_default();
        let note = inner.vault.note(id);
        let members = if inner.device.is_member(id) { inner.device.members(id).unwrap_or_default() } else { vec![] };
        // Let go of meanwhile.
        if secure && !self.secure.lock().contains_key(id) {
            return;
        }
        let looking = self.shared.is_focused(&self.key, id);
        // Replies kept to their threads count in their threads, not the channel.
        let seqs = secure_threads::by_seq(&items);
        let unread = if looking {
            0
        } else {
            items
                .iter()
                .filter(|i| i.kind == ItemKind::Text && !i.deleted && i.sender_id != self.me.id && i.seq > note.read)
                .filter(|i| !secure || secure_threads::in_channel(i, &seqs))
                .count() as u32
        };
        let shown: Vec<DmMember> = members
            .iter()
            .map(|m| DmMember {
                user_id: m.user_id.clone(),
                device_id: m.device_id.clone(),
                signature_key: m.signature_key.clone(),
            })
            .collect();
        let last = items.last().map(|i| i.seq).unwrap_or(0);
        if let Some(c) = conversation {
            let safety = self.safety_number(&c, &members);
            self.update(|d| {
                d.items.insert(id.to_owned(), items);
                d.unread.insert(id.to_owned(), unread);
                d.members.insert(id.to_owned(), shown);
                d.safety.insert(id.to_owned(), safety);
                if note.verified.is_empty() {
                    d.verified.remove(id);
                } else {
                    d.verified.insert(id.to_owned(), note.verified.clone());
                }
            });
        } else {
            // A secure channel's unread count goes with the server's channels.
            self.shared.instance(&self.key, |i| {
                i.dms.items.insert(id.to_owned(), items);
                i.dms.members.insert(id.to_owned(), shown);
                i.dms.thread_notes.insert(id.to_owned(), note.clone());
                i.unread.insert(id.to_owned(), unread);
            });
        }
        drop(inner);
        if looking && note.read < last {
            self.mark_read(id).await;
        }
    }

    /// Both people's safety number, from the devices in the group. Empty until both have one there.
    fn safety_number(&self, c: &pb::Conversation, members: &[Member]) -> String {
        let Some(partner) = c.users.iter().find(|u| u.id != self.me.id) else { return String::new() };
        let keys = |user: &str| -> Vec<Vec<u8>> {
            members.iter().filter(|m| m.user_id == user).map(|m| m.signature_key.clone()).collect()
        };
        let (mine, theirs) = (keys(&self.me.id), keys(&partner.id));
        if mine.is_empty() || theirs.is_empty() {
            return String::new();
        }
        fuwa_e2ee::safety_number((&self.me.id, &mine), (&partner.id, &theirs))
    }

    // ───────────────────────── Secure channels ─────────────────────────

    /// Starts following a secure channel (opened, or it had news): catches up on it.
    pub fn follow_channel(self: &Arc<Self>, server_id: &str, channel_id: &str) {
        self.secure
            .lock()
            .entry(channel_id.to_owned())
            .or_insert_with(|| SecureChannel { server_id: server_id.to_owned(), ..SecureChannel::default() });
        self.queue(channel_id.to_owned());
    }

    /// Follows a secure channel and catches up on it now, for whoever waits on it.
    pub async fn open_channel(self: &Arc<Self>, server_id: &str, channel_id: &str) {
        self.secure
            .lock()
            .entry(channel_id.to_owned())
            .or_insert_with(|| SecureChannel { server_id: server_id.to_owned(), ..SecureChannel::default() });
        self.catch_up_now(channel_id).await;
    }

    /// Follows the secure channels this device was already in, when their
    /// server comes in: what came while away is read.
    pub async fn follow_server(self: &Arc<Self>, server_id: &str, channel_ids: &[String]) {
        let noted: HashSet<String> = self.inner.lock().await.vault.noted().cloned().collect();
        for id in channel_ids.iter().filter(|id| noted.contains(*id)) {
            self.follow_channel(server_id, id);
        }
    }

    /// A server event, as it arrives live: secure channels' records, and
    /// changes to who can see them.
    pub fn on_server_event(self: &Arc<Self>, server_id: &str, payload: &pb::event::Payload) {
        use pb::event::Payload;
        match payload {
            Payload::SecureRecordAdded(added) => {
                if let Some(record) = &added.record {
                    self.follow_channel(server_id, &record.channel_id);
                }
            }
            Payload::SecureRecordDeleted(deleted) => {
                if self.secure.lock().contains_key(&deleted.channel_id) {
                    let (this, id, seq) = (self.clone(), deleted.channel_id.clone(), deleted.sequence);
                    tokio::spawn(async move { this.forget_deleted(&id, seq).await });
                }
            }
            Payload::ChannelDeleted(deleted) => {
                if self.secure.lock().contains_key(&deleted.channel_id) {
                    let (this, id) = (self.clone(), deleted.channel_id.clone());
                    tokio::spawn(async move { this.leave_channel(&id).await });
                }
            }
            Payload::ChannelUpdated(_)
            | Payload::RoleUpdated(_)
            | Payload::RoleDeleted(_)
            | Payload::MemberUpdated(_)
            | Payload::MemberLeft(_)
            | Payload::MemberJoined(_) => self.settle(server_id),
            _ => {}
        }
    }

    /// After a change to who can see what, brings the server's secure
    /// channels this device is in back in step: people who lost access go,
    /// people who gained it come in. Every device that's online would do it,
    /// so each waits a moment first; the first commit wins and the rest find
    /// nothing to do.
    fn settle(self: &Arc<Self>, server_id: &str) {
        let ids: Vec<String> =
            self.secure.lock().iter().filter(|(_, sc)| sc.server_id == server_id).map(|(id, _)| id.clone()).collect();
        if ids.is_empty() || !self.settling.lock().insert(server_id.to_owned()) {
            return;
        }
        // The device whose commit adds someone is the one that passes history
        // on, so devices that joined after the channel started (and hold less
        // of it) let the others go first.
        let late = self.shared.read(|s| {
            s.instance(&self.key).is_some_and(|i| {
                ids.iter().any(|id| {
                    i.dms.items.get(id).is_some_and(|items| {
                        items.iter().any(|it| it.kind == ItemKind::Joined && it.sender_id == self.me.id && it.seq > 1)
                    })
                })
            })
        });
        let wait = 400.0 + rand_unit() * ACCESS_SETTLE_MS + if late { LATE_SETTLE_MS } else { 0.0 };
        let (this, server_id) = (self.clone(), server_id.to_owned());
        tokio::spawn(async move {
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(wait as u64)) => {}
                () = this.stop.cancelled() => return,
            }
            this.settling.lock().remove(&server_id);
            for id in ids {
                // Only people who may write commit for the group; the server refuses the others'.
                if !this.secure.lock().contains_key(&id) || !this.can_write(&server_id, &id) {
                    continue;
                }
                {
                    let mut inner = this.inner.lock().await;
                    let result = async {
                        this.catch_up(&mut inner, &id, 0).await?;
                        match this.room(&id) {
                            Some(room) if inner.device.is_member(&id) => this.reconcile(&mut inner, &room).await,
                            _ => Ok(()),
                        }
                    }
                    .await;
                    if result.is_err() {
                        tracing::warn!("couldn't bring a secure channel in step");
                    }
                }
                this.refresh(&id).await;
            }
        });
    }

    /// After a channel's encryption started over: someone who may write
    /// starts its new group. Every such device that's online would, so each
    /// waits a moment; the first commit wins and the rest join from its welcome.
    fn restart(self: &Arc<Self>, server_id: &str, id: &str) {
        if !self.can_write(server_id, id) {
            return;
        }
        let wait = 400.0 + rand_unit() * ACCESS_SETTLE_MS;
        let (this, id) = (self.clone(), id.to_owned());
        tokio::spawn(async move {
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(wait as u64)) => {}
                () = this.stop.cancelled() => return,
            }
            if this.secure.lock().contains_key(&id) {
                let _ = this.prepare(&id).await;
            }
        });
    }

    /// Whether you may write in a server's channel, worked out as the server does, from what this app knows.
    fn can_write(&self, server_id: &str, channel_id: &str) -> bool {
        self.shared.read(|s| {
            let Some(i) = s.instance(&self.key) else { return false };
            let access = i.access(server_id);
            let timed_out = i
                .my_member(server_id)
                .and_then(|m| m.timed_out_until.as_ref())
                .is_some_and(|t| ts_ms(Some(t)) > now_ms());
            (access.owner || !timed_out) && access.has_in(channel_id, pb::Permission::SendMessages)
        })
    }

    /// A secure channel you can't see any more (or that was deleted): this
    /// device lets go of it and what it kept.
    pub async fn leave_channel(&self, id: &str) {
        self.secure.lock().remove(id);
        {
            let mut inner = self.inner.lock().await;
            let _ = inner.device.forget(id);
            let _ = inner.save(Change::default());
            if inner.vault.forget(id).is_err() {
                tracing::warn!("couldn't forget a secure channel's messages");
            }
        }
        self.shared.instance(&self.key, |i| {
            i.dms.items.remove(id);
            i.dms.members.remove(id);
            i.dms.blocked.remove(id);
            i.dms.secure_history.remove(id);
            i.unread.remove(id);
        });
    }
}

/// A signed payload for this room, if `key` signed it.
fn open_signed(room: &str, payload: &[u8], signature: &[u8], key: &[u8]) -> Option<OpenedSigned> {
    if !fuwa_e2ee::verify(key, payload, signature) {
        return None;
    }
    let opened = pb::SignedPayload::decode(payload).ok()?;
    (opened.conversation_id == room).then(|| OpenedSigned {
        payload: opened,
        signed: Signed { payload: payload.to_vec(), signature: signature.to_vec(), key: key.to_vec() },
    })
}

fn device_ref(m: &Member) -> DeviceRef {
    DeviceRef { user_id: m.user_id.clone(), device_id: m.device_id.clone() }
}

fn rand_unit() -> f64 {
    let mut b = [0u8; 4];
    let _ = getrandom::fill(&mut b);
    f64::from(u32::from_le_bytes(b)) / f64::from(u32::MAX)
}

/// Once a message has gone (it leaves the list) or hasn't (it stays, saying why).
pub fn settle_pending(dms: &mut DmState, id: &str, nonce: u64, result: &Result<()>) {
    let Some(list) = dms.sending.get_mut(id) else { return };
    match result {
        Ok(()) => list.retain(|p| p.nonce != nonce),
        Err(err) => {
            if let Some(p) = list.iter_mut().find(|p| p.nonce == nonce) {
                p.failed = Some(err.0.clone());
            }
        }
    }
    if list.is_empty() {
        dms.sending.remove(id);
    }
}

impl crate::core::Core {
    /// Lets go of a message that didn't send.
    pub fn dismiss_dm(&self, key: &str, id: &str, nonce: u64) {
        self.shared.instance(key, |i| {
            if let Some(list) = i.dms.sending.get_mut(id) {
                list.retain(|p| p.nonce != nonce);
            }
        });
    }

    /// Sends again a message that didn't go.
    pub async fn retry_dm(&self, key: &str, id: &str, nonce: u64) -> Result<()> {
        let text = self.shared.read(|s| {
            s.instance(key)?
                .dms
                .sending
                .get(id)?
                .iter()
                .find(|p| p.nonce == nonce)
                .map(|p| (p.text.clone(), p.thread, p.in_channel))
        });
        let Some((text, thread, in_channel)) = text else { return Ok(()) };
        self.dismiss_dm(key, id, nonce);
        let content = if thread > 0 {
            Content::Reply { text, thread, in_channel, files: Vec::new() }
        } else {
            Content::Text { text, reply_to: 0 }
        };
        self.send_dm(key, id, content).await
    }

    /// Locks or unlocks a secure channel's thread: a signed line only devices read, counted from people with Manage Messages.
    pub async fn lock_secure_thread(&self, key: &str, id: &str, parent: i64, locked: bool) -> Result<()> {
        self.send_dm(key, id, Content::Lock { parent, locked }).await
    }

    /// Follows a secure channel's thread by hand, or stops (kept on this device only).
    pub async fn follow_secure_thread(&self, key: &str, id: &str, parent: i64, on: bool) -> Result<()> {
        let engine = self.dm_engine(key).ok_or_else(|| DmError("Encrypted messages aren't ready yet.".into()))?;
        engine.follow_thread(id, parent, on).await;
        Ok(())
    }

    /// Notes that you've seen a secure channel's thread up to `seq`.
    pub async fn mark_secure_thread_read(&self, key: &str, id: &str, parent: i64, seq: i64) {
        if let Some(engine) = self.dm_engine(key) {
            engine.mark_thread_read(id, parent, seq).await;
        }
    }

    /// Every device these people are signed in on, as the instance lists them (for the encryption dialog).
    pub async fn devices_of(&self, key: &str, user_ids: Vec<String>) -> Result<Vec<pb::Device>> {
        let api = self.api(key).ok_or_else(|| DmError("That instance isn't here.".into()))?;
        let req = pb::ListDevicesRequest { user_ids };
        Ok(rpc!(api.dms(), list_devices(req)).await?.devices)
    }
}

#[cfg(test)]
mod pending_tests {
    use super::*;

    #[test]
    fn a_message_that_went_leaves_and_one_that_didnt_says_why() {
        let mut dms = DmState::default();
        let pending =
            |nonce| DmPending { nonce, text: "hi".into(), created_at: 0, failed: None, thread: 0, in_channel: false };
        dms.sending.insert("c".into(), vec![pending(1), pending(2)]);
        settle_pending(&mut dms, "c", 1, &Ok(()));
        settle_pending(&mut dms, "c", 2, &Err(DmError("nope".into())));
        let list = &dms.sending["c"];
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].failed.as_deref(), Some("nope"));
    }
}
