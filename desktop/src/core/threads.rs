//! Threads under messages: their replies, a channel's list of them, and
//! following and locking them. A port of the web app's thread actions in
//! `web/src/fuwa/actions.ts`, keeping replies in the store under
//! `thread_key` of the message they're under, as the web app does.

use std::collections::HashSet;

use crate::core::api::Problem;
use crate::core::store::{self, ChannelMessages, InstanceState};
use crate::core::{Core, reports};
use crate::pb;
use crate::rpc;

/// Where a thread's replies are kept in `messages` and `pending`.
pub fn thread_key(thread_id: &str) -> String {
    format!("t:{thread_id}")
}

/// A thread with no reply for longer than the server keeps threads open
/// (`Server.thread_archive_hours`, 0 for never) is archived.
pub fn is_archived(summary: &pb::ThreadSummary, archive_hours: i32, now_ms: i64) -> bool {
    let Some(last) = &summary.last_reply_at else { return false };
    archive_hours > 0
        && now_ms - (last.seconds * 1000 + i64::from(last.nanos) / 1_000_000) > i64::from(archive_hours) * 3_600_000
}

/// A summary worth showing: a thread with replies, or one locked before anyone replied.
fn shown(summary: Option<&pb::ThreadSummary>) -> Option<pb::ThreadSummary> {
    summary.filter(|t| t.reply_count > 0 || t.locked).cloned()
}

/// A thread's new summary, on the message it's under wherever that's kept.
pub fn with_thread_summary(
    i: &mut InstanceState,
    channel_id: &str,
    thread_id: &str,
    summary: Option<&pb::ThreadSummary>,
) {
    let summary = shown(summary);
    if let Some(m) = i.messages.get_mut(channel_id).and_then(|l| l.items.iter_mut().find(|m| m.id == thread_id)) {
        m.thread = summary.clone();
    }
    if let Some(parent) = i.thread_parents.get_mut(thread_id) {
        parent.thread = summary;
    }
}

/// Forgets a thread: its replies, what was being sent to it and its unread count.
pub fn forget_thread(i: &mut InstanceState, thread_id: &str) {
    let key = thread_key(thread_id);
    i.messages.remove(&key);
    i.pending.remove(&key);
    i.thread_parents.remove(thread_id);
    i.thread_unread.remove(thread_id);
}

/// Forgets the threads under messages of channels that are gone.
pub fn forget_threads_in(i: &mut InstanceState, channels: &HashSet<&str>) {
    let gone: Vec<String> =
        i.thread_parents.values().filter(|p| channels.contains(p.channel_id.as_str())).map(|p| p.id.clone()).collect();
    for id in gone {
        forget_thread(i, &id);
    }
}

/// Unread replies in followed threads under a channel's messages.
pub fn channel_thread_unread(i: &InstanceState, channel_id: &str) -> u32 {
    i.thread_unread
        .iter()
        .filter(|(id, _)| i.thread_parents.get(*id).is_some_and(|p| p.channel_id == channel_id))
        .map(|(_, n)| *n)
        .sum()
}

/// Whether you follow a thread, once the server said which ones you do.
pub fn follows(i: &InstanceState, server_id: &str, thread_id: &str) -> Option<bool> {
    i.followed.get(server_id).map(|f| f.contains(thread_id))
}

/// Where a reply goes: the thread under a message, and whether the channel shows it too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadTarget {
    pub thread_id: String,
    pub also_to_channel: bool,
}

impl Core {
    /// Loads a thread's latest replies, and the message it's under, the first
    /// time it opens; older replies with `older`.
    pub async fn load_thread(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        thread_id: &str,
        older: bool,
    ) -> Result<(), Problem> {
        self.load_list(key, server_id, channel_id, thread_id, older).await
    }

    /// Replies in a thread, and with `also_to_channel` in the channel too.
    pub async fn send_reply(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        content: &str,
        target: ThreadTarget,
    ) -> Result<(), Problem> {
        self.send_to(key, server_id, channel_id, content, Vec::new(), Some(&target)).await?;
        // Replying follows a thread unless you unfollowed it once, which only the server knows: ask it again.
        let ask = self.shared.read(|s| {
            s.instance(key).and_then(|i| i.followed.get(server_id)).is_some_and(|f| !f.contains(&target.thread_id))
        });
        if ask {
            self.shared.instance(key, |i| {
                i.followed.remove(server_id);
            });
            let _ = self.load_followed(key, server_id).await;
        }
        Ok(())
    }

    /// A channel's threads, the latest reply first, matching `query` when there is one.
    pub async fn list_threads(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        query: &str,
        archived: bool,
        after_thread_id: &str,
    ) -> Result<pb::ListThreadsResponse, Problem> {
        let api = self.api(key).ok_or_else(|| Problem::new(tonic::Code::NotFound, "That instance isn't here."))?;
        let res = rpc!(
            api.messages(),
            list_threads(pb::ListThreadsRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                query: query.chars().take(100).collect(),
                archived,
                limit: 25,
                after_thread_id: after_thread_id.into(),
            })
        )
        .await?;
        self.shared.instance(key, |i| {
            for user in &res.authors {
                i.users.insert(user.id.clone(), user.clone());
            }
            store::add_shared_authors(&mut i.users, &res.threads);
        });
        Ok(res)
    }

    /// Locks or unlocks a thread, for people who manage messages.
    pub async fn lock_thread(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        thread_id: &str,
        locked: bool,
    ) -> Result<(), Problem> {
        let Some(api) = self.api(key) else { return Ok(()) };
        let res = rpc!(
            api.messages(),
            update_thread(pb::UpdateThreadRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                thread_id: thread_id.into(),
                locked: Some(locked),
            })
        )
        .await?;
        reports::used(if locked { "thread.lock" } else { "thread.unlock" });
        self.shared.instance(key, |i| with_thread_summary(i, channel_id, thread_id, res.thread.as_ref()));
        Ok(())
    }

    /// The threads you follow in a server, once (replying and following keep it current after).
    pub async fn load_followed(&self, key: &str, server_id: &str) -> Result<(), Problem> {
        let Some(api) = self.api(key) else { return Ok(()) };
        if self.shared.read(|s| s.instance(key).is_some_and(|i| i.followed.contains_key(server_id))) {
            return Ok(());
        }
        let res =
            rpc!(api.messages(), list_followed_threads(pb::ListFollowedThreadsRequest { server_id: server_id.into() }))
                .await?;
        self.shared.instance(key, |i| {
            if i.server(server_id).is_some() {
                i.followed.insert(server_id.to_owned(), res.thread_ids.into_iter().collect());
            }
        });
        Ok(())
    }

    /// Follows a thread, to hear about its replies, or stops.
    pub async fn follow_thread(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        thread_id: &str,
        follow: bool,
    ) -> Result<(), Problem> {
        let Some(api) = self.api(key) else { return Ok(()) };
        rpc!(
            api.messages(),
            follow_thread(pb::FollowThreadRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                thread_id: thread_id.into(),
                follow,
            })
        )
        .await?;
        reports::used(if follow { "thread.follow" } else { "thread.unfollow" });
        self.shared.instance(key, |i| {
            let list = i.followed.entry(server_id.to_owned()).or_default();
            if follow {
                list.insert(thread_id.to_owned());
            } else {
                list.remove(thread_id);
                i.thread_unread.remove(thread_id);
            }
        });
        Ok(())
    }

    /// Marks a thread as open beside its channel and clears its unread count; `None` when it closes.
    pub fn focus_thread(&self, key: &str, thread_id: Option<&str>) {
        self.shared.update(|s| {
            let Some(focus) = s.focus.as_mut().filter(|f| f.instance == key) else { return };
            focus.thread = thread_id.map(str::to_owned);
            if let (Some(i), Some(id)) = (s.instances.get_mut(key), thread_id) {
                i.thread_unread.remove(id);
            }
        });
    }
}

/// Puts a loaded page of a thread's replies (or a channel's messages) away.
pub(crate) fn put_page(i: &mut InstanceState, at: &str, res: pb::ListMessagesResponse, fresh: bool, older: bool) {
    for user in res.authors {
        i.users.insert(user.id.clone(), user);
    }
    store::add_shared_authors(&mut i.users, &res.messages);
    if let Some(parent) = res.parent {
        store::add_shared_authors(&mut i.users, std::slice::from_ref(&parent));
        i.thread_parents.insert(parent.id.clone(), parent);
    }
    let entry: &mut ChannelMessages = i.messages.entry(at.to_owned()).or_default();
    for m in res.messages {
        store::put_message(&mut entry.items, m);
    }
    if older || fresh {
        entry.has_more = res.has_more;
    }
    entry.loading = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threads_archive_after_the_servers_hours() {
        let at = |seconds| pb::ThreadSummary {
            last_reply_at: Some(prost_types::Timestamp { seconds, nanos: 0 }),
            ..Default::default()
        };
        let now = 10 * 3_600_000;
        assert!(!is_archived(&at(9 * 3600), 2, now));
        assert!(is_archived(&at(7 * 3600), 2, now));
        // 0 keeps every thread open.
        assert!(!is_archived(&at(0), 0, now));
    }
}
