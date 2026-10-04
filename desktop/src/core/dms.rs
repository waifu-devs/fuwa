//! Encrypted direct messages for one signed-in account on one instance: this
//! app's device, kept in the vault, following every conversation. A port of
//! the web app's `web/src/e2ee/engine.ts`, on `fuwa-e2ee` directly (no
//! WebAssembly); `docs/e2ee.md` has the design.
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

use std::collections::{HashMap, HashSet};
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
use crate::core::vault::{Change, DeviceRef, Item, ItemKind, Note, Vault, sha256_hex};
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
    /// Messages on their way, per conversation.
    pub sending: HashMap<String, Vec<String>>,
    /// The calls going on, per conversation.
    pub calls: HashMap<String, pb::DmCall>,
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
    Text { text: String, reply_to: i64 },
    Edit { sequence: i64, text: String },
}

fn encode(content: &Content) -> Vec<u8> {
    use pb::direct_message_content::Body;
    let body = match content {
        Content::Text { text, reply_to } => {
            Body::Text(pb::DirectMessageText { content: text.clone(), reply_to_sequence: *reply_to })
        }
        Content::Edit { sequence, text } => {
            Body::Edit(pb::DirectMessageEdit { sequence: *sequence, content: text.clone() })
        }
    };
    pb::DirectMessageContent { body: Some(body) }.encode_to_vec()
}

fn ms(record: &pb::ConversationRecord) -> i64 {
    record.created_at.as_ref().map(|t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000).unwrap_or_else(now_ms)
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

/// What opening a record found.
enum Opened {
    Fine,
    /// The device lost track of the group and has to join again.
    Rejoin,
}

/// The device and its vault: only touched under the engine's lock.
struct Inner {
    device: Device,
    vault: Vault,
}

impl Inner {
    fn save(&mut self, mut change: Change) -> Result<()> {
        change.device = Some(self.device.save());
        Ok(self.vault.write(change)?)
    }
}

pub struct DmEngine {
    key: String,
    api: Api,
    me: pb::User,
    shared: Shared,
    device_id: String,
    inner: Mutex<Inner>,
    conversations: parking_lot::Mutex<HashMap<String, pb::Conversation>>,
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
            stop: CancellationToken::new(),
        }))
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn stop(&self) {
        self.stop.cancel();
    }

    fn stopped(&self) -> bool {
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
                if let Err(err) = self.resync().await {
                    tracing::warn!("couldn't list conversations: {err}");
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
                let this = self.clone();
                tokio::spawn(async move { this.forget_deleted(&record.conversation_id, record.sequence).await });
            }
            Payload::CallUpdated(call) => self.update(|s| calls::set_dm_call(&mut s.calls, call)),
        }
    }

    /// Catches up on a conversation in the background.
    pub fn queue(self: &Arc<Self>, conversation: String) {
        let this = self.clone();
        tokio::spawn(async move {
            if this.stopped() {
                return;
            }
            {
                let mut inner = this.inner.lock().await;
                if let Err(err) = this.catch_up(&mut inner, &conversation, 0).await {
                    tracing::warn!("couldn't catch up on a conversation: {err}");
                }
            }
            this.refresh(&conversation).await;
        });
    }

    // ───────────────────────── Under the lock ─────────────────────────

    fn allowed(c: &pb::Conversation) -> Vec<String> {
        c.users.iter().map(|u| u.id.clone()).collect()
    }

    /// Reads a conversation's records this device hasn't, joining it first if it isn't in.
    async fn catch_up(&self, inner: &mut Inner, id: &str, depth: u8) -> Result<()> {
        let Some(c) = self.conversation(id) else { return Ok(()) };
        let mut note = inner.vault.note(id);
        if !inner.device.is_member(id) {
            self.update(|d| {
                d.joining.insert(id.to_owned());
            });
            let joined = self.join(inner, &c, note.clone()).await;
            self.update(|d| {
                d.joining.remove(id);
            });
            match joined? {
                Some(next) => note = next,
                None => return Ok(()),
            }
        }
        loop {
            let page = rpc!(
                self.api.dms(),
                list_records(pb::ListRecordsRequest {
                    conversation_id: id.to_owned(),
                    after_sequence: note.cursor,
                    limit: PAGE,
                })
            )
            .await?;
            let mut change = Change::default();
            let mut rejoin = false;
            let mut fresh = Vec::new();
            for record in &page.records {
                let opened = self.open(inner, &c, record, &mut change, &mut fresh)?;
                note.cursor = record.sequence;
                if let Opened::Rejoin = opened {
                    rejoin = true;
                    break;
                }
            }
            change.notes.push((id.to_owned(), note.clone()));
            inner.save(change)?;
            for item in fresh {
                self.shared.notify_dm(&self.key, &c, &item);
            }
            if rejoin {
                if depth < 2 {
                    return Box::pin(self.catch_up(inner, id, depth + 1)).await;
                }
                return Ok(());
            }
            if !page.has_more || page.records.is_empty() {
                return Ok(());
            }
        }
    }

    /// Opens one record and notes what it said.
    fn open(
        &self,
        inner: &mut Inner,
        c: &pb::Conversation,
        record: &pb::ConversationRecord,
        change: &mut Change,
        fresh: &mut Vec<Item>,
    ) -> Result<Opened> {
        let seq = record.sequence;
        let at = ms(record);
        let id = c.id.clone();
        let known = |inner: &mut Inner, change: &Change, seq: i64| -> Result<Option<Item>> {
            if let Some((_, item)) = change.items.iter().rev().find(|(_, i)| i.seq == seq) {
                return Ok(Some(item.clone()));
            }
            Ok(inner.vault.items(&id)?.iter().find(|i| i.seq == seq).cloned())
        };
        if record.data.is_empty() {
            // Deleted before this device read it.
            if let Some(mut before) = known(inner, change, seq)?
                && !before.deleted
            {
                before.deleted = true;
                before.content.clear();
                change.items.push((id, before));
            }
            return Ok(Opened::Fine);
        }
        let own = record.sender_device_id == self.device_id;
        let out = match inner.device.process(&c.id, &record.data, own, &Self::allowed(c)) {
            Ok(out) => out,
            Err(err) => {
                // A commit this device can't follow leaves it out of the group: it joins again.
                if matches!(err, E2eeError::Behind) || record.kind == pb::ConversationRecordKind::Commit as i32 {
                    tracing::warn!("lost track of a conversation's group; joining it again: {err}");
                    inner.device.forget(&c.id)?;
                    return Ok(Opened::Rejoin);
                }
                let item = Item::new(seq, ItemKind::Unreadable, at, &record.sender_id, &record.sender_device_id);
                change.items.push((id, item));
                return Ok(Opened::Fine);
            }
        };
        match out {
            Processed::Message { sender, plaintext } => {
                self.read(inner, change, fresh, c, seq, at, &sender.user_id, &sender.device_id, &plaintext)?;
            }
            Processed::Own => {
                let hash = sha256_hex(&record.data);
                match inner.vault.sent(&hash) {
                    Some(plaintext) => {
                        let (me, device) = (self.me.id.clone(), self.device_id.clone());
                        self.read(inner, change, fresh, c, seq, at, &me, &device, &plaintext)?;
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
                    inner.device.forget(&c.id)?;
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

    /// What a message said: new text, or an edit of the sender's own earlier message.
    #[allow(clippy::too_many_arguments)]
    fn read(
        &self,
        inner: &mut Inner,
        change: &mut Change,
        fresh: &mut Vec<Item>,
        c: &pb::Conversation,
        seq: i64,
        at: i64,
        sender_id: &str,
        device_id: &str,
        plaintext: &[u8],
    ) -> Result<()> {
        use pb::direct_message_content::Body;
        let id = c.id.clone();
        let Ok(content) = pb::DirectMessageContent::decode(plaintext) else {
            change.items.push((id, Item::new(seq, ItemKind::Unreadable, at, sender_id, device_id)));
            return Ok(());
        };
        match content.body {
            Some(Body::Text(text)) => {
                let mut item = Item::new(seq, ItemKind::Text, at, sender_id, device_id);
                item.content = clip(&text.content);
                item.reply_to = text.reply_to_sequence;
                let had = inner.vault.items(&id)?.iter().any(|i| i.seq == seq);
                if !had && sender_id != self.me.id {
                    fresh.push(item.clone());
                }
                change.items.push((id, item));
            }
            Some(Body::Edit(edit)) => {
                let target = match change.items.iter().rev().find(|(_, i)| i.seq == edit.sequence) {
                    Some((_, item)) => Some(item.clone()),
                    None => inner.vault.items(&id)?.iter().find(|i| i.seq == edit.sequence).cloned(),
                };
                if let Some(mut target) = target
                    && target.kind == ItemKind::Text
                    && target.sender_id == sender_id
                    && !target.deleted
                {
                    target.content = clip(&edit.content);
                    target.edited_at = at;
                    change.items.push((id, target));
                }
            }
            // Voice messages play in the web app for now; here they're a line
            // saying one came, so the conversation still reads in order.
            Some(Body::Voice(voice)) => {
                let secs = voice.duration_ms / 1000;
                let mut item = Item::new(seq, ItemKind::Text, at, sender_id, device_id);
                item.content =
                    format!("Voice message ({}:{:02}), open it in the web app to play it", secs / 60, secs % 60);
                item.reply_to = voice.reply_to_sequence;
                let had = inner.vault.items(&id)?.iter().any(|i| i.seq == seq);
                if !had && sender_id != self.me.id {
                    fresh.push(item.clone());
                }
                change.items.push((id, item));
            }
            // Anything else is from a newer app (or for secure channels, which
            // this app doesn't open yet): there's nothing to show for it here.
            Some(Body::Signed(_) | Body::History(_)) | None => {}
        }
        Ok(())
    }

    /// Joins a conversation's group: from a welcome if someone added this
    /// device, or else by itself from the group's public state. None if nobody
    /// started the group yet.
    async fn join(&self, inner: &mut Inner, c: &pb::Conversation, note: Note) -> Result<Option<Note>> {
        let allowed = Self::allowed(c);
        let welcomes = rpc!(self.api.dms(), list_welcomes(pb::ListWelcomesRequest {})).await?.welcomes;
        if let Some(welcome) = welcomes.iter().find(|w| w.conversation_id == c.id) {
            match inner.device.join_from_welcome(&c.id, &welcome.data, &allowed) {
                Ok(_) => {
                    let next = Note { cursor: welcome.sequence, ..note };
                    let item = Item::new(welcome.sequence, ItemKind::Joined, now_ms(), &self.me.id, &self.device_id);
                    inner.save(Change {
                        notes: vec![(c.id.clone(), next.clone())],
                        items: vec![(c.id.clone(), item)],
                        ..Change::default()
                    })?;
                    return Ok(Some(next));
                }
                // Used up or out of date: join by itself instead.
                Err(err) => tracing::warn!("couldn't join a conversation from its welcome: {err}"),
            }
        }
        for _ in 0..3 {
            let info =
                rpc!(self.api.dms(), get_group_info(pb::GetGroupInfoRequest { conversation_id: c.id.clone() })).await?;
            if info.epoch == 0 || info.group_info.is_empty() {
                return Ok(None);
            }
            let commit = inner.device.join_by_itself(&c.id, &info.group_info, &allowed)?;
            inner.save(Change::default())?;
            let posted = rpc!(
                self.api.dms(),
                post_commit(pb::PostCommitRequest {
                    conversation_id: c.id.clone(),
                    commit: commit.commit,
                    group_info: commit.group_info,
                    welcome: Vec::new(),
                    welcome_device_ids: Vec::new(),
                })
            )
            .await;
            match posted {
                Ok(posted) => {
                    let seq = posted.record.as_ref().map(|r| r.sequence).unwrap_or(0);
                    let at = posted.record.as_ref().map(ms).unwrap_or_else(now_ms);
                    let next = Note { cursor: seq, ..note };
                    let item = Item::new(seq, ItemKind::Joined, at, &self.me.id, &self.device_id);
                    inner.save(Change {
                        notes: vec![(c.id.clone(), next.clone())],
                        items: vec![(c.id.clone(), item)],
                        ..Change::default()
                    })?;
                    return Ok(Some(next));
                }
                Err(err) => {
                    inner.device.forget(&c.id)?;
                    inner.save(Change::default())?;
                    if !is_precondition(&err) {
                        return Err(err.into());
                    }
                }
            }
        }
        Err(DmError("Couldn't join this conversation; try again.".into()))
    }

    /// Makes the group hold exactly the devices both people are signed in on:
    /// starts it if nobody has, adds new devices, drops ones whose sessions ended.
    async fn reconcile(&self, inner: &mut Inner, c: &pb::Conversation) -> Result<()> {
        for attempt in 0.. {
            let allowed = Self::allowed(c);
            let devices =
                rpc!(self.api.dms(), list_devices(pb::ListDevicesRequest { user_ids: allowed.clone() })).await?.devices;
            let partner = c.users.iter().find(|u| u.id != self.me.id);
            if let Some(partner) = partner
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
            let starting = !inner.device.is_member(&c.id);
            if starting {
                inner.device.create_group(&c.id)?;
            }
            let members = inner.device.members(&c.id)?;
            let present: HashSet<&str> = members.iter().map(|m| m.device_id.as_str()).collect();
            let expected: HashSet<&str> = devices.iter().map(|d| d.id.as_str()).collect();
            let adds: Vec<String> =
                devices.iter().filter(|d| !present.contains(d.id.as_str())).map(|d| d.id.clone()).collect();
            let removes: Vec<String> = members
                .iter()
                .filter(|m| !expected.contains(m.device_id.as_str()) && m.device_id != self.device_id)
                .map(|m| m.device_id.clone())
                .collect();
            let claimed = if adds.is_empty() {
                Vec::new()
            } else {
                rpc!(self.api.dms(), claim_key_packages(pb::ClaimKeyPackagesRequest { device_ids: adds }))
                    .await?
                    .key_packages
            };
            if claimed.is_empty() && removes.is_empty() {
                if starting {
                    inner.device.forget(&c.id)?;
                    if partner.is_some() {
                        return Err(DmError("Couldn't reach their devices yet; try again in a moment.".into()));
                    }
                }
                return Ok(());
            }
            let adds: Vec<(String, Vec<u8>)> = claimed.into_iter().map(|k| (k.device_id, k.key_package)).collect();
            let commit = inner.device.commit(&c.id, &adds, &removes, &allowed)?;
            inner.save(Change::default())?;
            let welcome_ids = if commit.welcome.is_some() {
                commit.added.iter().map(|m| m.device_id.clone()).collect()
            } else {
                vec![]
            };
            let posted = rpc!(
                self.api.dms(),
                post_commit(pb::PostCommitRequest {
                    conversation_id: c.id.clone(),
                    commit: commit.commit,
                    group_info: commit.group_info,
                    welcome: commit.welcome.unwrap_or_default(),
                    welcome_device_ids: welcome_ids,
                })
            )
            .await;
            match posted {
                Ok(_) => return self.catch_up(inner, &c.id, 0).await,
                Err(err) => {
                    if inner.device.epoch(&c.id).unwrap_or(0) == 0 {
                        inner.device.forget(&c.id)?;
                    } else {
                        inner.device.discard_pending(&c.id)?;
                    }
                    inner.save(Change::default())?;
                    if !is_precondition(&err) || attempt >= 2 {
                        return Err(err.into());
                    }
                    // Someone else's commit got there first.
                    self.catch_up(inner, &c.id, 0).await?;
                }
            }
        }
        unreachable!()
    }

    // ───────────────────────── What people do ─────────────────────────

    /// The secret a call in this conversation seals its sound with, and the
    /// epoch it's from (see [`calls::CALL_LABEL`]). When someone's frames
    /// say they're from a newer epoch than `seen`, this catches up first.
    pub async fn call_secret(&self, id: &str, seen: Option<u64>) -> Result<(u64, Vec<u8>)> {
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

    /// Gets a conversation ready to write in, saying why it can't be yet.
    pub async fn prepare(self: &Arc<Self>, id: &str) -> Result<()> {
        let result = async {
            let c = self.conversation(id).ok_or_else(|| DmError("That conversation isn't here.".into()))?;
            let mut inner = self.inner.lock().await;
            self.catch_up(&mut inner, id, 0).await?;
            self.reconcile(&mut inner, &c).await
        }
        .await;
        self.update(|d| match &result {
            Ok(()) => {
                d.blocked.remove(id);
            }
            Err(err) => {
                d.blocked.insert(id.to_owned(), err.0.clone());
            }
        });
        self.refresh(id).await;
        result
    }

    /// Encrypts and sends a message (or an edit) to everyone in the conversation.
    pub async fn send(self: &Arc<Self>, id: &str, content: Content) -> Result<()> {
        let result = async {
            let c = self.conversation(id).ok_or_else(|| DmError("That conversation isn't here.".into()))?;
            let mut inner = self.inner.lock().await;
            self.catch_up(&mut inner, id, 0).await?;
            self.reconcile(&mut inner, &c).await?;
            let plaintext = encode(&content);
            for attempt in 0.. {
                let ciphertext = inner.device.encrypt(id, &plaintext)?;
                let hash = sha256_hex(&ciphertext);
                // Kept first: this device can't open what it sent, so this is how it knows what it said.
                inner.save(Change { sent: vec![(hash.clone(), plaintext.clone())], ..Change::default() })?;
                let posted = rpc!(
                    self.api.dms(),
                    post_message(pb::PostMessageRequest {
                        conversation_id: id.to_owned(),
                        message: ciphertext,
                        ..Default::default()
                    }),
                )
                .await;
                match posted {
                    Ok(_) => break,
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
        if let Err(err) = &result {
            self.update(|d| {
                d.blocked.insert(id.to_owned(), err.0.clone());
            });
        } else {
            self.update(|d| {
                d.blocked.remove(id);
            });
        }
        self.refresh(id).await;
        result
    }

    /// Deletes a message you sent: from the instance, and from every device's copy.
    pub async fn remove(self: &Arc<Self>, id: &str, seq: i64) -> Result<()> {
        rpc!(self.api.dms(), delete_record(pb::DeleteRecordRequest { conversation_id: id.to_owned(), sequence: seq }))
            .await?;
        self.forget_deleted(id, seq).await;
        Ok(())
    }

    async fn forget_deleted(&self, id: &str, seq: i64) {
        {
            let mut inner = self.inner.lock().await;
            let before = inner.vault.items(id).ok().and_then(|items| items.iter().find(|i| i.seq == seq).cloned());
            if let Some(mut before) = before
                && !before.deleted
            {
                before.deleted = true;
                before.content.clear();
                let _ = inner.vault.write(Change { items: vec![(id.to_owned(), before)], ..Change::default() });
            }
        }
        self.refresh(id).await;
    }

    /// Notes that you've seen everything in a conversation so far.
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
        self.update(|d| {
            d.unread.insert(id.to_owned(), 0);
        });
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

    // ───────────────────────── Showing it ─────────────────────────

    /// Puts what this app knows about a conversation in the store.
    pub async fn refresh(&self, id: &str) {
        let Some(c) = self.conversation(id) else { return };
        if self.stopped() {
            return;
        }
        let (items, note, members) = {
            let mut inner = self.inner.lock().await;
            let items = inner.vault.items(id).map(|i| i.clone()).unwrap_or_default();
            let note = inner.vault.note(id);
            let members =
                if inner.device.is_member(id) { inner.device.members(id).unwrap_or_default() } else { vec![] };
            (items, note, members)
        };
        let looking = self.shared.is_focused(&self.key, id);
        let unread = if looking {
            0
        } else {
            items
                .iter()
                .filter(|i| i.kind == ItemKind::Text && !i.deleted && i.sender_id != self.me.id && i.seq > note.read)
                .count() as u32
        };
        let safety = self.safety_number(&c, &members);
        let last = items.last().map(|i| i.seq).unwrap_or(0);
        self.update(|d| {
            d.items.insert(id.to_owned(), items);
            d.unread.insert(id.to_owned(), unread);
            d.members.insert(
                id.to_owned(),
                members
                    .iter()
                    .map(|m| DmMember { user_id: m.user_id.clone(), device_id: m.device_id.clone() })
                    .collect(),
            );
            d.safety.insert(id.to_owned(), safety);
            if note.verified.is_empty() {
                d.verified.remove(id);
            } else {
                d.verified.insert(id.to_owned(), note.verified.clone());
            }
        });
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
}

fn device_ref(m: &Member) -> DeviceRef {
    DeviceRef { user_id: m.user_id.clone(), device_id: m.device_id.clone() }
}

fn rand_unit() -> f64 {
    let mut b = [0u8; 4];
    let _ = getrandom::fill(&mut b);
    f64::from(u32::from_le_bytes(b)) / f64::from(u32::MAX)
}
