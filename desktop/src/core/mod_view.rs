//! Mod View (docs/mod-view.md): reporting messages and people, and the
//! cases, people's pages and notes a server's moderators work through.
//! Everyone can report; only people with Use Mod View (MODERATE) see the
//! rest, and only on instances with the `mod-view` feature.
//!
//! The store keeps, per server, how many cases are open (for the badge), the
//! cases seen so far (so the queue follows `ModCaseUpdated` live) and the
//! notes of the people whose pages are open (`ModNoteAdded`/`Deleted`).

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::Core;
use crate::core::api::Problem;
use crate::core::store::InstanceState;
use crate::pb::{self, ModCaseKind as Kind, ModCaseStatus as Status, ModReportReason as Reason, Permission as P};
use crate::rpc;

fn missing() -> Problem {
    Problem::new(tonic::Code::NotFound, "That instance isn't here.")
}

/// The most a report's note may say.
pub const MAX_REPORT_NOTE: usize = 500;
/// The most a moderator's note may say.
pub const MAX_NOTE: usize = 1000;
/// The most a resolution may say.
pub const MAX_RESOLUTION: usize = 500;
/// Cases or messages asked for at a time.
pub const PAGE: i32 = 50;

/// The reasons a report can give, in the order the dialog offers them.
pub const REASONS: [Reason; 9] = [
    Reason::Spam,
    Reason::Harassment,
    Reason::Hate,
    Reason::Sexual,
    Reason::Violence,
    Reason::SelfHarm,
    Reason::Scam,
    Reason::Rules,
    Reason::Other,
];

/// What one server's Mod View knows.
#[derive(Debug, Clone, Default)]
pub struct ModView {
    /// How many cases are open in channels you can see; None until counted.
    pub open: Option<i32>,
    /// The count was asked for and hasn't come back (or failed) yet.
    pub asked: bool,
    /// Every case seen, by id, as it is now.
    pub cases: HashMap<String, pb::ModCase>,
    /// Notes about the people whose pages were opened, newest first, by who they're about.
    pub notes: HashMap<String, Vec<pb::ModNote>>,
}

/// The queue's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Open,
    Resolved,
    Dismissed,
    All,
}

pub const TABS: [Tab; 4] = [Tab::Open, Tab::Resolved, Tab::Dismissed, Tab::All];

impl Tab {
    /// The status the instance filters by (unspecified for every case).
    pub fn status(self) -> Status {
        match self {
            Tab::Open => Status::Open,
            Tab::Resolved => Status::Resolved,
            Tab::Dismissed => Status::Dismissed,
            Tab::All => Status::Unspecified,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Tab::Open => "desktop.modView.tab.open",
            Tab::Resolved => "desktop.modView.tab.resolved",
            Tab::Dismissed => "desktop.modView.tab.dismissed",
            Tab::All => "desktop.modView.tab.all",
        }
    }

    fn holds(self, case: &pb::ModCase) -> bool {
        self == Tab::All || case.status == self.status() as i32
    }
}

/// A report reason's words, as the translations name it.
pub fn reason_key(reason: Reason) -> &'static str {
    match reason {
        Reason::Unspecified | Reason::Other => "desktop.modView.reason.other",
        Reason::Spam => "desktop.modView.reason.spam",
        Reason::Harassment => "desktop.modView.reason.harassment",
        Reason::Hate => "desktop.modView.reason.hate",
        Reason::Sexual => "desktop.modView.reason.sexual",
        Reason::Violence => "desktop.modView.reason.violence",
        Reason::SelfHarm => "desktop.modView.reason.selfHarm",
        Reason::Scam => "desktop.modView.reason.scam",
        Reason::Rules => "desktop.modView.reason.rules",
    }
}

/// What a reason means, under its name in the report dialog.
pub fn reason_hint_key(reason: Reason) -> &'static str {
    match reason {
        Reason::Unspecified | Reason::Other => "desktop.modView.reasonHint.other",
        Reason::Spam => "desktop.modView.reasonHint.spam",
        Reason::Harassment => "desktop.modView.reasonHint.harassment",
        Reason::Hate => "desktop.modView.reasonHint.hate",
        Reason::Sexual => "desktop.modView.reasonHint.sexual",
        Reason::Violence => "desktop.modView.reasonHint.violence",
        Reason::SelfHarm => "desktop.modView.reasonHint.selfHarm",
        Reason::Scam => "desktop.modView.reasonHint.scam",
        Reason::Rules => "desktop.modView.reasonHint.rules",
    }
}

/// A case's kind, in words.
pub fn kind_key(kind: Kind) -> &'static str {
    match kind {
        Kind::AutoMod => "desktop.modView.kind.autoMod",
        Kind::MessageReport => "desktop.modView.kind.message",
        Kind::MemberReport | Kind::Unspecified => "desktop.modView.kind.member",
    }
}

/// A case's state, in words.
pub fn status_key(status: Status) -> &'static str {
    match status {
        Status::Open | Status::Unspecified => "desktop.modView.status.open",
        Status::Resolved => "desktop.modView.status.resolved",
        Status::Dismissed => "desktop.modView.status.dismissed",
    }
}

fn ms(t: Option<&prost_types::Timestamp>) -> i64 {
    t.map(|t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000).unwrap_or(0)
}

/// A case changed (or arrived): kept, and the open count moved with it.
/// When the count can't be told from what's known (a case never seen that
/// isn't new), it's forgotten so it's counted again.
pub fn case_updated(mv: &mut ModView, case: &pb::ModCase) {
    let open = case.status == Status::Open as i32;
    let delta = match mv.cases.get(&case.id) {
        Some(was) => {
            let was_open = was.status == Status::Open as i32;
            Some(i32::from(open) - i32::from(was_open))
        }
        // Brand new: opened just now, never reported again or closed.
        None if open && ms(case.created_at.as_ref()) == ms(case.updated_at.as_ref()) => Some(1),
        None => None,
    };
    match (delta, mv.open) {
        (Some(d), Some(n)) => mv.open = Some((n + d).max(0)),
        (Some(_), None) => {}
        (None, _) => {
            mv.open = None;
            mv.asked = false;
        }
    }
    mv.cases.insert(case.id.clone(), case.clone());
}

/// A note was written: shown at the top of its person's notes, if they're open.
pub fn note_added(mv: &mut ModView, note: &pb::ModNote) {
    if let Some(list) = mv.notes.get_mut(&note.subject_id)
        && !list.iter().any(|n| n.id == note.id)
    {
        list.insert(0, note.clone());
        list.sort_by(|a, b| b.id.cmp(&a.id));
    }
}

pub fn note_deleted(mv: &mut ModView, subject_id: &str, note_id: &str) {
    if let Some(list) = mv.notes.get_mut(subject_id) {
        list.retain(|n| n.id != note_id);
    }
}

/// The queue as it stands: the cases a tab (and maybe one person) holds,
/// newest first. Those `listed` came from the instance; cases met since (in
/// events) join them when they're newer than the last one listed, or
/// anywhere once the whole list is here.
pub fn queue(mv: &ModView, tab: Tab, subject: Option<&str>, listed: &[String], more: bool) -> Vec<pb::ModCase> {
    let oldest = listed.iter().min();
    let mut out: Vec<pb::ModCase> = mv
        .cases
        .values()
        .filter(|c| tab.holds(c) && subject.is_none_or(|s| c.subject_id == s))
        .filter(|c| listed.contains(&c.id) || !more || oldest.is_none_or(|o| c.id > *o))
        .cloned()
        .collect();
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

/// Whether a message may be reported from here: someone else's plain message
/// in this server's own text channel (not a secure channel, nor one shared
/// from another server), on an instance with Mod View.
pub fn can_report_message(i: &InstanceState, server_id: &str, channel_id: &str, message: &pb::Message) -> bool {
    if !i.has("mod-view") || message.kind != pb::MessageKind::Unspecified as i32 || message.webhook.is_some() {
        return false;
    }
    if i.me.as_ref().is_none_or(|me| me.id == message.author_id) {
        return false;
    }
    let Some(channel) = i.channel(server_id, channel_id) else { return false };
    let kind = pb::ChannelType::try_from(channel.r#type).unwrap_or(pb::ChannelType::Text);
    matches!(kind, pb::ChannelType::Text | pb::ChannelType::Announcement)
        && channel.shared.as_ref().is_none_or(|s| s.home)
}

/// Whether someone may be reported in a server: anyone but you, on an instance with Mod View.
pub fn can_report_member(i: &InstanceState, server_id: &str, user_id: &str) -> bool {
    i.has("mod-view") && i.server(server_id).is_some() && i.me.as_ref().is_some_and(|me| me.id != user_id)
}

/// Whether you may delete a note: your own, or anyone's with Manage Server.
pub fn can_delete_note(note: &pb::ModNote, me: &str, manage_server: bool) -> bool {
    manage_server || note.author_id == me
}

/// At most `max` characters of what was typed, without the space around it.
pub fn clipped(text: &str, max: usize) -> String {
    text.trim().chars().take(max).collect()
}

/// How old an account is, in its largest whole unit, as (count, unit key).
pub fn age(created_ms: i64, now_ms: i64) -> (i64, &'static str) {
    let days = (now_ms - created_ms).max(0) / 86_400_000;
    match days {
        0 => (0, "desktop.modView.age.today"),
        d if d < 30 => (d, "desktop.modView.age.days"),
        d if d < 365 => (d / 30, "desktop.modView.age.months"),
        d => (d / 365, "desktop.modView.age.years"),
    }
}

impl InstanceState {
    /// Whether you can open a server's Mod View.
    pub fn can_mod_view(&self, server_id: &str) -> bool {
        self.has("mod-view") && self.access(server_id).has(P::Moderate)
    }

    /// Whether Mod View can find people here, those who left or were banned too.
    pub fn can_find_people(&self, server_id: &str) -> bool {
        self.can_mod_view(server_id) && self.has("mod-view-find")
    }

    /// How many cases are open in a server's Mod View, once counted (0 without it).
    pub fn open_cases(&self, server_id: &str) -> Option<i32> {
        if !self.can_mod_view(server_id) {
            return Some(0);
        }
        self.mod_view.get(server_id).and_then(|m| m.open)
    }
}

/// A page of cases from the instance.
#[derive(Debug, Clone, Default)]
pub struct CasePage {
    pub ids: Vec<String>,
    pub more: bool,
}

/// A page of someone's messages.
#[derive(Debug, Clone, Default)]
pub struct MessagePage {
    pub messages: Vec<pb::Message>,
    pub more: bool,
}

impl Core {
    /// Counts a server's open cases for its badge, unless that's known or on its way.
    pub fn want_open_cases(self: &Arc<Self>, key: &str, server_id: &str) {
        let ask = self.shared.instance_if(key, &[], |i| {
            if !i.can_mod_view(server_id) {
                return false;
            }
            let mv = i.mod_view.entry(server_id.to_owned()).or_default();
            let ask = mv.open.is_none() && !mv.asked;
            mv.asked = true;
            ask
        });
        if !ask {
            return;
        }
        let (core, key, server_id) = (self.clone(), key.to_owned(), server_id.to_owned());
        drop(self.spawn(async move {
            if core.mod_cases(&key, &server_id, Tab::Open, "", "", 1).await.is_err() {
                // Asked again the next time it's wanted.
                core.shared.instance_in(&key, &[], |i| {
                    if let Some(mv) = i.mod_view.get_mut(&server_id) {
                        mv.asked = false;
                    }
                });
            }
        }));
    }

    /// Reports a message to the server's moderators.
    pub async fn report_message(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        message_id: &str,
        reason: Reason,
        note: &str,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.moderation(),
            report_message(pb::ReportMessageRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                message_id: message_id.into(),
                reason: reason as i32,
                note: clipped(note, MAX_REPORT_NOTE),
            })
        )
        .await?;
        Ok(())
    }

    /// Reports someone to the server's moderators.
    pub async fn report_member(
        &self,
        key: &str,
        server_id: &str,
        user_id: &str,
        reason: Reason,
        note: &str,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.moderation(),
            report_member(pb::ReportMemberRequest {
                server_id: server_id.into(),
                user_id: user_id.into(),
                reason: reason as i32,
                note: clipped(note, MAX_REPORT_NOTE),
            })
        )
        .await?;
        Ok(())
    }

    /// A page of cases, newest first (after `before`), kept in the store with
    /// the open count and the people they name.
    pub async fn mod_cases(
        &self,
        key: &str,
        server_id: &str,
        tab: Tab,
        subject: &str,
        before: &str,
        limit: i32,
    ) -> Result<CasePage, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.moderation(),
            list_mod_cases(pb::ListModCasesRequest {
                server_id: server_id.into(),
                status: tab.status() as i32,
                subject_id: subject.into(),
                before_id: before.into(),
                limit,
                ..Default::default()
            })
        )
        .await?;
        let ids = res.cases.iter().map(|c| c.id.clone()).collect();
        self.shared.instance(key, |i| {
            for u in res.users {
                i.users.entry(u.id.clone()).or_insert(u);
            }
            let mv = i.mod_view.entry(server_id.to_owned()).or_default();
            mv.open = Some(res.open_count);
            mv.asked = true;
            for c in res.cases {
                mv.cases.insert(c.id.clone(), c);
            }
        });
        Ok(CasePage { ids, more: res.has_more })
    }

    /// One case with every report in it and the message as it is now.
    pub async fn mod_case(&self, key: &str, server_id: &str, case_id: &str) -> Result<pb::GetModCaseResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.moderation(),
            get_mod_case(pb::GetModCaseRequest { server_id: server_id.into(), case_id: case_id.into() })
        )
        .await?;
        self.shared.instance(key, |i| {
            for u in &res.users {
                i.users.entry(u.id.clone()).or_insert(u.clone());
            }
            if let Some(c) = &res.mod_case {
                case_updated(i.mod_view.entry(server_id.to_owned()).or_default(), c);
            }
        });
        Ok(res)
    }

    /// Closes a case (resolved or dismissed) or opens it again.
    pub async fn set_case_status(
        &self,
        key: &str,
        server_id: &str,
        case_id: &str,
        status: Status,
        resolution: &str,
    ) -> Result<pb::ModCase, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.moderation(),
            update_mod_case(pb::UpdateModCaseRequest {
                server_id: server_id.into(),
                case_id: case_id.into(),
                status: status as i32,
                resolution: clipped(resolution, MAX_RESOLUTION),
            })
        )
        .await?;
        let case = res.mod_case.unwrap_or_default();
        self.shared.instance(key, |i| case_updated(i.mod_view.entry(server_id.to_owned()).or_default(), &case));
        Ok(case)
    }

    /// Everything about one person for a moderator; their notes are kept to follow live.
    pub async fn member_overview(
        &self,
        key: &str,
        server_id: &str,
        user_id: &str,
    ) -> Result<pb::GetMemberOverviewResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.moderation(),
            get_member_overview(pb::GetMemberOverviewRequest { server_id: server_id.into(), user_id: user_id.into() })
        )
        .await?;
        self.shared.instance(key, |i| {
            for u in res.users.iter().chain(res.user.iter()) {
                i.users.insert(u.id.clone(), u.clone());
            }
            let mv = i.mod_view.entry(server_id.to_owned()).or_default();
            mv.notes.insert(user_id.to_owned(), res.notes.clone());
        });
        Ok(res)
    }

    /// People who are or were in a server whose name, username or nickname
    /// holds `query`, or whose id it is, members first; who they are is kept.
    pub async fn find_people(&self, key: &str, server_id: &str, query: &str) -> Result<Vec<pb::ModPerson>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.moderation(),
            find_people(pb::FindPeopleRequest { server_id: server_id.into(), query: query.into(), limit: 20 })
        )
        .await?;
        self.shared.instance(key, |i| {
            for u in res.people.iter().filter_map(|p| p.user.as_ref()) {
                i.users.insert(u.id.clone(), u.clone());
            }
        });
        Ok(res.people)
    }

    /// What someone wrote, newest first, in the channels you can see.
    pub async fn member_messages(
        &self,
        key: &str,
        server_id: &str,
        user_id: &str,
        before: &str,
    ) -> Result<MessagePage, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.moderation(),
            list_member_messages(pb::ListMemberMessagesRequest {
                server_id: server_id.into(),
                user_id: user_id.into(),
                before_id: before.into(),
                limit: PAGE,
                ..Default::default()
            })
        )
        .await?;
        self.shared.instance(key, |i| {
            for u in res.authors {
                i.users.entry(u.id.clone()).or_insert(u);
            }
        });
        Ok(MessagePage { messages: res.messages, more: res.has_more })
    }

    pub async fn add_mod_note(&self, key: &str, server_id: &str, user_id: &str, content: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.moderation(),
            add_mod_note(pb::AddModNoteRequest {
                server_id: server_id.into(),
                user_id: user_id.into(),
                content: clipped(content, MAX_NOTE),
            })
        )
        .await?;
        if let Some(note) = res.note {
            self.shared.instance(key, |i| note_added(i.mod_view.entry(server_id.to_owned()).or_default(), &note));
        }
        Ok(())
    }

    pub async fn delete_mod_note(
        &self,
        key: &str,
        server_id: &str,
        subject_id: &str,
        note_id: &str,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.moderation(),
            delete_mod_note(pb::DeleteModNoteRequest { server_id: server_id.into(), note_id: note_id.into() })
        )
        .await?;
        self.shared
            .instance(key, |i| note_deleted(i.mod_view.entry(server_id.to_owned()).or_default(), subject_id, note_id));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: i64) -> Option<prost_types::Timestamp> {
        Some(prost_types::Timestamp { seconds: ms / 1000, nanos: ((ms % 1000) * 1_000_000) as i32 })
    }

    fn case(id: &str, status: Status, created: i64, updated: i64) -> pb::ModCase {
        pb::ModCase {
            id: id.into(),
            status: status as i32,
            subject_id: "u1".into(),
            created_at: at(created),
            updated_at: at(updated),
            ..Default::default()
        }
    }

    #[test]
    fn the_open_count_follows_cases() {
        let mut mv = ModView { open: Some(2), ..Default::default() };
        // A new case counts once, however often it arrives.
        case_updated(&mut mv, &case("c3", Status::Open, 1000, 1000));
        assert_eq!(mv.open, Some(3));
        case_updated(&mut mv, &case("c3", Status::Open, 1000, 1000));
        assert_eq!(mv.open, Some(3));
        // Reported again: still one.
        case_updated(&mut mv, &case("c3", Status::Open, 1000, 2000));
        assert_eq!(mv.open, Some(3));
        // Closed, then opened again.
        case_updated(&mut mv, &case("c3", Status::Resolved, 1000, 3000));
        assert_eq!(mv.open, Some(2));
        case_updated(&mut mv, &case("c3", Status::Resolved, 1000, 3000));
        assert_eq!(mv.open, Some(2));
        case_updated(&mut mv, &case("c3", Status::Open, 1000, 4000));
        assert_eq!(mv.open, Some(3));
    }

    #[test]
    fn a_case_never_seen_that_isnt_new_is_counted_again() {
        let mut mv = ModView { open: Some(4), asked: true, ..Default::default() };
        case_updated(&mut mv, &case("old", Status::Dismissed, 1000, 5000));
        assert_eq!(mv.open, None);
        assert!(!mv.asked, "asked again");
        assert!(mv.cases.contains_key("old"));
    }

    #[test]
    fn the_queue_holds_its_tab_newest_first() {
        let mut mv = ModView::default();
        for (id, status) in [("01", Status::Open), ("02", Status::Resolved), ("03", Status::Open), ("05", Status::Open)]
        {
            mv.cases.insert(id.into(), case(id, status, 0, 0));
        }
        let ids = |list: Vec<pb::ModCase>| list.into_iter().map(|c| c.id).collect::<Vec<_>>();
        // Listed 03 and 02 with more to come: 05 arrived later and joins; 01 waits for its page.
        let listed = vec!["03".to_owned(), "02".to_owned()];
        assert_eq!(ids(queue(&mv, Tab::Open, None, &listed, true)), ["05", "03"]);
        assert_eq!(ids(queue(&mv, Tab::All, None, &listed, true)), ["05", "03", "02"]);
        assert_eq!(ids(queue(&mv, Tab::Open, None, &listed, false)), ["05", "03", "01"]);
        assert_eq!(ids(queue(&mv, Tab::Resolved, None, &listed, true)), ["02"]);
        assert!(queue(&mv, Tab::Open, Some("someone else"), &listed, false).is_empty());
    }

    #[test]
    fn notes_follow_events_once_open() {
        let mut mv = ModView::default();
        let note = |id: &str| pb::ModNote { id: id.into(), subject_id: "u1".into(), ..Default::default() };
        // Nobody's page is open: nothing kept.
        note_added(&mut mv, &note("n1"));
        assert!(mv.notes.is_empty());
        mv.notes.insert("u1".into(), vec![note("n1")]);
        note_added(&mut mv, &note("n2"));
        note_added(&mut mv, &note("n2"));
        assert_eq!(mv.notes["u1"].iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["n2", "n1"]);
        note_deleted(&mut mv, "u1", "n1");
        assert_eq!(mv.notes["u1"].len(), 1);
    }

    #[test]
    fn notes_are_deleted_by_their_author_or_a_manager() {
        let note = pb::ModNote { author_id: "a".into(), ..Default::default() };
        assert!(can_delete_note(&note, "a", false));
        assert!(!can_delete_note(&note, "b", false));
        assert!(can_delete_note(&note, "b", true));
    }

    #[test]
    fn what_is_typed_is_clipped() {
        assert_eq!(clipped("  hi  ", 10), "hi");
        assert_eq!(clipped(&"é".repeat(600), MAX_REPORT_NOTE).chars().count(), 500);
    }

    #[test]
    fn ages_read_in_their_largest_unit() {
        let day = 86_400_000;
        assert_eq!(age(0, day / 2), (0, "desktop.modView.age.today"));
        assert_eq!(age(0, 3 * day), (3, "desktop.modView.age.days"));
        assert_eq!(age(0, 65 * day), (2, "desktop.modView.age.months"));
        assert_eq!(age(0, 800 * day), (2, "desktop.modView.age.years"));
    }

    #[test]
    fn only_plain_messages_from_others_in_this_servers_text_channels_are_reported() {
        let mut i = InstanceState::new("k", "https://x");
        i.me = Some(pb::User { id: "me".into(), ..Default::default() });
        i.node = Some(pb::Node {
            versions: Some(pb::Versions {
                features: vec![pb::Feature { id: "mod-view".into(), ..Default::default() }],
                ..Default::default()
            }),
            ..Default::default()
        });
        i.servers.push(pb::Server { id: "s".into(), ..Default::default() });
        let channel = |id: &str, kind: pb::ChannelType, shared: Option<bool>| pb::Channel {
            id: id.into(),
            server_id: "s".into(),
            r#type: kind as i32,
            shared: shared.map(|home| pb::SharedChannel { home, ..Default::default() }),
            ..Default::default()
        };
        i.channels.insert(
            "s".into(),
            vec![
                channel("text", pb::ChannelType::Text, None),
                channel("secure", pb::ChannelType::Secure, None),
                channel("guest", pb::ChannelType::Text, Some(false)),
                channel("home", pb::ChannelType::Text, Some(true)),
            ],
        );
        let msg = |author: &str| pb::Message { author_id: author.into(), ..Default::default() };
        assert!(can_report_message(&i, "s", "text", &msg("them")));
        assert!(can_report_message(&i, "s", "home", &msg("them")));
        assert!(!can_report_message(&i, "s", "text", &msg("me")));
        assert!(!can_report_message(&i, "s", "secure", &msg("them")));
        assert!(!can_report_message(&i, "s", "guest", &msg("them")));
        let join = pb::Message { kind: pb::MessageKind::MemberJoined as i32, ..msg("them") };
        assert!(!can_report_message(&i, "s", "text", &join));
        assert!(can_report_member(&i, "s", "them"));
        assert!(!can_report_member(&i, "s", "me"));
        // An instance without Mod View has nothing to report to.
        i.node = Some(pb::Node { versions: Some(pb::Versions::default()), ..Default::default() });
        assert!(!can_report_message(&i, "s", "text", &msg("them")));
        assert!(!can_report_member(&i, "s", "them"));
    }
}
