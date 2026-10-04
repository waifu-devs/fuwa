//! Direct messages: delivering end-to-end encrypted conversations. Every check
//! here reads only what MLS leaves unencrypted (`fuwa_e2ee::wire`): which
//! conversation and epoch a message is for, what kind it is, and whose keys a
//! key package carries. What's in a message only its devices can read.

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::time::Duration;

use futures::Stream;
use fuwa_e2ee::wire::{self, ContentType, Sender, WireFormat};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};

use super::friends::Reach;
use super::{Api, respond};
use crate::app::App;
use crate::dms::{ConversationRow, DeviceRow, KeyPackage, MAX_KEY_PACKAGES, NewRecord};
use crate::error::{Error, Result};
use crate::id::{now_ms, timestamp};
use crate::node::Account;
use crate::pb::{self, direct_message_service_server::DirectMessageService};

/// The largest key package taken.
const MAX_KEY_PACKAGE_BYTES: usize = 16 * 1024;
/// The largest encrypted message: 4000 characters of text, with room to spare.
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
/// The largest commit, group info or welcome. They grow with the number of
/// devices in a conversation.
const MAX_COMMIT_BYTES: usize = 512 * 1024;
/// The most people or devices one call asks about.
const MAX_LOOKUPS: usize = 100;
/// How often an idle stream gets a heartbeat, so proxies don't close it.
const HEARTBEAT: Duration = Duration::from_secs(25);
/// What an open stream gets when the instance stops.
const RESTARTING: &str = "this instance is restarting; watch again";

type WatchStream = Pin<Box<dyn Stream<Item = Result<pb::WatchResponse, Status>> + Send>>;

/// Someone signed in, on a device that registered for direct messages.
struct OnDevice {
    account: Account,
    device: DeviceRow,
}

/// Direct messages are end-to-end encrypted between people's devices; an
/// agent has none.
const AGENTS_HAVE_NO_DMS: &str = "agents can't use direct messages";

/// When `viewer` blocked someone in this conversation, the block's time if
/// the conversation is from before it; `Err(())` inside when it was opened
/// after (it's kept from them altogether).
async fn blocked_at(
    friends: &crate::friends::Friends,
    viewer: &str,
    conversation: &ConversationRow,
) -> Result<Option<std::result::Result<i64, ()>>> {
    let mut at: Option<i64> = None;
    for other in conversation.participants.iter().filter(|id| *id != viewer) {
        if let Some(link) = friends.link(viewer, other, 0).await?
            && link.state == pb::FriendState::Blocked
        {
            at = Some(at.map_or(link.created_at, |at| at.min(link.created_at)));
        }
    }
    Ok(at.map(|at| if conversation.created_at > at { Err(()) } else { Ok(at) }))
}

/// Keeps what someone `viewer` blocked sent out of what they read: a
/// message's ciphertext goes, as if deleted before they read it; the group's
/// commits still reach them, so the conversation works again once they
/// unblock. A conversation that person opened after the block isn't there
/// for `viewer` at all: false means leave the record out.
async fn shown_to(app: &App, viewer: &str, record: &mut pb::ConversationRecord) -> Result<bool> {
    if record.sender_id == viewer {
        return Ok(true);
    }
    let friends = app.friends()?;
    if !friends.blocked(viewer, &record.sender_id).await? {
        return Ok(true);
    }
    let Some(conversation) = app.dms()?.conversation(&record.conversation_id).await? else { return Ok(false) };
    if matches!(blocked_at(friends, viewer, &conversation).await?, Some(Err(()))) {
        return Ok(false);
    }
    if record.kind == pb::ConversationRecordKind::Message as i32 {
        record.data.clear();
    }
    Ok(true)
}

fn malformed(err: wire::Malformed) -> Error {
    Error::invalid(err.to_string())
}

fn epoch(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| Error::invalid("that epoch is out of range"))
}

/// Checks a key package is for `device` and the conversations' cipher suite,
/// and works out when it runs out.
fn key_package(device: &DeviceRow, data: &[u8], last_resort: bool, now: i64) -> Result<KeyPackage> {
    if data.len() > MAX_KEY_PACKAGE_BYTES {
        return Err(Error::invalid(format!("a key package can be at most {MAX_KEY_PACKAGE_BYTES} bytes")));
    }
    let info = wire::key_package_info(data).map_err(malformed)?;
    if info.cipher_suite != fuwa_e2ee::CIPHER_SUITE {
        return Err(Error::invalid(format!("key packages must use cipher suite {:#06x}", fuwa_e2ee::CIPHER_SUITE)));
    }
    if info.signature_key != device.signature_key {
        return Err(Error::invalid("a key package is for another device's key"));
    }
    if info.identity != device.account_id.as_bytes() {
        return Err(Error::invalid("a key package names another account"));
    }
    if info.last_resort != last_resort {
        return Err(Error::invalid(if last_resort {
            "the last-resort key package isn't marked last resort"
        } else {
            "a single-use key package is marked last resort"
        }));
    }
    let expires_at = i64::try_from(info.not_after).unwrap_or(i64::MAX / 1000).saturating_mul(1000);
    if expires_at <= now {
        return Err(Error::invalid("a key package has run out"));
    }
    Ok(KeyPackage { data: data.to_vec(), expires_at })
}

/// Shown for someone whose account was deleted, as servers show them.
fn deleted_user(id: &str) -> pb::User {
    pb::User {
        id: id.to_string(),
        username: "deleted".into(),
        display_name: "Deleted account".into(),
        ..Default::default()
    }
}

impl Api {
    /// The caller and the device their session registered.
    async fn on_device(&self, metadata: &MetadataMap) -> Result<OnDevice> {
        let caller = self.caller(metadata).await?;
        let session_id = self.app.node()?.session_id(&caller.token_hash).await?.ok_or(Error::Unauthenticated)?;
        let device = self
            .app
            .dms()?
            .session_device(&session_id)
            .await?
            .ok_or_else(|| Error::FailedPrecondition("register this device for direct messages first".into()))?;
        Ok(OnDevice { account: caller.account, device })
    }

    /// Someone's friends, who may open conversations with them without a
    /// server in common.
    async fn friend_set(&self, account_id: &str) -> Result<HashSet<String>> {
        Ok(self.app.friends()?.friend_ids(account_id).await?.into_iter().collect())
    }

    /// Someone's conversations as they see them: one with a person they
    /// blocked stays where it was when they blocked them (what's sent since
    /// is kept from them), and one that person opened since isn't there.
    async fn as_seen_by(&self, account_id: &str, mut rows: Vec<ConversationRow>) -> Result<Vec<ConversationRow>> {
        let friends = self.app.friends()?;
        let mut kept = Vec::with_capacity(rows.len());
        for mut row in rows.drain(..) {
            match blocked_at(friends, account_id, &row).await? {
                Some(Err(())) => continue,
                Some(Ok(at)) => row.updated_at = row.updated_at.min(at),
                None => {}
            }
            kept.push(row);
        }
        kept.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| b.id.cmp(&a.id)));
        Ok(kept)
    }

    /// The devices of these accounts whose sessions are still signed in.
    async fn live_devices(&self, account_ids: &[&str]) -> Result<Vec<DeviceRow>> {
        let live = self.app.node()?.live_session_ids(Some(account_ids)).await?;
        let mut devices = self.app.dms()?.devices_of(account_ids).await?;
        devices.retain(|device| live.contains(&device.session_id));
        Ok(devices)
    }

    /// Conversations as clients see them, with both people in each.
    async fn conversations_pb(&self, rows: &[ConversationRow]) -> Result<Vec<pb::Conversation>> {
        let mut ids: Vec<&str> = rows.iter().flat_map(|row| row.participants.iter().map(String::as_str)).collect();
        ids.sort_unstable();
        ids.dedup();
        let users: HashMap<String, pb::User> = self
            .app
            .node()?
            .accounts(&ids)
            .await?
            .into_iter()
            .map(|account| (account.id.clone(), account.user()))
            .collect();
        Ok(rows
            .iter()
            .map(|row| pb::Conversation {
                id: row.id.clone(),
                users: row
                    .participants
                    .iter()
                    .map(|id| users.get(id).cloned().unwrap_or_else(|| deleted_user(id)))
                    .collect(),
                epoch: row.epoch,
                last_sequence: row.last_seq,
                created_at: Some(timestamp(row.created_at)),
                updated_at: Some(timestamp(row.updated_at)),
            })
            .collect())
    }

    async fn register_device(
        &self,
        metadata: &MetadataMap,
        req: pb::RegisterDeviceRequest,
    ) -> Result<pb::RegisterDeviceResponse> {
        let caller = self.caller(metadata).await?;
        if caller.account.kind == pb::AccountKind::Agent {
            return Err(Error::FailedPrecondition(AGENTS_HAVE_NO_DMS.into()));
        }
        let node = self.app.node()?;
        let session_id = node.session_id(&caller.token_hash).await?.ok_or(Error::Unauthenticated)?;
        if req.signature_key.len() != 32 {
            return Err(Error::invalid("signature_key must be an Ed25519 public key (32 bytes)"));
        }
        if req.key_packages.len() > MAX_KEY_PACKAGES as usize {
            return Err(Error::invalid(format!("send at most {MAX_KEY_PACKAGES} key packages")));
        }
        let now = now_ms();
        let device = DeviceRow {
            id: fuwa_e2ee::device_id(&req.signature_key),
            account_id: caller.account.id.clone(),
            session_id: session_id.clone(),
            signature_key: req.signature_key,
            label: crate::dms::device_label(&crate::auth::user_agent(metadata)),
            created_at: now,
        };
        let packages =
            req.key_packages.iter().map(|data| key_package(&device, data, false, now)).collect::<Result<Vec<_>>>()?;
        let last_resort = match req.last_resort_key_package.is_empty() {
            true => None,
            false => Some(key_package(&device, &req.last_resort_key_package, true, now)?.data),
        };
        let live = node.live_session_ids(Some(&[caller.account.id.as_str()])).await?;
        let dms = self.app.dms()?;
        let count = dms.register_device(&device, &packages, last_resort.as_deref(), &live).await?;
        let stored = dms.session_device(&session_id).await?.unwrap_or(device);
        Ok(pb::RegisterDeviceResponse { device: Some(stored.to_pb()), key_packages: count as i32 })
    }

    async fn add_key_packages(
        &self,
        metadata: &MetadataMap,
        req: pb::AddKeyPackagesRequest,
    ) -> Result<pb::AddKeyPackagesResponse> {
        let OnDevice { device, .. } = self.on_device(metadata).await?;
        if req.key_packages.len() > MAX_KEY_PACKAGES as usize {
            return Err(Error::invalid(format!("send at most {MAX_KEY_PACKAGES} key packages")));
        }
        let now = now_ms();
        let packages =
            req.key_packages.iter().map(|data| key_package(&device, data, false, now)).collect::<Result<Vec<_>>>()?;
        let count = self.app.dms()?.add_key_packages(&device.id, &packages).await?;
        Ok(pb::AddKeyPackagesResponse { key_packages: count as i32 })
    }

    async fn list_devices(
        &self,
        metadata: &MetadataMap,
        req: pb::ListDevicesRequest,
    ) -> Result<pb::ListDevicesResponse> {
        let account = self.account(metadata).await?;
        if req.user_ids.len() > MAX_LOOKUPS {
            return Err(Error::invalid(format!("ask about at most {MAX_LOOKUPS} people at once")));
        }
        let partners = self.app.dms()?.partners(&account.id).await?;
        let friends = self.friend_set(&account.id).await?;
        let mut ids: Vec<&str> = req.user_ids.iter().map(String::as_str).collect();
        ids.sort_unstable();
        ids.dedup();
        for &id in &ids {
            if id != account.id
                && !partners.contains(id)
                && !friends.contains(id)
                && !self.app.index.share_a_server(&account.id, id)
            {
                return Err(Error::denied(
                    "you can only see the devices of friends and people you share a server with",
                ));
            }
        }
        let devices = self.live_devices(&ids).await?;
        Ok(pb::ListDevicesResponse { devices: devices.iter().map(DeviceRow::to_pb).collect() })
    }

    async fn claim_key_packages(
        &self,
        metadata: &MetadataMap,
        req: pb::ClaimKeyPackagesRequest,
    ) -> Result<pb::ClaimKeyPackagesResponse> {
        let account = self.account(metadata).await?;
        if req.device_ids.len() > MAX_LOOKUPS {
            return Err(Error::invalid(format!("claim at most {MAX_LOOKUPS} key packages at once")));
        }
        let dms = self.app.dms()?;
        let partners = dms.partners(&account.id).await?;
        let friends = self.friend_set(&account.id).await?;
        let mut ids: Vec<&str> = req.device_ids.iter().map(String::as_str).collect();
        ids.sort_unstable();
        ids.dedup();
        let devices = dms.devices(&ids).await?;
        // Secure channels add the devices of people who share a server with
        // you; those and your friends are who you could open a conversation with.
        if devices.iter().any(|device| {
            device.account_id != account.id
                && !partners.contains(&device.account_id)
                && !friends.contains(&device.account_id)
                && !self.app.index.share_a_server(&account.id, &device.account_id)
        }) {
            return Err(Error::denied("you can only add the devices of friends and people you share a server with"));
        }
        let mut owners: Vec<&str> = devices.iter().map(|device| device.account_id.as_str()).collect();
        owners.sort_unstable();
        owners.dedup();
        let live = self.app.node()?.live_session_ids(Some(&owners)).await?;
        let claimable: Vec<&str> = devices
            .iter()
            .filter(|device| live.contains(&device.session_id))
            .map(|device| device.id.as_str())
            .collect();
        // Everyone else's devices give up a limited number of single-use key
        // packages an hour, whatever they are to you: partners too, so a
        // partner who blocked you answers as one who didn't.
        let others: Vec<&str> = devices
            .iter()
            .filter(|device| device.account_id != account.id && claimable.contains(&device.id.as_str()))
            .map(|device| device.id.as_str())
            .collect();
        let allowed = dms.take_stranger_claims(&account.id, &others, now_ms());
        let last_resort_only: HashSet<&str> = others.into_iter().filter(|id| !allowed.contains(id)).collect();
        let claimed = dms.claim_key_packages(&claimable, &last_resort_only).await?;
        Ok(pb::ClaimKeyPackagesResponse {
            key_packages: claimed
                .into_iter()
                .map(|(device_id, key_package)| pb::ClaimedKeyPackage { device_id, key_package })
                .collect(),
        })
    }

    async fn open_conversation(
        &self,
        metadata: &MetadataMap,
        req: pb::OpenConversationRequest,
    ) -> Result<pb::OpenConversationResponse> {
        let account = self.account(metadata).await?;
        let with = req.user_id.trim();
        if with == account.id {
            return Err(Error::invalid("you can't start a conversation with yourself"));
        }
        let dms = self.app.dms()?;
        let existing = dms.partners(&account.id).await?.contains(with);
        // Who may start one is theirs to say (friends, people in a server
        // with them, nobody new). Someone they blocked gets one as usual,
        // but they aren't told of it.
        let reach = self.may_message(&account.id, with, existing).await?;
        let other = self.app.node()?.account(with).await?.ok_or(Error::NotFound("user"))?;
        if account.kind == pb::AccountKind::Agent || other.kind == pb::AccountKind::Agent {
            return Err(Error::FailedPrecondition(AGENTS_HAVE_NO_DMS.into()));
        }
        let (row, created) = dms.open_conversation(&account.id, with).await?;
        let conversation = self.conversations_pb(std::slice::from_ref(&row)).await?.remove(0);
        if created {
            let event = pb::DirectMessageEvent {
                payload: Some(pb::direct_message_event::Payload::ConversationOpened(conversation.clone())),
            };
            let told = if reach == Reach::Hidden { vec![account.id.clone()] } else { row.participants.clone() };
            dms.publish(&told, event);
        }
        Ok(pb::OpenConversationResponse { conversation: Some(conversation), created })
    }

    async fn post_commit(&self, metadata: &MetadataMap, req: pb::PostCommitRequest) -> Result<pb::PostCommitResponse> {
        let OnDevice { account, device } = self.on_device(metadata).await?;
        let dms = self.app.dms()?;
        let conversation = dms.conversation_of(&account.id, &req.conversation_id).await?;
        if [&req.commit, &req.group_info, &req.welcome].iter().any(|part| part.len() > MAX_COMMIT_BYTES) {
            return Err(Error::invalid(format!(
                "a commit, its group info and its welcome can each be at most {MAX_COMMIT_BYTES} bytes"
            )));
        }
        let header = wire::message_header(&req.commit).map_err(malformed)?;
        if header.group_id != conversation.id.as_bytes() {
            return Err(Error::invalid("that commit is for another conversation"));
        }
        if header.content_type != ContentType::Commit {
            return Err(Error::invalid("that isn't a commit"));
        }
        match (header.wire_format, header.sender) {
            (WireFormat::PrivateMessage, _) => {}
            // Joining by yourself needs the group info a first commit leaves.
            (WireFormat::PublicMessage, Some(Sender::NewMemberCommit)) if conversation.epoch > 0 => {}
            _ => return Err(Error::invalid("a commit must be encrypted, unless it's a device joining by itself")),
        }
        let info = wire::group_info_header(&req.group_info).map_err(malformed)?;
        if info.group_id != conversation.id.as_bytes()
            || info.cipher_suite != fuwa_e2ee::CIPHER_SUITE
            || info.epoch != header.epoch.saturating_add(1)
        {
            return Err(Error::invalid("group_info must be this conversation's group as the commit leaves it"));
        }
        let welcomed = match (req.welcome.is_empty(), req.welcome_device_ids.is_empty()) {
            (true, true) => None,
            (false, false) if wire::is_welcome(&req.welcome) => {
                let devices: HashSet<String> = self
                    .live_devices(&conversation.participants.iter().map(String::as_str).collect::<Vec<_>>())
                    .await?
                    .into_iter()
                    .map(|device| device.id)
                    .collect();
                if req.welcome_device_ids.iter().any(|id| !devices.contains(id)) {
                    return Err(Error::invalid(
                        "welcome_device_ids must be signed-in devices of this conversation's people",
                    ));
                }
                Some((req.welcome.as_slice(), req.welcome_device_ids.as_slice()))
            }
            _ => return Err(Error::invalid("send a welcome together with the devices it's for")),
        };
        let record = dms
            .append(&NewRecord {
                conversation_id: &conversation.id,
                kind: pb::ConversationRecordKind::Commit,
                epoch: epoch(header.epoch)?,
                sender_id: &account.id,
                sender_device_id: &device.id,
                data: &req.commit,
                group_info: Some(&req.group_info),
                welcome: welcomed,
            })
            .await?;
        Ok(pb::PostCommitResponse { record: Some(record) })
    }

    async fn post_message(
        &self,
        metadata: &MetadataMap,
        req: pb::PostMessageRequest,
    ) -> Result<pb::PostMessageResponse> {
        let OnDevice { account, device } = self.on_device(metadata).await?;
        let dms = self.app.dms()?;
        let conversation = dms.conversation_of(&account.id, &req.conversation_id).await?;
        if req.message.len() > MAX_MESSAGE_BYTES {
            return Err(Error::invalid(format!("a message can be at most {MAX_MESSAGE_BYTES} bytes")));
        }
        let header = wire::message_header(&req.message).map_err(malformed)?;
        if header.group_id != conversation.id.as_bytes() {
            return Err(Error::invalid("that message is for another conversation"));
        }
        if header.wire_format != WireFormat::PrivateMessage || header.content_type != ContentType::Application {
            return Err(Error::invalid("a message must be an encrypted application message"));
        }
        if conversation.epoch == 0 {
            return Err(Error::FailedPrecondition("add the conversation's devices before sending".into()));
        }
        // Taken even from someone the other blocked; it's kept from them
        // when they read (`shown_to`).
        for other in conversation.participants.iter().filter(|id| **id != account.id) {
            self.may_message(&account.id, other, true).await?;
        }
        let record = dms
            .append(&NewRecord {
                conversation_id: &conversation.id,
                kind: pb::ConversationRecordKind::Message,
                epoch: epoch(header.epoch)?,
                sender_id: &account.id,
                sender_device_id: &device.id,
                data: &req.message,
                group_info: None,
                welcome: None,
            })
            .await?;
        Ok(pb::PostMessageResponse { record: Some(record) })
    }
}

#[tonic::async_trait]
impl DirectMessageService for Api {
    async fn register_device(
        &self,
        request: Request<pb::RegisterDeviceRequest>,
    ) -> Result<Response<pb::RegisterDeviceResponse>, Status> {
        let (metadata, _, req) = request.into_parts();
        respond(Api::register_device(self, &metadata, req).await)
    }

    async fn add_key_packages(
        &self,
        request: Request<pb::AddKeyPackagesRequest>,
    ) -> Result<Response<pb::AddKeyPackagesResponse>, Status> {
        let (metadata, _, req) = request.into_parts();
        respond(Api::add_key_packages(self, &metadata, req).await)
    }

    async fn list_devices(
        &self,
        request: Request<pb::ListDevicesRequest>,
    ) -> Result<Response<pb::ListDevicesResponse>, Status> {
        let (metadata, _, req) = request.into_parts();
        respond(Api::list_devices(self, &metadata, req).await)
    }

    async fn claim_key_packages(
        &self,
        request: Request<pb::ClaimKeyPackagesRequest>,
    ) -> Result<Response<pb::ClaimKeyPackagesResponse>, Status> {
        let (metadata, _, req) = request.into_parts();
        respond(Api::claim_key_packages(self, &metadata, req).await)
    }

    async fn open_conversation(
        &self,
        request: Request<pb::OpenConversationRequest>,
    ) -> Result<Response<pb::OpenConversationResponse>, Status> {
        let (metadata, _, req) = request.into_parts();
        respond(Api::open_conversation(self, &metadata, req).await)
    }

    async fn list_conversations(
        &self,
        request: Request<pb::ListConversationsRequest>,
    ) -> Result<Response<pb::ListConversationsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let rows = self.app.dms()?.conversations(&account.id).await?;
                let rows = self.as_seen_by(&account.id, rows).await?;
                Ok(pb::ListConversationsResponse { conversations: self.conversations_pb(&rows).await? })
            }
            .await,
        )
    }

    async fn get_group_info(
        &self,
        request: Request<pb::GetGroupInfoRequest>,
    ) -> Result<Response<pb::GetGroupInfoResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let dms = self.app.dms()?;
                let conversation = dms.conversation_of(&account.id, &request.get_ref().conversation_id).await?;
                let (epoch, group_info) = dms.group_info(&conversation.id).await?;
                Ok(pb::GetGroupInfoResponse { epoch, group_info: group_info.unwrap_or_default() })
            }
            .await,
        )
    }

    async fn list_records(
        &self,
        request: Request<pb::ListRecordsRequest>,
    ) -> Result<Response<pb::ListRecordsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.get_ref();
                let dms = self.app.dms()?;
                let conversation = dms.conversation_of(&account.id, &req.conversation_id).await?;
                let limit = match req.limit {
                    0 => 100,
                    limit @ 1..=500 => i64::from(limit),
                    _ => return Err(Error::invalid("limit must be 1 to 500")),
                };
                let (mut records, has_more) = dms.records(&conversation.id, req.after_sequence.max(0), limit).await?;
                match blocked_at(self.app.friends()?, &account.id, &conversation).await? {
                    Some(Err(())) => return Err(Error::NotFound("conversation")),
                    Some(Ok(_)) => {
                        for record in &mut records {
                            shown_to(&self.app, &account.id, record).await?;
                        }
                    }
                    None => {}
                }
                Ok(pb::ListRecordsResponse { records, has_more })
            }
            .await,
        )
    }

    async fn list_welcomes(
        &self,
        request: Request<pb::ListWelcomesRequest>,
    ) -> Result<Response<pb::ListWelcomesResponse>, Status> {
        respond(
            async {
                let OnDevice { account, device } = self.on_device(request.metadata()).await?;
                let dms = self.app.dms()?;
                let mut welcomes = Vec::new();
                // None into a conversation someone they blocked opened since.
                for welcome in dms.welcomes(&device.id).await? {
                    let hidden = match dms.conversation(&welcome.conversation_id).await? {
                        Some(conversation) => {
                            matches!(blocked_at(self.app.friends()?, &account.id, &conversation).await?, Some(Err(())))
                        }
                        None => true,
                    };
                    if !hidden {
                        welcomes.push(welcome);
                    }
                }
                Ok(pb::ListWelcomesResponse { welcomes })
            }
            .await,
        )
    }

    async fn post_commit(
        &self,
        request: Request<pb::PostCommitRequest>,
    ) -> Result<Response<pb::PostCommitResponse>, Status> {
        let (metadata, _, req) = request.into_parts();
        respond(Api::post_commit(self, &metadata, req).await)
    }

    async fn post_message(
        &self,
        request: Request<pb::PostMessageRequest>,
    ) -> Result<Response<pb::PostMessageResponse>, Status> {
        let (metadata, _, req) = request.into_parts();
        respond(Api::post_message(self, &metadata, req).await)
    }

    async fn delete_record(
        &self,
        request: Request<pb::DeleteRecordRequest>,
    ) -> Result<Response<pb::DeleteRecordResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.get_ref();
                let dms = self.app.dms()?;
                let conversation = dms.conversation_of(&account.id, &req.conversation_id).await?;
                dms.delete_record(&account.id, &conversation.id, req.sequence).await?;
                Ok(pb::DeleteRecordResponse {})
            }
            .await,
        )
    }

    type WatchStream = WatchStream;

    async fn watch(&self, request: Request<pb::WatchRequest>) -> Result<Response<WatchStream>, Status> {
        let caller = self.caller(request.metadata()).await?;
        let mut events = self.app.dms()?.watch(&caller.account.id);
        let (tx, rx) = mpsc::channel::<Result<pb::WatchResponse, Status>>(64);
        let app = self.app.clone();
        tokio::spawn(async move {
            let send = async |item| tx.send(item).await.is_ok();
            if !send(Ok(pb::WatchResponse { ready: true, event: None })).await {
                return;
            }
            let mut heartbeat = tokio::time::interval(HEARTBEAT);
            heartbeat.tick().await;
            loop {
                tokio::select! {
                    _ = app.shutdown.cancelled() => {
                        // Stopping, say for a deploy: tell the client to watch again.
                        let _ = tx.try_send(Err(Status::unavailable(RESTARTING)));
                        return;
                    }
                    _ = tx.closed() => return,
                    _ = heartbeat.tick() => {
                        // A session signed out elsewhere stops getting events.
                        let live = async { app.node()?.session_live(&caller.token_hash).await }.await;
                        if matches!(live, Ok(false)) {
                            let _ = tx.send(Err(Error::Unauthenticated.into())).await;
                            return;
                        }
                        if !send(Ok(pb::WatchResponse { ready: false, event: None })).await {
                            return;
                        }
                    }
                    event = events.recv() => match event {
                        Ok(event) => {
                            let mut event = (*event).clone();
                            if let Some(
                                pb::direct_message_event::Payload::RecordAdded(record)
                                | pb::direct_message_event::Payload::RecordDeleted(record),
                            ) = &mut event.payload
                            {
                                match shown_to(&app, &caller.account.id, record).await {
                                    Ok(true) => {}
                                    Ok(false) => continue,
                                    Err(_) => {
                                        // Can't tell whether it's from someone they blocked:
                                        // they watch again and catch up by listing.
                                        let _ = tx.send(Err(Status::unavailable("watch again"))).await;
                                        return;
                                    }
                                }
                            }
                            let response = pb::WatchResponse { ready: false, event: Some(event) };
                            if !send(Ok(response)).await {
                                return;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            let _ = tx.send(Err(Status::aborted("fell behind; catch up and watch again"))).await;
                            return;
                        }
                        Err(broadcast::error::RecvError::Closed) => return,
                    },
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}
