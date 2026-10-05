//! Pinned messages in channels and threads (`MessageService.PinMessage` and
//! `ListPins`). A pin is the message row's `pinned_at`, so every read of a
//! message carries it and deleting the message drops it. Direct messages pin
//! by record instead, in `dms.rs`.

use super::messages::{MESSAGE_COLUMNS, load_message, message_row, with_extras};
use super::{Api, Seat, polls, shared, threads, users};
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{now_ms, timestamp};
use crate::pb::{self, Permission};
use crate::servers::{self as store, Audit, Payload, load_channel};

/// Most pinned messages one page of ListPins holds.
const MAX_PAGE: i32 = 100;

/// Which pins a message is counted with: its thread's when it's a reply that
/// stays in its thread, else its channel's (as the channel's list shows it).
fn scope(thread_id: &str) -> &'static str {
    if thread_id.is_empty() { "channel_id = ?1 AND (thread_id IS NULL OR in_channel = 1)" } else { "thread_id = ?1" }
}

/// The list a message's pin is in: `(thread id or "", scope id)`.
fn list_of(message: &pb::Message) -> (&str, &str) {
    if message.thread_id.is_empty() || message.also_in_channel {
        ("", &message.channel_id)
    } else {
        (&message.thread_id, &message.thread_id)
    }
}

/// Secure channels' messages aren't rows the server can read or mark; their
/// pins would travel encrypted, which isn't built yet.
fn refuse_secure(channel: &pb::Channel) -> Result<()> {
    if channel.r#type == pb::ChannelType::Secure as i32 {
        return Err(Error::FailedPrecondition("pins don't work in secure channels yet".into()));
    }
    Ok(())
}

impl Api {
    pub(super) async fn pin_message_impl(
        &self,
        account: &crate::node::Account,
        req: pb::PinMessageRequest,
    ) -> Result<pb::PinMessageResponse> {
        let Seat { sdb, access, .. } = self.membership(account, &req.server_id).await?;
        access.require_not_timed_out()?;
        access.require_in(&req.channel_id, Permission::ViewChannels)?;
        access.require_in(&req.channel_id, Permission::ManageMessages)?;
        if shared::link_of(&*sdb.read()?, &req.channel_id).await?.is_some() {
            return Err(Error::FailedPrecondition("only the channel's home server can pin its messages".into()));
        }
        let cap = self.app.settings().limits.pins_per_channel;
        let message = sdb
            .write(&account.id, async |conn, events| {
                let channel = load_channel(conn, &sdb.id, &req.channel_id).await?.ok_or(Error::NotFound("channel"))?;
                refuse_secure(&channel)?;
                let mut message = load_message(conn, &sdb.id, &req.message_id)
                    .await?
                    .filter(|m| m.channel_id == channel.id)
                    .ok_or(Error::NotFound("message"))?;
                if message.kind != pb::MessageKind::Unspecified as i32 {
                    return Err(Error::invalid("only messages someone wrote can be pinned"));
                }
                if message.pinned_at.is_some() == req.pinned {
                    return Ok(message);
                }
                let (thread_id, scope_id) = list_of(&message);
                if req.pinned
                    && let Some(cap) = cap
                {
                    let pinned = query_one(
                        conn,
                        &format!("SELECT count(*) FROM messages WHERE {} AND pinned_at IS NOT NULL", scope(thread_id)),
                        [scope_id],
                        |r| r.get::<i64>(0),
                    )
                    .await?
                    .unwrap_or(0);
                    if pinned >= cap {
                        return Err(Error::ResourceExhausted(
                            "this channel has as many pins as it can hold; unpin one first".into(),
                        ));
                    }
                }
                let at = req.pinned.then(now_ms);
                conn.execute("UPDATE messages SET pinned_at = ?2 WHERE id = ?1", (message.id.as_str(), at)).await?;
                message.pinned_at = at.map(timestamp);
                let action = if req.pinned { pb::AuditAction::MessagePin } else { pb::AuditAction::MessageUnpin };
                store::audit(conn, &account.id, Audit::new(action, &message.author_id).channel(channel.name.clone()))
                    .await?;
                events.push(Payload::MessagePinned(pb::MessagePinned {
                    channel_id: channel.id.clone(),
                    message_id: message.id.clone(),
                    thread_id: message.thread_id.clone(),
                    pinned_at: message.pinned_at,
                }));
                Ok(message)
            })
            .await?;
        Ok(pb::PinMessageResponse { message: Some(message) })
    }

    pub(super) async fn list_pins_impl(
        &self,
        account: &crate::node::Account,
        req: pb::ListPinsRequest,
    ) -> Result<pb::ListPinsResponse> {
        let Seat { sdb, access, .. } = self.membership(account, &req.server_id).await?;
        access.require_in(&req.channel_id, Permission::ViewChannels)?;
        let conn = sdb.read()?;
        let channel = load_channel(&conn, &sdb.id, &req.channel_id).await?.ok_or(Error::NotFound("channel"))?;
        refuse_secure(&channel)?;
        if shared::link_of(&conn, &req.channel_id).await?.is_some() {
            return Err(Error::FailedPrecondition("pins in channels shown from another server aren't here yet".into()));
        }
        let scope_id = if req.thread_id.is_empty() {
            req.channel_id.as_str()
        } else {
            load_message(&conn, &sdb.id, &req.thread_id)
                .await?
                .filter(|m| m.channel_id == req.channel_id && m.thread_id.is_empty())
                .ok_or(Error::NotFound("thread"))?;
            req.thread_id.as_str()
        };
        let limit = if req.limit <= 0 { 50 } else { req.limit.min(MAX_PAGE) } as i64;
        // The page after a pin: older pins, and among pins made in the same
        // millisecond, smaller ids. One that's no longer pinned starts over.
        let after = if req.after_id.is_empty() {
            None
        } else {
            query_one(&conn, "SELECT pinned_at FROM messages WHERE id = ?1", [req.after_id.as_str()], |r| {
                r.get::<Option<i64>>(0)
            })
            .await?
            .flatten()
        };
        let (cursor, at, id) = match after {
            Some(at) => ("AND (pinned_at < ?2 OR (pinned_at = ?2 AND id < ?3))", at, req.after_id.as_str()),
            None => ("AND ?2 = 0 AND ?3 = ''", 0, ""),
        };
        let rows = query_all(
            &conn,
            &format!(
                "SELECT {MESSAGE_COLUMNS} FROM messages WHERE {} AND pinned_at IS NOT NULL {cursor}
                 ORDER BY pinned_at DESC, id DESC LIMIT ?4",
                scope(&req.thread_id)
            ),
            (scope_id, at, id, limit + 1),
            message_row(&sdb.id),
        )
        .await?;
        let has_more = rows.len() as i64 > limit;
        let mut messages = rows.into_iter().take(limit as usize).map(with_extras).collect::<Result<Vec<_>>>()?;
        shared::mark_guests(&conn, &mut messages).await?;
        polls::attach(&conn, &mut messages).await?;
        polls::mark_mine(&conn, &account.id, &mut messages).await?;
        threads::attach(&conn, &mut messages).await?;
        let authors = users(&conn, &messages.iter().map(|m| m.author_id.as_str()).collect::<Vec<_>>()).await?;
        Ok(pb::ListPinsResponse { messages, authors, has_more })
    }
}
