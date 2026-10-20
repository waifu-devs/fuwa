//! Ephemeral messages (docs/ephemeral-messages.md): a message an agent sends
//! to one person in a channel, that only they see, for a while. Nothing is
//! stored: it goes out once to the Hub as `EphemeralMessageCreated` with
//! sequence 0, and `events::shown_to` passes it only to its person, on every
//! live connection whatever its focus. Apps keep it until it runs out.

use std::collections::HashMap;
use std::sync::Mutex;

use super::messages::{check_content, check_embed_links, check_extras, check_not_timed_out, mentioned_users};
use super::{Api, Seat, commands, shared};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::node::Account;
use crate::pb::{self, Permission};
use crate::servers::{Payload, load_channel};

/// How long apps show one when the agent doesn't say.
pub const DEFAULT_SECONDS: u32 = 15 * 60;
/// The longest an agent may ask for.
pub const MAX_SECONDS: u32 = 60 * 60;
/// Ephemeral messages one agent sends a minute, across its servers.
pub const PER_MINUTE: i64 = 60;

/// When each agent's minute started and how many it sent in it, in memory.
static PACE: Mutex<Option<HashMap<String, (i64, i64)>>> = Mutex::new(None);

fn pace(agent_id: &str, now: i64) -> Result<()> {
    let mut guard = PACE.lock().unwrap_or_else(|e| e.into_inner());
    pace_in(guard.get_or_insert_with(HashMap::new), agent_id, now)
}

fn pace_in(counts: &mut HashMap<String, (i64, i64)>, agent_id: &str, now: i64) -> Result<()> {
    let minute = now / 60_000;
    if counts.len() > 10_000 {
        counts.retain(|_, (at, _)| *at == minute);
    }
    let entry = counts.entry(agent_id.to_string()).or_insert((minute, 0));
    if entry.0 != minute {
        *entry = (minute, 0);
    }
    if entry.1 >= PER_MINUTE {
        let left = (minute + 1) * 60_000 - now;
        return Err(Error::Limited("too many ephemeral messages; try again in a minute".into(), left));
    }
    entry.1 += 1;
    Ok(())
}

/// How long the message shows, in milliseconds, from what was asked.
fn lifetime_ms(asked: u32) -> Result<i64> {
    match asked {
        0 => Ok(i64::from(DEFAULT_SECONDS) * 1000),
        s if s <= MAX_SECONDS => Ok(i64::from(s) * 1000),
        _ => Err(Error::invalid(format!("expires_in_seconds is at most {MAX_SECONDS}"))),
    }
}

impl Api {
    pub(super) async fn send_ephemeral_impl(
        &self,
        account: &Account,
        mut req: pb::SendEphemeralMessageRequest,
    ) -> Result<pb::SendEphemeralMessageResponse> {
        if account.kind != pb::AccountKind::Agent {
            return Err(Error::denied("only agents send ephemeral messages"));
        }
        let lifetime = lifetime_ms(req.expires_in_seconds)?;
        check_content(&req.content, !req.embeds.is_empty())?;
        check_extras(&mut [], &req.embeds)?;
        check_embed_links(&self.app, &mut req.embeds)?;
        let answering = !req.interaction_id.is_empty();
        if req.user_id.is_empty() && !answering {
            return Err(Error::invalid("say who sees it: user_id, or an interaction_id to answer"));
        }
        let Seat { sdb, member, access } = self.membership(account, &req.server_id).await?;
        check_not_timed_out(&member)?;
        access.require_in(&req.channel_id, Permission::SendMessages)?;
        if !req.embeds.is_empty() {
            access.require_in(&req.channel_id, Permission::EmbedLinks)?;
        }
        {
            let conn = sdb.read()?;
            let channel = load_channel(&conn, &sdb.id, &req.channel_id).await?.ok_or(Error::NotFound("channel"))?;
            if !matches!(
                pb::ChannelType::try_from(channel.r#type),
                Ok(pb::ChannelType::Text | pb::ChannelType::Announcement)
            ) {
                return Err(Error::invalid("ephemeral messages only go in text and announcement channels"));
            }
            if shared::link_of(&conn, &channel.id).await?.is_some()
                || super::polls::shared_out(&conn, &channel.id).await?
            {
                return Err(Error::invalid("ephemeral messages aren't in channels shared between servers yet"));
            }
        }
        let now = now_ms();
        // An answer counts toward the interaction's answers, and says who
        // sees it: whoever used the command or button.
        let interaction = if answering {
            let (agent_id, interaction_id, channel_id) =
                (account.id.clone(), req.interaction_id.clone(), req.channel_id.clone());
            Some(
                sdb.write(&account.id, async move |conn, _events| {
                    commands::answer(conn, &agent_id, &interaction_id, &channel_id, now).await
                })
                .await?,
            )
        } else {
            None
        };
        let user_id = match &interaction {
            Some(i) if req.user_id.is_empty() || req.user_id == i.user_id => i.user_id.clone(),
            Some(_) => return Err(Error::invalid("an answer goes to whoever used the command or button")),
            None => req.user_id.clone(),
        };
        // The person must be in the server and able to see the channel; who
        // isn't, or doesn't exist, looks the same.
        let (recipient, recipient_access) = sdb.member_access(&user_id).await?.ok_or(Error::NotFound("member"))?;
        if !recipient_access.can_see(&req.channel_id) {
            return Err(Error::NotFound("member"));
        }
        if recipient.user.as_ref().is_some_and(|u| u.kind == pb::AccountKind::Agent as i32) {
            return Err(Error::invalid("ephemeral messages are for people, not agents"));
        }
        pace(&account.id, now)?;
        let mention_user_ids = mentioned_users(&*sdb.read()?, &req.content).await?;
        let message = pb::Message {
            id: new_id(),
            server_id: sdb.id.clone(),
            channel_id: req.channel_id.clone(),
            author_id: account.id.clone(),
            content: req.content,
            embeds: req.embeds,
            created_at: Some(timestamp(now)),
            mention_user_ids,
            interaction,
            ephemeral: Some(pb::EphemeralInfo { user_id, expires_at: Some(timestamp(now + lifetime)) }),
            ..Default::default()
        };
        self.app.hub.publish([pb::Event {
            id: new_id(),
            server_id: sdb.id.clone(),
            sequence: 0,
            actor_id: account.id.clone(),
            created_at: Some(timestamp(now)),
            payload: Some(Payload::EphemeralMessageCreated(pb::EphemeralMessageCreated {
                message: Some(message.clone()),
            })),
        }]);
        Ok(pb::SendEphemeralMessageResponse { message: Some(message) })
    }
}

/// Whether `account_id` gets an ephemeral message: only the one it's for.
pub(super) fn shown_to(account_id: &str, created: &pb::EphemeralMessageCreated) -> bool {
    created.message.as_ref().and_then(|m| m.ephemeral.as_ref()).is_some_and(|e| e.user_id == account_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifetimes_default_and_cap() {
        assert_eq!(lifetime_ms(0).unwrap(), 900_000);
        assert_eq!(lifetime_ms(30).unwrap(), 30_000);
        assert_eq!(lifetime_ms(MAX_SECONDS).unwrap(), 3_600_000);
        assert!(lifetime_ms(MAX_SECONDS + 1).is_err());
    }

    #[test]
    fn paces_each_agent_by_the_minute() {
        let mut counts = HashMap::new();
        for _ in 0..PER_MINUTE {
            pace_in(&mut counts, "bot", 1_000).unwrap();
        }
        assert!(pace_in(&mut counts, "bot", 2_000).is_err());
        assert!(pace_in(&mut counts, "other", 2_000).is_ok());
        assert!(pace_in(&mut counts, "bot", 61_000).is_ok());
    }

    #[test]
    fn only_its_person_gets_it() {
        let created = pb::EphemeralMessageCreated {
            message: Some(pb::Message {
                ephemeral: Some(pb::EphemeralInfo { user_id: "ana".into(), expires_at: None }),
                ..Default::default()
            }),
        };
        assert!(shown_to("ana", &created));
        assert!(!shown_to("ben", &created));
        assert!(!shown_to("ana", &pb::EphemeralMessageCreated { message: None }));
    }
}
