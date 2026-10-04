//! Looking after an instance's people and its banner: every account, for
//! its admins to make admins, give new passwords or turn off, and the
//! announcement across the top of every app, as in the web app's
//! `settings/instance/Accounts.tsx` and `Announcement.tsx`.

use tonic::Code;

use crate::core::Core;
use crate::core::api::Problem;
use crate::pb;
use crate::rpc;

fn missing() -> Problem {
    Problem::new(Code::NotFound, "That instance isn't here.")
}

/// Accounts read at a time.
pub const PAGE: i32 = 50;
/// The longest reason for turning an account off.
pub const REASON_MAX: usize = 512;
/// The longest announcement.
pub const TEXT_MAX: usize = 300;

const HOUR: i64 = 3_600_000;
/// How long an announcement stays up: (label, milliseconds; None until taken down).
pub const LENGTHS: [(&str, Option<i64>); 5] = [
    ("Until taken down", None),
    ("1 hour", Some(HOUR)),
    ("4 hours", Some(4 * HOUR)),
    ("1 day", Some(24 * HOUR)),
    ("1 week", Some(7 * 24 * HOUR)),
];

fn ms(t: &prost_types::Timestamp) -> i64 {
    t.seconds * 1000 + i64::from(t.nanos) / 1_000_000
}

fn timestamp(ms: i64) -> prost_types::Timestamp {
    prost_types::Timestamp { seconds: ms.div_euclid(1000), nanos: (ms.rem_euclid(1000) * 1_000_000) as i32 }
}

/// Whether an announcement is up now: set, with text, and not past its end.
pub fn is_live(a: Option<&pb::Announcement>, now: i64) -> bool {
    a.is_some_and(|a| !a.text.is_empty() && a.ends_at.as_ref().is_none_or(|t| ms(t) > now))
}

/// The tone, with an unset one read as news.
pub fn tone_of(a: &pb::Announcement) -> pb::AnnouncementTone {
    match a.tone() {
        t @ (pb::AnnouncementTone::Warning | pb::AnnouncementTone::Critical) => t,
        _ => pb::AnnouncementTone::Info,
    }
}

/// When an announcement comes down, in milliseconds.
pub fn ends_ms(a: &pb::Announcement) -> Option<i64> {
    a.ends_at.as_ref().map(ms)
}

/// When it went up, in milliseconds.
pub fn created_ms(a: &pb::Announcement) -> Option<i64> {
    a.created_at.as_ref().map(ms)
}

/// "Today", "Yesterday", "Mar 4", or "Mar 4, 2025" in another year, as on the web.
pub fn day_label(at_ms: i64, now_ms: i64) -> String {
    use chrono::{Datelike as _, TimeZone as _};
    let (Some(at), Some(now)) =
        (chrono::Local.timestamp_millis_opt(at_ms).single(), chrono::Local.timestamp_millis_opt(now_ms).single())
    else {
        return String::new();
    };
    match (now.date_naive() - at.date_naive()).num_days() {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        _ if at.year() == now.year() => at.format("%b %-d").to_string(),
        _ => at.format("%b %-d, %Y").to_string(),
    }
}

/// "Today at 15:04", "Yesterday at 09:12", or "Mar 4, 15:04".
pub fn stamp_label(at_ms: i64, now_ms: i64) -> String {
    use chrono::TimeZone as _;
    let Some(at) = chrono::Local.timestamp_millis_opt(at_ms).single() else { return String::new() };
    let day = day_label(at_ms, now_ms);
    let time = at.format("%H:%M");
    if day == "Today" || day == "Yesterday" { format!("{day} at {time}") } else { format!("{day}, {time}") }
}

/// When a banner comes down: a time today, or a day and time.
pub fn ends_label(at_ms: i64, now_ms: i64) -> String {
    use chrono::TimeZone as _;
    match chrono::Local.timestamp_millis_opt(at_ms).single() {
        Some(at) if day_label(at_ms, now_ms) == "Today" => at.format("%H:%M").to_string(),
        _ => stamp_label(at_ms, now_ms),
    }
}

/// "Active now", "Active 5 minutes ago" and so on, as on the web's device lists.
pub fn active_ago(at_ms: i64, now_ms: i64) -> String {
    let minutes = ((now_ms - at_ms).max(0) as f64 / 60_000.0).round() as i64;
    let ago = |n: i64, unit: &str| format!("Active {n} {unit}{} ago", if n == 1 { "" } else { "s" });
    // Sessions note their use every few minutes.
    if minutes < 6 {
        return "Active now".into();
    }
    if minutes < 60 {
        return ago(minutes, "minute");
    }
    let hours = (minutes as f64 / 60.0).round() as i64;
    if hours < 24 {
        return ago(hours, "hour");
    }
    let days = (hours as f64 / 24.0).round() as i64;
    if days < 30 { ago(days, "day") } else { ago((days as f64 / 30.0).round() as i64, "month") }
}

/// The counts after `next` replaces `prev` in the list.
pub fn retotal(totals: &mut pb::AccountTotals, prev: &pb::AccountSummary, next: &pb::AccountSummary) {
    totals.admins += i64::from(next.admin) - i64::from(prev.admin);
    totals.disabled += i64::from(next.disabled) - i64::from(prev.disabled);
}

/// Whether an account still belongs on a list showing `filter`.
pub fn fits(a: &pb::AccountSummary, filter: pb::AccountFilter) -> bool {
    match filter {
        pb::AccountFilter::Admins => a.admin,
        pb::AccountFilter::Disabled => a.disabled,
        pb::AccountFilter::Unspecified => true,
    }
}

impl Core {
    /// A page of the instance's accounts, newest first. Admins only.
    pub async fn list_accounts(
        &self,
        key: &str,
        query: String,
        filter: pb::AccountFilter,
        before_id: String,
    ) -> Result<pb::ListAccountsResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.admin(),
            list_accounts(pb::ListAccountsRequest { query, filter: filter as i32, before_id, limit: PAGE })
        )
        .await
    }

    /// Makes an account an admin or not, or turns it off (with a reason only admins see) or back on.
    pub async fn update_account(
        &self,
        key: &str,
        account_id: String,
        admin: Option<bool>,
        disabled: Option<bool>,
        reason: String,
    ) -> Result<pb::AccountSummary, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res =
            rpc!(api.admin(), update_account(pb::UpdateAccountRequest { account_id, admin, disabled, reason })).await?;
        res.account.ok_or_else(|| Problem::new(Code::Internal, "The instance sent no account."))
    }

    /// A new random password for a standalone account, shown only this once.
    pub async fn reset_account_password(
        &self,
        key: &str,
        account_id: String,
        turn_off_two_factor: bool,
    ) -> Result<String, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.admin(),
            reset_account_password(pb::ResetAccountPasswordRequest { account_id, turn_off_two_factor })
        )
        .await?;
        Ok(res.password)
    }

    /// Puts up the banner (empty text takes it down), ending at `ends_at` if set,
    /// and shows what's up now straight away.
    pub async fn set_announcement(
        &self,
        key: &str,
        text: String,
        tone: pb::AnnouncementTone,
        ends_at: Option<i64>,
    ) -> Result<Option<pb::Announcement>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.admin(),
            set_announcement(pb::SetAnnouncementRequest {
                announcement: Some(pb::Announcement {
                    text,
                    tone: tone as i32,
                    ends_at: ends_at.map(timestamp),
                    ..Default::default()
                }),
            })
        )
        .await?;
        let now = res.announcement.filter(|a| !a.text.is_empty());
        self.shared.instance(key, |i| {
            if let Some(node) = i.node.as_mut() {
                node.announcement = now.clone();
            }
        });
        Ok(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: i64) -> Option<prost_types::Timestamp> {
        Some(timestamp(ms))
    }

    #[test]
    fn live_while_it_has_words_and_hasnt_ended() {
        let up = pb::Announcement { text: "Hi".into(), ..Default::default() };
        assert!(is_live(Some(&up), 1_000));
        assert!(!is_live(None, 1_000));
        assert!(!is_live(Some(&pb::Announcement::default()), 1_000));
        let ending = pb::Announcement { ends_at: at(2_000), ..up.clone() };
        assert!(is_live(Some(&ending), 1_999));
        assert!(!is_live(Some(&ending), 2_000));
        assert_eq!(ends_ms(&ending), Some(2_000));
        assert_eq!(timestamp(-1), prost_types::Timestamp { seconds: -1, nanos: 999_000_000 });
    }

    #[test]
    fn unset_tone_is_news() {
        let a = pb::Announcement::default();
        assert_eq!(tone_of(&a), pb::AnnouncementTone::Info);
        let a = pb::Announcement { tone: pb::AnnouncementTone::Critical as i32, ..Default::default() };
        assert_eq!(tone_of(&a), pb::AnnouncementTone::Critical);
    }

    #[test]
    fn activity_reads_like_the_web() {
        let now = 100 * 86_400_000;
        assert_eq!(active_ago(now - 5 * 60_000, now), "Active now");
        assert_eq!(active_ago(now + 60_000, now), "Active now");
        assert_eq!(active_ago(now - 7 * 60_000, now), "Active 7 minutes ago");
        assert_eq!(active_ago(now - 60 * 60_000, now), "Active 1 hour ago");
        assert_eq!(active_ago(now - 3 * 86_400_000, now), "Active 3 days ago");
        assert_eq!(active_ago(now - 65 * 86_400_000, now), "Active 2 months ago");
    }

    #[test]
    fn days_read_like_the_web() {
        let now = chrono::Local::now().timestamp_millis();
        assert_eq!(day_label(now, now), "Today");
        assert_eq!(day_label(now - 86_400_000, now), "Yesterday");
        assert!(stamp_label(now, now).starts_with("Today at "));
        assert!(!ends_label(now, now).contains("Today"));
    }

    #[test]
    fn counts_follow_a_change() {
        let mut totals = pb::AccountTotals { all: 10, admins: 1, disabled: 0 };
        let prev = pb::AccountSummary { admin: true, ..Default::default() };
        let next = pb::AccountSummary { disabled: true, ..Default::default() };
        retotal(&mut totals, &prev, &next);
        assert_eq!((totals.all, totals.admins, totals.disabled), (10, 0, 1));
        assert!(fits(&next, pb::AccountFilter::Disabled));
        assert!(!fits(&next, pb::AccountFilter::Admins));
        assert!(fits(&next, pb::AccountFilter::Unspecified));
    }
}
