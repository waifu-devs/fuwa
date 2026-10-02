use std::collections::{HashMap, HashSet};

use tonic::{Request, Response, Status};

use super::{Api, respond, text};
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::pb::{self, channel_service_server::ChannelService};
use crate::servers::{self as store, Audit, Payload, UsageChange, usage_count};

const CHANNEL_COLUMNS: &str = "id, name, type, parent_id, topic, position, created_at, updated_at, slowmode_seconds";

/// The longest slow mode, as Discord has it: six hours.
const MAX_SLOWMODE: i32 = 6 * 60 * 60;

fn channel_row(server_id: &str) -> impl Fn(&turso::Row) -> turso::Result<pb::Channel> + '_ {
    move |r| {
        Ok(pb::Channel {
            id: r.get(0)?,
            server_id: server_id.to_string(),
            name: r.get(1)?,
            r#type: r.get(2)?,
            parent_id: r.get::<Option<String>>(3)?.unwrap_or_default(),
            topic: r.get(4)?,
            position: r.get(5)?,
            created_at: Some(timestamp(r.get(6)?)),
            updated_at: Some(timestamp(r.get(7)?)),
            slowmode_seconds: r.get(8)?,
        })
    }
}

pub(super) async fn load_channel(
    conn: &turso::Connection,
    server_id: &str,
    channel_id: &str,
) -> Result<Option<pb::Channel>> {
    query_one(
        conn,
        &format!("SELECT {CHANNEL_COLUMNS} FROM channels WHERE id = ?1"),
        [channel_id],
        channel_row(server_id),
    )
    .await
}

/// Channel names read like `#general`: lowercase, words joined by dashes.
/// Categories keep their name as typed.
fn channel_name(name: &str, kind: pb::ChannelType) -> Result<String> {
    let name = text("name", name, 1, 100)?;
    if kind == pb::ChannelType::Category {
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
                let (sdb, _) = self.manager(&account, &req.server_id).await?;
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
                let (sdb, _) = self.membership(&account, &req.server_id).await?;
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
                let (sdb, _) = self.membership(&account, &request.get_ref().server_id).await?;
                let channels = query_all(
                    &sdb.read()?,
                    &format!("SELECT {CHANNEL_COLUMNS} FROM channels ORDER BY position, id"),
                    (),
                    channel_row(&sdb.id),
                )
                .await?;
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
                let (sdb, _) = self.manager(&account, &req.server_id).await?;
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
            let (sdb, _) = self.manager(&account, &req.server_id).await?;
            // Alone, so no message lands in the channel while it goes.
            let server = sdb.write_alone(&account.id, async |conn, events| {
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
                conn.execute("DELETE FROM slowmode WHERE channel_id = ?1", [req.channel_id.as_str()]).await?;
                conn.execute("DELETE FROM channels WHERE id = ?1", [req.channel_id.as_str()]).await?;
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
                    UsageChange { messages: -messages, message_bytes: -bytes, attachments: -attachments, ..Default::default() },
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
                    return Ok(Some(server));
                }
                Ok(None)
            })
            .await?;
            if let Some(server) = server {
                self.app.server_changed(&server).await;
            }
            self.forget_notifications(&sdb.id, Some(&req.channel_id), None).await;
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
                let (sdb, _) = self.manager(&account, &req.server_id).await?;
                let channels = sdb
                    .write(&account.id, async |conn, events| {
                        let current = query_all(
                            conn,
                            &format!("SELECT {CHANNEL_COLUMNS} FROM channels"),
                            (),
                            channel_row(&sdb.id),
                        )
                        .await?;
                        let by_id: HashMap<&str, &pb::Channel> = current.iter().map(|c| (c.id.as_str(), c)).collect();
                        let listed: HashSet<&str> = req.channels.iter().map(|p| p.channel_id.as_str()).collect();
                        if listed.len() != req.channels.len()
                            || listed.len() != current.len()
                            || !listed.iter().all(|id| by_id.contains_key(id))
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
                            if by_id
                                .get(placement.parent_id.as_str())
                                .is_none_or(|p| p.r#type != pb::ChannelType::Category as i32)
                            {
                                return Err(Error::invalid("channels can only sit inside a category"));
                            }
                        }
                        let now = now_ms();
                        let mut moved = vec![];
                        for (position, placement) in req.channels.iter().enumerate() {
                            let before = by_id[placement.channel_id.as_str()];
                            if before.position == position as i32 && before.parent_id == placement.parent_id {
                                continue;
                            }
                            conn.execute(
                                "UPDATE channels SET position = ?2, parent_id = ?3, updated_at = ?4 WHERE id = ?1",
                                (
                                    placement.channel_id.as_str(),
                                    position as i64,
                                    (!placement.parent_id.is_empty()).then_some(placement.parent_id.as_str()),
                                    now,
                                ),
                            )
                            .await?;
                            moved.push(placement.channel_id.as_str());
                        }
                        let mut channels = Vec::with_capacity(req.channels.len());
                        for placement in &req.channels {
                            let channel = load_channel(conn, &sdb.id, &placement.channel_id)
                                .await?
                                .ok_or(Error::NotFound("channel"))?;
                            if moved.contains(&channel.id.as_str()) {
                                events.push(Payload::ChannelUpdated(pb::ChannelUpdated {
                                    channel: Some(channel.clone()),
                                }));
                            }
                            channels.push(channel);
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
}
