//! Timing out, kicking and banning members, as the web app's
//! `ModerateDialog.tsx`. Each takes a reason, kept in the server's audit log.

use tonic::Code;

use crate::core::api::Problem;
use crate::core::permissions::Access;
use crate::core::store::InstanceState;
use crate::core::{Core, store};
use crate::pb::{self, Permission as P};
use crate::rpc;

/// The longest time-out a server allows.
pub const MAX_TIME_OUT: i64 = 28 * 86_400;

/// What to do to someone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Stop them talking for this many seconds; zero ends a time-out early.
    TimeOut(i64),
    Kick,
    /// Remove them for good, with their messages from this many seconds back.
    Ban(i64),
}

impl Action {
    pub fn permission(self) -> P {
        match self {
            Action::TimeOut(_) => P::TimeOutMembers,
            Action::Kick => P::KickMembers,
            Action::Ban(_) => P::BanMembers,
        }
    }
}

/// Whether one person ranks above another: the owner above everyone, others
/// by their highest roles.
pub fn outranks(a: &Access, other: &Access) -> bool {
    a.owner || (!other.owner && a.rank > other.rank)
}

impl InstanceState {
    /// Someone's standing in a server, for `outranks`.
    pub fn standing(&self, server_id: &str, user_id: &str) -> Access {
        let Some(server) = self.server(server_id) else { return Access::default() };
        let owner = server.owner_id == user_id;
        let member = self
            .members
            .get(server_id)
            .and_then(|l| l.iter().find(|m| m.user.as_ref().is_some_and(|u| u.id == user_id)));
        let rank = if owner {
            i64::MAX
        } else {
            let held = member.map(|m| m.role_ids.as_slice()).unwrap_or_default();
            self.roles
                .get(server_id)
                .into_iter()
                .flatten()
                .filter(|r| held.contains(&r.id))
                .map(|r| i64::from(r.position))
                .max()
                .unwrap_or(0)
                .max(0)
        };
        Access { owner, rank, ..Access::default() }
    }

    /// What you may do to someone in a server: each action you hold the
    /// permission for, when you rank above them. Never to yourself.
    pub fn can_moderate(&self, server_id: &str, user_id: &str) -> Vec<P> {
        if self.me.as_ref().is_none_or(|me| me.id == user_id) {
            return Vec::new();
        }
        let mine = self.access(server_id);
        if !outranks(&mine, &self.standing(server_id, user_id)) {
            return Vec::new();
        }
        [P::TimeOutMembers, P::KickMembers, P::BanMembers].into_iter().filter(|p| mine.has(*p)).collect()
    }
}

/// When someone's time-out ends, in ms since the epoch, while it lasts.
pub fn timed_out_until(member: &pb::Member, now_ms: i64) -> Option<i64> {
    let t = member.timed_out_until.as_ref()?;
    let ms = t.seconds * 1000 + i64::from(t.nanos) / 1_000_000;
    (ms > now_ms).then_some(ms)
}

impl Core {
    /// Times out, kicks or bans someone. For a ban, how many of their
    /// messages went with them.
    pub async fn moderate(
        &self,
        key: &str,
        server_id: &str,
        user_id: &str,
        action: Action,
        reason: &str,
    ) -> Result<i64, Problem> {
        let api = self.api(key).ok_or_else(|| Problem::new(Code::NotFound, "That instance isn't here."))?;
        let (server, user, reason) = (server_id.to_owned(), user_id.to_owned(), reason.trim().to_owned());
        match action {
            Action::TimeOut(seconds) => {
                let res = rpc!(
                    api.servers(),
                    time_out_member(pb::TimeOutMemberRequest {
                        server_id: server,
                        user_id: user,
                        seconds: seconds.clamp(0, MAX_TIME_OUT),
                        reason,
                    })
                )
                .await?;
                if let Some(member) = res.member {
                    self.shared.instance(key, |i| {
                        let list = i.members.entry(server_id.to_owned()).or_default();
                        list.retain(|m| m.user.as_ref().is_none_or(|u| u.id != user_id));
                        list.push(member);
                        store::sort_members(list);
                    });
                }
                Ok(0)
            }
            Action::Kick => {
                rpc!(api.servers(), kick_member(pb::KickMemberRequest { server_id: server, user_id: user, reason }))
                    .await?;
                self.forget_member(key, server_id, user_id);
                Ok(0)
            }
            Action::Ban(delete) => {
                let res = rpc!(
                    api.servers(),
                    ban_member(pb::BanMemberRequest {
                        server_id: server,
                        user_id: user,
                        reason,
                        delete_message_seconds: delete.clamp(0, 7 * 86_400),
                    })
                )
                .await?;
                self.forget_member(key, server_id, user_id);
                Ok(res.deleted_messages)
            }
        }
    }

    /// Takes someone off a server's member list now, before its event arrives.
    fn forget_member(&self, key: &str, server_id: &str, user_id: &str) {
        self.shared.instance(key, |i| {
            let Some(list) = i.members.get_mut(server_id) else { return };
            let before = list.len();
            list.retain(|m| m.user.as_ref().is_none_or(|u| u.id != user_id));
            if list.len() != before
                && let Some(server) = i.servers.iter_mut().find(|s| s.id == server_id)
            {
                server.member_count -= 1;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_outranks_everyone_and_ties_dont() {
        let owner = Access { owner: true, rank: i64::MAX, ..Access::default() };
        let mod_ = Access { rank: 5, ..Access::default() };
        let peer = Access { rank: 5, ..Access::default() };
        assert!(outranks(&owner, &mod_));
        assert!(!outranks(&mod_, &owner));
        assert!(!outranks(&mod_, &peer));
        assert!(outranks(&mod_, &Access::default()));
    }

    #[test]
    fn time_out_counts_only_while_it_lasts() {
        let mut m = pb::Member::default();
        assert_eq!(timed_out_until(&m, 1_000), None);
        m.timed_out_until = Some(prost_types::Timestamp { seconds: 10, nanos: 0 });
        assert_eq!(timed_out_until(&m, 1_000), Some(10_000));
        assert_eq!(timed_out_until(&m, 20_000), None);
    }
}
