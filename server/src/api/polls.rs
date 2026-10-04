//! Polls: a message with a question and answers people vote on. The poll
//! itself is a row in `polls` beside its message, with its tally; votes are
//! rows in `poll_votes`, one per account and answer. Votes of anonymous
//! polls are only ever read back for the voter themselves: their events
//! carry no actor, and ListPollVoters refuses them.

use std::collections::HashMap;
use std::sync::Mutex;

use prost::Message as _;
use tonic::{Request, Response, Status};

use super::{Api, Seat, respond, text, users};
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{now_ms, timestamp};
use crate::pb::{self, Permission};
use crate::servers::{self as store, Audit, Payload, load_channel};

pub const MIN_ANSWERS: usize = 2;
pub const MAX_ANSWERS: usize = 10;
const MAX_QUESTION: usize = 300;
const MAX_ANSWER: usize = 55;
/// Two weeks.
const MAX_HOURS: i32 = 336;
/// Votes (and changes and take-backs) one account may make in a minute,
/// across the polls of one server process.
const VOTES_A_MINUTE: u32 = 30;

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

/// Stores a new message's poll, inside the write that stores the message.
pub(super) async fn insert(conn: &turso::Connection, message: &pb::Message) -> Result<()> {
    let Some(poll) = &message.poll else { return Ok(()) };
    let answers = Answers { answers: poll.answers.iter().map(|a| pb::PollAnswer { votes: 0, ..a.clone() }).collect() };
    let tally = Tally { votes: vec![0; poll.answers.len()], voters: 0 };
    conn.execute(
        "INSERT INTO polls (message_id, channel_id, question, answers, multiple, anonymous, ends_at, tally)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        (
            message.id.as_str(),
            message.channel_id.as_str(),
            poll.question.as_str(),
            answers.encode_to_vec(),
            poll.multiple as i64,
            poll.anonymous as i64,
            poll.ends_at.as_ref().map(crate::id::millis),
            tally.encode_to_vec(),
        ),
    )
    .await?;
    Ok(())
}

/// A stored poll, its message's channel and author, and the counts it has.
struct Row {
    poll: pb::Poll,
    channel_id: String,
    author_id: String,
    tally: Tally,
}

const POLL_COLUMNS: &str = "p.message_id, p.channel_id, p.question, p.answers, p.multiple, p.anonymous, p.ends_at, \
                            p.ended_at, p.ended_by_id, p.tally, m.author_id";

/// A poll's row as read: message id, channel, author, the poll without its
/// answers, then its answers and tally as stored.
type Stored = (String, String, String, pb::Poll, Vec<u8>, Option<Vec<u8>>);

fn poll_row(r: &turso::Row) -> turso::Result<Stored> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(10)?,
        pb::Poll {
            question: r.get(2)?,
            multiple: r.get::<i64>(4)? != 0,
            anonymous: r.get::<i64>(5)? != 0,
            ends_at: r.get::<Option<i64>>(6)?.map(timestamp),
            ended_at: r.get::<Option<i64>>(7)?.map(timestamp),
            ended_by_id: r.get::<Option<String>>(8)?.unwrap_or_default(),
            ..Default::default()
        },
        r.get(3)?,
        r.get(9)?,
    ))
}

fn assemble((_, channel_id, author_id, mut poll, answers, tally): Stored) -> Result<Row> {
    poll.answers = Answers::decode(answers.as_slice())?.answers;
    let mut tally = tally.map(|t| Tally::decode(t.as_slice())).transpose()?.unwrap_or_default();
    tally.votes.resize(poll.answers.len(), 0);
    for (answer, votes) in poll.answers.iter_mut().zip(&tally.votes) {
        answer.votes = *votes;
    }
    poll.voters = tally.voters;
    Ok(Row { poll, channel_id, author_id, tally })
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
/// `poll` when their extras were read), with everyone's counts.
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
    let mut found = HashMap::new();
    for row in rows {
        let id = row.0.clone();
        found.insert(id, assemble(row)?.poll);
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
    let placeholders = (2..=ids.len() + 1).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    let mut params = vec![turso::Value::from(account_id)];
    params.extend(ids.iter().map(|id| turso::Value::from(*id)));
    let rows = query_all(
        conn,
        &format!(
            "SELECT message_id, answer_id FROM poll_votes WHERE account_id = ?1 AND message_id IN ({placeholders}) ORDER BY answer_id"
        ),
        params,
        |r| Ok((r.get::<String>(0)?, r.get::<i64>(1)? as u32)),
    )
    .await?;
    for message in messages.iter_mut() {
        if let Some(poll) = &mut message.poll {
            poll.my_answer_ids = rows.iter().filter(|(id, _)| *id == message.id).map(|(_, a)| *a).collect();
        }
    }
    Ok(())
}

async fn mine(conn: &turso::Connection, message_id: &str, account_id: &str) -> Result<Vec<u32>> {
    query_all(
        conn,
        "SELECT answer_id FROM poll_votes WHERE message_id = ?1 AND account_id = ?2 ORDER BY answer_id",
        (message_id, account_id),
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

/// Whether polls can go in a channel: not one shared with other servers,
/// whose people couldn't vote on them.
pub(super) async fn shared_out(conn: &turso::Connection, channel_id: &str) -> Result<bool> {
    Ok(query_one(conn, "SELECT 1 FROM channel_guests WHERE channel_id = ?1 LIMIT 1", [channel_id], |_| Ok(()))
        .await?
        .is_some())
}

/// Votes per account per minute, kept in memory: a page of votes is cheap,
/// but every one is an event kept in the server's log.
static PACE: Mutex<Option<HashMap<String, (i64, u32)>>> = Mutex::new(None);

fn pace(account_id: &str, now: i64) -> Result<()> {
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
    if entry.1 >= VOTES_A_MINUTE {
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

/// What everyone may see of a poll: its counts, without anyone's own vote.
fn shared_view(mut poll: pb::Poll) -> pb::Poll {
    poll.my_answer_ids.clear();
    poll
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
        // polls never say, and a poll's anonymity never changes.
        let anonymous = load(&sdb.read()?, &req.message_id)
            .await?
            .filter(|row| access.can_see(&row.channel_id))
            .ok_or(Error::NotFound("poll"))?
            .poll
            .anonymous;
        pace(&account.id, now_ms())?;
        let actor = if anonymous { "" } else { account.id.as_str() };
        let poll = sdb
            .write(actor, async |conn, events| {
                let Row { mut poll, channel_id, mut tally, .. } = load(conn, &req.message_id)
                    .await?
                    .filter(|row| access.can_see(&row.channel_id))
                    .ok_or(Error::NotFound("poll"))?;
                if closed(&poll, now_ms()) {
                    return Err(Error::FailedPrecondition("this poll has ended".into()));
                }
                let picked = picked(&poll, &req.answer_ids)?;
                let before = mine(conn, &req.message_id, &account.id).await?;
                if before == picked {
                    poll.my_answer_ids = picked;
                    return Ok(poll);
                }
                conn.execute(
                    "DELETE FROM poll_votes WHERE message_id = ?1 AND account_id = ?2",
                    (req.message_id.as_str(), account.id.as_str()),
                )
                .await?;
                let now = now_ms();
                for answer_id in &picked {
                    conn.execute(
                        "INSERT INTO poll_votes (message_id, account_id, answer_id, created_at) VALUES (?1, ?2, ?3, ?4)",
                        (req.message_id.as_str(), account.id.as_str(), i64::from(*answer_id), now),
                    )
                    .await?;
                }
                let place = |id: u32| poll.answers.iter().position(|a| a.id == id);
                for id in &before {
                    if let Some(n) = place(*id) {
                        tally.votes[n] = (tally.votes[n] - 1).max(0);
                    }
                }
                for id in &picked {
                    if let Some(n) = place(*id) {
                        tally.votes[n] += 1;
                    }
                }
                tally.voters = (tally.voters + i64::from(!picked.is_empty()) - i64::from(!before.is_empty())).max(0);
                // The one row every vote changes: votes at once clash here
                // and run again, so the counts in events follow each other.
                conn.execute(
                    "UPDATE polls SET tally = ?2 WHERE message_id = ?1",
                    (req.message_id.as_str(), tally.encode_to_vec()),
                )
                .await?;
                for (answer, votes) in poll.answers.iter_mut().zip(&tally.votes) {
                    answer.votes = *votes;
                }
                poll.voters = tally.voters;
                let (voter_id, voter_answer_ids) =
                    if poll.anonymous { (String::new(), vec![]) } else { (account.id.clone(), picked.clone()) };
                events.push(Payload::PollUpdated(pb::PollUpdated {
                    channel_id,
                    message_id: req.message_id.clone(),
                    poll: Some(shared_view(poll.clone())),
                    voter_id,
                    voter_answer_ids,
                }));
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
                let Row { mut poll, channel_id, author_id, .. } = load(conn, &req.message_id)
                    .await?
                    .filter(|row| access.can_see(&row.channel_id))
                    .ok_or(Error::NotFound("poll"))?;
                if author_id != account.id && !access.has_in(&channel_id, Permission::ManageMessages) {
                    return Err(Error::denied("only its creator or a moderator can end a poll"));
                }
                let now = now_ms();
                if closed(&poll, now) {
                    return Err(Error::FailedPrecondition("this poll has already ended".into()));
                }
                conn.execute(
                    "UPDATE polls SET ended_at = ?2, ended_by_id = ?3 WHERE message_id = ?1",
                    (req.message_id.as_str(), now, account.id.as_str()),
                )
                .await?;
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
                    poll: Some(shared_view(poll.clone())),
                    ..Default::default()
                }));
                poll.my_answer_ids = mine(conn, &req.message_id, &account.id).await?;
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
            "SELECT account_id FROM poll_votes WHERE message_id = ?1 AND answer_id = ?2 AND account_id > ?3
             ORDER BY account_id LIMIT ?4",
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
        for _ in 0..VOTES_A_MINUTE {
            pace("pacer", now).unwrap();
        }
        assert!(pace("pacer", now).is_err());
        assert!(pace("someone else", now).is_ok());
        assert!(pace("pacer", now + 60_000).is_ok());
    }
}
