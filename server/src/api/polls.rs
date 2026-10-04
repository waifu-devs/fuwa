//! Polls: a message with a question and answers people vote on. The poll
//! itself is a row in `polls` beside its message, with its tally; votes are
//! rows in `poll_votes`, one per voter and answer.
//!
//! Anonymous polls name nobody, to anyone: their votes are stored under an
//! HMAC of the account id keyed by the poll's own random `voter_key`, read
//! back only for the voter themselves; their events carry no actor and no
//! voter; ListPollVoters refuses them; exports leave their votes out; and
//! their counts stay hidden until they close (a count going up by one right
//! after someone was seen typing would say what they picked). When one
//! closes, its votes and key are deleted from the live tables and only the
//! counts are kept. What remains: while it runs, the operator holds the key
//! and the votes in the same file, so someone with the database could work
//! out who voted for what, and the replica's history and the file's log can
//! hold deleted rows for a while after.

use std::collections::HashMap;
use std::sync::Mutex;

use hmac::{Hmac, Mac};
use prost::Message as _;
use sha2::Sha256;
use tonic::{Request, Response, Status};

use super::{Api, Seat, respond, text, users};
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{now_ms, timestamp};
use crate::pb::{self, Permission};
use crate::servers::{self as store, Audit, Payload, ServerDb, load_channel};

pub const MIN_ANSWERS: usize = 2;
pub const MAX_ANSWERS: usize = 10;
const MAX_QUESTION: usize = 300;
const MAX_ANSWER: usize = 55;
/// Two weeks.
const MAX_HOURS: i32 = 336;

/// What's stored in `polls.answers`.
#[derive(Clone, PartialEq, prost::Message)]
struct Answers {
    #[prost(message, repeated, tag = "1")]
    answers: Vec<pb::PollAnswer>,
}

/// What's stored in `polls.tally`: votes per answer, in the answers' order,
/// and how many people voted.
#[derive(Clone, PartialEq, prost::Message)]
struct Tally {
    #[prost(int64, repeated, tag = "1")]
    votes: Vec<i64>,
    #[prost(int64, tag = "2")]
    voters: i64,
}

/// A poll its creator wrote, checked: what goes into its message.
pub(super) fn check(new: &pb::NewPoll, now: i64) -> Result<pb::Poll> {
    let question = text("a poll's question", &new.question, 1, MAX_QUESTION)?;
    if new.answers.len() < MIN_ANSWERS || new.answers.len() > MAX_ANSWERS {
        return Err(Error::invalid(format!("a poll needs {MIN_ANSWERS} to {MAX_ANSWERS} answers")));
    }
    let mut answers = Vec::with_capacity(new.answers.len());
    for (n, answer) in new.answers.iter().enumerate() {
        answers.push(pb::PollAnswer {
            id: n as u32 + 1,
            text: text("each answer", &answer.text, 1, MAX_ANSWER)?,
            emoji: emoji(&answer.emoji)?,
            votes: 0,
        });
    }
    if !(0..=MAX_HOURS).contains(&new.duration_hours) {
        return Err(Error::invalid("a poll runs for 1 hour to 2 weeks, or until it's ended"));
    }
    let ends_at = (new.duration_hours > 0).then(|| timestamp(now + i64::from(new.duration_hours) * 3_600_000));
    Ok(pb::Poll { question, answers, multiple: new.multiple, anonymous: new.anonymous, ends_at, ..Default::default() })
}

/// An answer's emoji: empty, a server emoji token (`<:name:id>` or
/// `<a:name:id>`), or a few characters with no letters, digits words or
/// spaces in them, such as "🍕" or "1️⃣".
fn emoji(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    let token = value
        .strip_prefix("<a:")
        .or_else(|| value.strip_prefix("<:"))
        .and_then(|rest| rest.strip_suffix('>'))
        .and_then(|rest| rest.split_once(':'))
        .is_some_and(|(name, id)| {
            (1..=32).contains(&name.len())
                && (1..=32).contains(&id.len())
                && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                && id.bytes().all(|b| b.is_ascii_alphanumeric())
        });
    let glyph = value.chars().count() <= 16
        && value.chars().all(|c| {
            !c.is_whitespace() && !c.is_control() && !c.is_ascii_alphabetic() && !"<>:@#*_`~|[]()\\".contains(c)
        });
    if token || glyph { Ok(value.to_string()) } else { Err(Error::invalid("an answer's emoji must be one emoji")) }
}

/// The bytes a poll adds to its message, counted in the server's storage.
pub(super) fn bytes(poll: &pb::Poll) -> i64 {
    (poll.question.len() + poll.answers.iter().map(|a| a.text.len() + a.emoji.len()).sum::<usize>()) as i64
}

/// The text AutoMod reads for a poll: the question, then each answer on a line.
pub(super) fn words(poll: &pb::Poll) -> String {
    let mut words = poll.question.clone();
    for answer in &poll.answers {
        words.push('\n');
        words.push_str(&answer.text);
    }
    words
}

/// Whether a poll is closed at `now`: ended early, or past its time.
fn closed(poll: &pb::Poll, now: i64) -> bool {
    poll.ended_at.is_some() || poll.ends_at.as_ref().is_some_and(|t| crate::id::millis(t) <= now)
}

/// What anyone is shown of a poll at `now`: an anonymous poll that's still
/// running shows how many people voted, never how many picked each answer.
fn shown(mut poll: pb::Poll, now: i64) -> pb::Poll {
    if poll.anonymous && !closed(&poll, now) {
        for answer in &mut poll.answers {
            answer.votes = 0;
        }
    }
    poll
}

/// How `account_id` is written in a poll's votes: as itself in public polls,
/// keyed by the poll's own secret in anonymous ones.
fn voter(key: Option<&[u8]>, account_id: &str) -> String {
    match key {
        Some(key) => {
            let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes keys of any length");
            mac.update(account_id.as_bytes());
            mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
        }
        None => account_id.to_string(),
    }
}

/// Stores a new message's poll, inside the write that stores the message.
pub(super) async fn insert(conn: &turso::Connection, message: &pb::Message) -> Result<()> {
    let Some(poll) = &message.poll else { return Ok(()) };
    let answers = Answers { answers: poll.answers.iter().map(|a| pb::PollAnswer { votes: 0, ..a.clone() }).collect() };
    let tally = Tally { votes: vec![0; poll.answers.len()], voters: 0 };
    let voter_key = poll.anonymous.then(|| {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).expect("the OS random number generator failed");
        key.to_vec()
    });
    conn.execute(
        "INSERT INTO polls (message_id, channel_id, question, answers, multiple, anonymous, ends_at, tally, voter_key)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        (
            message.id.as_str(),
            message.channel_id.as_str(),
            poll.question.as_str(),
            answers.encode_to_vec(),
            poll.multiple as i64,
            poll.anonymous as i64,
            poll.ends_at.as_ref().map(crate::id::millis),
            tally.encode_to_vec(),
            voter_key,
        ),
    )
    .await?;
    Ok(())
}

/// A stored poll, its message's channel and author, the counts it has, and
/// its voter key while it runs anonymously.
struct Row {
    poll: pb::Poll,
    channel_id: String,
    author_id: String,
    tally: Tally,
    voter_key: Option<Vec<u8>>,
}

impl Row {
    fn voter(&self, account_id: &str) -> String {
        voter(self.voter_key.as_deref(), account_id)
    }
}

const POLL_COLUMNS: &str = "p.message_id, p.channel_id, p.question, p.answers, p.multiple, p.anonymous, p.ends_at, \
                            p.ended_at, p.ended_by_id, p.tally, m.author_id, p.voter_key";

/// A poll's row as read, before its answers and tally are decoded.
struct Stored {
    message_id: String,
    channel_id: String,
    author_id: String,
    poll: pb::Poll,
    answers: Vec<u8>,
    tally: Option<Vec<u8>>,
    voter_key: Option<Vec<u8>>,
}

fn poll_row(r: &turso::Row) -> turso::Result<Stored> {
    Ok(Stored {
        message_id: r.get(0)?,
        channel_id: r.get(1)?,
        author_id: r.get(10)?,
        poll: pb::Poll {
            question: r.get(2)?,
            multiple: r.get::<i64>(4)? != 0,
            anonymous: r.get::<i64>(5)? != 0,
            ends_at: r.get::<Option<i64>>(6)?.map(timestamp),
            ended_at: r.get::<Option<i64>>(7)?.map(timestamp),
            ended_by_id: r.get::<Option<String>>(8)?.unwrap_or_default(),
            ..Default::default()
        },
        answers: r.get(3)?,
        tally: r.get(9)?,
        voter_key: r.get(11)?,
    })
}

fn assemble(Stored { channel_id, author_id, mut poll, answers, tally, voter_key, .. }: Stored) -> Result<Row> {
    poll.answers = Answers::decode(answers.as_slice())?.answers;
    let mut tally = tally.map(|t| Tally::decode(t.as_slice())).transpose()?.unwrap_or_default();
    tally.votes.resize(poll.answers.len(), 0);
    for (answer, votes) in poll.answers.iter_mut().zip(&tally.votes) {
        answer.votes = *votes;
    }
    poll.voters = tally.voters;
    Ok(Row { poll, channel_id, author_id, tally, voter_key })
}

async fn load(conn: &turso::Connection, message_id: &str) -> Result<Option<Row>> {
    query_one(
        conn,
        &format!("SELECT {POLL_COLUMNS} FROM polls p JOIN messages m ON m.id = p.message_id WHERE p.message_id = ?1"),
        [message_id],
        poll_row,
    )
    .await?
    .map(assemble)
    .transpose()
}

/// Fills in the polls of the messages that have one (marked with an empty
/// `poll` when their extras were read), with the counts everyone may see.
pub(super) async fn attach(conn: &turso::Connection, messages: &mut [pb::Message]) -> Result<()> {
    let ids: Vec<&str> = messages.iter().filter(|m| m.poll.is_some()).map(|m| m.id.as_str()).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders = (1..=ids.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    let rows = query_all(
        conn,
        &format!("SELECT {POLL_COLUMNS} FROM polls p JOIN messages m ON m.id = p.message_id WHERE p.message_id IN ({placeholders})"),
        ids.iter().map(|id| turso::Value::from(*id)).collect::<Vec<_>>(),
        poll_row,
    )
    .await?;
    let now = now_ms();
    let mut found = HashMap::new();
    for row in rows {
        let id = row.message_id.clone();
        found.insert(id, shown(assemble(row)?.poll, now));
    }
    for message in messages.iter_mut().filter(|m| m.poll.is_some()) {
        message.poll = found.remove(&message.id);
    }
    Ok(())
}

/// The answers `account_id` picked in each of these messages' polls, put in
/// `my_answer_ids`. Only ever for the person asking.
pub(super) async fn mark_mine(conn: &turso::Connection, account_id: &str, messages: &mut [pb::Message]) -> Result<()> {
    let ids: Vec<&str> = messages.iter().filter(|m| m.poll.is_some()).map(|m| m.id.as_str()).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders = (1..=ids.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    let keys: HashMap<String, Option<Vec<u8>>> = query_all(
        conn,
        &format!("SELECT message_id, voter_key FROM polls WHERE message_id IN ({placeholders})"),
        ids.iter().map(|id| turso::Value::from(*id)).collect::<Vec<_>>(),
        |r| Ok((r.get::<String>(0)?, r.get::<Option<Vec<u8>>>(1)?)),
    )
    .await?
    .into_iter()
    .collect();
    for message in messages.iter_mut() {
        let Some(key) = keys.get(&message.id) else { continue };
        if let Some(poll) = &mut message.poll {
            poll.my_answer_ids = mine(conn, &message.id, &voter(key.as_deref(), account_id)).await?;
        }
    }
    Ok(())
}

async fn mine(conn: &turso::Connection, message_id: &str, voter: &str) -> Result<Vec<u32>> {
    query_all(
        conn,
        "SELECT answer_id FROM poll_votes WHERE message_id = ?1 AND voter = ?2 ORDER BY answer_id",
        (message_id, voter),
        |r| Ok(r.get::<i64>(0)? as u32),
    )
    .await
}

/// Drops a deleted message's poll and votes, inside the write that deletes it.
pub(super) async fn forget(conn: &turso::Connection, message_id: &str) -> Result<()> {
    conn.execute("DELETE FROM poll_votes WHERE message_id = ?1", [message_id]).await?;
    conn.execute("DELETE FROM polls WHERE message_id = ?1", [message_id]).await?;
    Ok(())
}

/// Drops the polls and votes of a channel that's being deleted.
pub(super) async fn forget_channel(conn: &turso::Connection, channel_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM poll_votes WHERE message_id IN (SELECT message_id FROM polls WHERE channel_id = ?1)",
        [channel_id],
    )
    .await?;
    conn.execute("DELETE FROM polls WHERE channel_id = ?1", [channel_id]).await?;
    Ok(())
}

/// Drops the polls and votes of a thread's replies before they're deleted.
pub(super) async fn forget_thread(conn: &turso::Connection, thread_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM poll_votes WHERE message_id IN (SELECT id FROM messages WHERE thread_id = ?1)",
        [thread_id],
    )
    .await?;
    conn.execute(
        "DELETE FROM polls WHERE message_id IN (SELECT id FROM messages WHERE thread_id = ?1)",
        [thread_id],
    )
    .await?;
    Ok(())
}

/// Whether polls can go in a channel: not one shared with other servers,
/// whose people couldn't vote on them.
pub(super) async fn shared_out(conn: &turso::Connection, channel_id: &str) -> Result<bool> {
    Ok(query_one(conn, "SELECT 1 FROM channel_guests WHERE channel_id = ?1 LIMIT 1", [channel_id], |_| Ok(()))
        .await?
        .is_some())
}

/// Whether a channel has polls still running, which keeps it from being
/// shared: the guests' people couldn't vote on them.
pub(super) async fn running_in(conn: &turso::Connection, channel_id: &str) -> Result<bool> {
    Ok(query_one(
        conn,
        "SELECT 1 FROM polls WHERE channel_id = ?1 AND ended_at IS NULL AND (ends_at IS NULL OR ends_at > ?2) LIMIT 1",
        (channel_id, now_ms()),
        |_| Ok(()),
    )
    .await?
    .is_some())
}

/// Votes per account per minute (the instance's `poll_votes_per_minute`,
/// unlimited unless set), kept in memory.
static PACE: Mutex<Option<HashMap<String, (i64, i64)>>> = Mutex::new(None);

fn pace(account_id: &str, now: i64, per_minute: Option<i64>) -> Result<()> {
    let Some(per_minute) = per_minute else { return Ok(()) };
    let mut guard = PACE.lock().unwrap_or_else(|e| e.into_inner());
    let counts = guard.get_or_insert_with(HashMap::new);
    let minute = now / 60_000;
    if counts.len() > 10_000 {
        counts.retain(|_, (at, _)| *at == minute);
    }
    let entry = counts.entry(account_id.to_string()).or_insert((minute, 0));
    if entry.0 != minute {
        *entry = (minute, 0);
    }
    if entry.1 >= per_minute {
        return Err(Error::ResourceExhausted("you're voting too fast; try again in a minute".into()));
    }
    entry.1 += 1;
    Ok(())
}

/// The answers someone asked for, checked against the poll: each once, all
/// real, only one unless it's multiple choice.
fn picked(poll: &pb::Poll, asked: &[u32]) -> Result<Vec<u32>> {
    let mut picked = asked.to_vec();
    picked.sort_unstable();
    picked.dedup();
    if picked.len() != asked.len() || picked.iter().any(|id| !poll.answers.iter().any(|a| a.id == *id)) {
        return Err(Error::invalid("pick answers from the poll, each once"));
    }
    if picked.len() > 1 && !poll.multiple {
        return Err(Error::invalid("this poll takes one answer"));
    }
    Ok(picked)
}

/// What everyone may see of a poll at `now`: the counts they may see, without
/// anyone's own vote.
fn shared_view(mut poll: pb::Poll, now: i64) -> pb::Poll {
    poll.my_answer_ids.clear();
    shown(poll, now)
}

/// Takes `picks` (the answers one voter had) off a poll's tally.
fn take_off(poll: &pb::Poll, tally: &mut Tally, picks: &[u32]) {
    for id in picks {
        if let Some(n) = poll.answers.iter().position(|a| a.id == *id) {
            tally.votes[n] = (tally.votes[n] - 1).max(0);
        }
    }
    if !picks.is_empty() {
        tally.voters = (tally.voters - 1).max(0);
    }
}

/// Puts a poll's counts from its tally on it.
fn count(poll: &mut pb::Poll, tally: &Tally) {
    for (answer, votes) in poll.answers.iter_mut().zip(&tally.votes) {
        answer.votes = *votes;
    }
    poll.voters = tally.voters;
}

/// Closes an anonymous poll's books, inside the write that closes it (or
/// finds it closed): its votes and key go, its counts stay.
async fn seal(conn: &turso::Connection, message_id: &str) -> Result<()> {
    conn.execute("DELETE FROM poll_votes WHERE message_id = ?1", [message_id]).await?;
    conn.execute("UPDATE polls SET voter_key = NULL WHERE message_id = ?1", [message_id]).await?;
    Ok(())
}

/// Seals the anonymous polls whose time ran out, a few at a time, and shows
/// everyone their counts. Runs every minute where servers are kept.
pub(crate) async fn close_due(sdb: &ServerDb, now: i64) -> Result<()> {
    let due = query_all(
        &sdb.read()?,
        "SELECT message_id FROM polls WHERE voter_key IS NOT NULL AND ends_at <= ?1 LIMIT 100",
        [now],
        |r| r.get::<String>(0),
    )
    .await?;
    for message_id in due {
        sdb.write("", async |conn, events| {
            let Some(row) = load(conn, &message_id).await? else { return Ok(()) };
            if row.voter_key.is_none() {
                return Ok(());
            }
            seal(conn, &message_id).await?;
            events.push(Payload::PollUpdated(pb::PollUpdated {
                channel_id: row.channel_id,
                message_id: message_id.clone(),
                poll: Some(shared_view(row.poll, now)),
                ..Default::default()
            }));
            Ok(())
        })
        .await?;
    }
    Ok(())
}

/// Takes a deleted account's votes off the polls that still keep them,
/// inside the write that forgets it there. Sealed anonymous polls keep only
/// counts, which stay as they are.
pub(crate) async fn forget_voter(conn: &turso::Connection, account_id: &str, events: &mut Vec<Payload>) -> Result<()> {
    let mut polls = query_all(
        conn,
        "SELECT DISTINCT v.message_id FROM poll_votes v JOIN polls p ON p.message_id = v.message_id
         WHERE p.voter_key IS NULL AND v.voter = ?1",
        [account_id],
        |r| r.get::<String>(0),
    )
    .await?;
    // Anonymous polls past their time, not yet sealed, show their counts:
    // taking a vote off one now would show what it was, so it stays.
    let now = now_ms();
    let keyed = query_all(
        conn,
        "SELECT message_id, voter_key FROM polls
         WHERE voter_key IS NOT NULL AND ended_at IS NULL AND (ends_at IS NULL OR ends_at > ?1)",
        [now],
        |r| Ok((r.get::<String>(0)?, r.get::<Vec<u8>>(1)?)),
    )
    .await?;
    for (message_id, key) in keyed {
        if !mine(conn, &message_id, &voter(Some(&key), account_id)).await?.is_empty() {
            polls.push(message_id);
        }
    }
    for message_id in polls {
        let Some(Row { mut poll, channel_id, mut tally, voter_key, .. }) = load(conn, &message_id).await? else {
            continue;
        };
        let voter = voter(voter_key.as_deref(), account_id);
        let picks = mine(conn, &message_id, &voter).await?;
        conn.execute(
            "DELETE FROM poll_votes WHERE message_id = ?1 AND voter = ?2",
            (message_id.as_str(), voter.as_str()),
        )
        .await?;
        take_off(&poll, &mut tally, &picks);
        conn.execute("UPDATE polls SET tally = ?2 WHERE message_id = ?1", (message_id.as_str(), tally.encode_to_vec()))
            .await?;
        count(&mut poll, &tally);
        events.push(Payload::PollUpdated(pb::PollUpdated {
            channel_id,
            message_id,
            poll: Some(shared_view(poll, now)),
            ..Default::default()
        }));
    }
    Ok(())
}

impl Api {
    pub(super) async fn vote(&self, request: Request<pb::VotePollRequest>) -> Result<pb::VotePollResponse> {
        let account = self.account(request.metadata()).await?;
        let req = request.into_inner();
        let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
        access.require_not_timed_out()?;
        if access.pending {
            return Err(Error::FailedPrecondition("agree to the server's rules first".into()));
        }
        // Who's named on the event is decided before the write: anonymous
        // polls never say, and a poll's anonymity never changes. A poll's
        // answers don't change either, so the vote is checked here too,
        // before it counts against the voter's pace.
        let before_write = load(&sdb.read()?, &req.message_id)
            .await?
            .filter(|row| access.can_see(&row.channel_id))
            .ok_or(Error::NotFound("poll"))?;
        if closed(&before_write.poll, now_ms()) {
            return Err(Error::FailedPrecondition("this poll has ended".into()));
        }
        picked(&before_write.poll, &req.answer_ids)?;
        pace(&account.id, now_ms(), self.app.settings().limits.poll_votes_per_minute)?;
        let actor = if before_write.poll.anonymous { "" } else { account.id.as_str() };
        let poll = sdb
            .write(actor, async |conn, events| {
                let row = load(conn, &req.message_id)
                    .await?
                    .filter(|row| access.can_see(&row.channel_id))
                    .ok_or(Error::NotFound("poll"))?;
                let now = now_ms();
                if closed(&row.poll, now) {
                    return Err(Error::FailedPrecondition("this poll has ended".into()));
                }
                if shared_out(conn, &row.channel_id).await? {
                    return Err(Error::invalid("polls can't be voted on in channels shared with other servers"));
                }
                let voter = row.voter(&account.id);
                let Row { mut poll, channel_id, mut tally, .. } = row;
                let picked = picked(&poll, &req.answer_ids)?;
                let before = mine(conn, &req.message_id, &voter).await?;
                if before == picked {
                    let mut poll = shown(poll, now);
                    poll.my_answer_ids = picked;
                    return Ok(poll);
                }
                conn.execute(
                    "DELETE FROM poll_votes WHERE message_id = ?1 AND voter = ?2",
                    (req.message_id.as_str(), voter.as_str()),
                )
                .await?;
                for answer_id in &picked {
                    conn.execute(
                        "INSERT INTO poll_votes (message_id, voter, answer_id) VALUES (?1, ?2, ?3)",
                        (req.message_id.as_str(), voter.as_str(), i64::from(*answer_id)),
                    )
                    .await?;
                }
                take_off(&poll, &mut tally, &before);
                for id in &picked {
                    if let Some(n) = poll.answers.iter().position(|a| a.id == *id) {
                        tally.votes[n] += 1;
                    }
                }
                tally.voters += i64::from(!picked.is_empty());
                // The one row every vote changes: votes at once clash here
                // and run again, so the counts in events follow each other.
                conn.execute(
                    "UPDATE polls SET tally = ?2 WHERE message_id = ?1",
                    (req.message_id.as_str(), tally.encode_to_vec()),
                )
                .await?;
                count(&mut poll, &tally);
                let (voter_id, voter_answer_ids) =
                    if poll.anonymous { (String::new(), vec![]) } else { (account.id.clone(), picked.clone()) };
                events.push(Payload::PollUpdated(pb::PollUpdated {
                    channel_id,
                    message_id: req.message_id.clone(),
                    poll: Some(shared_view(poll.clone(), now)),
                    voter_id,
                    voter_answer_ids,
                }));
                let mut poll = shown(poll, now);
                poll.my_answer_ids = picked;
                Ok(poll)
            })
            .await?;
        Ok(pb::VotePollResponse { poll: Some(poll) })
    }

    pub(super) async fn end(&self, request: Request<pb::EndPollRequest>) -> Result<pb::EndPollResponse> {
        let account = self.account(request.metadata()).await?;
        let req = request.into_inner();
        let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
        access.require_not_timed_out()?;
        let poll = sdb
            .write(&account.id, async |conn, events| {
                let row = load(conn, &req.message_id)
                    .await?
                    .filter(|row| access.can_see(&row.channel_id))
                    .ok_or(Error::NotFound("poll"))?;
                if row.author_id != account.id && !access.has_in(&row.channel_id, Permission::ManageMessages) {
                    return Err(Error::denied("only its creator or a moderator can end a poll"));
                }
                let now = now_ms();
                if closed(&row.poll, now) {
                    return Err(Error::FailedPrecondition("this poll has already ended".into()));
                }
                // Read before an anonymous poll's votes are sealed away, so
                // the one ending it still sees their own pick this time.
                let my_answer_ids = mine(conn, &req.message_id, &row.voter(&account.id)).await?;
                let Row { mut poll, channel_id, author_id, voter_key, .. } = row;
                conn.execute(
                    "UPDATE polls SET ended_at = ?2, ended_by_id = ?3 WHERE message_id = ?1",
                    (req.message_id.as_str(), now, account.id.as_str()),
                )
                .await?;
                if voter_key.is_some() {
                    seal(conn, &req.message_id).await?;
                }
                poll.ended_at = Some(timestamp(now));
                poll.ended_by_id = account.id.clone();
                if author_id != account.id {
                    let channel = load_channel(conn, &sdb.id, &channel_id).await?.map(|c| c.name).unwrap_or_default();
                    store::audit(conn, &account.id, Audit::new(pb::AuditAction::PollEnd, &author_id).channel(channel))
                        .await?;
                }
                events.push(Payload::PollUpdated(pb::PollUpdated {
                    channel_id,
                    message_id: req.message_id.clone(),
                    poll: Some(shared_view(poll.clone(), now)),
                    ..Default::default()
                }));
                poll.my_answer_ids = my_answer_ids;
                Ok(poll)
            })
            .await?;
        Ok(pb::EndPollResponse { poll: Some(poll) })
    }

    pub(super) async fn poll_voters(
        &self,
        request: Request<pb::ListPollVotersRequest>,
    ) -> Result<pb::ListPollVotersResponse> {
        let account = self.account(request.metadata()).await?;
        let req = request.into_inner();
        let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
        let conn = sdb.read()?;
        let row = load(&conn, &req.message_id)
            .await?
            .filter(|row| access.can_see(&row.channel_id))
            .ok_or(Error::NotFound("poll"))?;
        if row.poll.anonymous {
            return Err(Error::denied("this poll's votes are anonymous"));
        }
        if !row.poll.answers.iter().any(|a| a.id == req.answer_id) {
            return Err(Error::NotFound("answer"));
        }
        let limit = if req.limit <= 0 { 50 } else { req.limit.min(100) } as i64;
        let ids = query_all(
            &conn,
            "SELECT voter FROM poll_votes WHERE message_id = ?1 AND answer_id = ?2 AND voter > ?3
             ORDER BY voter LIMIT ?4",
            (req.message_id.as_str(), i64::from(req.answer_id), req.after_id.as_str(), limit + 1),
            |r| r.get::<String>(0),
        )
        .await?;
        let has_more = ids.len() as i64 > limit;
        let ids: Vec<&str> = ids.iter().take(limit as usize).map(String::as_str).collect();
        let mut found = users(&conn, &ids).await?;
        found.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(pb::ListPollVotersResponse { users: found, has_more })
    }
}

/// The gRPC entry points, called from `MessageService`'s implementation.
pub(super) async fn vote_poll(
    api: &Api,
    request: Request<pb::VotePollRequest>,
) -> Result<Response<pb::VotePollResponse>, Status> {
    respond(api.vote(request).await)
}

pub(super) async fn end_poll(
    api: &Api,
    request: Request<pb::EndPollRequest>,
) -> Result<Response<pb::EndPollResponse>, Status> {
    respond(api.end(request).await)
}

pub(super) async fn list_poll_voters(
    api: &Api,
    request: Request<pb::ListPollVotersRequest>,
) -> Result<Response<pb::ListPollVotersResponse>, Status> {
    respond(api.poll_voters(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_poll(answers: &[&str]) -> pb::NewPoll {
        pb::NewPoll {
            question: "Pizza or ramen?".into(),
            answers: answers.iter().map(|a| pb::NewPollAnswer { text: (*a).into(), emoji: String::new() }).collect(),
            duration_hours: 24,
            ..Default::default()
        }
    }

    #[test]
    fn checks_polls() {
        let poll = check(&new_poll(&["Pizza", " Ramen "]), 0).unwrap();
        assert_eq!(
            poll.answers.iter().map(|a| (a.id, a.text.as_str())).collect::<Vec<_>>(),
            [(1, "Pizza"), (2, "Ramen")]
        );
        assert_eq!(poll.ends_at.as_ref().map(crate::id::millis), Some(24 * 3_600_000));
        assert!(check(&new_poll(&["Only one"]), 0).is_err());
        assert!(check(&new_poll(&["a"; 11]), 0).is_err());
        assert!(check(&new_poll(&["a", ""]), 0).is_err());
        assert!(check(&pb::NewPoll { duration_hours: 337, ..new_poll(&["a", "b"]) }, 0).is_err());
        assert!(check(&pb::NewPoll { duration_hours: 0, ..new_poll(&["a", "b"]) }, 0).unwrap().ends_at.is_none());
    }

    #[test]
    fn checks_emoji() {
        for ok in ["", "🍕", "1️⃣", "👩‍👩‍👧", "<:wave:01ABC>", "<a:dance:XYZ9>"] {
            assert!(emoji(ok).is_ok(), "{ok}");
        }
        for bad in
            ["pizza", "🍕 🍕", "<:no spaces:1>", "<:x:>", "@everyone", "[x](y)", "🍕🍕🍕🍕🍕🍕🍕🍕🍕🍕🍕🍕🍕🍕🍕🍕🍕"]
        {
            assert!(emoji(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn picks_answers() {
        let mut poll = check(&new_poll(&["a", "b", "c"]), 0).unwrap();
        assert_eq!(picked(&poll, &[2]).unwrap(), [2]);
        assert!(picked(&poll, &[]).unwrap().is_empty());
        assert!(picked(&poll, &[1, 2]).is_err());
        assert!(picked(&poll, &[4]).is_err());
        poll.multiple = true;
        assert_eq!(picked(&poll, &[3, 1]).unwrap(), [1, 3]);
        assert!(picked(&poll, &[1, 1]).is_err());
    }

    #[test]
    fn paces_votes() {
        let now = 7 * 60_000;
        for _ in 0..3 {
            pace("pacer", now, Some(3)).unwrap();
        }
        assert!(pace("pacer", now, Some(3)).is_err());
        assert!(pace("pacer", now, None).is_ok());
        assert!(pace("someone else", now, Some(3)).is_ok());
        assert!(pace("pacer", now + 60_000, Some(3)).is_ok());
    }

    #[test]
    fn hides_anonymous_counts_until_the_end() {
        let mut poll = check(&pb::NewPoll { anonymous: true, ..new_poll(&["a", "b"]) }, 0).unwrap();
        poll.answers[0].votes = 2;
        poll.voters = 2;
        let running = shown(poll.clone(), 1);
        assert_eq!((running.answers[0].votes, running.voters), (0, 2));
        assert_eq!(shown(poll.clone(), 24 * 3_600_000).answers[0].votes, 2);
        poll.anonymous = false;
        assert_eq!(shown(poll, 1).answers[0].votes, 2);
    }

    #[test]
    fn keys_anonymous_voters() {
        let key = [7u8; 32];
        let keyed = voter(Some(&key), "01ACCOUNT");
        assert_eq!(keyed.len(), 64);
        assert!(!keyed.contains("01ACCOUNT"));
        assert_eq!(keyed, voter(Some(&key), "01ACCOUNT"));
        assert_ne!(keyed, voter(Some(&[8u8; 32]), "01ACCOUNT"));
        assert_eq!(voter(None, "01ACCOUNT"), "01ACCOUNT");
    }
}
