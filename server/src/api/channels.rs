use std::collections::{HashMap, HashSet};

use tonic::{Request, Response, Status};

use super::{Api, PictureOwner, Seat, respond, shared, text};
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::pb::{self, Permission, channel_service_server::ChannelService};
use crate::permissions::{self, Bits};
use crate::servers::{self as store, Audit, Payload, UsageChange, load_channel, load_channels, usage_count};

/// The longest slow mode, as Discord has it: six hours.
const MAX_SLOWMODE: i32 = 6 * 60 * 60;
/// Most overwrites one channel can have.
const MAX_OVERWRITES: usize = 100;

/// Channel names read like `#general`: lowercase, words joined by dashes.
/// Categories and voice channels keep their name as typed ("Lounge").
pub(super) fn channel_name(name: &str, kind: pb::ChannelType) -> Result<String> {
    let name = text("name", name, 1, 100)?;
    if matches!(kind, pb::ChannelType::Category | pb::ChannelType::Voice) {
        return Ok(name);
    }
    let slug = name
        .to_lowercase()
        .split(|c: char| c.is_whitespace() || c == '-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        return Err(Error::invalid("name must have at least one letter or digit"));
    }
    Ok(slug)
}

/// Checks that a parent exists and is a category.
async fn check_parent(conn: &turso::Connection, server_id: &str, parent_id: &str) -> Result<()> {
    if parent_id.is_empty() {
        return Ok(());
    }
    let parent = load_channel(conn, server_id, parent_id).await?.ok_or(Error::NotFound("parent channel"))?;
    if parent.r#type != pb::ChannelType::Category as i32 {
        return Err(Error::invalid("channels can only sit inside a category"));
    }
    Ok(())
}

#[tonic::async_trait]
impl ChannelService for Api {
    async fn create_channel(
        &self,
        request: Request<pb::CreateChannelRequest>,
    ) -> Result<Response<pb::CreateChannelResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                if req.parent_id.is_empty() {
                    access.require(Permission::ManageChannels)?;
                } else {
                    access.require_in(&req.parent_id, Permission::ManageChannels)?;
                }
                let kind = match pb::ChannelType::try_from(req.r#type) {
                    Ok(pb::ChannelType::Unspecified) | Err(_) => pb::ChannelType::Text,
                    Ok(kind) => kind,
                };
                let name = channel_name(&req.name, kind)?;
                let topic = text("topic", &req.topic, 0, 1024)?;
                let limits = sdb.limits(&self.app.settings().limits).await?;
                let channel = sdb
                    .write(&account.id, async |conn, events| {
                        if let Some(limit) = limits.channels
                            && usage_count(conn, "channels").await? >= limit
                        {
                            return Err(Error::ResourceExhausted(format!(
                                "this server can have at most {limit} channels"
                            )));
                        }
                        if kind == pb::ChannelType::Category && !req.parent_id.is_empty() {
                            return Err(Error::invalid("categories can't sit inside other channels"));
                        }
                        check_parent(conn, &sdb.id, &req.parent_id).await?;
                        let position =
                            query_one(conn, "SELECT coalesce(max(position) + 1, 0) FROM channels", (), |r| {
                                r.get::<i64>(0)
                            })
                            .await?
                            .unwrap_or(0);
                        let now = now_ms();
                        let channel = pb::Channel {
                            id: new_id(),
                            server_id: sdb.id.clone(),
                            name: name.clone(),
                            r#type: kind as i32,
                            parent_id: req.parent_id.clone(),
                            topic: topic.clone(),
                            position: position as i32,
                            created_at: Some(timestamp(now)),
                            updated_at: Some(timestamp(now)),
                            slowmode_seconds: 0,
                            permission_overwrites: vec![],
                            shared: None,
                        };
                        conn.execute(
                            "INSERT INTO channels (id, name, type, parent_id, topic, position, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                            (
                                channel.id.as_str(),
                                channel.name.as_str(),
                                kind as i64,
                                (!channel.parent_id.is_empty()).then_some(channel.parent_id.as_str()),
                                channel.topic.as_str(),
                                position,
                                now,
                            ),
                        )
                        .await?;
                        if kind == pb::ChannelType::Secure {
                            conn.execute(
                                "INSERT INTO secure_groups (channel_id, updated_at) VALUES (?1, ?2)",
                                (channel.id.as_str(), now),
                            )
                            .await?;
                        }
                        conn.execute("UPDATE usage SET channels = channels + 1, updated_at = ?1 WHERE id = 1", [now])
                            .await?;
                        store::audit(
                            conn,
                            &account.id,
                            Audit::new(pb::AuditAction::ChannelCreate, &channel.id).channel(&channel.name),
                        )
                        .await?;
                        events.push(Payload::ChannelCreated(pb::ChannelCreated { channel: Some(channel.clone()) }));
                        Ok(channel)
                    })
                    .await?;
                Ok(pb::CreateChannelResponse { channel: Some(channel) })
            }
            .await,
        )
    }

    async fn get_channel(
        &self,
        request: Request<pb::GetChannelRequest>,
    ) -> Result<Response<pb::GetChannelResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                access.require_in(&req.channel_id, Permission::ViewChannels)?;
                let channel =
                    load_channel(&sdb.read()?, &sdb.id, &req.channel_id).await?.ok_or(Error::NotFound("channel"))?;
                Ok(pb::GetChannelResponse { channel: Some(channel) })
            }
            .await,
        )
    }

    async fn list_channels(
        &self,
        request: Request<pb::ListChannelsRequest>,
    ) -> Result<Response<pb::ListChannelsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let Seat { sdb, access, .. } = self.membership(&account, &request.get_ref().server_id).await?;
                let mut channels = load_channels(&sdb.read()?, &sdb.id).await?;
                channels.retain(|c| access.can_see(&c.id));
                Ok(pb::ListChannelsResponse { channels })
            }
            .await,
        )
    }

    async fn update_channel(
        &self,
        request: Request<pb::UpdateChannelRequest>,
    ) -> Result<Response<pb::UpdateChannelResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                access.require_in(&req.channel_id, Permission::ManageChannels)?;
                if let Some(parent_id) = req.parent_id.as_deref().filter(|p| !p.is_empty()) {
                    access.require_in(parent_id, Permission::ManageChannels)?;
                }
                let topic = req.topic.as_deref().map(|v| text("topic", v, 0, 1024)).transpose()?;
                if req.slowmode_seconds.is_some_and(|v| !(0..=MAX_SLOWMODE).contains(&v)) {
                    return Err(Error::invalid(format!("slow mode can be 0 to {MAX_SLOWMODE} seconds")));
                }
                let channel = sdb
                    .write(&account.id, async |conn, events| {
                        let current =
                            load_channel(conn, &sdb.id, &req.channel_id).await?.ok_or(Error::NotFound("channel"))?;
                        let kind = pb::ChannelType::try_from(current.r#type).unwrap_or(pb::ChannelType::Text);
                        let name = req.name.as_deref().map(|v| channel_name(v, kind)).transpose()?;
                        if let Some(parent_id) = &req.parent_id {
                            // Moving a channel changes which overwrites apply to it, so it
                            // takes Manage Channels for the whole server, not only here.
                            if *parent_id != current.parent_id {
                                access.require(Permission::ManageChannels)?;
                            }
                            if kind == pb::ChannelType::Category && !parent_id.is_empty() {
                                return Err(Error::invalid("categories can't sit inside other channels"));
                            }
                            check_parent(conn, &sdb.id, parent_id).await?;
                        }
                        conn.execute(
                            "UPDATE channels SET name = coalesce(?2, name), topic = coalesce(?3, topic),
                         position = coalesce(?4, position),
                         parent_id = CASE WHEN ?5 IS NULL THEN parent_id WHEN ?5 = '' THEN NULL ELSE ?5 END,
                         slowmode_seconds = coalesce(?6, slowmode_seconds),
                         updated_at = ?7
                         WHERE id = ?1",
                            (
                                current.id.as_str(),
                                name,
                                topic,
                                req.position,
                                req.parent_id.as_deref(),
                                req.slowmode_seconds,
                                now_ms(),
                            ),
                        )
                        .await?;
                        if req.slowmode_seconds == Some(0) {
                            conn.execute("DELETE FROM slowmode WHERE channel_id = ?1", [current.id.as_str()]).await?;
                        }
                        let channel =
                            load_channel(conn, &sdb.id, &current.id).await?.ok_or(Error::NotFound("channel"))?;
                        let entry = Audit::new(pb::AuditAction::ChannelUpdate, &channel.id)
                            .channel(&current.name)
                            .change("name", &current.name, &channel.name)
                            .change("topic", &current.topic, &channel.topic)
                            .change("parent_id", &current.parent_id, &channel.parent_id)
                            .change("position", current.position, channel.position)
                            .change("slowmode_seconds", current.slowmode_seconds, channel.slowmode_seconds);
                        if !entry.changes.is_empty() {
                            store::audit(conn, &account.id, entry).await?;
                        }
                        events.push(Payload::ChannelUpdated(pb::ChannelUpdated { channel: Some(channel.clone()) }));
                        Ok(channel)
                    })
                    .await?;
                Ok(pb::UpdateChannelResponse { channel: Some(channel) })
            }
            .await,
        )
    }

    async fn delete_channel(
        &self,
        request: Request<pb::DeleteChannelRequest>,
    ) -> Result<Response<pb::DeleteChannelResponse>, Status> {
        respond(async {
            let account = self.account(request.metadata()).await?;
            let req = request.into_inner();
            let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
            access.require_in(&req.channel_id, Permission::ManageChannels)?;
            // Alone, so no message lands in the channel while it goes.
            let (server, pictures, ended) = sdb.write_alone(&account.id, async |conn, events| {
                let channel = load_channel(conn, &sdb.id, &req.channel_id).await?.ok_or(Error::NotFound("channel"))?;
                // Its messages go with it; take them off the usage totals first.
                let (messages, bytes, attachments) = query_one(
                    conn,
                    "SELECT count(*), coalesce(sum(size), 0), coalesce(sum(attachment_count), 0) FROM messages WHERE channel_id = ?1",
                    [req.channel_id.as_str()],
                    |r| Ok((r.get::<i64>(0)?, r.get::<i64>(1)?, r.get::<i64>(2)?)),
                )
                .await?
                .unwrap_or_default();
                conn.execute("DELETE FROM messages WHERE channel_id = ?1", [req.channel_id.as_str()]).await?;
                conn.execute(
                    "DELETE FROM thread_follows WHERE thread_id IN (SELECT id FROM threads WHERE channel_id = ?1)",
                    [req.channel_id.as_str()],
                )
                .await?;
                conn.execute("DELETE FROM threads WHERE channel_id = ?1", [req.channel_id.as_str()]).await?;
                let (secure_messages, secure_bytes) = super::secure::forget_channel(conn, &req.channel_id).await?;
                conn.execute("DELETE FROM slowmode WHERE channel_id = ?1", [req.channel_id.as_str()]).await?;
                // Its webhooks go too: they have nowhere left to post.
                let pictures = query_all(conn, "SELECT avatar_url FROM webhooks WHERE channel_id = ?1 AND avatar_url <> ''", [req.channel_id.as_str()], |r| {
                    r.get::<String>(0)
                })
                .await?;
                conn.execute("DELETE FROM webhooks WHERE channel_id = ?1", [req.channel_id.as_str()]).await?;
                let ended = shared::take_channel(conn, &req.channel_id).await?;
                conn.execute("DELETE FROM channels WHERE id = ?1", [req.channel_id.as_str()]).await?;
                conn.execute("DELETE FROM channel_overwrites WHERE channel_id = ?1", [req.channel_id.as_str()]).await?;
                // A deleted category's channels move to the top level.
                let children = query_all(conn, "SELECT id FROM channels WHERE parent_id = ?1", [req.channel_id.as_str()], |r| {
                    r.get::<String>(0)
                })
                .await?;
                conn.execute("UPDATE channels SET parent_id = NULL, updated_at = ?2 WHERE parent_id = ?1", (req.channel_id.as_str(), now_ms()))
                    .await?;
                conn.execute("UPDATE usage SET channels = channels - 1, updated_at = ?1 WHERE id = 1", [now_ms()]).await?;
                store::add_usage(
                    conn,
                    UsageChange { messages: -messages - secure_messages, message_bytes: -bytes - secure_bytes, attachments: -attachments, ..Default::default() },
                )
                .await?;
                store::audit(conn, &account.id, Audit::new(pb::AuditAction::ChannelDelete, &channel.id).channel(&channel.name))
                    .await?;
                events.push(Payload::ChannelDeleted(pb::ChannelDeleted { channel_id: req.channel_id.clone() }));
                for child in children {
                    if let Some(channel) = load_channel(conn, &sdb.id, &child).await? {
                        events.push(Payload::ChannelUpdated(pb::ChannelUpdated { channel: Some(channel) }));
                    }
                }
                // Join messages stop when their channel goes.
                if conn.execute("UPDATE server SET system_channel_id = NULL, updated_at = ?2 WHERE system_channel_id = ?1", (req.channel_id.as_str(), now_ms())).await? > 0 {
                    let server = store::load_server(conn).await?;
                    events.push(Payload::ServerUpdated(pb::ServerUpdated { server: Some(server.clone()) }));
                    return Ok((Some(server), pictures, ended));
                }
                Ok((None, pictures, ended))
            })
            .await?;
            if let Some(server) = server {
                self.app.server_changed(&server).await;
            }
            for picture in pictures {
                self.drop_picture(&picture, "", PictureOwner::Server(&sdb.id)).await;
            }
            self.forget_notifications(&sdb.id, Some(&req.channel_id), None).await;
            shared::tell_ended(&self.app, &sdb.id, &account.id, ended).await;
            Ok(pb::DeleteChannelResponse {})
        }
        .await)
    }

    async fn reorder_channels(
        &self,
        request: Request<pb::ReorderChannelsRequest>,
    ) -> Result<Response<pb::ReorderChannelsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::ManageChannels).await?;
                let channels = sdb
                    .write(&account.id, async |conn, events| {
                        let current = load_channels(conn, &sdb.id).await?;
                        let by_id: HashMap<&str, &pb::Channel> = current.iter().map(|c| (c.id.as_str(), c)).collect();
                        let listed: HashSet<&str> = req.channels.iter().map(|p| p.channel_id.as_str()).collect();
                        let visible: Vec<&pb::Channel> = current.iter().filter(|c| access.can_see(&c.id)).collect();
                        if listed.len() != req.channels.len()
                            || listed.len() != visible.len()
                            || !visible.iter().all(|c| listed.contains(c.id.as_str()))
                        {
                            return Err(Error::FailedPrecondition(
                                "the channels changed while you were moving them; try again".into(),
                            ));
                        }
                        for placement in &req.channels {
                            if placement.parent_id.is_empty() {
                                continue;
                            }
                            let kind = by_id[placement.channel_id.as_str()].r#type;
                            if kind == pb::ChannelType::Category as i32 {
                                return Err(Error::invalid("categories can't sit inside other channels"));
                            }
                            // A category the caller can't see is as good as missing to them.
                            if by_id
                                .get(placement.parent_id.as_str())
                                .is_none_or(|p| p.r#type != pb::ChannelType::Category as i32 || !access.can_see(&p.id))
                            {
                                return Err(Error::invalid("channels can only sit inside a category"));
                            }
                        }
                        // The channels the caller can see take their new order; the
                        // ones they can't keep their places among them.
                        let mut placed = req.channels.iter();
                        let order: Vec<(&str, &str)> = current
                            .iter()
                            .map(|c| match access.can_see(&c.id) {
                                true => placed.next().map_or((c.id.as_str(), c.parent_id.as_str()), |p| {
                                    (p.channel_id.as_str(), p.parent_id.as_str())
                                }),
                                false => (c.id.as_str(), c.parent_id.as_str()),
                            })
                            .collect();
                        let now = now_ms();
                        let mut moved = vec![];
                        for (position, &(id, parent_id)) in order.iter().enumerate() {
                            let before = by_id[id];
                            if before.position == position as i32 && before.parent_id == parent_id {
                                continue;
                            }
                            conn.execute(
                                "UPDATE channels SET position = ?2, parent_id = ?3, updated_at = ?4 WHERE id = ?1",
                                (id, position as i64, (!parent_id.is_empty()).then_some(parent_id), now),
                            )
                            .await?;
                            moved.push(id);
                        }
                        let mut channels = Vec::with_capacity(req.channels.len());
                        for &(id, _) in &order {
                            let channel = load_channel(conn, &sdb.id, id).await?.ok_or(Error::NotFound("channel"))?;
                            if moved.contains(&id) {
                                events.push(Payload::ChannelUpdated(pb::ChannelUpdated {
                                    channel: Some(channel.clone()),
                                }));
                            }
                            if access.can_see(id) {
                                channels.push(channel);
                            }
                        }
                        if !moved.is_empty() {
                            store::audit(conn, &account.id, Audit::new(pb::AuditAction::ChannelsReorder, "")).await?;
                        }
                        Ok(channels)
                    })
                    .await?;
                Ok(pb::ReorderChannelsResponse { channels })
            }
            .await,
        )
    }

    async fn set_channel_permissions(
        &self,
        request: Request<pb::SetChannelPermissionsRequest>,
    ) -> Result<Response<pb::SetChannelPermissionsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                access.require_in(&req.channel_id, Permission::ManageRoles)?;
                let wanted = overwrites(&req.overwrites)?;
                let have = access.in_channel(&req.channel_id);
                let channel = sdb
                    .write(&account.id, async |conn, events| {
                        let before = load_channel(conn, &sdb.id, &req.channel_id).await?.ok_or(Error::NotFound("channel"))?;
                        for o in &wanted {
                            let exists = if o.member {
                                store::member(conn, &sdb.id, &o.target_id).await?.is_some()
                            } else {
                                permissions::role(conn, &sdb.id, &o.target_id).await?.is_some()
                            };
                            if !exists {
                                return Err(Error::NotFound(if o.member { "member" } else { "role" }));
                            }
                        }
                        // Only permissions the caller has here can change, either way.
                        let old = overwrites(&before.permission_overwrites)?;
                        let find = |list: &[Overwrite], id: &str| {
                            list.iter().find(|o| o.target_id == id).map_or((0, 0), |o| (o.allow, o.deny))
                        };
                        let changed = old.iter().chain(&wanted).fold(0, |changed, o| {
                            let (a, d) = find(&old, &o.target_id);
                            let (b, e) = find(&wanted, &o.target_id);
                            changed | (a ^ b) | (d ^ e)
                        });
                        if !access.may_change(changed, have) {
                            return Err(Error::denied("you can only change permissions you have in this channel"));
                        }
                        // And only for roles and members below them (@everyone and
                        // their own are always theirs to change).
                        for o in old.iter().chain(&wanted) {
                            if find(&old, &o.target_id) == find(&wanted, &o.target_id)
                                || o.target_id == account.id
                                || (!o.member && o.target_id == sdb.id)
                            {
                                continue;
                            }
                            let below = if o.member {
                                match store::member_access(conn, &sdb.id, &o.target_id).await? {
                                    Some((_, theirs)) => access.outranks(&theirs),
                                    // Gone from the server: their overwrite is only clutter.
                                    None => true,
                                }
                            } else {
                                match permissions::role(conn, &sdb.id, &o.target_id).await? {
                                    Some(role) => access.above(i64::from(role.position)),
                                    None => true,
                                }
                            };
                            if !below {
                                return Err(Error::denied(
                                    "you can only change permissions for roles and members below your highest role",
                                ));
                            }
                        }
                        conn.execute("DELETE FROM channel_overwrites WHERE channel_id = ?1", [before.id.as_str()]).await?;
                        for o in &wanted {
                            let target = if o.member { pb::OverwriteTarget::Member } else { pb::OverwriteTarget::Role };
                            conn.execute(
                                "INSERT INTO channel_overwrites (channel_id, target_id, target, allow, deny) VALUES (?1, ?2, ?3, ?4, ?5)",
                                (before.id.as_str(), o.target_id.as_str(), target as i64, o.allow as i64, o.deny as i64),
                            )
                            .await?;
                        }
                        conn.execute("UPDATE channels SET updated_at = ?2 WHERE id = ?1", (before.id.as_str(), now_ms()))
                            .await?;
                        let channel = load_channel(conn, &sdb.id, &before.id).await?.ok_or(Error::NotFound("channel"))?;
                        if changed != 0 {
                            store::audit(
                                conn,
                                &account.id,
                                Audit::new(pb::AuditAction::ChannelPermissionsUpdate, &channel.id).channel(&channel.name),
                            )
                            .await?;
                        }
                        events.push(Payload::ChannelUpdated(pb::ChannelUpdated { channel: Some(channel.clone()) }));
                        Ok(channel)
                    })
                    .await?;
                Ok(pb::SetChannelPermissionsResponse { channel: Some(channel) })
            }
            .await,
        )
    }
}

struct Overwrite {
    target_id: String,
    member: bool,
    allow: Bits,
    deny: Bits,
}

/// Checks overwrites as a client sent them: one per role or member, channel
/// permissions only, none both allowed and denied. Empty ones are dropped.
fn overwrites(list: &[pb::PermissionOverwrite]) -> Result<Vec<Overwrite>> {
    if list.len() > MAX_OVERWRITES {
        return Err(Error::invalid(format!("a channel can have at most {MAX_OVERWRITES} overwrites")));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(list.len());
    for o in list {
        let member = match pb::OverwriteTarget::try_from(o.target) {
            Ok(pb::OverwriteTarget::Role) => false,
            Ok(pb::OverwriteTarget::Member) => true,
            _ => return Err(Error::invalid("an overwrite is for a role or a member")),
        };
        let target_id = crate::id::parse_id("target_id", &o.target_id)?;
        let (allow, deny) = (permissions::from_list(&o.allow)?, permissions::from_list(&o.deny)?);
        if (allow | deny) & !permissions::CHANNEL != 0 {
            return Err(Error::invalid("only channel permissions can change per channel"));
        }
        if allow & deny != 0 {
            return Err(Error::invalid("a permission can't be both allowed and denied"));
        }
        if !seen.insert(target_id.clone()) {
            return Err(Error::invalid("a channel has one overwrite per role or member"));
        }
        if allow | deny != 0 {
            out.push(Overwrite { target_id, member, allow, deny });
        }
    }
    Ok(out)
}
