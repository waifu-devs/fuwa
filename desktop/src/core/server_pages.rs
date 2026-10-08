//! The calls behind the server settings pages the web has beyond the basics:
//! the rules and questions, applications, single sign-on, usage, and handing
//! the server over, as in the web app's `actions.ts`.

use tonic::Code;

use crate::core::Core;
use crate::core::api::Problem;
use crate::pb;
use crate::rpc;

fn missing() -> Problem {
    Problem::new(Code::NotFound, "That instance isn't here.")
}

/// As on the server: rules, their length, questions and their length.
pub const MAX_RULES: usize = 16;
pub const RULE_MAX: usize = 300;
pub const MAX_QUESTIONS: usize = 5;
pub const PROMPT_MAX: usize = 200;
/// How long a reason for turning someone down may be.
pub const REASON_MAX: usize = 512;
/// Accounts younger than this get a "new account" tag, to spot throwaways.
pub const NEW_ACCOUNT_MS: i64 = 7 * 86_400_000;

/// The minimum account ages a server can pick, in seconds, with their labels' keys (`ACCOUNT_AGES`).
pub const ACCOUNT_AGES: [(i32, &str); 5] = [
    (0, "serversettings.access.age.any"),
    (600, "serversettings.access.age.tenMinutes"),
    (3_600, "serversettings.access.age.hour"),
    (86_400, "serversettings.access.age.day"),
    (604_800, "serversettings.access.age.week"),
];

/// How often members sign in again, in days (0 never).
pub const RECHECKS: [i32; 4] = [7, 30, 90, 0];

/// Who may come in: what changes; `None` keeps it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessPatch {
    pub discoverable: Option<bool>,
    pub applications: Option<bool>,
    pub linked_only: Option<bool>,
    pub min_account_age_seconds: Option<i32>,
}

/// A form's draft: the rules and questions as they'll be saved (blank ones left out).
pub fn cleaned_form(rules: &[String], questions: &[pb::JoinQuestion]) -> pb::JoinForm {
    pb::JoinForm {
        rules: rules.iter().map(|r| r.trim().to_owned()).filter(|r| !r.is_empty()).collect(),
        questions: questions
            .iter()
            .filter(|q| !q.prompt.trim().is_empty())
            .map(|q| pb::JoinQuestion { prompt: q.prompt.trim().to_owned(), ..q.clone() })
            .collect(),
    }
}

/// How many parts of a form changed: the rules, the questions, each counted once.
pub fn form_changes(draft: &pb::JoinForm, saved: &pb::JoinForm) -> usize {
    usize::from(draft.rules != saved.rules) + usize::from(draft.questions != saved.questions)
}

/// The starter rules on offer while the rules are only ever starters: the ones not used yet.
pub fn ideas<'a>(rules: &[String], starters: &'a [String]) -> Vec<&'a String> {
    let only_starters = rules.iter().all(|r| starters.contains(r) || r.trim().is_empty());
    if !only_starters {
        return Vec::new();
    }
    starters.iter().filter(|s| !rules.contains(s)).collect()
}

/// A provider's fingerprint for spotting edits: everything but secrets kept on the server.
pub fn provider_print(p: Option<&pb::IdentityProvider>) -> String {
    crate::core::sso::provider_print(p)
}

/// Whether a provider has what it needs to sign people in.
pub fn provider_ready(p: Option<&pb::IdentityProvider>) -> bool {
    let Some(p) = p.filter(|p| !p.name.trim().is_empty()) else { return false };
    match pb::SsoProtocol::try_from(p.protocol).unwrap_or(pb::SsoProtocol::Unspecified) {
        pb::SsoProtocol::Oidc => p.oidc.as_ref().is_some_and(|o| {
            !o.issuer.trim().is_empty()
                && !o.client_id.trim().is_empty()
                && (!o.client_secret.is_empty() || o.client_secret_set)
        }),
        pb::SsoProtocol::Saml => p.saml.as_ref().is_some_and(|s| {
            !s.entity_id.trim().is_empty()
                && !s.sso_url.trim().is_empty()
                && s.certificates.contains("BEGIN CERTIFICATE")
        }),
        pb::SsoProtocol::Unspecified => false,
    }
}

impl Core {
    /// Who may come in: Browse, applications, waifu.dev only, account age.
    pub async fn update_access(&self, key: &str, server_id: &str, patch: AccessPatch) -> Result<pb::Server, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.servers(),
            update_server(pb::UpdateServerRequest {
                server_id: server_id.into(),
                discoverable: patch.discoverable,
                applications: patch.applications,
                linked_only: patch.linked_only,
                min_account_age_seconds: patch.min_account_age_seconds,
                ..Default::default()
            })
        )
        .await?;
        let server = res.server.unwrap_or_default();
        self.keep_server(key, &server);
        Ok(server)
    }

    fn keep_server(&self, key: &str, server: &pb::Server) {
        self.shared.instance(key, |i| {
            if let Some(s) = i.servers.iter_mut().find(|s| s.id == server.id) {
                *s = server.clone();
            }
        });
    }

    /// The rules new members agree to and the questions applicants answer.
    pub async fn get_join_form(&self, key: &str, server_id: &str) -> Result<pb::JoinForm, Problem> {
        self.join_form(key, server_id, "").await
    }

    pub async fn set_join_form(&self, key: &str, server_id: &str, form: pb::JoinForm) -> Result<pb::JoinForm, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res =
            rpc!(api.join(), set_join_form(pb::SetJoinFormRequest { server_id: server_id.into(), form: Some(form) }))
                .await?;
        Ok(res.form.unwrap_or_default())
    }

    /// People asking to join, oldest first.
    pub async fn list_applications(&self, key: &str, server_id: &str) -> Result<Vec<pb::Application>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res =
            rpc!(api.join(), list_applications(pb::ListApplicationsRequest { server_id: server_id.into() })).await?;
        Ok(res.applications)
    }

    /// Lets someone in, or turns them down with a reason they'll see.
    pub async fn review_application(
        &self,
        key: &str,
        server_id: &str,
        user_id: &str,
        approve: bool,
        reason: &str,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.join(),
            review_application(pb::ReviewApplicationRequest {
                server_id: server_id.into(),
                user_id: user_id.into(),
                approve,
                reason: reason.into(),
            })
        )
        .await?;
        Ok(())
    }

    /// The server's single sign-on, and your own sign-in through it.
    pub async fn get_server_sso(
        &self,
        key: &str,
        server_id: &str,
    ) -> Result<(pb::ServerSso, Option<pb::SsoIdentity>), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(api.sso(), get_server_sso(pb::GetServerSsoRequest { server_id: server_id.into() })).await?;
        Ok((res.sso.unwrap_or_default(), res.mine))
    }

    pub async fn update_server_sso(
        &self,
        key: &str,
        req: pb::UpdateServerSsoRequest,
    ) -> Result<pb::ServerSso, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(api.sso(), update_server_sso(req)).await?;
        Ok(res.sso.unwrap_or_default())
    }

    /// What the server holds, its caps and its own caps.
    pub async fn server_usage(&self, key: &str, server_id: &str) -> Result<pb::GetServerUsageResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(api.servers(), get_server_usage(pb::GetServerUsageRequest { server_id: server_id.into() })).await
    }

    /// Someone else's nickname in the server (empty clears it), with Manage Nicknames.
    pub async fn set_member_nickname(
        &self,
        key: &str,
        server_id: &str,
        user_id: &str,
        nickname: &str,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.servers(),
            update_member(pb::UpdateMemberRequest {
                server_id: server_id.into(),
                user_id: user_id.into(),
                nickname: Some(nickname.into()),
            })
        )
        .await?;
        if let Some(member) = res.member {
            self.put_member(key, server_id, member);
        }
        Ok(())
    }

    /// Hands the server to another member; you stay on as an admin.
    pub async fn transfer_ownership(&self, key: &str, server_id: &str, user_id: &str) -> Result<pb::Server, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.servers(),
            transfer_ownership(pb::TransferOwnershipRequest { server_id: server_id.into(), user_id: user_id.into() })
        )
        .await?;
        let server = res.server.unwrap_or_default();
        self.keep_server(key, &server);
        Ok(server)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_drop_blanks_and_count_changes() {
        let q = |p: &str| pb::JoinQuestion { prompt: p.into(), paragraph: false, required: true };
        let draft = cleaned_form(&[" Be kind ".into(), "  ".into()], &[q(" Why? "), q("")]);
        assert_eq!(draft.rules, vec!["Be kind"]);
        assert_eq!(draft.questions, vec![q("Why?")]);
        let saved = pb::JoinForm { rules: vec!["Be kind".into()], questions: vec![] };
        assert_eq!(form_changes(&draft, &saved), 1);
        assert_eq!(form_changes(&saved, &saved), 0);
    }

    #[test]
    fn starters_show_only_while_rules_are_starters() {
        let starters = vec!["A".to_owned(), "B".to_owned()];
        assert_eq!(ideas(&[], &starters).len(), 2);
        assert_eq!(ideas(&["A".into()], &starters), vec![&"B".to_owned()]);
        assert!(ideas(&["Mine".into()], &starters).is_empty());
    }
}
