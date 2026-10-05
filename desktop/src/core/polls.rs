//! Polls in a server's channels, as the web app's `Poll.tsx` and
//! `PollEditor.tsx`: making one, voting, ending it early and seeing who
//! voted. Not in private conversations, secure channels or shared channels.

use crate::core::api::Problem;
use crate::core::store::{InstanceState, upsert_message};
use crate::core::{Core, reports};
use crate::pb;
use crate::rpc;

/// The longest question.
pub const QUESTION: usize = 300;
/// The longest answer.
pub const ANSWER: usize = 55;
/// The fewest and most answers.
pub const MIN_ANSWERS: usize = 2;
pub const MAX_ANSWERS: usize = 10;
/// How long a poll can run, as the editor offers it: 0 runs until it's ended.
pub const DURATIONS: [(&str, i32); 7] =
    [("1 hour", 1), ("4 hours", 4), ("1 day", 24), ("3 days", 72), ("1 week", 168), ("2 weeks", 336), ("No end", 0)];
/// Voters come a page at a time.
pub const VOTERS_PAGE: i32 = 50;

/// What the poll editor makes: a question, its answers and how it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub question: String,
    /// Each answer's text and emoji (empty for none).
    pub answers: Vec<(String, String)>,
    pub multiple: bool,
    pub anonymous: bool,
    /// 1 to 336, or 0 to run until someone ends it.
    pub hours: i32,
}

impl Draft {
    /// The answers that count: those with something written.
    pub fn filled(&self) -> usize {
        self.answers.iter().filter(|(text, _)| !text.trim().is_empty()).count()
    }

    /// Whether it can go out as it is.
    pub fn ready(&self) -> bool {
        let question = self.question.trim().chars().count();
        (1..=QUESTION).contains(&question)
            && self.filled() >= MIN_ANSWERS
            && self.answers.len() <= MAX_ANSWERS
            && self.answers.iter().all(|(text, _)| text.trim().chars().count() <= ANSWER)
    }

    fn poll(&self) -> pb::NewPoll {
        pb::NewPoll {
            question: self.question.trim().into(),
            answers: self
                .answers
                .iter()
                .filter(|(text, _)| !text.trim().is_empty())
                .map(|(text, emoji)| pb::NewPollAnswer { text: text.trim().into(), emoji: emoji.clone() })
                .collect(),
            multiple: self.multiple,
            anonymous: self.anonymous,
            duration_hours: self.hours,
        }
    }
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            question: String::new(),
            answers: vec![Default::default(), Default::default()],
            multiple: false,
            anonymous: false,
            hours: 24,
        }
    }
}

/// Puts a poll's new standing on its message. `mine` is your own vote when
/// the answer says it; otherwise the one already known stays.
pub fn with_poll(i: &mut InstanceState, channel_id: &str, message_id: &str, poll: &pb::Poll, mine: Option<&[u32]>) {
    let Some(message) =
        i.messages.get_mut(channel_id).and_then(|loaded| loaded.items.iter_mut().find(|m| m.id == message_id))
    else {
        return;
    };
    let my_answer_ids = match mine {
        Some(ids) => ids.to_vec(),
        None => message.poll.as_ref().map(|p| p.my_answer_ids.clone()).unwrap_or_default(),
    };
    message.poll = Some(pb::Poll { my_answer_ids, ..poll.clone() });
}

/// Whether a poll is over at `now_ms`: ended early or past its time.
pub fn closed(poll: &pb::Poll, now_ms: i64) -> bool {
    poll.ended_at.is_some() || poll.ends_at.as_ref().is_some_and(|t| ms(t) <= now_ms)
}

/// When a poll ends by itself, in milliseconds (0 when it runs until it's ended).
pub fn ends_at_ms(poll: &pb::Poll) -> i64 {
    poll.ends_at.as_ref().map_or(0, ms)
}

fn ms(t: &prost_types::Timestamp) -> i64 {
    t.seconds * 1000 + i64::from(t.nanos) / 1_000_000
}

/// "47 minutes left", "5 hours left", "3 days left".
pub fn left(ms: i64) -> String {
    let minutes = ((ms + 59_999) / 60_000).max(1);
    if minutes < 60 {
        return format!("{minutes} {} left", if minutes == 1 { "minute" } else { "minutes" });
    }
    let hours = (minutes as f64 / 60.0).round() as i64;
    if hours < 48 {
        return format!("{hours} {} left", if hours == 1 { "hour" } else { "hours" });
    }
    format!("{} days left", (hours as f64 / 24.0).round() as i64)
}

/// The answers you'd hold after clicking one: one-answer polls swap (or take
/// it back on a second click), others add or remove it, kept in order.
pub fn after_pick(poll: &pb::Poll, chosen: &[u32], answer: u32) -> Vec<u32> {
    if poll.multiple {
        let mut ids: Vec<u32> = chosen.iter().copied().filter(|id| *id != answer).collect();
        if !chosen.contains(&answer) {
            ids.push(answer);
            ids.sort_unstable();
        }
        ids
    } else if chosen == [answer] {
        Vec::new()
    } else {
        vec![answer]
    }
}

impl Core {
    /// Sends a poll as a message of its own.
    pub async fn send_poll(&self, key: &str, server_id: &str, channel_id: &str, draft: &Draft) -> Result<(), Problem> {
        let Some(api) = self.api(key) else { return Ok(()) };
        reports::used("poll.create");
        let sent = rpc!(
            api.messages(),
            send_message(pb::SendMessageRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                poll: Some(draft.poll()),
                ..Default::default()
            })
        )
        .await?;
        if let Some(message) = sent.message {
            self.shared.instance(key, |i| {
                if let Some(loaded) = i.messages.get_mut(channel_id) {
                    upsert_message(&mut loaded.items, message);
                }
            });
        }
        Ok(())
    }

    /// Votes in a poll, replacing your vote; no answers takes it back.
    pub async fn vote_poll(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        message_id: &str,
        answer_ids: Vec<u32>,
    ) -> Result<(), Problem> {
        let Some(api) = self.api(key) else { return Ok(()) };
        reports::used(if answer_ids.is_empty() { "poll.unvote" } else { "poll.vote" });
        let res = rpc!(
            api.messages(),
            vote_poll(pb::VotePollRequest { server_id: server_id.into(), message_id: message_id.into(), answer_ids })
        )
        .await?;
        if let Some(poll) = res.poll {
            self.shared.instance(key, |i| with_poll(i, channel_id, message_id, &poll, Some(&poll.my_answer_ids)));
        }
        Ok(())
    }

    /// Ends a poll before its time: its creator, or a moderator.
    pub async fn end_poll(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        message_id: &str,
    ) -> Result<(), Problem> {
        let Some(api) = self.api(key) else { return Ok(()) };
        reports::used("poll.end");
        let res = rpc!(
            api.messages(),
            end_poll(pb::EndPollRequest { server_id: server_id.into(), message_id: message_id.into() })
        )
        .await?;
        if let Some(poll) = res.poll {
            self.shared.instance(key, |i| with_poll(i, channel_id, message_id, &poll, Some(&poll.my_answer_ids)));
        }
        Ok(())
    }

    /// Who voted for one answer of a public poll, a page at a time: the
    /// people, and whether there are more after them.
    pub async fn poll_voters(
        &self,
        key: &str,
        server_id: &str,
        message_id: &str,
        answer_id: u32,
        after_id: &str,
    ) -> Result<(Vec<pb::User>, bool), Problem> {
        let Some(api) = self.api(key) else { return Ok((Vec::new(), false)) };
        let res = rpc!(
            api.messages(),
            list_poll_voters(pb::ListPollVotersRequest {
                server_id: server_id.into(),
                message_id: message_id.into(),
                answer_id,
                limit: VOTERS_PAGE,
                after_id: after_id.into(),
            })
        )
        .await?;
        Ok((res.users, res.has_more))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::store::ChannelMessages;

    fn poll(multiple: bool) -> pb::Poll {
        pb::Poll { question: "Snacks?".into(), multiple, ..Default::default() }
    }

    #[test]
    fn picking_swaps_toggles_and_takes_back() {
        let one = poll(false);
        assert_eq!(after_pick(&one, &[], 2), vec![2]);
        assert_eq!(after_pick(&one, &[2], 3), vec![3]);
        assert_eq!(after_pick(&one, &[2], 2), Vec::<u32>::new());
        let many = poll(true);
        assert_eq!(after_pick(&many, &[3], 1), vec![1, 3]);
        assert_eq!(after_pick(&many, &[1, 3], 3), vec![1]);
    }

    #[test]
    fn time_left_reads_like_the_web() {
        assert_eq!(left(30_000), "1 minute left");
        assert_eq!(left(47 * 60_000), "47 minutes left");
        assert_eq!(left(5 * 3_600_000), "5 hours left");
        assert_eq!(left(72 * 3_600_000), "3 days left");
    }

    #[test]
    fn a_draft_needs_a_question_and_two_answers() {
        let mut d = Draft::default();
        assert!(!d.ready());
        d.question = "  Snacks?  ".into();
        d.answers[0].0 = "Chips".into();
        assert!(!d.ready());
        d.answers[1].0 = "Fruit".into();
        d.answers.push(("   ".into(), "🍰".into()));
        assert!(d.ready());
        let p = d.poll();
        assert_eq!(p.question, "Snacks?");
        assert_eq!(p.answers.len(), 2);
        d.answers[0].0 = "x".repeat(ANSWER + 1);
        assert!(!d.ready());
    }

    #[test]
    fn an_event_keeps_your_vote_unless_it_is_yours() {
        let mut i = InstanceState::new("k", "https://k");
        let mut loaded = ChannelMessages::default();
        loaded.items.push(pb::Message {
            id: "m".into(),
            poll: Some(pb::Poll { my_answer_ids: vec![2], ..poll(false) }),
            ..Default::default()
        });
        i.messages.insert("c".into(), loaded);
        let counted = pb::Poll { voters: 3, ..poll(false) };
        with_poll(&mut i, "c", "m", &counted, None);
        let now = i.messages["c"].items[0].poll.clone().unwrap();
        assert_eq!((now.voters, now.my_answer_ids), (3, vec![2]));
        with_poll(&mut i, "c", "m", &counted, Some(&[]));
        assert!(i.messages["c"].items[0].poll.as_ref().unwrap().my_answer_ids.is_empty());
    }
}
