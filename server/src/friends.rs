//! Friends, friend requests and blocks (docs/friends.md), kept in node.db's
//! `friend_links` and `friend_settings`, and who has a fuwa app open now.
//!
//! Each person has a row about the other, so one side can change without the
//! other: a block, or a declined request, which its sender isn't told about.
//! Every change that touches both people writes both rows, so two at once
//! (each asking the other, say) clash and one runs again on what the other
//! left, instead of both going through.
//!
//! Who's online lives only in memory: an account is online while it has a
//! `WatchFriends` stream open here. Split instances answer these on the
//! directory, so there's one count per instance.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as SyncMutex};

use tokio::sync::broadcast;
use turso::{Connection, Row, Value};

use crate::db::{self, Db, query_all, query_one};
use crate::error::{Error, Result};
use crate::pb::{self, FriendState};

const HOUR_MS: i64 = 60 * 60 * 1000;
/// How long a request waits for an answer before it runs out.
pub const REQUEST_LIFETIME_MS: i64 = 30 * 24 * HOUR_MS;
/// New requests one account may send in an hour.
pub const REQUESTS_PER_HOUR: u32 = 30;
/// Requests one account may have waiting at once.
pub const MAX_WAITING: i64 = 100;
/// The most mutual friends a profile shows.
pub const MAX_MUTUAL: usize = 50;
/// Events a watcher can fall behind by before its stream is ended.
const BUFFER: usize = 128;

/// One person's row about another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub account_id: String,
    pub other_id: String,
    pub state: FriendState,
    pub created_at: i64,
    pub expires_at: Option<i64>,
}

const LINK_COLUMNS: &str = "account_id, other_id, state, created_at, expires_at";

fn link_row(row: &Row) -> turso::Result<Link> {
    Ok(Link {
        account_id: row.get(0)?,
        other_id: row.get(1)?,
        state: FriendState::try_from(row.get::<i32>(2)?).unwrap_or(FriendState::Unspecified),
        created_at: row.get(3)?,
        expires_at: row.get(4)?,
    })
}

/// What sending a request did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sent {
    /// A new request. `delivered` is false when they blocked the sender: the
    /// sender's side looks the same, but theirs has nothing.
    Asked { mine: Link, delivered: bool },
    /// They had asked first (or once declined it, keeping theirs waiting), so
    /// it's a friendship now: both rows.
    Friends { mine: Link, theirs: Link },
    /// Nothing changed: already friends, or already asked.
    Already(Link),
}

/// What ending a link did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    /// The other person's row went too, so they're told.
    pub theirs: bool,
}

/// Friends' store and live state, where accounts are kept.
pub struct Friends {
    db: Arc<Db>,
    watchers: SyncMutex<HashMap<String, broadcast::Sender<Arc<pb::FriendEvent>>>>,
    /// Open `WatchFriends` streams per account.
    online: SyncMutex<HashMap<String, u32>>,
    /// New requests each account sent this hour: when the hour started, and how many.
    sent: SyncMutex<HashMap<String, (i64, u32)>>,
}

fn lock<T>(mutex: &SyncMutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

async fn link_of(conn: &Connection, account_id: &str, other_id: &str, now: i64) -> Result<Option<Link>> {
    query_one(
        conn,
        &format!(
            "SELECT {LINK_COLUMNS} FROM friend_links WHERE account_id = ?1 AND other_id = ?2
             AND (expires_at IS NULL OR expires_at > ?3)"
        ),
        (account_id, other_id, now),
        link_row,
    )
    .await
}

async fn put(conn: &Connection, link: &Link) -> Result<()> {
    conn.execute(
        "INSERT INTO friend_links (account_id, other_id, state, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (account_id, other_id) DO UPDATE SET
           state = excluded.state, created_at = excluded.created_at, expires_at = excluded.expires_at",
        (link.account_id.as_str(), link.other_id.as_str(), link.state as i64, link.created_at, link.expires_at),
    )
    .await?;
    Ok(())
}

async fn delete(conn: &Connection, account_id: &str, other_id: &str) -> Result<()> {
    conn.execute("DELETE FROM friend_links WHERE account_id = ?1 AND other_id = ?2", (account_id, other_id)).await?;
    Ok(())
}

fn friendship(account_id: &str, other_id: &str, now: i64) -> Link {
    Link {
        account_id: account_id.into(),
        other_id: other_id.into(),
        state: FriendState::Friend,
        created_at: now,
        expires_at: None,
    }
}

fn settings_row(row: &Row) -> turso::Result<pb::FriendSettings> {
    Ok(pb::FriendSettings {
        requests_from: row.get(0)?,
        direct_messages_from: row.get(1)?,
        hide_online: row.get(2)?,
        hide_mutual_friends: row.get(3)?,
    })
}

impl Friends {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db, watchers: SyncMutex::default(), online: SyncMutex::default(), sent: SyncMutex::default() }
    }

    fn read(&self) -> Result<Connection> {
        db::connect(&self.db)
    }

    // ───────────────────────── Reading ─────────────────────────

    /// Someone's rows about others, left out ones that ran out.
    pub async fn links(&self, account_id: &str, now: i64) -> Result<Vec<Link>> {
        query_all(
            &self.read()?,
            &format!(
                "SELECT {LINK_COLUMNS} FROM friend_links WHERE account_id = ?1
                 AND (expires_at IS NULL OR expires_at > ?2) ORDER BY created_at DESC"
            ),
            (account_id, now),
            link_row,
        )
        .await
    }

    /// One person's row about another.
    pub async fn link(&self, account_id: &str, other_id: &str, now: i64) -> Result<Option<Link>> {
        link_of(&self.read()?, account_id, other_id, now).await
    }

    /// Whether `by` blocked `whom`.
    pub async fn blocked(&self, by: &str, whom: &str) -> Result<bool> {
        Ok(self.link(by, whom, 0).await?.is_some_and(|link| link.state == FriendState::Blocked))
    }

    /// Everyone `by` blocked, with when.
    pub async fn blocks(&self, by: &str) -> Result<HashMap<String, i64>> {
        let rows = query_all(
            &self.read()?,
            "SELECT other_id, created_at FROM friend_links WHERE account_id = ?1 AND state = ?2",
            (by, FriendState::Blocked as i64),
            |row| Ok((row.get::<String>(0)?, row.get::<i64>(1)?)),
        )
        .await?;
        Ok(rows.into_iter().collect())
    }

    /// Whether the two are friends.
    pub async fn are_friends(&self, a: &str, b: &str) -> Result<bool> {
        Ok(self.link(a, b, 0).await?.is_some_and(|link| link.state == FriendState::Friend))
    }

    /// The ids of someone's friends.
    pub async fn friend_ids(&self, account_id: &str) -> Result<Vec<String>> {
        query_all(
            &self.read()?,
            "SELECT other_id FROM friend_links WHERE account_id = ?1 AND state = ?2",
            (account_id, FriendState::Friend as i64),
            |row| row.get::<String>(0),
        )
        .await
    }

    /// Friends both people have.
    pub async fn mutual(&self, a: &str, b: &str) -> Result<Vec<String>> {
        query_all(
            &self.read()?,
            "SELECT mine.other_id FROM friend_links mine JOIN friend_links theirs ON theirs.other_id = mine.other_id
             WHERE mine.account_id = ?1 AND theirs.account_id = ?2 AND mine.state = ?3 AND theirs.state = ?3
             ORDER BY mine.other_id",
            (a, b, FriendState::Friend as i64),
            |row| row.get::<String>(0),
        )
        .await
    }

    /// Requests someone has waiting that they sent.
    async fn waiting(conn: &Connection, account_id: &str, now: i64) -> Result<i64> {
        Ok(query_one(
            conn,
            "SELECT count(*) FROM friend_links WHERE account_id = ?1 AND state = ?2 AND expires_at > ?3",
            (account_id, FriendState::Outgoing as i64, now),
            |row| row.get::<i64>(0),
        )
        .await?
        .unwrap_or(0))
    }

    pub async fn settings(&self, account_id: &str) -> Result<pb::FriendSettings> {
        Ok(self.settings_of(&[account_id]).await?.remove(account_id).unwrap_or_default())
    }

    /// Several people's settings; people with none are left out (every default).
    pub async fn settings_of(&self, ids: &[&str]) -> Result<HashMap<String, pb::FriendSettings>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = (1..=ids.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
        query_all(
            &self.read()?,
            &format!(
                "SELECT requests_from, direct_messages_from, hide_online, hide_mutual_friends, account_id
                 FROM friend_settings WHERE account_id IN ({placeholders})"
            ),
            ids.iter().map(|id| Value::from(*id)).collect::<Vec<_>>(),
            |row| Ok((row.get::<String>(4)?, settings_row(row)?)),
        )
        .await
        .map(|rows| rows.into_iter().collect())
    }

    // ───────────────────────── Changing ─────────────────────────

    pub async fn save_settings(&self, account_id: &str, settings: &pb::FriendSettings) -> Result<()> {
        let all_default = *settings == pb::FriendSettings::default();
        db::write(&self.db, async |conn| {
            if all_default {
                conn.execute("DELETE FROM friend_settings WHERE account_id = ?1", [account_id]).await?;
            } else {
                conn.execute(
                    "INSERT INTO friend_settings (account_id, requests_from, direct_messages_from, hide_online, hide_mutual_friends)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT (account_id) DO UPDATE SET requests_from = excluded.requests_from,
                       direct_messages_from = excluded.direct_messages_from, hide_online = excluded.hide_online,
                       hide_mutual_friends = excluded.hide_mutual_friends",
                    (
                        account_id,
                        i64::from(settings.requests_from),
                        i64::from(settings.direct_messages_from),
                        settings.hide_online,
                        settings.hide_mutual_friends,
                    ),
                )
                .await?;
            }
            Ok(())
        })
        .await
    }

    /// `from` asks `to` to be friends. Whether `to` takes requests from
    /// `from` at all is checked before this; a block isn't, since it must
    /// look like any other request to its sender.
    pub async fn request(&self, from: &str, to: &str, now: i64) -> Result<Sent> {
        db::write(&self.db, async |conn| {
            let mine = link_of(conn, from, to, now).await?;
            let theirs = link_of(conn, to, from, now).await?;
            let mine_state = mine.as_ref().map(|link| link.state);
            let theirs_state = theirs.as_ref().map(|link| link.state);
            match (mine_state, theirs_state) {
                (Some(FriendState::Blocked), _) => {
                    return Err(Error::FailedPrecondition("you blocked them; unblock them first".into()));
                }
                (Some(FriendState::Friend | FriendState::Outgoing), _) => {
                    return Ok(Sent::Already(mine.expect("matched Some")));
                }
                // They asked: taking it is what asking back means. Their
                // request still waiting after being declined counts too.
                (Some(FriendState::Incoming), _) | (None, Some(FriendState::Outgoing | FriendState::Friend)) => {
                    let (mine, theirs) = (friendship(from, to, now), friendship(to, from, now));
                    put(conn, &mine).await?;
                    put(conn, &theirs).await?;
                    return Ok(Sent::Friends { mine, theirs });
                }
                _ => {}
            }
            // Every request from `from` writes this row, so two at once clash
            // and the one run again counts the other's.
            conn.execute(
                "INSERT INTO friend_senders (account_id, last_request_at) VALUES (?1, ?2)
                 ON CONFLICT (account_id) DO UPDATE SET last_request_at = excluded.last_request_at",
                (from, now),
            )
            .await?;
            if Self::waiting(conn, from, now).await? >= MAX_WAITING {
                return Err(Error::ResourceExhausted(format!(
                    "you have {MAX_WAITING} friend requests waiting; cancel some first"
                )));
            }
            let expires_at = Some(now + REQUEST_LIFETIME_MS);
            let mine = Link {
                account_id: from.into(),
                other_id: to.into(),
                state: FriendState::Outgoing,
                created_at: now,
                expires_at,
            };
            put(conn, &mine).await?;
            let delivered = theirs_state != Some(FriendState::Blocked);
            if delivered {
                put(
                    conn,
                    &Link {
                        account_id: to.into(),
                        other_id: from.into(),
                        state: FriendState::Incoming,
                        ..mine.clone()
                    },
                )
                .await?;
            }
            Ok(Sent::Asked { mine, delivered })
        })
        .await
    }

    /// `me` takes the request `other` sent: both rows become a friendship.
    pub async fn accept(&self, me: &str, other: &str, now: i64) -> Result<(Link, Link)> {
        db::write(&self.db, async |conn| {
            match link_of(conn, me, other, now).await? {
                Some(link) if link.state == FriendState::Incoming => {}
                _ => return Err(Error::NotFound("friend request")),
            }
            let (mine, theirs) = (friendship(me, other, now), friendship(other, me, now));
            put(conn, &mine).await?;
            put(conn, &theirs).await?;
            Ok((mine, theirs))
        })
        .await
    }

    /// Ends a friendship or a request, from `me`'s side. A request `me`
    /// declines keeps waiting on its sender's side, so they aren't told.
    pub async fn remove(&self, me: &str, other: &str, now: i64) -> Result<Removed> {
        db::write(&self.db, async |conn| {
            let mine = link_of(conn, me, other, now).await?.ok_or(Error::NotFound("friend"))?;
            let theirs = link_of(conn, other, me, now).await?;
            let take_theirs = match mine.state {
                FriendState::Friend => theirs.as_ref().is_some_and(|t| t.state == FriendState::Friend),
                FriendState::Outgoing => theirs.as_ref().is_some_and(|t| t.state == FriendState::Incoming),
                FriendState::Incoming => false,
                FriendState::Blocked => {
                    return Err(Error::FailedPrecondition("you blocked them; unblock them instead".into()));
                }
                FriendState::Unspecified => false,
            };
            delete(conn, me, other).await?;
            if take_theirs {
                delete(conn, other, me).await?;
            }
            Ok(Removed { theirs: take_theirs })
        })
        .await
    }

    /// `me` blocks `other`: a friendship ends, a request `me` sent goes, and
    /// a request `other` sent stays waiting on their side (they aren't told).
    pub async fn block(&self, me: &str, other: &str, now: i64) -> Result<(Link, Removed)> {
        db::write(&self.db, async |conn| {
            let theirs = link_of(conn, other, me, now).await?;
            let take_theirs =
                theirs.as_ref().is_some_and(|t| matches!(t.state, FriendState::Friend | FriendState::Incoming));
            let mine = match link_of(conn, me, other, now).await? {
                Some(link) if link.state == FriendState::Blocked => link,
                _ => Link {
                    account_id: me.into(),
                    other_id: other.into(),
                    state: FriendState::Blocked,
                    created_at: now,
                    expires_at: None,
                },
            };
            put(conn, &mine).await?;
            if take_theirs {
                delete(conn, other, me).await?;
            }
            Ok((mine, Removed { theirs: take_theirs }))
        })
        .await
    }

    pub async fn unblock(&self, me: &str, other: &str) -> Result<()> {
        db::write(&self.db, async |conn| match link_of(conn, me, other, 0).await? {
            Some(link) if link.state == FriendState::Blocked => delete(conn, me, other).await,
            _ => Err(Error::NotFound("block")),
        })
        .await
    }

    /// Forgets a deleted account: every row about it either way, and its
    /// settings. Returns who had a row about it, to tell them.
    pub async fn forget_account(&self, account_id: &str) -> Result<Vec<String>> {
        db::write(&self.db, async |conn| {
            let told =
                query_all(conn, "SELECT account_id FROM friend_links WHERE other_id = ?1", [account_id], |row| {
                    row.get::<String>(0)
                })
                .await?;
            conn.execute("DELETE FROM friend_links WHERE account_id = ?1 OR other_id = ?1", [account_id]).await?;
            conn.execute("DELETE FROM friend_settings WHERE account_id = ?1", [account_id]).await?;
            conn.execute("DELETE FROM friend_senders WHERE account_id = ?1", [account_id]).await?;
            Ok(told)
        })
        .await
    }

    /// Deletes requests that ran out, a batch at a time so no one write is long.
    pub async fn sweep(&self, now: i64) -> Result<u64> {
        const BATCH: i64 = 500;
        let mut total = 0;
        loop {
            let gone = db::write(&self.db, async |conn| {
                Ok(conn
                    .execute(
                        "DELETE FROM friend_links WHERE rowid IN (SELECT rowid FROM friend_links
                         WHERE expires_at IS NOT NULL AND expires_at <= ?1 LIMIT ?2)",
                        (now, BATCH),
                    )
                    .await?)
            })
            .await?;
            total += gone;
            if gone < BATCH as u64 {
                return Ok(total);
            }
        }
    }

    // ───────────────────────── Limits ─────────────────────────

    /// Takes one of `account_id`'s new requests for this hour, if any are
    /// left, in one step so requests at once can't both take the last. Give
    /// it back with [`Friends::refund`] when no new request came of it.
    pub fn reserve(&self, account_id: &str, now: i64) -> bool {
        let mut sent = lock(&self.sent);
        if sent.len() > 65_536 {
            sent.retain(|_, (start, _)| now - *start < HOUR_MS);
        }
        let (start, count) = sent.entry(account_id.to_string()).or_insert((now, 0));
        if now - *start >= HOUR_MS {
            (*start, *count) = (now, 0);
        }
        if *count >= REQUESTS_PER_HOUR {
            return false;
        }
        *count += 1;
        true
    }

    /// Gives back a request [`Friends::reserve`] took.
    pub fn refund(&self, account_id: &str) {
        if let Some((_, count)) = lock(&self.sent).get_mut(account_id) {
            *count = count.saturating_sub(1);
        }
    }

    // ───────────────────────── Live ─────────────────────────

    pub fn watch(&self, account_id: &str) -> broadcast::Receiver<Arc<pb::FriendEvent>> {
        let mut watchers = lock(&self.watchers);
        watchers.entry(account_id.to_string()).or_insert_with(|| broadcast::channel(BUFFER).0).subscribe()
    }

    /// Sends an event to everyone watching these accounts.
    pub fn publish<'a>(&self, account_ids: impl IntoIterator<Item = &'a str>, event: pb::FriendEvent) {
        let event = Arc::new(event);
        let mut watchers = lock(&self.watchers);
        for account_id in account_ids {
            let idle = match watchers.get(account_id) {
                None => continue,
                Some(sender) if sender.receiver_count() == 0 => true,
                Some(sender) => sender.send(event.clone()).is_err(),
            };
            if idle {
                watchers.remove(account_id);
            }
        }
    }

    /// Whether someone has a stream open.
    pub fn is_online(&self, account_id: &str) -> bool {
        lock(&self.online).get(account_id).is_some_and(|n| *n > 0)
    }

    /// Of these accounts, the ones with a stream open.
    pub fn online_of<'a>(&self, ids: impl IntoIterator<Item = &'a str>) -> HashSet<String> {
        let online = lock(&self.online);
        ids.into_iter().filter(|id| online.get(*id).is_some_and(|n| *n > 0)).map(str::to_string).collect()
    }

    /// Counts a stream opening; true when it's the account's first.
    pub fn opened(&self, account_id: &str) -> bool {
        let mut online = lock(&self.online);
        let count = online.entry(account_id.to_string()).or_insert(0);
        *count += 1;
        *count == 1
    }

    /// Counts a stream closing; true when it was the account's last.
    pub fn closed(&self, account_id: &str) -> bool {
        let mut online = lock(&self.online);
        let Some(count) = online.get_mut(account_id) else { return false };
        *count = count.saturating_sub(1);
        if *count == 0 {
            online.remove(account_id);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn open() -> (tempfile::TempDir, Friends) {
        let dir = tempfile::tempdir().unwrap();
        let node = crate::node::NodeDb::open(&dir.path().join("node.db"), None).await.unwrap();
        (dir, Friends::new(node.db().clone()))
    }

    fn state(link: Option<Link>) -> Option<FriendState> {
        link.map(|l| l.state)
    }

    #[tokio::test]
    async fn asking_back_makes_friends_and_removing_ends_it_for_both() {
        let (_dir, friends) = open().await;
        let Sent::Asked { delivered: true, .. } = friends.request("a", "b", 1).await.unwrap() else { panic!() };
        assert_eq!(state(friends.link("b", "a", 2).await.unwrap()), Some(FriendState::Incoming));
        let Sent::Friends { .. } = friends.request("b", "a", 2).await.unwrap() else { panic!() };
        assert!(friends.are_friends("a", "b").await.unwrap());
        assert!(matches!(friends.request("a", "b", 3).await.unwrap(), Sent::Already(_)));
        assert_eq!(friends.remove("a", "b", 4).await.unwrap(), Removed { theirs: true });
        assert_eq!(friends.link("b", "a", 4).await.unwrap(), None);
    }

    #[tokio::test]
    async fn declining_is_silent_and_asking_later_makes_friends() {
        let (_dir, friends) = open().await;
        friends.request("a", "b", 1).await.unwrap();
        assert_eq!(friends.remove("b", "a", 2).await.unwrap(), Removed { theirs: false });
        assert_eq!(state(friends.link("a", "b", 2).await.unwrap()), Some(FriendState::Outgoing));
        assert_eq!(friends.link("b", "a", 2).await.unwrap(), None);
        let Sent::Friends { .. } = friends.request("b", "a", 3).await.unwrap() else { panic!() };
    }

    #[tokio::test]
    async fn a_blocked_sender_sees_a_request_that_never_arrives() {
        let (_dir, friends) = open().await;
        friends.request("a", "b", 1).await.unwrap();
        friends.request("b", "a", 1).await.unwrap(); // friends
        let (_, removed) = friends.block("b", "a", 2).await.unwrap();
        assert!(removed.theirs);
        assert!(friends.blocked("b", "a").await.unwrap());
        assert_eq!(friends.blocks("b").await.unwrap(), HashMap::from([("a".to_string(), 2)]));
        assert!(friends.blocks("a").await.unwrap().is_empty());
        assert_eq!(
            friends.request("a", "b", 3).await.unwrap(),
            Sent::Asked {
                mine: Link {
                    account_id: "a".into(),
                    other_id: "b".into(),
                    state: FriendState::Outgoing,
                    created_at: 3,
                    expires_at: Some(3 + REQUEST_LIFETIME_MS),
                },
                delivered: false,
            }
        );
        assert_eq!(state(friends.link("b", "a", 3).await.unwrap()), Some(FriendState::Blocked));
        // The blocker can't ask, nor be asked into a friendship.
        assert!(friends.request("b", "a", 4).await.is_err());
        friends.unblock("b", "a").await.unwrap();
        let Sent::Friends { .. } = friends.request("b", "a", 5).await.unwrap() else { panic!() };
    }

    #[tokio::test]
    async fn requests_run_out_and_are_swept() {
        let (_dir, friends) = open().await;
        friends.request("a", "b", 1).await.unwrap();
        let later = 1 + REQUEST_LIFETIME_MS;
        assert!(friends.links("a", later).await.unwrap().is_empty());
        assert!(friends.accept("b", "a", later).await.is_err());
        assert_eq!(friends.sweep(later).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn mutual_friends_and_forgetting_an_account() {
        let (_dir, friends) = open().await;
        for (x, y) in [("a", "c"), ("b", "c"), ("a", "d")] {
            friends.request(x, y, 1).await.unwrap();
            friends.accept(y, x, 1).await.unwrap();
        }
        assert_eq!(friends.mutual("a", "b").await.unwrap(), vec!["c".to_string()]);
        let mut told = friends.forget_account("c").await.unwrap();
        told.sort();
        assert_eq!(told, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(friends.friend_ids("a").await.unwrap(), vec!["d".to_string()]);
    }

    #[tokio::test]
    async fn sending_is_limited_per_account() {
        let (_dir, friends) = open().await;
        for _ in 0..REQUESTS_PER_HOUR {
            assert!(friends.reserve("a", 0));
        }
        assert!(!friends.reserve("a", 0));
        friends.refund("a");
        assert!(friends.reserve("a", 0));
        assert!(!friends.reserve("a", 0));
        assert!(friends.reserve("b", 0));
        assert!(friends.reserve("a", HOUR_MS));
    }

    #[tokio::test]
    async fn sweep_deletes_what_ran_out_in_batches() {
        let (_dir, friends) = open().await;
        for n in 0..600 {
            friends.request("a", &format!("p{n}"), 0).await.ok();
            friends.request(&format!("q{n}"), "a", 0).await.unwrap();
        }
        friends.request("a", "late", REQUEST_LIFETIME_MS).await.unwrap();
        assert_eq!(friends.sweep(REQUEST_LIFETIME_MS).await.unwrap(), 2 * (MAX_WAITING as u64 + 600));
        assert_eq!(friends.links("a", REQUEST_LIFETIME_MS).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn settings_round_trip() {
        let (_dir, friends) = open().await;
        assert_eq!(friends.settings("a").await.unwrap(), pb::FriendSettings::default());
        let settings = pb::FriendSettings {
            requests_from: pb::FriendRequestsFrom::Nobody as i32,
            hide_online: true,
            ..Default::default()
        };
        friends.save_settings("a", &settings).await.unwrap();
        assert_eq!(friends.settings("a").await.unwrap(), settings);
        friends.save_settings("a", &pb::FriendSettings::default()).await.unwrap();
        assert!(friends.settings_of(&["a"]).await.unwrap().is_empty());
    }
}
