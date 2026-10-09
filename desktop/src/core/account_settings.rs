//! The account pages' calls, as the web's `fuwa/actions.ts` makes them:
//! two-step sign-in and backup codes, ways to sign in (linking Google, X or
//! Twitch through the browser), agents, a server profile's nickname,
//! downloading your data and deleting your account.

use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tonic::Code;

use crate::core::Core;
use crate::core::api::Problem;
use crate::core::{linked, vault};
use crate::pb;
use crate::rpc;

fn missing() -> Problem {
    Problem::new(Code::NotFound, "That instance isn't here.")
}

/// How long one piece of an export may take to come.
const EXPORT_WAIT: Duration = Duration::from_secs(60);

impl Core {
    pub async fn two_factor(&self, key: &str) -> Result<pb::GetTwoFactorResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(api.account(), get_two_factor(pb::GetTwoFactorRequest {})).await
    }

    /// Starts setting two-step sign-in up: a secret for the authenticator app.
    pub async fn set_up_two_factor(&self, key: &str, password: &str) -> Result<pb::SetUpTwoFactorResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(api.account(), set_up_two_factor(pb::SetUpTwoFactorRequest { password: password.into() })).await
    }

    /// Turns two-step sign-in on with a code from the app; returns the backup codes.
    pub async fn enable_two_factor(&self, key: &str, code: &str) -> Result<Vec<String>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        Ok(rpc!(api.account(), enable_two_factor(pb::EnableTwoFactorRequest { code: code.into() })).await?.backup_codes)
    }

    pub async fn disable_two_factor(&self, key: &str, password: &str, code: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.account(),
            disable_two_factor(pb::DisableTwoFactorRequest { password: password.into(), code: code.into() })
        )
        .await?;
        Ok(())
    }

    pub async fn new_backup_codes(&self, key: &str, password: &str) -> Result<Vec<String>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        Ok(rpc!(api.account(), regenerate_backup_codes(pb::RegenerateBackupCodesRequest { password: password.into() }))
            .await?
            .backup_codes)
    }

    pub async fn sign_in_methods(&self, key: &str) -> Result<pb::ListSignInMethodsResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(api.account(), list_sign_in_methods(pb::ListSignInMethodsRequest {})).await
    }

    pub async fn unlink_provider(&self, key: &str, provider: &str, password: &str, code: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.account(),
            unlink_provider(pb::UnlinkProviderRequest {
                provider: provider.into(),
                password: password.into(),
                code: code.into()
            })
        )
        .await?;
        Ok(())
    }

    /// Links Google, X or Twitch to your account: the provider's page opens in the browser,
    /// which comes back to this computer, and the instance keeps the link.
    pub async fn link_provider(
        self: &Arc<Self>,
        key: &str,
        provider: &str,
        password: &str,
        code: &str,
        open_page: impl FnOnce(&str) + Send,
    ) -> Result<pb::SignInMethod, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let callback = linked::Callback::listen().await.map_err(|e| Problem::new(Code::Internal, e.to_string()))?;
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| Problem::new(Code::Internal, e.to_string()))?;
        let secret = vault::sha256_hex(&bytes);
        let secret_hash = vault::sha256_hex(secret.as_bytes());
        let started = rpc!(
            api.account(),
            start_provider_link(pb::StartProviderLinkRequest {
                provider: provider.into(),
                return_origin: callback.origin.clone(),
                secret_hash,
                password: password.into(),
                code: code.into(),
            })
        )
        .await?;
        if !linked::safe_sign_in_page(&started.authorize_url) {
            return Err(Problem::new(
                Code::PermissionDenied,
                "The instance gave a sign-in page that isn't https, so fuwa won't open it.",
            ));
        }
        open_page(&started.authorize_url);
        let (state, code) = match callback.wait(&started.state).await {
            linked::Returned::Code { code, state } => (state, code),
            linked::Returned::Failed(message) => return Err(Problem::new(Code::Cancelled, message)),
        };
        let res =
            rpc!(api.account(), finish_provider_link(pb::FinishProviderLinkRequest { state, code, secret })).await?;
        Ok(res.method.unwrap_or_default())
    }

    pub async fn agents(&self, key: &str) -> Result<Vec<pb::Agent>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        Ok(rpc!(api.agents(), list_agents(pb::ListAgentsRequest {})).await?.agents)
    }

    /// Makes an agent; its token comes back once.
    pub async fn create_agent(&self, key: &str, username: &str, name: &str) -> Result<(pb::Agent, String), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.agents(),
            create_agent(pb::CreateAgentRequest { username: username.into(), display_name: name.into() })
        )
        .await?;
        Ok((res.agent.unwrap_or_default(), res.token))
    }

    pub async fn update_agent(&self, key: &str, req: pb::UpdateAgentRequest) -> Result<pb::Agent, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        Ok(rpc!(api.agents(), update_agent(req)).await?.agent.unwrap_or_default())
    }

    /// A new token for an agent; the old one stops working.
    pub async fn reset_agent_token(&self, key: &str, agent_id: &str) -> Result<String, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        Ok(rpc!(api.agents(), reset_agent_token(pb::ResetAgentTokenRequest { agent_id: agent_id.into() })).await?.token)
    }

    pub async fn delete_agent(&self, key: &str, agent_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(api.agents(), delete_agent(pb::DeleteAgentRequest { agent_id: agent_id.into() })).await?;
        Ok(())
    }

    /// An agent's endpoint (docs/agent-endpoints.md), made with its secret the first time it's asked for.
    pub async fn agent_endpoint(&self, key: &str, agent_id: &str) -> Result<pb::AgentEndpoint, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res =
            rpc!(api.agents(), get_agent_endpoint(pb::GetAgentEndpointRequest { agent_id: agent_id.into() })).await?;
        Ok(res.endpoint.unwrap_or_default())
    }

    /// Sets where an agent's events go (an empty URL turns it off) and which ones; the
    /// instance checks the URL answers its challenge before it saves.
    pub async fn set_agent_endpoint(
        &self,
        key: &str,
        agent_id: &str,
        url: &str,
        events: Vec<String>,
    ) -> Result<pb::AgentEndpoint, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.agents(),
            set_agent_endpoint(pb::SetAgentEndpointRequest { agent_id: agent_id.into(), url: url.into(), events })
        )
        .await?;
        Ok(res.endpoint.unwrap_or_default())
    }

    /// A new signing secret for an agent's endpoint; the old one stops being used at once.
    pub async fn reset_agent_endpoint_secret(&self, key: &str, agent_id: &str) -> Result<pb::AgentEndpoint, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.agents(),
            reset_agent_endpoint_secret(pb::ResetAgentEndpointSecretRequest { agent_id: agent_id.into() })
        )
        .await?;
        Ok(res.endpoint.unwrap_or_default())
    }

    /// Your nickname in a server (empty clears it).
    pub async fn set_nickname(&self, key: &str, server_id: &str, nickname: &str) -> Result<pb::Member, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let me = self.shared.read(|s| s.instance(key).and_then(|i| i.me.as_ref().map(|m| m.id.clone())));
        let member = rpc!(
            api.servers(),
            update_member(pb::UpdateMemberRequest {
                server_id: server_id.into(),
                user_id: me.unwrap_or_default(),
                nickname: Some(nickname.into()),
                ..Default::default()
            })
        )
        .await?
        .member
        .unwrap_or_default();
        self.shared.instance(key, |i| {
            if let Some(list) = i.members.get_mut(server_id)
                && let Some(m) =
                    list.iter_mut().find(|m| m.user.as_ref().map(|u| &u.id) == member.user.as_ref().map(|u| &u.id))
            {
                m.nickname = member.nickname.clone();
            }
        });
        Ok(member)
    }

    /// Everything the instance keeps about you, as one JSON file at `path` (written beside it
    /// first, put in place once whole). `progress` hears the bytes so far.
    pub async fn export_data(&self, key: &str, path: &Path, progress: impl Fn(u64) + Send) -> Result<u64, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let mut stream = tokio::time::timeout(EXPORT_WAIT, api.account().export_data(pb::ExportDataRequest {}))
            .await
            .map_err(|_| Problem::new(Code::DeadlineExceeded, "The instance took too long to answer."))?
            .map_err(Problem::from)?
            .into_inner();
        let fail = |e: std::io::Error| Problem::new(Code::Internal, format!("Couldn't save the file: {e}"));
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(".part");
        let part = path.with_file_name(name);
        let mut file = std::fs::File::create(&part).map_err(fail)?;
        let mut bytes = 0u64;
        let result = async {
            loop {
                let next = tokio::time::timeout(EXPORT_WAIT, stream.message())
                    .await
                    .map_err(|_| Problem::new(Code::DeadlineExceeded, "The instance stopped sending the file."))?
                    .map_err(Problem::from)?;
                let Some(piece) = next else { break };
                file.write_all(&piece.chunk).map_err(fail)?;
                bytes += piece.chunk.len() as u64;
                progress(bytes);
            }
            file.flush().map_err(fail)?;
            Ok::<_, Problem>(())
        }
        .await;
        drop(file);
        match result {
            Ok(()) => {
                std::fs::rename(&part, path).map_err(fail)?;
                Ok(bytes)
            }
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                Err(e)
            }
        }
    }

    /// Deletes your account: with the password (and a code), or for an account without one,
    /// the username typed out. Signs this computer out of the instance after.
    pub async fn delete_account(&self, key: &str, password: &str, code: &str, username: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.account(),
            delete_account(pb::DeleteAccountRequest {
                password: password.into(),
                code: code.into(),
                username: username.into()
            })
        )
        .await?;
        Ok(())
    }
}

/// Every event an endpoint can ask for: the fields of `Event.payload`, read
/// from the protocol this app was built with (the web takes them from its
/// generated `EventSchema`), in their order there.
pub static EVENT_NAMES: std::sync::LazyLock<Vec<String>> =
    std::sync::LazyLock::new(|| payload_names(include_str!("../../../proto/fuwa/v1/types.proto")));

/// The field names of `Event`'s `payload` oneof in a types.proto.
fn payload_names(proto: &str) -> Vec<String> {
    let Some(start) = proto.find("\nmessage Event {") else { return Vec::new() };
    let rest = &proto[start..];
    let Some(oneof) = rest.find("oneof payload {") else { return Vec::new() };
    let body = &rest[oneof + "oneof payload {".len()..];
    let body = &body[..body.find('}').unwrap_or(body.len())];
    body.lines()
        .filter_map(|l| {
            let l = l.split("//").next()?.trim();
            let mut words = l.split_whitespace();
            let (_, name, eq) = (words.next()?, words.next()?, words.next()?);
            (eq == "=").then(|| name.to_owned())
        })
        .collect()
}

/// Events the instance never stores, so it never posts them to an endpoint.
pub const UNDELIVERED: [&str; 4] =
    ["voice_state_updated", "voice_state_removed", "live_tile_updated", "live_tile_ended"];

/// The groups events are offered in, in this order (the web's `lib/agent-events.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventGroup {
    Messages,
    Interactions,
    Members,
    Reactions,
    Other,
}

pub const EVENT_GROUPS: [EventGroup; 5] =
    [EventGroup::Messages, EventGroup::Interactions, EventGroup::Members, EventGroup::Reactions, EventGroup::Other];

/// The group an event goes under: anything not named here is `Other`.
pub fn group_of(name: &str) -> EventGroup {
    match name {
        "message_created" | "message_updated" | "message_deleted" | "message_pinned" | "thread_updated"
        | "poll_updated" => EventGroup::Messages,
        "interaction_created" => EventGroup::Interactions,
        "member_joined" | "member_left" | "member_updated" => EventGroup::Members,
        "reaction_updated" | "reactions_cleared" => EventGroup::Reactions,
        _ => EventGroup::Other,
    }
}

/// The names worth offering: what an endpoint can get, plus any already chosen (so they can be taken off).
pub fn offered(names: &[String], chosen: &[String]) -> Vec<String> {
    let mut out: Vec<String> = names.iter().filter(|n| !UNDELIVERED.contains(&n.as_str())).cloned().collect();
    for c in chosen {
        if !out.contains(c) {
            out.push(c.clone());
        }
    }
    out
}

/// The names in their groups, in `EVENT_GROUPS` order, leaving out empty groups; names keep their order.
pub fn grouped(names: &[String]) -> Vec<(EventGroup, Vec<String>)> {
    EVENT_GROUPS
        .iter()
        .map(|g| (*g, names.iter().filter(|n| group_of(n) == *g).cloned().collect::<Vec<_>>()))
        .filter(|(_, n)| !n.is_empty())
        .collect()
}

/// A name as words, for events without a label of their own: "shared_channels_updated" → "Shared channels updated".
pub fn readable(name: &str) -> String {
    let words = name.split('_').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    let mut chars = words.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// Adds the name, or takes it off when it's there.
pub fn toggle_event(chosen: &[String], name: &str) -> Vec<String> {
    if chosen.iter().any(|c| c == name) {
        chosen.iter().filter(|c| *c != name).cloned().collect()
    } else {
        chosen.iter().cloned().chain([name.to_owned()]).collect()
    }
}

/// Adds every name of a group, or takes them all off when they're all there already.
pub fn toggle_all_events(chosen: &[String], names: &[String]) -> Vec<String> {
    if names.iter().all(|n| chosen.contains(n)) {
        chosen.iter().filter(|c| !names.contains(c)).cloned().collect()
    } else {
        chosen.iter().cloned().chain(names.iter().filter(|n| !chosen.contains(n)).cloned()).collect()
    }
}

/// Whether two choices are the same events, in any order.
pub fn same_events(a: &[String], b: &[String]) -> bool {
    let (a, b): (std::collections::BTreeSet<_>, std::collections::BTreeSet<_>) =
        (a.iter().collect(), b.iter().collect());
    a == b
}

/// How an agent's endpoint is doing, from what the instance says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointStatus {
    /// No URL.
    Off,
    /// A URL, and nothing delivered yet.
    Waiting,
    /// Deliveries go through; the last one at this time.
    Delivered(i64),
    /// Deliveries have been failing since then, the last for this reason.
    Failing(i64, String),
    /// The instance turned it off after a day of failures, the last for this reason.
    Disabled(String),
}

fn ms(t: Option<&prost_types::Timestamp>) -> i64 {
    t.map_or(0, |t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000)
}

pub fn endpoint_status(e: &pb::AgentEndpoint) -> EndpointStatus {
    if e.disabled_at.is_some() {
        EndpointStatus::Disabled(e.last_error.clone())
    } else if e.url.is_empty() {
        EndpointStatus::Off
    } else if e.failing_since.is_some() {
        EndpointStatus::Failing(ms(e.failing_since.as_ref()), e.last_error.clone())
    } else if e.last_delivered_at.is_some() {
        EndpointStatus::Delivered(ms(e.last_delivered_at.as_ref()))
    } else {
        EndpointStatus::Waiting
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn events_come_from_the_protocol() {
        assert!(EVENT_NAMES.contains(&"message_created".to_owned()));
        assert!(EVENT_NAMES.contains(&"interaction_created".to_owned()));
        assert!(EVENT_NAMES.contains(&"voice_state_updated".to_owned()));
        let offered = offered(&EVENT_NAMES, &[]);
        assert!(!offered.iter().any(|n| n.starts_with("voice_state") || n.starts_with("live_tile")));
        assert_eq!(offered.len(), EVENT_NAMES.len() - UNDELIVERED.len());
        assert_eq!(
            payload_names(
                "\nmessage Event {\n  string id = 1;\n  oneof payload {\n    A a_b = 2; // x\n    C c = 3;\n  }\n}"
            ),
            ["a_b", "c"]
        );
    }

    #[test]
    fn events_group_and_toggle_like_the_web() {
        let all = names(&["message_created", "member_joined", "role_created", "interaction_created"]);
        let groups = grouped(&offered(&all, &names(&["future_thing"])));
        assert_eq!(
            groups.iter().map(|(g, _)| *g).collect::<Vec<_>>(),
            [EventGroup::Messages, EventGroup::Interactions, EventGroup::Members, EventGroup::Other]
        );
        assert_eq!(groups[3].1, ["role_created", "future_thing"]);
        assert_eq!(readable("shared_channels_updated"), "Shared channels updated");
        let chosen = toggle_event(&[], "member_left");
        assert_eq!(chosen, ["member_left"]);
        assert!(toggle_event(&chosen, "member_left").is_empty());
        let group = names(&["member_joined", "member_left"]);
        assert_eq!(toggle_all_events(&chosen, &group), ["member_left", "member_joined"]);
        assert!(toggle_all_events(&group, &group).is_empty());
        assert!(same_events(&names(&["a", "b"]), &names(&["b", "a"])));
        assert!(!same_events(&names(&["a"]), &names(&["a", "b"])));
    }

    #[test]
    fn endpoint_status_reads_the_instance() {
        let at = |s| Some(prost_types::Timestamp { seconds: s, nanos: 0 });
        let mut e = pb::AgentEndpoint::default();
        assert_eq!(endpoint_status(&e), EndpointStatus::Off);
        e.url = "https://agent.example.com/".into();
        assert_eq!(endpoint_status(&e), EndpointStatus::Waiting);
        e.last_delivered_at = at(10);
        assert_eq!(endpoint_status(&e), EndpointStatus::Delivered(10_000));
        e.failing_since = at(20);
        e.last_error = "answered 500".into();
        assert_eq!(endpoint_status(&e), EndpointStatus::Failing(20_000, "answered 500".into()));
        e.disabled_at = at(30);
        assert_eq!(endpoint_status(&e), EndpointStatus::Disabled("answered 500".into()));
    }
}
