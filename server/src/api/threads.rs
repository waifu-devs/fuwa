//! Threads: replies under a message, kept out of the channel's own list
//! (docs/threads.md). A reply is an ordinary message with a `thread_id`, so
//! AutoMod, slow mode, mentions and permissions treat it like any other; the
//! `threads` table only sums each thread up for the channel to show.

use std::collections::{HashMap, VecDeque};

use super::{Api, Seat, shared, users};
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{now_ms, timestamp};
use crate::pb::{self, Permission};
use crate::permissions::Access;
use crate::servers::{self as store, Audit, Payload, UsageChange, load_channel};

/// Most participants a summary names.
const MAX_PARTICIPANTS: usize = 5;
/// Most threads one page of ListThreads holds.
const MAX_PAGE: i32 = 50;
/// Threads a search looks through, the latest first, before it stops.
const MAX_SEARCHED: i64 = 500;
/// Replies a search reads in one thread.
const MAX_SEARCHED_REPLIES: i64 = 1000;
/// Replies one search reads in all, across the threads it looks through.
const MAX_SEARCH_READ: i64 = 5000;
/// Searches one account may make a minute, over all its servers.
const SEARCHES_PER_MINUTE: usize = 30;
const MINUTE_MS: i64 = 60_000;
/// Longest search, in characters.
const MAX_QUERY: usize = 100;
/// Most followed threads ListFollowedThreads returns.
pub(super) const MAX_FOLLOWED: i64 = 500;

fn summary_row(r: &turso::Row) -> turso::Result<(String, pb::ThreadSummary)> {
    let participants = r.get::<String>(3)?;
    Ok((
        r.get(0)?,
        pb::ThreadSummary {
            reply_count: r.get(1)?,
            last_reply_at: Some(timestamp(r.get(2)?)),
            participant_ids: participants.split(',').filter(|id| !id.is_empty()).map(str::to_string).collect(),
            locked: r.get(4)?,
        },
    ))
}

const SUMMARY_COLUMNS: &str = "id, reply_count, last_reply_at, participants, locked";

/// A thread's summary, if there's a thread under `id`.
pub(super) async fn load(conn: &turso::Connection, id: &str) -> Result<Option<pb::ThreadSummary>> {
    Ok(query_one(conn, &format!("SELECT {SUMMARY_COLUMNS} FROM threads WHERE id = ?1"), [id], summary_row)
        .await?
        .map(|(_, summary)| summary))
}

/// Puts each message's thread summary on it, for the ones with a thread under them.
pub(super) async fn attach(conn: &turso::Connection, messages: &mut [pb::Message]) -> Result<()> {
    let ids = messages.iter().filter(|m| m.thread_id.is_empty()).map(|m| m.id.as_str()).collect::<Vec<_>>();
    let mut found = std::collections::HashMap::new();
    for chunk in ids.chunks(100) {
        let placeholders = (1..=chunk.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
        let rows = query_all(
            conn,
            &format!("SELECT {SUMMARY_COLUMNS} FROM threads WHERE id IN ({placeholders})"),
            chunk.iter().map(|id| turso::Value::from(*id)).collect::<Vec<_>>(),
            summary_row,
        )
        .await?;
        found.extend(rows);
    }
    for message in messages {
        if let Some(summary) = found.remove(&message.id) {
            message.thread = Some(summary);
        }
    }
    Ok(())
}

/// Works out a thread's summary again from its replies, inside a write, and
/// says so to everyone who sees the channel. A thread with no replies left
/// goes, unless it's locked.
pub(super) async fn refresh(
    conn: &turso::Connection,
    channel_id: &str,
    thread_id: &str,
    events: &mut Vec<Payload>,
) -> Result<pb::ThreadSummary> {
    let (count, last_id, last_at) = query_one(
        conn,
        "SELECT count(*), max(id), max(created_at) FROM messages WHERE thread_id = ?1",
        [thread_id],
        |r| Ok((r.get::<i64>(0)?, r.get::<Option<String>>(1)?, r.get::<Option<i64>>(2)?)),
    )
    .await?
    .unwrap_or((0, None, None));
    let locked = load(conn, thread_id).await?.is_some_and(|t| t.locked);
    let participants = query_all(
        conn,
        "SELECT author_id FROM messages WHERE thread_id = ?1 GROUP BY author_id ORDER BY max(id) DESC LIMIT ?2",
        (thread_id, MAX_PARTICIPANTS as i64),
        |r| r.get::<String>(0),
    )
    .await?;
    let started = query_one(conn, "SELECT created_at FROM messages WHERE id = ?1", [thread_id], |r| r.get::<i64>(0))
        .await?
        .unwrap_or_else(now_ms);
    let last_at = last_at.unwrap_or(started);
    let summary = pb::ThreadSummary {
        reply_count: count as i32,
        last_reply_at: Some(timestamp(last_at)),
        participant_ids: participants.clone(),
        locked,
    };
    if count == 0 && !locked {
        conn.execute("DELETE FROM threads WHERE id = ?1", [thread_id]).await?;
    } else {
        conn.execute(
            "INSERT INTO threads (id, channel_id, reply_count, last_reply_id, last_reply_at, participants, locked)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (id) DO UPDATE SET reply_count = excluded.reply_count, last_reply_id = excluded.last_reply_id,
               last_reply_at = excluded.last_reply_at, participants = excluded.participants, locked = excluded.locked",
            (
                thread_id,
                channel_id,
                count,
                last_id.as_deref().unwrap_or(thread_id),
                last_at,
                participants.join(","),
                locked,
            ),
        )
        .await?;
    }
    events.push(Payload::ThreadUpdated(pb::ThreadUpdated {
        channel_id: channel_id.to_string(),
        thread_id: thread_id.to_string(),
        thread: Some(summary.clone()),
    }));
    Ok(summary)
}

/// Follows a thread for someone, unless they unfollowed it before.
pub(super) async fn follow_quietly(conn: &turso::Connection, thread_id: &str, user_id: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO thread_follows (thread_id, user_id, follow) VALUES (?1, ?2, 1)",
        (thread_id, user_id),
    )
    .await?;
    Ok(())
}

/// What a reply needs from its thread, inside the write that sends it: the
/// message it's under, in this channel and not a reply itself; permission to
/// start the thread if it isn't there yet; and the thread not locked.
pub(super) async fn check_reply(
    conn: &turso::Connection,
    server_id: &str,
    access: &Access,
    channel: &pb::Channel,
    thread_id: &str,
) -> Result<pb::Message> {
    let parent = super::messages::load_message(conn, server_id, thread_id)
        .await?
        .filter(|m| m.channel_id == channel.id)
        .ok_or(Error::NotFound("message the thread is under"))?;
    if !parent.thread_id.is_empty() {
        return Err(Error::invalid("threads can't go under a reply in a thread"));
    }
    if parent.kind != pb::MessageKind::Unspecified as i32 {
        return Err(Error::invalid("threads only go under messages someone wrote"));
    }
    match load(conn, thread_id).await? {
        None => access.require_in(&channel.id, Permission::CreateThreads)?,
        Some(t) if t.locked && !access.has_in(&channel.id, Permission::ManageMessages) => {
            return Err(Error::denied("this thread is locked"));
        }
        Some(_) => {}
    }
    Ok(parent)
}

/// Takes away a thread with the message it's under, inside a write: its
/// replies, summary and follows, off the totals too. Replies also in the
/// channel are said to be deleted; the rest go with the parent's
/// MessageDeleted. Returns the replies' files, to drop after the write
/// ([`crate::attachments::drop_soon`]).
pub(super) async fn remove(
    conn: &turso::Connection,
    channel_id: &str,
    thread_id: &str,
    events: &mut Vec<Payload>,
) -> Result<Vec<String>> {
    let (count, bytes, attachments) = query_one(
        conn,
        "SELECT count(*), coalesce(sum(size), 0), coalesce(sum(attachment_count), 0) FROM messages WHERE thread_id = ?1",
        [thread_id],
        |r| Ok((r.get::<i64>(0)?, r.get::<i64>(1)?, r.get::<i64>(2)?)),
    )
    .await?
    .unwrap_or((0, 0, 0));
    let shown = query_all(conn, "SELECT id FROM messages WHERE thread_id = ?1 AND in_channel = 1", [thread_id], |r| {
        r.get::<String>(0)
    })
    .await?;
    super::polls::forget_thread(conn, thread_id).await?;
    let files = crate::attachments::forget_thread(conn, thread_id).await?;
    conn.execute("DELETE FROM messages WHERE thread_id = ?1", [thread_id]).await?;
    conn.execute("DELETE FROM threads WHERE id = ?1", [thread_id]).await?;
    conn.execute("DELETE FROM thread_follows WHERE thread_id = ?1", [thread_id]).await?;
    if count > 0 {
        store::add_usage(
            conn,
            UsageChange { messages: -count, message_bytes: -bytes, attachments: -attachments, ..Default::default() },
        )
        .await?;
    }
    for message_id in shown {
        events.push(Payload::MessageDeleted(pb::MessageDeleted { channel_id: channel_id.to_string(), message_id }));
    }
    Ok(files)
}

/// After a message is deleted, inside the same write: the thread it was a
/// reply in sums itself up again, and a thread under it goes with it.
/// Returns the files of the replies that went, to drop after the write.
pub(super) async fn after_delete(
    conn: &turso::Connection,
    channel_id: &str,
    message_id: &str,
    thread_id: &str,
    events: &mut Vec<Payload>,
) -> Result<Vec<String>> {
    if !thread_id.is_empty() {
        // Gone already when its parent went first.
        if query_one(conn, "SELECT 1 FROM messages WHERE id = ?1", [thread_id], |_| Ok(())).await?.is_some() {
            refresh(conn, channel_id, thread_id, events).await?;
        }
    } else if load(conn, message_id).await?.is_some() {
        return remove(conn, channel_id, message_id, events).await;
    }
    Ok(vec![])
}

/// Whether someone other than `author_id` replied in the thread under a message.
pub(super) async fn others_replied(conn: &turso::Connection, thread_id: &str, author_id: &str) -> Result<bool> {
    Ok(query_one(
        conn,
        "SELECT 1 FROM messages WHERE thread_id = ?1 AND author_id <> ?2 LIMIT 1",
        (thread_id, author_id),
        |_| Ok(()),
    )
    .await?
    .is_some())
}

/// The thread under `thread_id` in a channel the member can see, for the calls
/// that act on one.
async fn find(
    conn: &turso::Connection,
    server_id: &str,
    access: &Access,
    channel_id: &str,
    thread_id: &str,
) -> Result<(pb::Channel, pb::Message, pb::ThreadSummary)> {
    access.require_in(channel_id, Permission::ViewChannels)?;
    find_in(conn, server_id, channel_id, thread_id).await
}

/// The thread under `thread_id` in the channel, whoever asks: the message
/// it's under is one people wrote there, not a reply itself.
pub(super) async fn find_in(
    conn: &turso::Connection,
    server_id: &str,
    channel_id: &str,
    thread_id: &str,
) -> Result<(pb::Channel, pb::Message, pb::ThreadSummary)> {
    let channel = load_channel(conn, server_id, channel_id).await?.ok_or(Error::NotFound("channel"))?;
    let parent = super::messages::load_message(conn, server_id, thread_id)
        .await?
        .filter(|m| {
            m.channel_id == channel.id && m.thread_id.is_empty() && m.kind == pb::MessageKind::Unspecified as i32
        })
        .ok_or(Error::NotFound("thread"))?;
    let summary = load(conn, thread_id).await?.ok_or(Error::NotFound("thread"))?;
    Ok((channel, parent, summary))
}

/// A search's text as threads are searched by, checked and counted for
/// `searcher` (an account here, or a guest's id at a channel's home).
pub(super) fn search_text(query: &str, searcher: &str) -> Result<String> {
    let query = query.trim().to_lowercase();
    if query.chars().count() > MAX_QUERY {
        return Err(Error::invalid(format!("searches are at most {MAX_QUERY} characters")));
    }
    if !query.is_empty() {
        take_search(searcher, now_ms())?;
    }
    Ok(query)
}

/// A page of a channel's threads, as [`Api::list_threads_impl`] describes,
/// with `query` already from [`search_text`].
pub(super) async fn page(
    conn: &turso::Connection,
    server_id: &str,
    channel: &pb::Channel,
    query: &str,
    req: &pb::ListThreadsRequest,
) -> Result<pb::ListThreadsResponse> {
    let hours = store::load_server(conn).await?.thread_archive_hours;
    if req.archived && hours == 0 {
        return Ok(pb::ListThreadsResponse::default());
    }
    let cutoff = if hours == 0 { 0 } else { now_ms() - i64::from(hours) * 3_600_000 };
    let limit = if req.limit <= 0 { 25 } else { req.limit.min(MAX_PAGE) } as usize;
    let after = if req.after_thread_id.is_empty() {
        None
    } else {
        query_one(
            conn,
            "SELECT last_reply_id FROM threads WHERE id = ?1 AND channel_id = ?2",
            (req.after_thread_id.as_str(), channel.id.as_str()),
            |r| r.get::<String>(0),
        )
        .await?
    };
    let (age, cursor) = match (req.archived, &after) {
        (false, Some(_)) => ("last_reply_at >= ?2 AND last_reply_id < ?3", after.as_deref().unwrap_or_default()),
        (false, None) => ("last_reply_at >= ?2 AND ?3 = ''", ""),
        (true, Some(_)) => ("last_reply_at < ?2 AND last_reply_id < ?3", after.as_deref().unwrap_or_default()),
        (true, None) => ("last_reply_at < ?2 AND ?3 = ''", ""),
    };
    // A search reads through a bounded number of threads; a page without one stops at the limit.
    let scan = if query.is_empty() { limit as i64 + 1 } else { MAX_SEARCHED };
    let candidates = query_all(
        conn,
        &format!(
            "SELECT {SUMMARY_COLUMNS} FROM threads WHERE channel_id = ?1 AND {age} ORDER BY last_reply_id DESC LIMIT ?4"
        ),
        (channel.id.as_str(), cutoff, cursor, scan),
        summary_row,
    )
    .await?;
    let mut threads = Vec::new();
    let mut has_more = false;
    let mut budget = MAX_SEARCH_READ;
    let searched_all = (candidates.len() as i64) < scan;
    // The last thread looked at, where the next page carries on.
    let mut looked_at = String::new();
    for (id, summary) in candidates {
        if threads.len() == limit {
            has_more = true;
            break;
        }
        let Some(mut parent) = super::messages::load_message(conn, server_id, &id).await? else {
            looked_at = id;
            continue;
        };
        if !query.is_empty() && !parent.content.to_lowercase().contains(query) {
            // A search reads a bounded number of replies in all; past that it
            // stops and says there's more, so a busy channel never answers
            // "nothing found" without having looked everywhere.
            if budget <= 0 {
                has_more = true;
                break;
            }
            let replies = query_all(
                conn,
                "SELECT content FROM messages WHERE thread_id = ?1 ORDER BY id DESC LIMIT ?2",
                (id.as_str(), budget.min(MAX_SEARCHED_REPLIES)),
                |r| r.get::<String>(0),
            )
            .await?;
            budget -= replies.len().max(1) as i64;
            if !replies.iter().any(|c| c.to_lowercase().contains(query)) {
                looked_at = id;
                continue;
            }
        }
        looked_at = id;
        parent.thread = Some(summary);
        threads.push(parent);
    }
    // A search that read its whole share of threads may have more past them.
    if !query.is_empty() && !searched_all {
        has_more = true;
    }
    let mut ids = threads.iter().map(|m| m.author_id.as_str()).collect::<Vec<_>>();
    ids.extend(threads.iter().flat_map(|m| m.thread.iter().flat_map(|t| t.participant_ids.iter().map(String::as_str))));
    let authors = users(conn, &ids).await?;
    let next_after_thread_id = if has_more { looked_at } else { String::new() };
    Ok(pb::ListThreadsResponse { threads, authors, has_more, next_after_thread_id })
}

/// The threads someone follows in the channels they can see, latest reply first.
pub(super) async fn followed(
    conn: &turso::Connection,
    user_id: &str,
    can_see: impl Fn(&str) -> bool,
) -> Result<Vec<String>> {
    let rows = query_all(
        conn,
        "SELECT t.id, t.channel_id FROM thread_follows f JOIN threads t ON t.id = f.thread_id
         WHERE f.user_id = ?1 AND f.follow = 1 ORDER BY t.last_reply_id DESC LIMIT ?2",
        (user_id, MAX_FOLLOWED),
        |r| Ok((r.get::<String>(0)?, r.get::<String>(1)?)),
    )
    .await?;
    Ok(rows.into_iter().filter(|(_, channel)| can_see(channel)).map(|(id, _)| id).collect())
}

/// Follows or unfollows a thread for someone, by hand, inside a write.
pub(super) async fn set_follow(conn: &turso::Connection, thread_id: &str, user_id: &str, follow: bool) -> Result<()> {
    conn.execute(
        "INSERT INTO thread_follows (thread_id, user_id, follow) VALUES (?1, ?2, ?3)
         ON CONFLICT (thread_id, user_id) DO UPDATE SET follow = excluded.follow",
        (thread_id, user_id, follow),
    )
    .await?;
    Ok(())
}

impl Api {
    pub(super) async fn list_threads_impl(
        &self,
        account: &crate::node::Account,
        req: pb::ListThreadsRequest,
    ) -> Result<pb::ListThreadsResponse> {
        let Seat { sdb, access, .. } = self.membership(account, &req.server_id).await?;
        access.require_in(&req.channel_id, Permission::ViewChannels)?;
        let conn = sdb.read()?;
        let channel = load_channel(&conn, &sdb.id, &req.channel_id).await?.ok_or(Error::NotFound("channel"))?;
        // Shown from another server: its home keeps the threads and counts the search.
        if let Some(link) = shared::link_of(&conn, &channel.id).await? {
            let guest = shared::guest_of(&self.app, &conn, &sdb.id, account, &access, &link).await?;
            drop(conn);
            return shared::guest_threads(&self.app, &sdb.id, &link, guest, &req).await;
        }
        let query = search_text(&req.query, &account.id)?;
        page(&conn, &sdb.id, &channel, &query, &req).await
    }

    pub(super) async fn update_thread_impl(
        &self,
        account: &crate::node::Account,
        req: pb::UpdateThreadRequest,
    ) -> Result<pb::UpdateThreadResponse> {
        let Seat { sdb, access, .. } = self.membership(account, &req.server_id).await?;
        access.require_not_timed_out()?;
        access.require_in(&req.channel_id, Permission::ManageMessages)?;
        if shared::link_of(&*sdb.read()?, &req.channel_id).await?.is_some() {
            return Err(Error::FailedPrecondition("only the channel's home server can lock its threads".into()));
        }
        let thread = sdb
            .write(&account.id, async |conn, events| {
                let (channel, parent, summary) = find(conn, &sdb.id, &access, &req.channel_id, &req.thread_id).await?;
                let Some(locked) = req.locked.filter(|&l| l != summary.locked) else { return Ok(summary) };
                conn.execute("UPDATE threads SET locked = ?2 WHERE id = ?1", (parent.id.as_str(), locked)).await?;
                let action = if locked { pb::AuditAction::ThreadLock } else { pb::AuditAction::ThreadUnlock };
                store::audit(conn, &account.id, Audit::new(action, &parent.author_id).channel(channel.name.clone()))
                    .await?;
                refresh(conn, &channel.id, &parent.id, events).await
            })
            .await?;
        Ok(pb::UpdateThreadResponse { thread: Some(thread) })
    }

    pub(super) async fn follow_thread_impl(
        &self,
        account: &crate::node::Account,
        req: pb::FollowThreadRequest,
    ) -> Result<pb::FollowThreadResponse> {
        let Seat { sdb, access, .. } = self.membership(account, &req.server_id).await?;
        {
            let conn = sdb.read()?;
            if let Some(link) = shared::link_of(&conn, &req.channel_id).await? {
                access.require_in(&req.channel_id, Permission::ViewChannels)?;
                let guest = shared::guest_of(&self.app, &conn, &sdb.id, account, &access, &link).await?;
                drop(conn);
                shared::guest_follow(&self.app, &sdb.id, &link, guest, &req.thread_id, req.follow).await?;
                return Ok(pb::FollowThreadResponse {});
            }
        }
        // Following changes only what reaches you, so it's no event and no audit entry.
        sdb.write(&account.id, async |conn, _events| {
            find(conn, &sdb.id, &access, &req.channel_id, &req.thread_id).await?;
            set_follow(conn, &req.thread_id, &account.id, req.follow).await
        })
        .await?;
        Ok(pb::FollowThreadResponse {})
    }

    pub(super) async fn list_followed_threads_impl(
        &self,
        account: &crate::node::Account,
        req: pb::ListFollowedThreadsRequest,
    ) -> Result<pb::ListFollowedThreadsResponse> {
        let Seat { sdb, access, .. } = self.membership(account, &req.server_id).await?;
        let conn = sdb.read()?;
        let mut thread_ids = followed(&conn, &account.id, |channel| access.can_see(channel)).await?;
        // And the ones in channels shown here from other servers, from their
        // homes, all at once; one that doesn't answer adds none.
        let mut asks = Vec::new();
        for link in shared::links_in(&conn).await? {
            if !access.can_see(link.channel_id()) {
                continue;
            }
            // One link they can't reach now adds none rather than failing the rest.
            let Ok(guest) = shared::guest_of(&self.app, &conn, &sdb.id, account, &access, &link).await else {
                continue;
            };
            asks.push(shared::guest_followed(&self.app, &sdb.id, link, guest));
        }
        drop(conn);
        for ids in futures::future::join_all(asks).await {
            thread_ids.extend(ids.unwrap_or_default());
        }
        Ok(pb::ListFollowedThreadsResponse { thread_ids })
    }
}

/// Deleting a message with a thread under it: allowed for its author when every
/// reply is theirs too, and for moderators.
pub(super) async fn may_delete_with_thread(
    conn: &turso::Connection,
    access: &Access,
    account_id: &str,
    message: &pb::Message,
) -> Result<bool> {
    if !message.thread_id.is_empty() || load(conn, &message.id).await?.is_none() {
        return Ok(false);
    }
    if access.has_in(&message.channel_id, Permission::ManageMessages) {
        return Ok(true);
    }
    if message.author_id == account_id && !others_replied(conn, &message.id, account_id).await? {
        return Ok(true);
    }
    Err(Error::denied("others replied in the thread under this message; a moderator can delete it"))
}

/// The last minute of thread searches, per account, so no one can keep a
/// server busy reading replies. Kept in memory: limits reset on restart.
static SEARCHES: std::sync::LazyLock<std::sync::Mutex<HashMap<String, VecDeque<i64>>>> =
    std::sync::LazyLock::new(Default::default);

/// Counts a search now, or refuses it while the account is over its minute's share.
fn take_search(account_id: &str, now: i64) -> Result<()> {
    let mut searches = SEARCHES.lock().unwrap_or_else(|p| p.into_inner());
    // Forget accounts that have been quiet a minute, so the map stays small.
    if searches.len() > 4096 {
        searches.retain(|_, times| times.back().is_some_and(|&t| now - t < MINUTE_MS));
    }
    let times = searches.entry(account_id.to_string()).or_default();
    while times.front().is_some_and(|&t| now - t >= MINUTE_MS) {
        times.pop_front();
    }
    if times.len() >= SEARCHES_PER_MINUTE {
        return Err(Error::ResourceExhausted("too many thread searches; try again in a minute".into()));
    }
    times.push_back(now);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_are_limited_per_account_per_minute() {
        let now = 1_000_000;
        for _ in 0..SEARCHES_PER_MINUTE {
            take_search("limited", now).unwrap();
        }
        assert!(take_search("limited", now + 1).is_err());
        take_search("someone else", now + 1).unwrap();
        take_search("limited", now + MINUTE_MS).unwrap();
    }
}
