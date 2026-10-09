//! Reactions to messages in channels and threads (`MessageService.React`,
//! `ListReactors` and `ClearReactions`): one row per person, emoji and
//! message in the server file's `reactions`. Reads add up each message's
//! (`attach`); writes send `ReactionUpdated` or `ReactionsCleared`. Direct
//! messages' and secure channels' reactions travel inside the encryption
//! (`DirectMessageReaction`) and never reach here. In a shared channel the
//! home keeps them: a guest server passes its people's reactions there
//! (`shared::guest_react`), and the home's `apply` writes them like its own.

use std::collections::HashMap;

use super::messages::load_message;
use super::{Api, Seat, shared, users};
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

/// What a standard emoji is stored and matched by: its characters without
/// variation selectors, which emoji lists and keyboards add or leave out.
fn key_of(emoji: &str) -> String {
    emoji.replace('\u{FE0F}', "")
}

/// A standard emoji as sent: a few characters, at least one beyond ASCII,
/// with no letters, spaces or the marks Markdown and mentions use.
fn standard(value: &str) -> Result<String> {
    let value = value.trim();
    let fits = !key_of(value).is_empty()
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
            Ok((key_of(&emoji), pb::Reaction { emoji, ..Default::default() }))
        }
        (true, false) => {
            if !is_custom(emoji_id) {
                return Err(Error::NotFound("emoji"));
            }
            let (emoji_name, animated, emoji_url) =
                query_one(conn, "SELECT name, animated, url FROM emojis WHERE id = ?1", [emoji_id], |r| {
                    Ok((r.get::<String>(0)?, r.get::<bool>(1)?, r.get::<String>(2)?))
                })
                .await?
                .ok_or(Error::NotFound("emoji"))?;
            Ok((
                emoji_id.to_string(),
                pb::Reaction { emoji_id: emoji_id.to_string(), emoji_name, animated, emoji_url, ..Default::default() },
            ))
        }
        _ => Err(Error::invalid("name one emoji: emoji or emoji_id")),
    }
}

/// Checks the emoji a call names looks like one, before it goes to another
/// server: one standard emoji, or one custom emoji's id.
pub(super) fn check_emoji(emoji: &str, emoji_id: &str) -> Result<()> {
    match (emoji.is_empty(), emoji_id.is_empty()) {
        (false, true) => standard(emoji).map(|_| ()),
        (true, false) if is_custom(emoji_id) && emoji_id.len() <= 32 => Ok(()),
        (true, false) => Err(Error::NotFound("emoji")),
        _ => Err(Error::invalid("name one emoji: emoji or emoji_id")),
    }
}

/// A reaction from another instance, as this one keeps it: one emoji (a
/// standard one, or a custom one with a name and a picture), a count.
/// `None` for anything else.
pub(super) fn arrived(reaction: pb::Reaction) -> Option<pb::Reaction> {
    let custom = !reaction.emoji_id.is_empty();
    let fits = if custom {
        is_custom(&reaction.emoji_id)
            && reaction.emoji_id.len() <= 32
            && reaction.emoji.is_empty()
            && (2..=32).contains(&reaction.emoji_name.len())
            && reaction.emoji_name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && !reaction.emoji_url.is_empty()
    } else {
        standard(&reaction.emoji).is_ok()
            && reaction.emoji_name.is_empty()
            && reaction.emoji_url.is_empty()
            && !reaction.animated
    };
    fits.then_some(pb::Reaction { me: false, ..reaction })
}

/// How many reacted to `message_id` with `key`.
async fn count(conn: &turso::Connection, message_id: &str, key: &str) -> Result<u32> {
    Ok(query_one(conn, "SELECT count(*) FROM reactions WHERE message_id = ?1 AND emoji = ?2", (message_id, key), |r| {
        r.get::<i64>(0)
    })
    .await?
    .unwrap_or(0) as u32)
}

/// How a standard emoji's reaction to a message is shown: as its first
/// reactor wrote it, or `fallback` when nobody has reacted with it.
async fn shown_as(conn: &turso::Connection, message_id: &str, key: &str, fallback: &str) -> Result<String> {
    if is_custom(key) {
        return Ok(String::new());
    }
    let first = query_one(
        conn,
        "SELECT shown FROM reactions WHERE message_id = ?1 AND emoji = ?2 ORDER BY created_at, account_id LIMIT 1",
        (message_id, key),
        |r| r.get::<String>(0),
    )
    .await?;
    Ok(first.filter(|s| !s.is_empty()).unwrap_or_else(|| fallback.to_string()))
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
                    sum(CASE WHEN r.account_id = ?1 THEN 1 ELSE 0 END), e.name, e.animated, e.url,
                    (SELECT f.shown FROM reactions f WHERE f.message_id = r.message_id AND f.emoji = r.emoji
                     ORDER BY f.created_at, f.account_id LIMIT 1)
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
                r.get::<Option<String>>(7)?,
                r.get::<Option<String>>(8)?,
            ))
        },
    )
    .await?;
    let mut found: HashMap<String, Vec<pb::Reaction>> = HashMap::new();
    for (message_id, key, count, mine, name, animated, url, shown) in rows {
        let reaction = if is_custom(&key) {
            let Some(name) = name else { continue };
            pb::Reaction {
                emoji_id: key,
                emoji_name: name,
                animated: animated.unwrap_or(false),
                emoji_url: url.unwrap_or_default(),
                ..Default::default()
            }
        } else {
            let emoji = shown.filter(|s| !s.is_empty()).unwrap_or(key);
            pb::Reaction { emoji, ..Default::default() }
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
    let mut before = Vec::with_capacity(theirs.len());
    for (message_id, key, ..) in &theirs {
        before.push(shown_as(conn, message_id, key, key).await?);
    }
    conn.execute("DELETE FROM reactions WHERE account_id = ?1", [account_id]).await?;
    for ((message_id, key, channel_id, thread_id), shown) in theirs.into_iter().zip(before) {
        let Ok((_, mut reaction)) =
            (if is_custom(&key) { named(conn, "", &key).await } else { named(conn, &key, "").await })
        else {
            continue;
        };
        if !is_custom(&key) {
            reaction.emoji = shown_as(conn, &message_id, &key, &shown).await?;
        }
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

/// Refuses secure channels, whose reactions travel encrypted.
fn refuse_secure(channel: &pb::Channel) -> Result<()> {
    if channel.r#type == pb::ChannelType::Secure as i32 {
        return Err(Error::FailedPrecondition("reactions in secure channels go through their devices".into()));
    }
    Ok(())
}

/// Puts `user_id`'s reaction on `message` or takes it off, inside a write,
/// telling members: the emoji's reaction as it is now, `me` for them. A
/// shared channel's home calls this for guests too.
#[allow(clippy::too_many_arguments)]
pub(super) async fn apply(
    conn: &turso::Connection,
    message: &pb::Message,
    emoji: &str,
    emoji_id: &str,
    user_id: &str,
    reacted: bool,
    cap: Option<i64>,
    events: &mut Vec<Payload>,
) -> Result<pb::Reaction> {
    let (key, mut reaction) = named(conn, emoji, emoji_id).await?;
    let sent = std::mem::take(&mut reaction.emoji);
    let before = shown_as(conn, &message.id, &key, &sent).await?;
    let had = query_one(
        conn,
        "SELECT 1 FROM reactions WHERE message_id = ?1 AND emoji = ?2 AND account_id = ?3",
        (message.id.as_str(), key.as_str(), user_id),
        |r| r.get::<i64>(0),
    )
    .await?
    .is_some();
    if had == reacted {
        let count = count(conn, &message.id, &key).await?;
        return Ok(pb::Reaction { count, me: had, emoji: before, ..reaction });
    }
    // Every reaction to a message rewrites its row, so two at once clash and
    // one runs again: the cap holds, and the counts in events follow each other.
    conn.execute("UPDATE messages SET kind = kind WHERE id = ?1", [message.id.as_str()]).await?;
    if reacted {
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
            "INSERT INTO reactions (message_id, emoji, account_id, created_at, shown) VALUES (?1, ?2, ?3, ?4, ?5)",
            (message.id.as_str(), key.as_str(), user_id, now_ms(), sent.as_str()),
        )
        .await?;
    } else {
        conn.execute(
            "DELETE FROM reactions WHERE message_id = ?1 AND emoji = ?2 AND account_id = ?3",
            (message.id.as_str(), key.as_str(), user_id),
        )
        .await?;
    }
    let count = count(conn, &message.id, &key).await?;
    // Shown as before, even once the last one's gone, so apps find the chip
    // they have.
    reaction.emoji = shown_as(conn, &message.id, &key, &before).await?;
    events.push(Payload::ReactionUpdated(pb::ReactionUpdated {
        channel_id: message.channel_id.clone(),
        message_id: message.id.clone(),
        thread_id: message.thread_id.clone(),
        reaction: Some(pb::Reaction { count, ..reaction.clone() }),
        user_id: user_id.to_string(),
        added: reacted,
    }));
    Ok(pb::Reaction { count, me: reacted, ..reaction })
}

/// A page of who reacted to `message_id` with an emoji, the earliest first.
pub(super) async fn reactor_page(
    conn: &turso::Connection,
    message_id: &str,
    emoji: &str,
    emoji_id: &str,
    limit: i32,
    after_id: &str,
) -> Result<pb::ListReactorsResponse> {
    let (key, _) = named(conn, emoji, emoji_id).await?;
    let limit = if limit <= 0 { 50 } else { limit.min(MAX_PAGE) } as i64;
    // The page after a person: later reactions, and among those made in the
    // same millisecond, larger ids. One who took theirs off starts over.
    let after = if after_id.is_empty() {
        None
    } else {
        query_one(
            conn,
            "SELECT created_at FROM reactions WHERE message_id = ?1 AND emoji = ?2 AND account_id = ?3",
            (message_id, key.as_str(), after_id),
            |r| r.get::<i64>(0),
        )
        .await?
    };
    let (cursor, at, id) = match after {
        Some(at) => ("AND (created_at > ?3 OR (created_at = ?3 AND account_id > ?4))", at, after_id),
        None => ("AND ?3 = 0 AND ?4 = ''", 0, ""),
    };
    let ids = query_all(
        conn,
        &format!(
            "SELECT account_id FROM reactions WHERE message_id = ?1 AND emoji = ?2 {cursor}
             ORDER BY created_at, account_id LIMIT ?5"
        ),
        (message_id, key.as_str(), at, id, limit + 1),
        |r| r.get::<String>(0),
    )
    .await?;
    let has_more = ids.len() as i64 > limit;
    let ids: Vec<&str> = ids.iter().take(limit as usize).map(String::as_str).collect();
    let mut found: HashMap<String, pb::User> =
        users(conn, &ids).await?.into_iter().map(|u| (u.id.clone(), u)).collect();
    let users = ids.iter().filter_map(|id| found.remove(*id)).collect();
    Ok(pb::ListReactorsResponse { users, has_more })
}

impl Api {
    /// Refuses a channel this server shows from another one: only its home
    /// clears reactions there.
    async fn refuse_shown(&self, sdb: &crate::servers::ServerDb, channel_id: &str) -> Result<()> {
        if shared::link_of(&*sdb.read()?, channel_id).await?.is_some() {
            return Err(Error::FailedPrecondition("only the channel's home server can clear its reactions".into()));
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
        if let Some((link, guest)) =
            shared::locate(&self.app, &*sdb.read()?, &sdb.id, account, &access, &req.channel_id, &req.message_id)
                .await?
        {
            check_emoji(&req.emoji, &req.emoji_id)?;
            let reaction = shared::guest_react(&self.app, &sdb.id, &link, guest, &req).await?;
            return Ok(pb::ReactResponse { reaction: Some(reaction) });
        }
        let cap = self.app.settings().limits.reactions_per_message;
        let reaction = sdb
            .write(&account.id, async |conn, events| {
                let channel = load_channel(conn, &sdb.id, &req.channel_id).await?.ok_or(Error::NotFound("channel"))?;
                refuse_secure(&channel)?;
                let message = load_message(conn, &sdb.id, &req.message_id)
                    .await?
                    .filter(|m| m.channel_id == channel.id)
                    .ok_or(Error::NotFound("message"))?;
                apply(conn, &message, &req.emoji, &req.emoji_id, &account.id, req.reacted, cap, events).await
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
        if let Some((link, guest)) =
            shared::locate(&self.app, &*sdb.read()?, &sdb.id, account, &access, &req.channel_id, &req.message_id)
                .await?
        {
            check_emoji(&req.emoji, &req.emoji_id)?;
            return shared::guest_reactors(&self.app, &sdb.id, &link, guest, &req).await;
        }
        let conn = sdb.read()?;
        load_message(&conn, &sdb.id, &req.message_id)
            .await?
            .filter(|m| m.channel_id == req.channel_id)
            .ok_or(Error::NotFound("message"))?;
        reactor_page(&conn, &req.message_id, &req.emoji, &req.emoji_id, req.limit, &req.after_id).await
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
                let shown = shown_as(conn, &message.id, &key, &reaction.emoji).await?;
                let gone = conn
                    .execute(
                        "DELETE FROM reactions WHERE message_id = ?1 AND emoji = ?2",
                        (message.id.as_str(), key.as_str()),
                    )
                    .await?;
                (gone, shown, reaction.emoji_id)
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
        assert!(standard("\u{FE0F}").is_err(), "a variation selector alone is nothing");
        assert_eq!(key_of("👍\u{FE0F}"), key_of("👍"));
        assert_eq!(key_of("❤\u{FE0F}"), "❤");
    }

    #[test]
    fn tells_custom_from_standard() {
        assert!(is_custom("01J9ZK8Q4V3M2N1P0R9S8T7U6W"));
        assert!(!is_custom("🍕"));
        assert!(!is_custom("1️⃣"));
        assert!(!is_custom(""));
    }
}
