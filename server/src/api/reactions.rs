//! Reactions to messages in channels and threads (`MessageService.React`,
//! `ListReactors` and `ClearReactions`): one row per person, emoji and
//! message in the server file's `reactions`. Reads add up each message's
//! (`attach`); writes send `ReactionUpdated` or `ReactionsCleared`. Direct
//! messages' and secure channels' reactions travel inside the encryption
//! (`DirectMessageReaction`) and never reach here.

use std::collections::HashMap;

use super::messages::load_message;
use super::{Api, Seat, polls, shared, users};
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::now_ms;
use crate::pb::{self, Permission};
use crate::servers::{self as store, Audit, Payload, load_channel};

/// Most people one page of ListReactors holds.
const MAX_PAGE: i32 = 100;
/// Longest standard emoji taken, in bytes: families and flags with their
/// joiners fit.
const MAX_EMOJI_BYTES: usize = 32;

/// Whether a stored key names a custom emoji (its id) rather than a standard
/// one, which never has a letter in it.
fn is_custom(key: &str) -> bool {
    !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// A standard emoji as sent: a few characters, at least one beyond ASCII,
/// with no letters, spaces or the marks Markdown and mentions use.
fn standard(value: &str) -> Result<String> {
    let value = value.trim();
    let fits = !value.is_empty()
        && value.len() <= MAX_EMOJI_BYTES
        && !value.is_ascii()
        && value.chars().all(|c| {
            !c.is_whitespace() && !c.is_control() && !c.is_ascii_alphabetic() && !"<>:@_`~|[]()\\".contains(c)
        });
    if fits { Ok(value.to_string()) } else { Err(Error::invalid("react with one emoji")) }
}

/// The key a request names (`emoji` or `emoji_id`, one of the two), and the
/// reaction it's shown as, its count still 0. A custom emoji must be one the
/// server has now.
async fn named(conn: &turso::Connection, emoji: &str, emoji_id: &str) -> Result<(String, pb::Reaction)> {
    match (emoji.is_empty(), emoji_id.is_empty()) {
        (false, true) => {
            let emoji = standard(emoji)?;
            Ok((emoji.clone(), pb::Reaction { emoji, ..Default::default() }))
        }
        (true, false) => {
            if !is_custom(emoji_id) {
                return Err(Error::NotFound("emoji"));
            }
            let (name, animated) =
                query_one(conn, "SELECT name, animated FROM emojis WHERE id = ?1", [emoji_id], |r| {
                    Ok((r.get::<String>(0)?, r.get::<bool>(1)?))
                })
                .await?
                .ok_or(Error::NotFound("emoji"))?;
            Ok((
                emoji_id.to_string(),
                pb::Reaction { emoji_id: emoji_id.to_string(), emoji_name: name, animated, ..Default::default() },
            ))
        }
        _ => Err(Error::invalid("name one emoji: emoji or emoji_id")),
    }
}

/// How many reacted to `message_id` with `key`.
async fn count(conn: &turso::Connection, message_id: &str, key: &str) -> Result<u32> {
    Ok(query_one(conn, "SELECT count(*) FROM reactions WHERE message_id = ?1 AND emoji = ?2", (message_id, key), |r| {
        r.get::<i64>(0)
    })
    .await?
    .unwrap_or(0) as u32)
}

/// Each message's reactions, in the order each emoji was first used, with
/// `me` for `viewer`. Custom emoji the server no longer has are left out.
pub(super) async fn attach(conn: &turso::Connection, viewer: &str, messages: &mut [pb::Message]) -> Result<()> {
    if messages.is_empty() {
        return Ok(());
    }
    let placeholders = (2..messages.len() + 2).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    let mut params = vec![turso::Value::from(viewer)];
    params.extend(messages.iter().map(|m| turso::Value::from(m.id.as_str())));
    let rows = query_all(
        conn,
        &format!(
            "SELECT r.message_id, r.emoji, count(*), min(r.created_at),
                    sum(CASE WHEN r.account_id = ?1 THEN 1 ELSE 0 END), e.name, e.animated
             FROM reactions r LEFT JOIN emojis e ON e.id = r.emoji
             WHERE r.message_id IN ({placeholders})
             GROUP BY r.message_id, r.emoji
             ORDER BY min(r.created_at), r.emoji"
        ),
        params,
        |r| {
            Ok((
                r.get::<String>(0)?,
                r.get::<String>(1)?,
                r.get::<i64>(2)?,
                r.get::<i64>(4)?,
                r.get::<Option<String>>(5)?,
                r.get::<Option<bool>>(6)?,
            ))
        },
    )
    .await?;
    let mut found: HashMap<String, Vec<pb::Reaction>> = HashMap::new();
    for (message_id, key, count, mine, name, animated) in rows {
        let reaction = if is_custom(&key) {
            let Some(name) = name else { continue };
            pb::Reaction { emoji_id: key, emoji_name: name, animated: animated.unwrap_or(false), ..Default::default() }
        } else {
            pb::Reaction { emoji: key, ..Default::default() }
        };
        found.entry(message_id).or_default().push(pb::Reaction { count: count as u32, me: mine > 0, ..reaction });
    }
    for message in messages {
        message.reactions = found.remove(&message.id).unwrap_or_default();
    }
    Ok(())
}

/// Drops a deleted message's reactions, inside the write that deletes it.
pub(super) async fn forget(conn: &turso::Connection, message_id: &str) -> Result<()> {
    conn.execute("DELETE FROM reactions WHERE message_id = ?1", [message_id]).await?;
    Ok(())
}

/// Drops the reactions to a channel's messages, before they're deleted.
pub(super) async fn forget_channel(conn: &turso::Connection, channel_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM reactions WHERE message_id IN (SELECT id FROM messages WHERE channel_id = ?1)",
        [channel_id],
    )
    .await?;
    Ok(())
}

/// Drops the reactions to a thread's replies, before they're deleted.
pub(super) async fn forget_thread(conn: &turso::Connection, thread_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM reactions WHERE message_id IN (SELECT id FROM messages WHERE thread_id = ?1)",
        [thread_id],
    )
    .await?;
    Ok(())
}

/// Takes off every reaction of an account that's being deleted, telling
/// members each count that went down.
pub(crate) async fn forget_reactor(
    conn: &turso::Connection,
    account_id: &str,
    events: &mut Vec<Payload>,
) -> Result<()> {
    let theirs = query_all(
        conn,
        "SELECT r.message_id, r.emoji, m.channel_id, m.thread_id FROM reactions r
         JOIN messages m ON m.id = r.message_id WHERE r.account_id = ?1",
        [account_id],
        |r| Ok((r.get::<String>(0)?, r.get::<String>(1)?, r.get::<String>(2)?, r.get::<Option<String>>(3)?)),
    )
    .await?;
    conn.execute("DELETE FROM reactions WHERE account_id = ?1", [account_id]).await?;
    for (message_id, key, channel_id, thread_id) in theirs {
        let Ok((_, reaction)) =
            (if is_custom(&key) { named(conn, "", &key).await } else { named(conn, &key, "").await })
        else {
            continue;
        };
        let count = count(conn, &message_id, &key).await?;
        events.push(Payload::ReactionUpdated(pb::ReactionUpdated {
            channel_id,
            message_id,
            thread_id: thread_id.unwrap_or_default(),
            reaction: Some(pb::Reaction { count, ..reaction }),
            user_id: account_id.to_string(),
            added: false,
        }));
    }
    Ok(())
}

/// Refuses what reactions don't reach yet: secure channels (whose reactions
/// travel encrypted) and channels shared between servers.
async fn refuse_elsewhere(conn: &turso::Connection, channel: &pb::Channel) -> Result<()> {
    if channel.r#type == pb::ChannelType::Secure as i32 {
        return Err(Error::FailedPrecondition("reactions in secure channels go through their devices".into()));
    }
    if polls::shared_out(conn, &channel.id).await? {
        return Err(Error::FailedPrecondition("reactions don't work in channels shared with other servers yet".into()));
    }
    Ok(())
}

impl Api {
    /// Refuses a channel this server shows from another one.
    async fn refuse_shown(&self, sdb: &crate::servers::ServerDb, channel_id: &str) -> Result<()> {
        if shared::link_of(&*sdb.read()?, channel_id).await?.is_some() {
            return Err(Error::FailedPrecondition(
                "reactions don't work in channels shared with other servers yet".into(),
            ));
        }
        Ok(())
    }

    pub(super) async fn react_impl(
        &self,
        account: &crate::node::Account,
        req: pb::ReactRequest,
    ) -> Result<pb::ReactResponse> {
        let Seat { sdb, access, .. } = self.membership(account, &req.server_id).await?;
        access.require_not_timed_out()?;
        access.require_in(&req.channel_id, Permission::ViewChannels)?;
        if req.reacted {
            access.require_in(&req.channel_id, Permission::AddReactions)?;
        }
        self.refuse_shown(&sdb, &req.channel_id).await?;
        let cap = self.app.settings().limits.reactions_per_message;
        let reaction = sdb
            .write(&account.id, async |conn, events| {
                let channel = load_channel(conn, &sdb.id, &req.channel_id).await?.ok_or(Error::NotFound("channel"))?;
                refuse_elsewhere(conn, &channel).await?;
                let message = load_message(conn, &sdb.id, &req.message_id)
                    .await?
                    .filter(|m| m.channel_id == channel.id)
                    .ok_or(Error::NotFound("message"))?;
                let (key, reaction) = named(conn, &req.emoji, &req.emoji_id).await?;
                let had = query_one(
                    conn,
                    "SELECT 1 FROM reactions WHERE message_id = ?1 AND emoji = ?2 AND account_id = ?3",
                    (message.id.as_str(), key.as_str(), account.id.as_str()),
                    |r| r.get::<i64>(0),
                )
                .await?
                .is_some();
                if had == req.reacted {
                    let count = count(conn, &message.id, &key).await?;
                    return Ok(pb::Reaction { count, me: had, ..reaction });
                }
                // Every reaction to a message rewrites its row, so two at once
                // clash and one runs again: the cap holds, and the counts in
                // events follow each other.
                conn.execute("UPDATE messages SET kind = kind WHERE id = ?1", [message.id.as_str()]).await?;
                if req.reacted {
                    if let Some(cap) = cap {
                        let (kinds, used) = query_one(
                            conn,
                            "SELECT count(DISTINCT emoji), sum(CASE WHEN emoji = ?2 THEN 1 ELSE 0 END)
                             FROM reactions WHERE message_id = ?1",
                            (message.id.as_str(), key.as_str()),
                            |r| Ok((r.get::<i64>(0)?, r.get::<Option<i64>>(1)?.unwrap_or(0))),
                        )
                        .await?
                        .unwrap_or((0, 0));
                        if used == 0 && kinds >= cap {
                            return Err(Error::ResourceExhausted(
                                "this message has as many different reactions as it can hold".into(),
                            ));
                        }
                    }
                    conn.execute(
                        "INSERT INTO reactions (message_id, emoji, account_id, created_at) VALUES (?1, ?2, ?3, ?4)",
                        (message.id.as_str(), key.as_str(), account.id.as_str(), now_ms()),
                    )
                    .await?;
                } else {
                    conn.execute(
                        "DELETE FROM reactions WHERE message_id = ?1 AND emoji = ?2 AND account_id = ?3",
                        (message.id.as_str(), key.as_str(), account.id.as_str()),
                    )
                    .await?;
                }
                let count = count(conn, &message.id, &key).await?;
                events.push(Payload::ReactionUpdated(pb::ReactionUpdated {
                    channel_id: channel.id.clone(),
                    message_id: message.id.clone(),
                    thread_id: message.thread_id.clone(),
                    reaction: Some(pb::Reaction { count, ..reaction.clone() }),
                    user_id: account.id.clone(),
                    added: req.reacted,
                }));
                Ok(pb::Reaction { count, me: req.reacted, ..reaction })
            })
            .await?;
        Ok(pb::ReactResponse { reaction: Some(reaction) })
    }

    pub(super) async fn list_reactors_impl(
        &self,
        account: &crate::node::Account,
        req: pb::ListReactorsRequest,
    ) -> Result<pb::ListReactorsResponse> {
        let Seat { sdb, access, .. } = self.membership(account, &req.server_id).await?;
        access.require_in(&req.channel_id, Permission::ViewChannels)?;
        self.refuse_shown(&sdb, &req.channel_id).await?;
        let conn = sdb.read()?;
        load_message(&conn, &sdb.id, &req.message_id)
            .await?
            .filter(|m| m.channel_id == req.channel_id)
            .ok_or(Error::NotFound("message"))?;
        let (key, _) = named(&conn, &req.emoji, &req.emoji_id).await?;
        let limit = if req.limit <= 0 { 50 } else { req.limit.min(MAX_PAGE) } as i64;
        // The page after a person: later reactions, and among those made in
        // the same millisecond, larger ids. One who took theirs off starts over.
        let after = if req.after_id.is_empty() {
            None
        } else {
            query_one(
                &conn,
                "SELECT created_at FROM reactions WHERE message_id = ?1 AND emoji = ?2 AND account_id = ?3",
                (req.message_id.as_str(), key.as_str(), req.after_id.as_str()),
                |r| r.get::<i64>(0),
            )
            .await?
        };
        let (cursor, at, id) = match after {
            Some(at) => ("AND (created_at > ?3 OR (created_at = ?3 AND account_id > ?4))", at, req.after_id.as_str()),
            None => ("AND ?3 = 0 AND ?4 = ''", 0, ""),
        };
        let ids = query_all(
            &conn,
            &format!(
                "SELECT account_id FROM reactions WHERE message_id = ?1 AND emoji = ?2 {cursor}
                 ORDER BY created_at, account_id LIMIT ?5"
            ),
            (req.message_id.as_str(), key.as_str(), at, id, limit + 1),
            |r| r.get::<String>(0),
        )
        .await?;
        let has_more = ids.len() as i64 > limit;
        let ids: Vec<&str> = ids.iter().take(limit as usize).map(String::as_str).collect();
        let mut found: HashMap<String, pb::User> =
            users(&conn, &ids).await?.into_iter().map(|u| (u.id.clone(), u)).collect();
        let users = ids.iter().filter_map(|id| found.remove(*id)).collect();
        Ok(pb::ListReactorsResponse { users, has_more })
    }

    pub(super) async fn clear_reactions_impl(
        &self,
        account: &crate::node::Account,
        req: pb::ClearReactionsRequest,
    ) -> Result<pb::ClearReactionsResponse> {
        let Seat { sdb, access, .. } = self.membership(account, &req.server_id).await?;
        access.require_not_timed_out()?;
        access.require_in(&req.channel_id, Permission::ViewChannels)?;
        access.require_in(&req.channel_id, Permission::ManageMessages)?;
        self.refuse_shown(&sdb, &req.channel_id).await?;
        sdb.write(&account.id, async |conn, events| {
            let channel = load_channel(conn, &sdb.id, &req.channel_id).await?.ok_or(Error::NotFound("channel"))?;
            let message = load_message(conn, &sdb.id, &req.message_id)
                .await?
                .filter(|m| m.channel_id == channel.id)
                .ok_or(Error::NotFound("message"))?;
            conn.execute("UPDATE messages SET kind = kind WHERE id = ?1", [message.id.as_str()]).await?;
            let (cleared, emoji, emoji_id) = if req.emoji.is_empty() && req.emoji_id.is_empty() {
                let gone = conn.execute("DELETE FROM reactions WHERE message_id = ?1", [message.id.as_str()]).await?;
                (gone, String::new(), String::new())
            } else {
                // A custom emoji deleted since is still cleared by its id.
                let (key, reaction) = if req.emoji.is_empty() && is_custom(&req.emoji_id) {
                    (req.emoji_id.clone(), pb::Reaction { emoji_id: req.emoji_id.clone(), ..Default::default() })
                } else {
                    named(conn, &req.emoji, &req.emoji_id).await?
                };
                let gone = conn
                    .execute(
                        "DELETE FROM reactions WHERE message_id = ?1 AND emoji = ?2",
                        (message.id.as_str(), key.as_str()),
                    )
                    .await?;
                (gone, reaction.emoji, reaction.emoji_id)
            };
            if cleared == 0 {
                return Ok(());
            }
            store::audit(
                conn,
                &account.id,
                Audit::new(pb::AuditAction::ReactionsClear, &message.author_id).channel(channel.name.clone()),
            )
            .await?;
            events.push(Payload::ReactionsCleared(pb::ReactionsCleared {
                channel_id: channel.id.clone(),
                message_id: message.id.clone(),
                thread_id: message.thread_id.clone(),
                emoji,
                emoji_id,
            }));
            Ok(())
        })
        .await?;
        Ok(pb::ClearReactionsResponse {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_one_emoji() {
        for ok in ["🍕", "1️⃣", "👩‍👩‍👧", "🇯🇵", "❤️", " 👍🏽 "] {
            assert!(standard(ok).is_ok(), "{ok}");
        }
        for bad in ["", "pizza", "1", "🍕 🍕", ":pizza:", "<:x:1>", "@everyone", "🍕🍕🍕🍕🍕🍕🍕🍕🍕"]
        {
            assert!(standard(bad).is_err(), "{bad}");
        }
        assert_eq!(standard(" 👍🏽 ").unwrap(), "👍🏽");
    }

    #[test]
    fn tells_custom_from_standard() {
        assert!(is_custom("01J9ZK8Q4V3M2N1P0R9S8T7U6W"));
        assert!(!is_custom("🍕"));
        assert!(!is_custom("1️⃣"));
        assert!(!is_custom(""));
    }
}
