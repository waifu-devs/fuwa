//! Secure channels: delivering a community server's end-to-end encrypted
//! channels. As with direct messages (`api/dms.rs`), every check here reads
//! only what MLS leaves unencrypted (`fuwa_e2ee::wire`): which channel and
//! epoch a record is for and what kind it is. Who may be in a channel's group
//! is who its permissions let see it; the devices are the ones people
//! registered for direct messages, which the directory keeps
//! ([`App::secure_devices`](crate::app::App)).

use std::collections::HashSet;

use fuwa_e2ee::wire::{self, ContentType, Sender, WireFormat};
use tonic::{Request, Response, Status};

use super::messages::{check_not_timed_out, check_slowmode};
use super::{Api, Seat, respond};
use crate::auth::Caller;
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{millis, now_ms, timestamp};
use crate::pb::{self, Permission, secure_channel_service_server::SecureChannelService};
use crate::permissions;
use crate::servers::{self as store, Audit, MEMBER_COLUMNS, Payload, ServerDb, UsageChange, load_channel, member_row};

/// The most people one secure channel can hold. Every device of each of them
/// is in its MLS group, and the group info a joining device downloads grows
/// with every one.
pub const MAX_SECURE_MEMBERS: usize = 500;
/// The largest encrypted message: 4000 characters of text, with room to spare.
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
/// The largest commit, group info or welcome. They grow with the number of
/// devices in the channel.
const MAX_COMMIT_BYTES: usize = 2 * 1024 * 1024;

const RECORD_COLUMNS: &str =
    "channel_id, seq, kind, epoch, sender_id, sender_device_id, data, created_at, deleted_at, deleted_by";

fn record_row(r: &turso::Row) -> turso::Result<pb::SecureRecord> {
    Ok(pb::SecureRecord {
        channel_id: r.get(0)?,
        sequence: r.get(1)?,
        kind: r.get(2)?,
        epoch: r.get(3)?,
        sender_id: r.get(4)?,
        sender_device_id: r.get(5)?,
        data: r.get::<Option<Vec<u8>>>(6)?.unwrap_or_default(),
        created_at: Some(timestamp(r.get(7)?)),
        deleted_at: r.get::<Option<i64>>(8)?.map(timestamp),
        deleted_by: r.get::<Option<String>>(9)?.unwrap_or_default(),
    })
}

fn malformed(err: wire::Malformed) -> Error {
    Error::invalid(err.to_string())
}

fn epoch(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| Error::invalid("that epoch is out of range"))
}

/// The people whose devices belong in a secure channel's group: every member
/// who can see it now, as their roles, the channel's overwrites, single
/// sign-on and time-outs leave them. Agents have no devices, so never.
pub async fn secure_members(conn: &turso::Connection, server_id: &str, channel_id: &str) -> Result<Vec<String>> {
    let mut members = query_all(
        conn,
        &format!("SELECT {MEMBER_COLUMNS} FROM members JOIN users ON users.id = members.user_id ORDER BY users.id"),
        (),
        member_row(server_id),
    )
    .await?;
    members.retain(|m| m.user.as_ref().is_some_and(|u| u.kind != pb::AccountKind::Agent as i32));
    permissions::attach_roles(conn, &mut members).await?;
    let rules = permissions::load(conn, server_id).await?;
    let sso = store::load_sso(conn).await?;
    let now = now_ms();
    Ok(members
        .into_iter()
        .filter_map(|member| {
            let id = member.user?.id;
            let mut access = rules.access(&id, &member.role_ids);
            if sso.required && !sso.fresh(member.sso_signed_in_at.as_ref().map(millis), now) {
                access.lock_out();
            }
            access.can_see(channel_id).then_some(id)
        })
        .collect())
}

/// The channel, if it's a secure one the caller can see.
async fn secure_channel(conn: &turso::Connection, seat: &Seat, channel_id: &str) -> Result<pb::Channel> {
    seat.access.require_in(channel_id, Permission::ViewChannels)?;
    let channel = load_channel(conn, &seat.sdb.id, channel_id).await?.ok_or(Error::NotFound("channel"))?;
    if channel.r#type != pb::ChannelType::Secure as i32 {
        return Err(Error::invalid("that isn't a secure channel"));
    }
    Ok(channel)
}

/// The group's epoch, last sequence and group info. A channel whose first
/// commit hasn't come is at epoch 0 with none.
async fn group(conn: &turso::Connection, channel_id: &str) -> Result<(i64, i64, Option<Vec<u8>>)> {
    Ok(query_one(
        conn,
        "SELECT epoch, last_seq, group_info FROM secure_groups WHERE channel_id = ?1",
        [channel_id],
        |r| Ok((r.get::<i64>(0)?, r.get::<i64>(1)?, r.get::<Option<Vec<u8>>>(2)?)),
    )
    .await?
    .unwrap_or((0, 0, None)))
}

/// A record to add to a channel's log.
struct NewRecord<'a> {
    channel_id: &'a str,
    kind: pb::SecureRecordKind,
    epoch: i64,
    sender_id: &'a str,
    sender_device_id: &'a str,
    data: &'a [u8],
    group_info: Option<&'a [u8]>,
    welcome: Option<(&'a [u8], &'a [String])>,
}

/// Adds a record inside a write, if it's for the group's current epoch (a
/// commit moves it on), and sends its event.
async fn append(
    conn: &turso::Connection,
    record: &NewRecord<'_>,
    events: &mut Vec<Payload>,
) -> Result<pb::SecureRecord> {
    let commit = record.kind == pb::SecureRecordKind::Commit;
    let now = now_ms();
    conn.execute(
        "INSERT OR IGNORE INTO secure_groups (channel_id, updated_at) VALUES (?1, ?2)",
        (record.channel_id, now),
    )
    .await?;
    let moved = conn
        .execute(
            "UPDATE secure_groups SET epoch = epoch + ?3, last_seq = last_seq + 1, updated_at = ?4,
             group_info = coalesce(?5, group_info) WHERE channel_id = ?1 AND epoch = ?2",
            (
                record.channel_id,
                record.epoch,
                i64::from(commit),
                now,
                record.group_info.filter(|_| commit).map(<[u8]>::to_vec),
            ),
        )
        .await?;
    let (current, last_seq, _) = group(conn, record.channel_id).await?;
    if moved == 0 {
        return Err(Error::FailedPrecondition(format!(
            "the channel is at epoch {current}, not {}: catch up and try again",
            record.epoch
        )));
    }
    let stored = pb::SecureRecord {
        channel_id: record.channel_id.to_string(),
        sequence: last_seq,
        kind: record.kind as i32,
        epoch: record.epoch,
        sender_id: record.sender_id.to_string(),
        sender_device_id: record.sender_device_id.to_string(),
        data: record.data.to_vec(),
        created_at: Some(timestamp(now)),
        deleted_at: None,
        deleted_by: String::new(),
    };
    conn.execute(
        &format!("INSERT INTO secure_records ({RECORD_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, NULL)"),
        (
            record.channel_id,
            stored.sequence,
            stored.kind,
            stored.epoch,
            record.sender_id,
            record.sender_device_id,
            record.data.to_vec(),
            now,
        ),
    )
    .await?;
    // The sender is in: whatever welcome it had here is used up.
    conn.execute(
        "DELETE FROM secure_welcomes WHERE channel_id = ?1 AND device_id = ?2",
        (record.channel_id, record.sender_device_id),
    )
    .await?;
    if let Some((welcome, device_ids)) = record.welcome.filter(|_| commit) {
        for device_id in device_ids {
            conn.execute(
                "INSERT OR REPLACE INTO secure_welcomes (channel_id, device_id, seq, data, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                (record.channel_id, device_id.as_str(), stored.sequence, welcome.to_vec(), now),
            )
            .await?;
        }
    }
    events.push(Payload::SecureRecordAdded(pb::SecureRecordAdded { record: Some(stored.clone()) }));
    Ok(stored)
}

/// Deletes what a secure channel kept, inside the write that deletes the
/// channel, and says how many messages and bytes of them went.
pub(super) async fn forget_channel(conn: &turso::Connection, channel_id: &str) -> Result<(i64, i64)> {
    let gone = query_one(
        conn,
        "SELECT count(*), coalesce(sum(length(data)), 0) FROM secure_records WHERE channel_id = ?1 AND kind = ?2 AND data IS NOT NULL",
        (channel_id, pb::SecureRecordKind::Message as i64),
        |r| Ok((r.get::<i64>(0)?, r.get::<i64>(1)?)),
    )
    .await?
    .unwrap_or_default();
    for table in ["secure_records", "secure_welcomes", "secure_groups"] {
        conn.execute(&format!("DELETE FROM {table} WHERE channel_id = ?1"), [channel_id]).await?;
    }
    Ok(gone)
}

impl Api {
    /// The caller, their place in the server, and the device their session
    /// registered for end-to-end encryption.
    async fn on_secure_device(
        &self,
        metadata: &tonic::metadata::MetadataMap,
        server_id: &str,
    ) -> Result<(Caller, Seat, String)> {
        let caller = self.caller(metadata).await?;
        let seat = self.membership(&caller.account, server_id).await?;
        let device = self
            .app
            .secure_devices(&caller.token_hash, &[])
            .await?
            .caller
            .ok_or_else(|| Error::FailedPrecondition("register this device for encrypted messages first".into()))?;
        Ok((caller, seat, device))
    }

    async fn post_secure_commit(
        &self,
        metadata: &tonic::metadata::MetadataMap,
        req: pb::PostSecureCommitRequest,
    ) -> Result<pb::PostSecureCommitResponse> {
        let (caller, seat, device) = self.on_secure_device(metadata, &req.server_id).await?;
        let conn = seat.sdb.read()?;
        let channel = secure_channel(&conn, &seat, &req.channel_id).await?;
        if [&req.commit, &req.group_info, &req.welcome].iter().any(|part| part.len() > MAX_COMMIT_BYTES) {
            return Err(Error::invalid(format!(
                "a commit, its group info and its welcome can each be at most {MAX_COMMIT_BYTES} bytes"
            )));
        }
        let header = wire::message_header(&req.commit).map_err(malformed)?;
        if header.group_id != channel.id.as_bytes() {
            return Err(Error::invalid("that commit is for another channel"));
        }
        if header.content_type != ContentType::Commit {
            return Err(Error::invalid("that isn't a commit"));
        }
        let (current, _, _) = group(&conn, &channel.id).await?;
        match (header.wire_format, header.sender) {
            (WireFormat::PrivateMessage, _) => {}
            // Joining by yourself needs the group info a first commit leaves.
            (WireFormat::PublicMessage, Some(Sender::NewMemberCommit)) if current > 0 => {}
            _ => return Err(Error::invalid("a commit must be encrypted, unless it's a device joining by itself")),
        }
        let info = wire::group_info_header(&req.group_info).map_err(malformed)?;
        if info.group_id != channel.id.as_bytes()
            || info.cipher_suite != fuwa_e2ee::CIPHER_SUITE
            || info.epoch != header.epoch.saturating_add(1)
        {
            return Err(Error::invalid("group_info must be this channel's group as the commit leaves it"));
        }
        let welcomed = match (req.welcome.is_empty(), req.welcome_device_ids.is_empty()) {
            (true, true) => None,
            (false, false) if wire::is_welcome(&req.welcome) => {
                let members = secure_members(&conn, &seat.sdb.id, &channel.id).await?;
                if members.len() > MAX_SECURE_MEMBERS {
                    return Err(too_many());
                }
                let devices: HashSet<String> = self
                    .app
                    .secure_devices(&caller.token_hash, &members)
                    .await?
                    .devices
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect();
                if req.welcome_device_ids.iter().any(|id| !devices.contains(id)) {
                    return Err(Error::invalid(
                        "welcome_device_ids must be signed-in devices of people who can see this channel",
                    ));
                }
                Some((req.welcome.as_slice(), req.welcome_device_ids.as_slice()))
            }
            _ => return Err(Error::invalid("send a welcome together with the devices it's for")),
        };
        let record = seat
            .sdb
            .write(&caller.account.id, async |conn, events| {
                append(
                    conn,
                    &NewRecord {
                        channel_id: &channel.id,
                        kind: pb::SecureRecordKind::Commit,
                        epoch: epoch(header.epoch)?,
                        sender_id: &caller.account.id,
                        sender_device_id: &device,
                        data: &req.commit,
                        group_info: Some(&req.group_info),
                        welcome: welcomed,
                    },
                    events,
                )
                .await
            })
            .await?;
        Ok(pb::PostSecureCommitResponse { record: Some(record) })
    }

    async fn post_secure_message(
        &self,
        metadata: &tonic::metadata::MetadataMap,
        req: pb::PostSecureMessageRequest,
    ) -> Result<pb::PostSecureMessageResponse> {
        let (caller, seat, device) = self.on_secure_device(metadata, &req.server_id).await?;
        check_not_timed_out(&seat.member)?;
        let conn = seat.sdb.read()?;
        let channel = secure_channel(&conn, &seat, &req.channel_id).await?;
        seat.access.require_in(&channel.id, Permission::SendMessages)?;
        if req.message.len() > MAX_MESSAGE_BYTES {
            return Err(Error::invalid(format!("a message can be at most {MAX_MESSAGE_BYTES} bytes")));
        }
        let header = wire::message_header(&req.message).map_err(malformed)?;
        if header.group_id != channel.id.as_bytes() {
            return Err(Error::invalid("that message is for another channel"));
        }
        if header.wire_format != WireFormat::PrivateMessage || header.content_type != ContentType::Application {
            return Err(Error::invalid("a message must be an encrypted application message"));
        }
        let limits = seat.sdb.limits(&self.app.settings().limits).await?;
        if let Some(limit) = limits.storage_bytes
            && seat.sdb.storage_bytes() >= limit
        {
            return Err(Error::ResourceExhausted("this server is out of storage".into()));
        }
        let exempt = seat.access.has_in(&channel.id, Permission::ManageMessages)
            || seat.access.has_in(&channel.id, Permission::ManageChannels);
        let record = seat
            .sdb
            .write(&caller.account.id, async |conn, events| {
                let (current, _, _) = group(conn, &channel.id).await?;
                if current == 0 {
                    return Err(Error::FailedPrecondition("add the channel's devices before sending".into()));
                }
                if !exempt {
                    check_slowmode(conn, &channel, &caller.account.id, now_ms()).await?;
                }
                let record = append(
                    conn,
                    &NewRecord {
                        channel_id: &channel.id,
                        kind: pb::SecureRecordKind::Message,
                        epoch: epoch(header.epoch)?,
                        sender_id: &caller.account.id,
                        sender_device_id: &device,
                        data: &req.message,
                        group_info: None,
                        welcome: None,
                    },
                    events,
                )
                .await?;
                store::add_usage(
                    conn,
                    UsageChange {
                        messages: 1,
                        messages_sent: 1,
                        message_bytes: req.message.len() as i64,
                        ..Default::default()
                    },
                )
                .await?;
                Ok(record)
            })
            .await?;
        Ok(pb::PostSecureMessageResponse { record: Some(record) })
    }
}

fn too_many() -> Error {
    Error::FailedPrecondition(format!(
        "a secure channel can hold at most {MAX_SECURE_MEMBERS} people; narrow who can see it"
    ))
}

async fn list_records(
    sdb: &ServerDb,
    channel_id: &str,
    after: i64,
    limit: i64,
) -> Result<(Vec<pb::SecureRecord>, bool)> {
    let conn = sdb.read()?;
    let mut records = query_all(
        &conn,
        &format!(
            "SELECT {RECORD_COLUMNS} FROM secure_records WHERE channel_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3"
        ),
        (channel_id, after, limit + 1),
        record_row,
    )
    .await?;
    let has_more = records.len() as i64 > limit;
    records.truncate(limit as usize);
    Ok((records, has_more))
}

#[tonic::async_trait]
impl SecureChannelService for Api {
    async fn get_secure_channel(
        &self,
        request: Request<pb::GetSecureChannelRequest>,
    ) -> Result<Response<pb::GetSecureChannelResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.get_ref();
                let seat = self.membership(&account, &req.server_id).await?;
                let conn = seat.sdb.read()?;
                let channel = secure_channel(&conn, &seat, &req.channel_id).await?;
                let member_ids = secure_members(&conn, &seat.sdb.id, &channel.id).await?;
                if member_ids.len() > MAX_SECURE_MEMBERS {
                    return Err(too_many());
                }
                let (epoch, last_sequence, _) = group(&conn, &channel.id).await?;
                Ok(pb::GetSecureChannelResponse { epoch, last_sequence, member_ids })
            }
            .await,
        )
    }

    async fn get_secure_group_info(
        &self,
        request: Request<pb::GetSecureGroupInfoRequest>,
    ) -> Result<Response<pb::GetSecureGroupInfoResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.get_ref();
                let seat = self.membership(&account, &req.server_id).await?;
                let conn = seat.sdb.read()?;
                let channel = secure_channel(&conn, &seat, &req.channel_id).await?;
                let (epoch, _, group_info) = group(&conn, &channel.id).await?;
                Ok(pb::GetSecureGroupInfoResponse { epoch, group_info: group_info.unwrap_or_default() })
            }
            .await,
        )
    }

    async fn list_secure_records(
        &self,
        request: Request<pb::ListSecureRecordsRequest>,
    ) -> Result<Response<pb::ListSecureRecordsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.get_ref();
                let seat = self.membership(&account, &req.server_id).await?;
                let channel = secure_channel(&seat.sdb.read()?, &seat, &req.channel_id).await?;
                let limit = match req.limit {
                    0 => 100,
                    limit @ 1..=500 => i64::from(limit),
                    _ => return Err(Error::invalid("limit must be 1 to 500")),
                };
                let (records, has_more) =
                    list_records(&seat.sdb, &channel.id, req.after_sequence.max(0), limit).await?;
                Ok(pb::ListSecureRecordsResponse { records, has_more })
            }
            .await,
        )
    }

    async fn list_secure_welcomes(
        &self,
        request: Request<pb::ListSecureWelcomesRequest>,
    ) -> Result<Response<pb::ListSecureWelcomesResponse>, Status> {
        respond(
            async {
                let (_, seat, device) = self.on_secure_device(request.metadata(), &request.get_ref().server_id).await?;
                let conn = seat.sdb.read()?;
                let mut welcomes = query_all(
                    &conn,
                    "SELECT channel_id, seq, data FROM secure_welcomes WHERE device_id = ?1 ORDER BY channel_id",
                    [device.as_str()],
                    |r| Ok(pb::SecureWelcome { channel_id: r.get(0)?, sequence: r.get(1)?, data: r.get(2)? }),
                )
                .await?;
                welcomes.retain(|w| seat.access.can_see(&w.channel_id));
                Ok(pb::ListSecureWelcomesResponse { welcomes })
            }
            .await,
        )
    }

    async fn post_secure_commit(
        &self,
        request: Request<pb::PostSecureCommitRequest>,
    ) -> Result<Response<pb::PostSecureCommitResponse>, Status> {
        let (metadata, _, req) = request.into_parts();
        respond(Api::post_secure_commit(self, &metadata, req).await)
    }

    async fn post_secure_message(
        &self,
        request: Request<pb::PostSecureMessageRequest>,
    ) -> Result<Response<pb::PostSecureMessageResponse>, Status> {
        let (metadata, _, req) = request.into_parts();
        respond(Api::post_secure_message(self, &metadata, req).await)
    }

    async fn delete_secure_record(
        &self,
        request: Request<pb::DeleteSecureRecordRequest>,
    ) -> Result<Response<pb::DeleteSecureRecordResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let seat = self.membership(&account, &req.server_id).await?;
                let channel = secure_channel(&seat.sdb.read()?, &seat, &req.channel_id).await?;
                let moderator = seat.access.has_in(&channel.id, Permission::ManageMessages);
                seat.sdb
                    .write(&account.id, async |conn, events| {
                        let record = query_one(
                            conn,
                            &format!("SELECT {RECORD_COLUMNS} FROM secure_records WHERE channel_id = ?1 AND seq = ?2"),
                            (channel.id.as_str(), req.sequence),
                            record_row,
                        )
                        .await?
                        .ok_or(Error::NotFound("record"))?;
                        let own = record.sender_id == account.id;
                        if !own && !moderator {
                            return Err(permissions::missing(Permission::ManageMessages));
                        }
                        if own {
                            seat.access.require_not_timed_out()?;
                        }
                        if record.kind != pb::SecureRecordKind::Message as i32 {
                            return Err(Error::invalid("only messages can be deleted"));
                        }
                        if record.deleted_at.is_some() {
                            return Ok(());
                        }
                        let deleted_by = (!own).then_some(account.id.as_str());
                        conn.execute(
                            "UPDATE secure_records SET data = NULL, deleted_at = ?3, deleted_by = ?4 WHERE channel_id = ?1 AND seq = ?2",
                            (channel.id.as_str(), req.sequence, now_ms(), deleted_by),
                        )
                        .await?;
                        if !own {
                            store::audit(
                                conn,
                                &account.id,
                                Audit::new(pb::AuditAction::MessageDelete, &record.sender_id).channel(&channel.name),
                            )
                            .await?;
                        }
                        store::add_usage(
                            conn,
                            UsageChange { messages: -1, message_bytes: -(record.data.len() as i64), ..Default::default() },
                        )
                        .await?;
                        events.push(Payload::SecureRecordDeleted(pb::SecureRecordDeleted {
                            channel_id: channel.id.clone(),
                            sequence: req.sequence,
                            deleted_by: deleted_by.unwrap_or_default().to_string(),
                        }));
                        Ok(())
                    })
                    .await?;
                Ok(pb::DeleteSecureRecordResponse {})
            }
            .await,
        )
    }
}
