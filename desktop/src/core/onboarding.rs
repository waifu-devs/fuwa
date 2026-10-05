//! A server's onboarding (docs/servers.md): the steps new members go through
//! once they're in (picking what they're into, which hands out roles and
//! suggests channels; the rules; saying hello), and the settings that make
//! them. Like the web app's `join/Onboarding.tsx` and the onboarding actions
//! in `fuwa/actions.ts`.

use tonic::Code;

use crate::core::api::Problem;
use crate::core::store::{self, InstanceState};
use crate::core::{Core, reports};
use crate::pb;
use crate::pb::Permission as P;
use crate::rpc;

pub const MAX_STEPS: usize = 6;
pub const TITLE_MAX: usize = 80;
pub const DESCRIPTION_MAX: usize = 200;
pub const MAX_OPTIONS: usize = 12;
pub const LABEL_MAX: usize = 50;
pub const OPTION_DESCRIPTION_MAX: usize = 100;
pub const MAX_PICKS: usize = 5;
pub const HELLO_MAX: usize = 200;
/// What the hello box starts with when the step doesn't say.
pub const HELLO: &str = "Hi everyone! 👋";
/// How long after joining someone still counts as new.
const NEW_FOR_MS: i64 = 7 * 86_400_000;

pub const PICK: i32 = pb::OnboardingStepKind::Pick as i32;
pub const RULES: i32 = pb::OnboardingStepKind::Rules as i32;
pub const SAY_HELLO: i32 = pb::OnboardingStepKind::Hello as i32;

/// Permissions that make a role too strong for onboarding to hand out, as the web app counts them.
pub const NOT_HARMLESS: [P; 16] = [
    P::Administrator,
    P::ManageServer,
    P::ManageRoles,
    P::ManageChannels,
    P::ManageEmoji,
    P::ManageWebhooks,
    P::ManageMessages,
    P::MentionEveryone,
    P::ViewAuditLog,
    P::KickMembers,
    P::BanMembers,
    P::TimeOutMembers,
    P::ManageNicknames,
    P::MuteMembers,
    P::MoveMembers,
    P::Record,
];

/// Whether onboarding may hand out a role with these permissions.
pub fn harmless(permissions: &[i32]) -> bool {
    !NOT_HARMLESS.iter().any(|p| permissions.contains(&(*p as i32)))
}

/// Whether a member should go through the onboarding without asking: it's
/// on, they're new, they can't manage the server, and they haven't been through it.
pub fn due(i: &InstanceState, server_id: &str, now_ms: i64) -> Option<bool> {
    let server = i.server(server_id)?;
    let me = i.my_member(server_id)?;
    let joined = me.joined_at.as_ref().map(|t| t.seconds * 1000).unwrap_or_default();
    Some(
        server.has_onboarding
            && me.onboarded_at.is_none()
            && !i.access(server_id).has(P::ManageServer)
            && now_ms - joined < NEW_FOR_MS,
    )
}

/// The steps someone goes through: rules only for someone who still has to
/// agree (added before the first hello when the server has none), hellos
/// only in channels they can see, and choices only where there are some.
pub fn steps_for(
    onboarding: &pb::Onboarding,
    must_agree: bool,
    can_see: impl Fn(&str) -> bool,
) -> Vec<pb::OnboardingStep> {
    let mut steps: Vec<pb::OnboardingStep> = onboarding
        .steps
        .iter()
        .filter(|s| match s.kind {
            RULES => must_agree,
            SAY_HELLO => can_see(&s.channel_id),
            PICK => !s.options.is_empty(),
            _ => false,
        })
        .cloned()
        .collect();
    if must_agree && !steps.iter().any(|s| s.kind == RULES) {
        let rules =
            pb::OnboardingStep { id: "rules".into(), kind: RULES, title: "The rules".into(), ..Default::default() };
        let at = steps.iter().position(|s| s.kind == SAY_HELLO).unwrap_or(steps.len());
        steps.insert(at, rules);
    }
    steps
}

/// Picks one option of a step: in a step of one choice it replaces the
/// others; picking one again unpicks it.
pub fn toggle(picked: &mut Vec<String>, step: &pb::OnboardingStep, option_id: &str) {
    if let Some(at) = picked.iter().position(|p| p == option_id) {
        picked.remove(at);
        return;
    }
    if !step.multiple {
        picked.retain(|p| !step.options.iter().any(|o| o.id == *p));
    }
    picked.push(option_id.to_owned());
}

/// Whether a step has something picked.
pub fn has_pick(picked: &[String], step: &pb::OnboardingStep) -> bool {
    step.options.iter().any(|o| picked.contains(&o.id))
}

/// A place to go first, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoHere {
    pub channel_id: String,
    pub note: String,
    pub emoji: String,
}

/// Where to go first: the channels of the choices picked ("Because you
/// picked Art"), then the welcome screen's; each once, only ones that
/// exist, six at most.
pub fn go_here_first(
    steps: &[pb::OnboardingStep],
    picked: &[String],
    welcome: Option<&pb::WelcomeScreen>,
    exists: impl Fn(&str) -> bool,
) -> Vec<GoHere> {
    let mut out: Vec<GoHere> = Vec::new();
    let mut add = |channel_id: &str, note: String, emoji: &str| {
        if out.len() < 6 && exists(channel_id) && !out.iter().any(|g| g.channel_id == channel_id) {
            out.push(GoHere { channel_id: channel_id.to_owned(), note, emoji: emoji.to_owned() });
        }
    };
    for option in steps.iter().flat_map(|s| &s.options).filter(|o| picked.contains(&o.id)) {
        for channel in &option.channel_ids {
            add(channel, format!("Because you picked {}", option.label), &option.emoji);
        }
    }
    if let Some(screen) = welcome.filter(|w| w.enabled) {
        for w in &screen.channels {
            add(&w.channel_id, w.description.clone(), &w.emoji);
        }
    }
    out
}

/// A new step of a kind, as the editor adds it.
pub fn new_step(kind: i32, first_text_channel: Option<&str>) -> pb::OnboardingStep {
    match kind {
        RULES => pb::OnboardingStep { kind, title: "Our rules".into(), ..Default::default() },
        SAY_HELLO => pb::OnboardingStep {
            kind,
            title: "Say hello".into(),
            skippable: true,
            channel_id: first_text_channel.unwrap_or_default().to_owned(),
            hello: HELLO.into(),
            ..Default::default()
        },
        _ => pb::OnboardingStep {
            kind: PICK,
            title: "What are you into?".into(),
            multiple: true,
            options: vec![new_option()],
            ..Default::default()
        },
    }
}

pub fn new_option() -> pb::OnboardingOption {
    pb::OnboardingOption { emoji: "✨".into(), ..Default::default() }
}

/// The onboarding as it's saved: its words trimmed, and the rules never skippable.
pub fn cleaned(mut onboarding: pb::Onboarding) -> pb::Onboarding {
    for step in &mut onboarding.steps {
        step.title = step.title.trim().to_owned();
        step.description = step.description.trim().to_owned();
        step.hello = step.hello.trim().to_owned();
        if step.kind == RULES {
            step.skippable = false;
        }
        if step.kind != PICK {
            step.options.clear();
        }
        for option in &mut step.options {
            option.label = option.label.trim().to_owned();
            option.description = option.description.trim().to_owned();
        }
    }
    onboarding.set_by.clear();
    onboarding
}

impl Core {
    /// A server's onboarding: the whole of it for managers, only what's on
    /// (with channels you can see) for everyone else.
    pub async fn onboarding(&self, key: &str, server_id: &str) -> Result<pb::Onboarding, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(api.join(), get_onboarding(pb::GetOnboardingRequest { server_id: server_id.into() })).await?;
        Ok(res.onboarding.unwrap_or_default())
    }

    pub async fn set_onboarding(
        &self,
        key: &str,
        server_id: &str,
        onboarding: pb::Onboarding,
    ) -> Result<pb::Onboarding, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.join(),
            set_onboarding(pb::SetOnboardingRequest {
                server_id: server_id.into(),
                onboarding: Some(cleaned(onboarding))
            })
        )
        .await?;
        let saved = res.onboarding.unwrap_or_default();
        self.shared.instance(key, |i| {
            if let Some(s) = i.servers.iter_mut().find(|s| s.id == server_id) {
                s.has_onboarding = saved.enabled;
            }
        });
        Ok(saved)
    }

    /// Went through the onboarding picking these (or none, to skip it): their roles are handed out.
    pub async fn finish_onboarding(&self, key: &str, server_id: &str, option_ids: Vec<String>) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let skipped = option_ids.is_empty();
        let res = rpc!(
            api.join(),
            finish_onboarding(pb::FinishOnboardingRequest { server_id: server_id.into(), option_ids })
        )
        .await?;
        reports::used(if skipped { "onboarding.skip" } else { "onboarding.finish" });
        if let Some(member) = res.member {
            self.shared.instance(key, |i| {
                let Some(user) = member.user.clone() else { return };
                let list = i.members.entry(server_id.to_owned()).or_default();
                list.retain(|m| !m.user.as_ref().is_some_and(|u| u.id == user.id));
                list.push(member);
                store::sort_members(list);
            });
        }
        Ok(())
    }
}

fn missing() -> Problem {
    Problem::new(Code::NotFound, "That instance isn't here.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(id: &str, kind: i32) -> pb::OnboardingStep {
        pb::OnboardingStep { id: id.into(), kind, ..Default::default() }
    }

    fn option(id: &str, label: &str, channels: &[&str]) -> pb::OnboardingOption {
        pb::OnboardingOption {
            id: id.into(),
            label: label.into(),
            channel_ids: channels.iter().map(|c| c.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn steps_show_only_what_applies() {
        let mut pick = step("p", PICK);
        pick.options.push(option("a", "Art", &[]));
        let mut hello = step("h", SAY_HELLO);
        hello.channel_id = "c1".into();
        let mut hidden = step("h2", SAY_HELLO);
        hidden.channel_id = "secret".into();
        let onboarding = pb::Onboarding {
            enabled: true,
            steps: vec![pick, step("empty", PICK), hello, hidden, step("r", RULES)],
            ..Default::default()
        };
        let ids =
            |must: bool| steps_for(&onboarding, must, |c| c != "secret").into_iter().map(|s| s.id).collect::<Vec<_>>();
        assert_eq!(ids(false), ["p", "h"]);
        assert_eq!(ids(true), ["p", "h", "r"]);
        // Without a rules step, one goes in before the first hello.
        let mut no_rules = onboarding.clone();
        no_rules.steps.retain(|s| s.kind != RULES);
        let got: Vec<_> = steps_for(&no_rules, true, |_| true).into_iter().map(|s| s.id).collect();
        assert_eq!(got, ["p", "rules", "h", "h2"]);
    }

    #[test]
    fn picking_one_or_several() {
        let mut s = step("p", PICK);
        s.options = vec![option("a", "A", &[]), option("b", "B", &[])];
        let mut picked = vec!["elsewhere".to_owned()];
        toggle(&mut picked, &s, "a");
        toggle(&mut picked, &s, "b");
        assert_eq!(picked, ["elsewhere", "b"], "one choice: b replaces a");
        toggle(&mut picked, &s, "b");
        assert_eq!(picked, ["elsewhere"], "again unpicks");
        s.multiple = true;
        toggle(&mut picked, &s, "a");
        toggle(&mut picked, &s, "b");
        assert!(has_pick(&picked, &s));
        assert_eq!(picked.len(), 3);
    }

    #[test]
    fn where_to_go_first() {
        let mut s = step("p", PICK);
        s.options = vec![option("a", "Art", &["art", "gone"]), option("b", "Books", &["books"])];
        let welcome = pb::WelcomeScreen {
            enabled: true,
            channels: vec![
                pb::WelcomeChannel { channel_id: "art".into(), description: "dup".into(), ..Default::default() },
                pb::WelcomeChannel { channel_id: "general".into(), description: "Say hi".into(), ..Default::default() },
            ],
            ..Default::default()
        };
        let got = go_here_first(&[s], &["a".into()], Some(&welcome), |c| c != "gone");
        let ids: Vec<_> = got.iter().map(|g| g.channel_id.as_str()).collect();
        assert_eq!(ids, ["art", "general"]);
        assert_eq!(got[0].note, "Because you picked Art");
    }

    #[test]
    fn saving_trims_and_keeps_rules_unskippable() {
        let mut rules = new_step(RULES, None);
        rules.skippable = true;
        rules.options.push(new_option());
        let mut pick = new_step(PICK, None);
        pick.title = "  Hi  ".into();
        pick.options[0].label = " Art ".into();
        let saved = cleaned(pb::Onboarding { steps: vec![rules, pick], set_by: "x".into(), ..Default::default() });
        assert!(!saved.steps[0].skippable && saved.steps[0].options.is_empty());
        assert_eq!(saved.steps[1].title, "Hi");
        assert_eq!(saved.steps[1].options[0].label, "Art");
        assert!(saved.set_by.is_empty());
        assert!(harmless(&[P::SendMessages as i32]));
        assert!(!harmless(&[P::KickMembers as i32]));
    }
}
