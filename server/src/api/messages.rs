use prost::Message as _;
use tonic::{Request, Response, Status};

use super::channels::load_channel;
use super::{Api, can_manage, respond, url};
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::pb::{self, message_service_server::MessageService};
use crate::servers::{self as store, Payload, UsageChange};

/// The longest a message can be, in characters.
pub const MAX_MESSAGE_LENGTH: usize = 4000;
const MAX_ATTACHMENTS: usize = 10;
const MAX_EMBEDS: usize = 10;

/// What's stored in a message's `extras` column.
#[derive(Clone, PartialEq, prost::Message)]
struct Extras {
    #[prost(message, repeated, tag = "1")]
    attachments: Vec<pb::Attachment>,
    #[prost(message, repeated, tag = "2")]
    embeds: Vec<pb::Embed>,
}

const MESSAGE_COLUMNS: &str = "id, channel_id, author_id, content, extras, reply_to_id, created_at, edited_at";

fn message_row(server_id: &str) -> impl Fn(&turso::Row) -> turso::Result<(pb::Message, Option<Vec<u8>>)> + '_ {
    move |r| {
        Ok((
            pb::Message {
                id: r.get(0)?,
                server_id: server_id.to_string(),
                channel_id: r.get(1)?,
                author_id: r.get(2)?,
                content: r.get(3)?,
                attachments: vec![],
                embeds: vec![],
                reply_to_id: r.get::<Option<String>>(5)?.unwrap_or_default(),
                created_at: Some(timestamp(r.get(6)?)),
                edited_at: r.get::<Option<i64>>(7)?.map(timestamp),
            },
            r.get::<Option<Vec<u8>>>(4)?,
        ))
    }
}

fn with_extras((mut message, extras): (pb::Message, Option<Vec<u8>>)) -> Result<pb::Message> {
    if let Some(bytes) = extras {
        let extras = Extras::decode(bytes.as_slice())?;
        message.attachments = extras.attachments;
        message.embeds = extras.embeds;
    }
    Ok(message)
}

async fn load_message(conn: &turso::Connection, server_id: &str, message_id: &str) -> Result<Option<pb::Message>> {
    query_one(
        conn,
        &format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE id = ?1"),
        [message_id],
        message_row(server_id),
    )
    .await?
    .map(with_extras)
    .transpose()
}

fn check_content(content: &str, has_extras: bool) -> Result<()> {
    if content.trim().is_empty() && !has_extras {
        return Err(Error::invalid("a message needs text, an attachment or an embed"));
    }
    if content.chars().count() > MAX_MESSAGE_LENGTH {
        return Err(Error::invalid(format!("messages can be at most {MAX_MESSAGE_LENGTH} characters")));
    }
    Ok(())
}

fn check_extras(attachments: &mut [pb::Attachment], embeds: &[pb::Embed]) -> Result<()> {
    if attachments.len() > MAX_ATTACHMENTS || embeds.len() > MAX_EMBEDS {
        return Err(Error::invalid(format!(
            "at most {MAX_ATTACHMENTS} attachments and {MAX_EMBEDS} embeds per message"
        )));
    }
    for attachment in attachments.iter_mut() {
        attachment.url = url("attachment url", &attachment.url)?;
        if attachment.url.is_empty() || attachment.filename.chars().count() > 255 {
            return Err(Error::invalid("attachments need a URL and a filename of at most 255 characters"));
        }
        attachment.id = new_id();
    }
    Ok(())
}

async fn authors(conn: &turso::Connection, messages: &[pb::Message]) -> Result<Vec<pb::User>> {
    let mut ids: Vec<&str> = messages.iter().map(|m| m.author_id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let placeholders = (1..=ids.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    query_all(
        conn,
        &format!("SELECT id, username, display_name, avatar_url, kind FROM users WHERE id IN ({placeholders})"),
        ids.iter().map(|id| turso::Value::from(*id)).collect::<Vec<_>>(),
        |r| {
            Ok(pb::User {
                id: r.get(0)?,
                username: r.get(1)?,
                display_name: r.get(2)?,
                avatar_url: r.get(3)?,
                kind: r.get(4)?,
            })
        },
    )
    .await
}

#[tonic::async_trait]
impl MessageService for Api {
    async fn send_message(
        &self,
        request: Request<pb::SendMessageRequest>,
    ) -> Result<Response<pb::SendMessageResponse>, Status> {
        respond(async {
            let account = self.account(request.metadata()).await?;
            let mut req = request.into_inner();
            let (sdb, _) = self.membership(&account, &req.server_id).await?;
            check_content(&req.content, !req.attachments.is_empty() || !req.embeds.is_empty())?;
            check_extras(&mut req.attachments, &req.embeds)?;
            let limits = sdb.limits(&self.app.settings().limits).await?;
            if let Some(limit) = limits.storage_bytes
                && sdb.storage_bytes() >= limit
            {
                return Err(Error::ResourceExhausted("this server is out of storage".into()));
            }
            let message = sdb
                .write(&account.id, async |conn, events| {
                    let channel = load_channel(conn, &sdb.id, &req.channel_id).await?.ok_or(Error::NotFound("channel"))?;
                    if !matches!(
                        pb::ChannelType::try_from(channel.r#type),
                        Ok(pb::ChannelType::Text | pb::ChannelType::Announcement | pb::ChannelType::Thread)
                    ) {
                        return Err(Error::invalid("messages can only go in text channels"));
                    }
                    if !req.reply_to_id.is_empty() {
                        let replied = load_message(conn, &sdb.id, &req.reply_to_id).await?;
                        if replied.is_none_or(|m| m.channel_id != channel.id) {
                            return Err(Error::NotFound("message being replied to"));
                        }
                    }
                    let now = now_ms();
                    let has_extras = !req.attachments.is_empty() || !req.embeds.is_empty();
                    let extras = has_extras.then(|| {
                        Extras { attachments: req.attachments.clone(), embeds: req.embeds.clone() }.encode_to_vec()
                    });
                    let message = pb::Message {
                        id: new_id(),
                        server_id: sdb.id.clone(),
                        channel_id: channel.id.clone(),
                        author_id: account.id.clone(),
                        content: req.content.clone(),
                        attachments: req.attachments.clone(),
                        embeds: req.embeds.clone(),
                        reply_to_id: req.reply_to_id.clone(),
                        created_at: Some(timestamp(now)),
                        edited_at: None,
                    };
                    let size = message.content.len() as i64;
                    let attachment_count = message.attachments.len() as i64;
                    conn.execute(
                        "INSERT INTO messages (id, channel_id, author_id, content, size, extras, attachment_count, reply_to_id, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        (
                            message.id.as_str(),
                            message.channel_id.as_str(),
                            message.author_id.as_str(),
                            message.content.as_str(),
                            size,
                            extras,
                            attachment_count,
                            (!message.reply_to_id.is_empty()).then_some(message.reply_to_id.as_str()),
                            now,
                        ),
                    )
                    .await?;
                    store::add_usage(
                        conn,
                        UsageChange { messages: 1, messages_sent: 1, message_bytes: size, attachments: attachment_count, ..Default::default() },
                    )
                    .await?;
                    events.push(Payload::MessageCreated(pb::MessageCreated { message: Some(message.clone()) }));
                    Ok(message)
                })
                .await?;
            Ok(pb::SendMessageResponse { message: Some(message) })
        }
        .await)
    }

    async fn get_message(
        &self,
        request: Request<pb::GetMessageRequest>,
    ) -> Result<Response<pb::GetMessageResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let (sdb, _) = self.membership(&account, &req.server_id).await?;
                let conn = sdb.read()?;
                let message = load_message(&conn, &sdb.id, &req.message_id).await?.ok_or(Error::NotFound("message"))?;
                let author = authors(&conn, std::slice::from_ref(&message)).await?.into_iter().next();
                Ok(pb::GetMessageResponse { message: Some(message), author })
            }
            .await,
        )
    }

    async fn list_messages(
        &self,
        request: Request<pb::ListMessagesRequest>,
    ) -> Result<Response<pb::ListMessagesResponse>, Status> {
        respond(async {
            let account = self.account(request.metadata()).await?;
            let req = request.into_inner();
            let (sdb, _) = self.membership(&account, &req.server_id).await?;
            let conn = sdb.read()?;
            load_channel(&conn, &sdb.id, &req.channel_id).await?.ok_or(Error::NotFound("channel"))?;
            let limit = if req.limit <= 0 { 50 } else { req.limit.min(100) } as i64;
            let (condition, cursor, newest_first) = match (req.before_id.is_empty(), req.after_id.is_empty()) {
                (false, _) => ("AND id < ?2", req.before_id.as_str(), true),
                (true, false) => ("AND id > ?2", req.after_id.as_str(), false),
                (true, true) => ("AND ?2 = ''", "", true),
            };
            let order = if newest_first { "DESC" } else { "ASC" };
            let rows = query_all(
                &conn,
                &format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE channel_id = ?1 {condition} ORDER BY id {order} LIMIT ?3"),
                (req.channel_id.as_str(), cursor, limit + 1),
                message_row(&sdb.id),
            )
            .await?;
            let has_more = rows.len() as i64 > limit;
            let mut messages = rows.into_iter().take(limit as usize).map(with_extras).collect::<Result<Vec<_>>>()?;
            if newest_first {
                messages.reverse();
            }
            let authors = authors(&conn, &messages).await?;
            Ok(pb::ListMessagesResponse { messages, authors, has_more })
        }
        .await)
    }

    async fn update_message(
        &self,
        request: Request<pb::UpdateMessageRequest>,
    ) -> Result<Response<pb::UpdateMessageResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let (sdb, _) = self.membership(&account, &req.server_id).await?;
                let message = sdb
                    .write(&account.id, async |conn, events| {
                        let mut message =
                            load_message(conn, &sdb.id, &req.message_id).await?.ok_or(Error::NotFound("message"))?;
                        if message.author_id != account.id {
                            return Err(Error::denied("you can only edit your own messages"));
                        }
                        check_content(&req.content, !message.attachments.is_empty() || !message.embeds.is_empty())?;
                        let now = now_ms();
                        let growth = req.content.len() as i64 - message.content.len() as i64;
                        conn.execute(
                            "UPDATE messages SET content = ?2, size = ?3, edited_at = ?4 WHERE id = ?1",
                            (message.id.as_str(), req.content.as_str(), req.content.len() as i64, now),
                        )
                        .await?;
                        store::add_usage(conn, UsageChange { message_bytes: growth, ..Default::default() }).await?;
                        message.content = req.content.clone();
                        message.edited_at = Some(timestamp(now));
                        events.push(Payload::MessageUpdated(pb::MessageUpdated { message: Some(message.clone()) }));
                        Ok(message)
                    })
                    .await?;
                Ok(pb::UpdateMessageResponse { message: Some(message) })
            }
            .await,
        )
    }

    async fn delete_message(
        &self,
        request: Request<pb::DeleteMessageRequest>,
    ) -> Result<Response<pb::DeleteMessageResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let (sdb, member) = self.membership(&account, &req.server_id).await?;
                sdb.write(&account.id, async |conn, events| {
                    let message =
                        load_message(conn, &sdb.id, &req.message_id).await?.ok_or(Error::NotFound("message"))?;
                    if message.author_id != account.id && !can_manage(&member) {
                        return Err(Error::denied("you can only delete your own messages"));
                    }
                    conn.execute("DELETE FROM messages WHERE id = ?1", [message.id.as_str()]).await?;
                    store::add_usage(
                        conn,
                        UsageChange {
                            messages: -1,
                            message_bytes: -(message.content.len() as i64),
                            attachments: -(message.attachments.len() as i64),
                            ..Default::default()
                        },
                    )
                    .await?;
                    events.push(Payload::MessageDeleted(pb::MessageDeleted {
                        channel_id: message.channel_id.clone(),
                        message_id: message.id.clone(),
                    }));
                    Ok(())
                })
                .await?;
                Ok(pb::DeleteMessageResponse {})
            }
            .await,
        )
    }
}
