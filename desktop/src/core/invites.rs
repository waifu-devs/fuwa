//! Inviting people, as the web app's `lib/invites.ts` and `InviteDialog.tsx`:
//! opening the dialog reuses a link of yours that's still good for a while,
//! otherwise makes one that lasts a week, and "Edit invite link" makes one
//! that lasts longer or lets in fewer people.

use crate::core::api::Problem;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::{Core, reports};
use crate::pb;
use crate::rpc;

/// How long a new invite lasts, in seconds (0 is forever), and its label's catalog key.
pub const EXPIRE_AFTER: [(i32, &str); 7] = [
    (1800, "workspace.invite.expire.minutes30"),
    (3600, "workspace.invite.expire.hour"),
    (6 * 3600, "workspace.invite.expire.hours6"),
    (12 * 3600, "workspace.invite.expire.hours12"),
    (86_400, "workspace.invite.expire.day"),
    (7 * 86_400, "workspace.invite.expire.days7"),
    (0, "workspace.invite.expire.never"),
];

/// How many people one invite lets in; 0 is anyone with the link.
pub const MAX_USES: [i32; 7] = [0, 1, 5, 10, 25, 50, 100];

/// What a fresh invite starts as, like Discord's: a week, for anyone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub max_age_seconds: i32,
    pub max_uses: i32,
}

impl Default for Options {
    fn default() -> Self {
        Self { max_age_seconds: 7 * 86_400, max_uses: 0 }
    }
}

/// When an invite stops working, in unix milliseconds, if it does.
pub fn expires_at(invite: &pb::Invite) -> Option<i64> {
    invite.expires_at.as_ref().map(|t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000)
}

/// Whether an invite still lets people in.
pub fn works(invite: &pb::Invite, now: i64) -> bool {
    (invite.max_uses == 0 || invite.uses < invite.max_uses) && expires_at(invite).is_none_or(|at| at > now)
}

/// An invite's address, on the instance itself.
pub fn link(base: &str, code: &str) -> String {
    format!("{}/invite/{code}", base.trim_end_matches('/'))
}

/// Time until something, in its largest unit, rounded: "45 minutes", "6 hours", "7 days".
pub fn time_left(ms: i64) -> String {
    let minutes = ((ms as f64) / 60_000.0).round().max(1.0) as i64;
    let (n, unit) = if minutes >= 1440 {
        ((minutes as f64 / 1440.0).round() as i64, "day")
    } else if minutes >= 60 {
        ((minutes as f64 / 60.0).round() as i64, "hour")
    } else {
        (minutes, "minute")
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// "Your invite link expires in 7 days.", with its limit when it has one.
pub fn terms(invite: &pb::Invite, now: i64) -> String {
    let limit = i64::from(invite.max_uses);
    match expires_at(invite) {
        None if limit > 0 => t_with("workspace.invite.neverUses", &[("count", Arg::Num(limit))]),
        None => t("workspace.invite.never"),
        Some(at) => {
            let time = time_left(at - now);
            if limit > 0 {
                t_with("workspace.invite.expiresInUses", &[("time", Arg::Str(&time)), ("count", Arg::Num(limit))])
            } else {
                t_with("workspace.invite.expiresIn", &[("time", Arg::Str(&time))])
            }
        }
    }
}

impl Core {
    /// The address links to an instance read with: its public URL, else the one it's reached at.
    pub fn public_base(&self, key: &str) -> String {
        let public = self.shared.read(|s| {
            s.instance(key).and_then(|i| i.node.as_ref()).map(|n| n.public_url.clone()).filter(|u| !u.is_empty())
        });
        public.or_else(|| self.api(key).map(|a| a.url.clone())).unwrap_or_default()
    }

    /// A link of yours to the server (or `channel`) that's still good for a
    /// day or more and lets anyone in, or a new one that lasts a week.
    pub async fn invite_for(&self, key: &str, server_id: &str, channel: &str) -> Result<pb::Invite, Problem> {
        let api = self.api(key).ok_or_else(|| Problem::new(tonic::Code::NotFound, "That instance isn't here."))?;
        let me = self.shared.read(|s| s.instance(key).and_then(|i| i.me.as_ref().map(|m| m.id.clone())));
        // Listing needs Manage Server; without it, a new link is still fine.
        if let Ok(res) = rpc!(api.invites(), list_invites(pb::ListInvitesRequest { server_id: server_id.into() })).await
        {
            let now = crate::core::dms::now_ms();
            let mine = res.invites.into_iter().find(|i| {
                Some(&i.inviter_id) == me.as_ref()
                    && i.channel_id == channel
                    && i.max_uses == 0
                    && works(i, now)
                    && expires_at(i).is_none_or(|at| at - now > 86_400_000)
            });
            if let Some(mine) = mine {
                return Ok(mine);
            }
        }
        self.make_invite(key, server_id, channel, Options::default()).await
    }

    /// Makes an invite with `options`.
    pub async fn make_invite(
        &self,
        key: &str,
        server_id: &str,
        channel: &str,
        options: Options,
    ) -> Result<pb::Invite, Problem> {
        let api = self.api(key).ok_or_else(|| Problem::new(tonic::Code::NotFound, "That instance isn't here."))?;
        let res = rpc!(
            api.invites(),
            create_invite(pb::CreateInviteRequest {
                server_id: server_id.into(),
                channel_id: channel.into(),
                max_uses: options.max_uses,
                max_age_seconds: options.max_age_seconds,
            })
        )
        .await?;
        reports::used("invite.create");
        res.invite.ok_or_else(|| Problem::new(tonic::Code::Internal, "The server didn't send the invite back."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invite(max_uses: i32, uses: i32, expires: Option<i64>) -> pb::Invite {
        pb::Invite {
            max_uses,
            uses,
            expires_at: expires.map(|ms| prost_types::Timestamp { seconds: ms / 1000, nanos: 0 }),
            ..Default::default()
        }
    }

    #[test]
    fn invites_stop_working_when_used_up_or_expired() {
        assert!(works(&invite(0, 9, None), 1_000));
        assert!(!works(&invite(2, 2, None), 1_000));
        assert!(works(&invite(0, 0, Some(5_000)), 1_000));
        assert!(!works(&invite(0, 0, Some(5_000)), 6_000));
    }

    #[test]
    fn time_left_reads_in_its_largest_unit() {
        assert_eq!(time_left(45 * 60_000), "45 minutes");
        assert_eq!(time_left(6 * 3_600_000 - 1000), "6 hours");
        assert_eq!(time_left(7 * 86_400_000), "7 days");
        assert_eq!(time_left(10), "1 minute");
    }

    #[test]
    fn links_read_like_the_web() {
        assert_eq!(link("https://fuwa.chat/", "abc"), "https://fuwa.chat/invite/abc");
    }
}
