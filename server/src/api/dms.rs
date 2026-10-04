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

use super::{Api, respond};
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
/// The most one account's message backup holds.
pub const MAX_BACKUP_BYTES: i64 = 64 * 1024 * 1024;
/// The largest backup part.
const MAX_BACKUP_PART_BYTES: usize = 256 * 1024;
/// The most backup part bytes one ListBackupParts call returns.
const MAX_BACKUP_LIST_BYTES: usize = 3 * 1024 * 1024;
/// How long a key check is.
const KEY_CHECK_BYTES: usize = 32;
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
    /// The caller, for message backups: people only (an agent reads nothing
    /// encrypted).
    async fn backup_owner(&self, metadata: &MetadataMap) -> Result<Account> {
        let account = self.account(metadata).await?;
        if account.kind == pb::AccountKind::Agent {
            return Err(Error::FailedPrecondition(AGENTS_HAVE_NO_DMS.into()));
        }
        Ok(account)
    }

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
        let mut ids: Vec<&str> = req.user_ids.iter().map(String::as_str).collect();
        ids.sort_unstable();
        ids.dedup();
        for &id in &ids {
            if id != account.id && !partners.contains(id) && !self.app.index.share_a_server(&account.id, id) {
                return Err(Error::denied("you can only see the devices of people you share a server with"));
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
        let mut ids: Vec<&str> = req.device_ids.iter().map(String::as_str).collect();
        ids.sort_unstable();
        ids.dedup();
        let devices = dms.devices(&ids).await?;
        // Secure channels add the devices of people who share a server with
        // you, the same people you could open a conversation with.
        if devices.iter().any(|device| {
            device.account_id != account.id
                && !partners.contains(&device.account_id)
                && !self.app.index.share_a_server(&account.id, &device.account_id)
        }) {
            return Err(Error::denied("you can only add the devices of people you share a server with"));
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
        // Strangers' devices (people you share only a server with) give up a
        // limited number of single-use key packages an hour.
        let strangers: Vec<&str> = devices
            .iter()
            .filter(|device| {
                device.account_id != account.id
                    && !partners.contains(&device.account_id)
                    && claimable.contains(&device.id.as_str())
            })
            .map(|device| device.id.as_str())
            .collect();
        let allowed = dms.take_stranger_claims(&account.id, &strangers, now_ms());
        let last_resort_only: HashSet<&str> = strangers.into_iter().filter(|id| !allowed.contains(id)).collect();
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
        if !self.app.index.share_a_server(&account.id, with) && !dms.partners(&account.id).await?.contains(with) {
            return Err(Error::denied("you can only message people you share a server with"));
        }
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
            dms.publish(&row.participants, event);
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
                let (records, has_more) = dms.records(&conversation.id, req.after_sequence.max(0), limit).await?;
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
                let OnDevice { device, .. } = self.on_device(request.metadata()).await?;
                Ok(pb::ListWelcomesResponse { welcomes: self.app.dms()?.welcomes(&device.id).await? })
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

    async fn get_backup(
        &self,
        request: Request<pb::GetBackupRequest>,
    ) -> Result<Response<pb::GetBackupResponse>, Status> {
        respond(
            async {
                let account = self.backup_owner(request.metadata()).await?;
                let backup = self.app.dms()?.backup(&account.id).await?;
                Ok(pb::GetBackupResponse { backup: backup.map(|b| b.to_pb(MAX_BACKUP_BYTES)) })
            }
            .await,
        )
    }

    async fn start_backup(
        &self,
        request: Request<pb::StartBackupRequest>,
    ) -> Result<Response<pb::StartBackupResponse>, Status> {
        respond(
            async {
                let account = self.backup_owner(request.metadata()).await?;
                let req = request.get_ref();
                if req.key_check.len() != KEY_CHECK_BYTES {
                    return Err(Error::invalid(format!("a key check is {KEY_CHECK_BYTES} bytes")));
                }
                let backup = self.app.dms()?.start_backup(&account.id, &req.key_check, req.replace).await?;
                Ok(pb::StartBackupResponse { backup: Some(backup.to_pb(MAX_BACKUP_BYTES)) })
            }
            .await,
        )
    }

    async fn add_backup_part(
        &self,
        request: Request<pb::AddBackupPartRequest>,
    ) -> Result<Response<pb::AddBackupPartResponse>, Status> {
        respond(
            async {
                let account = self.backup_owner(request.metadata()).await?;
                let req = request.get_ref();
                if req.data.is_empty() || req.data.len() > MAX_BACKUP_PART_BYTES {
                    return Err(Error::invalid(format!("a backup part is 1 to {} KiB", MAX_BACKUP_PART_BYTES / 1024)));
                }
                let backup = self
                    .app
                    .dms()?
                    .add_backup_part(&account.id, &req.key_check, req.sequence, &req.data, MAX_BACKUP_BYTES)
                    .await?;
                Ok(pb::AddBackupPartResponse { sequence: req.sequence, backup: Some(backup.to_pb(MAX_BACKUP_BYTES)) })
            }
            .await,
        )
    }

    async fn list_backup_parts(
        &self,
        request: Request<pb::ListBackupPartsRequest>,
    ) -> Result<Response<pb::ListBackupPartsResponse>, Status> {
        respond(
            async {
                let account = self.backup_owner(request.metadata()).await?;
                let req = request.get_ref();
                let limit = match req.limit {
                    0 => 50,
                    n => i64::from(n.clamp(1, 100)),
                };
                let (parts, has_more) =
                    self.app.dms()?.backup_parts(&account.id, req.after_sequence, limit, MAX_BACKUP_LIST_BYTES).await?;
                Ok(pb::ListBackupPartsResponse { parts, has_more })
            }
            .await,
        )
    }

    async fn delete_backup(
        &self,
        request: Request<pb::DeleteBackupRequest>,
    ) -> Result<Response<pb::DeleteBackupResponse>, Status> {
        respond(
            async {
                let account = self.backup_owner(request.metadata()).await?;
                self.app.dms()?.delete_backup(&account.id).await?;
                Ok(pb::DeleteBackupResponse {})
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
                            let response = pb::WatchResponse { ready: false, event: Some((*event).clone()) };
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
