//! Finding and joining servers: Browse, invites from any instance, joining,
//! and applying to servers that let people in by hand, with the
//! applications you're waiting on kept on this computer so they wait in the
//! rail. A port of the web app's `discover`, `joinServer`, `applyToJoin`,
//! `withdrawApplication`, `checkApplied` and `lookUpInvite` (fuwa/actions.ts),
//! `lib/applied.ts` and `lib/invites.ts`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use prost::Message as _;
use serde::{Deserialize, Serialize};

use crate::core::api::{Api, Problem, instance_key, normalize_url};
use crate::core::store::{self, InstanceState};
use crate::core::{Core, reports};
use crate::pb;
use crate::rpc;

/// A server you applied to and aren't in yet: it waits in the rail with an
/// hourglass until someone lets you in, or says it was turned down.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub server: pb::Server,
    /// PENDING or REJECTED.
    pub status: pb::ApplicationStatus,
    /// Why it was turned down, when they said.
    pub reason: String,
    /// Unix milliseconds.
    pub applied_at: i64,
    /// The invite they applied with, to apply again with if it's turned down.
    pub invite_code: String,
}

impl Applied {
    pub fn waiting(&self) -> bool {
        self.status == pb::ApplicationStatus::Pending
    }
}

/// How an application is kept on disk: the server as protobuf, in base64.
#[derive(Serialize, Deserialize)]
struct Stored {
    server: String,
    status: i32,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    applied_at: i64,
    #[serde(default)]
    invite_code: String,
}

fn stored(a: &Applied) -> Stored {
    Stored {
        server: base64::engine::general_purpose::STANDARD.encode(a.server.encode_to_vec()),
        status: a.status as i32,
        reason: a.reason.clone(),
        applied_at: a.applied_at,
        invite_code: a.invite_code.clone(),
    }
}

fn from_stored(s: Stored) -> Option<Applied> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(s.server).ok()?;
    let server = pb::Server::decode(bytes.as_slice()).ok()?;
    if server.id.is_empty() {
        return None;
    }
    let status = if s.status == pb::ApplicationStatus::Rejected as i32 {
        pb::ApplicationStatus::Rejected
    } else {
        pb::ApplicationStatus::Pending
    };
    Some(Applied { server, status, reason: s.reason, applied_at: s.applied_at, invite_code: s.invite_code })
}

/// Reads a saved list back; anything that doesn't parse is left out.
pub fn parse_applied(text: &str) -> HashMap<String, Applied> {
    serde_json::from_str::<Vec<Stored>>(text)
        .unwrap_or_default()
        .into_iter()
        .filter_map(from_stored)
        .map(|a| (a.server.id.clone(), a))
        .collect()
}

/// The list as it's saved, oldest application first so the file reads the same each time.
pub fn write_applied(applied: &HashMap<String, Applied>) -> String {
    let mut list: Vec<&Applied> = applied.values().collect();
    list.sort_by(|a, b| a.applied_at.cmp(&b.applied_at).then_with(|| a.server.id.cmp(&b.server.id)));
    serde_json::to_string(&list.into_iter().map(stored).collect::<Vec<_>>()).unwrap_or_else(|_| "[]".into())
}

/// Which way in a server's button offers, first match wins (the web's `kindOf`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Joined,
    Open,
    LinkedOnly,
    Waiting,
    Sso,
    Apply,
    Join,
}

/// What decides the button.
#[derive(Debug, Clone, Copy, Default)]
pub struct At {
    pub joined: bool,
    pub member: bool,
    pub linked_only: bool,
    pub waiting: bool,
    pub sso: bool,
    pub applications: bool,
}

pub fn kind_of(at: At) -> Kind {
    if at.joined {
        Kind::Joined
    } else if at.member {
        Kind::Open
    } else if at.linked_only {
        Kind::LinkedOnly
    } else if at.waiting {
        Kind::Waiting
    } else if at.sso {
        Kind::Sso
    } else if at.applications {
        Kind::Apply
    } else {
        Kind::Join
    }
}

/// The button for a server on an instance, as `useJoinKind` works it out.
/// `signed_in` is whether you signed in through its provider lately.
pub fn kind_for(i: &InstanceState, server: &pb::Server, joined: bool, signed_in: bool) -> Kind {
    let me = i.me.as_ref();
    kind_of(At {
        joined,
        member: i.servers.iter().any(|s| s.id == server.id),
        linked_only: server.linked_only && me.is_some_and(|m| m.kind != pb::AccountKind::Linked as i32),
        waiting: i.applied.as_ref().and_then(|a| a.get(&server.id)).is_some_and(Applied::waiting),
        sso: server.sso_required && !signed_in,
        applications: server.applications,
    })
}

/// What someone pasted as an invite: a link from any instance, or just a
/// code, which then belongs to the instance they're looking at (`here`).
/// Gives the instance's key and the code.
pub fn parse_invite(text: &str, here: &str) -> Option<(String, String)> {
    let pasted = text.trim();
    let pasted = pasted.split(['?', '#']).next().unwrap_or_default().trim_end_matches('/');
    let code_ok = |c: &str| !c.is_empty() && c.len() <= 32 && c.chars().all(|ch| ch.is_ascii_alphanumeric());
    if code_ok(pasted) {
        return Some((here.to_owned(), pasted.to_owned()));
    }
    let at = pasted.rfind("/invite/")?;
    if at == 0 {
        return None;
    }
    let code = &pasted[at + "/invite/".len()..];
    if !code_ok(code) {
        return None;
    }
    let url = normalize_url(&pasted[..at]).ok()?;
    Some((instance_key(&url), code.to_owned()))
}

/// An instance's address from its key ("chat.example.com~fuwa" is chat.example.com/fuwa).
pub fn address_of(key: &str) -> String {
    key.replace('~', "/")
}

/// Where an invite leads, on an instance you may not be signed in to yet.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub url: String,
    pub node: pb::Node,
    pub invite: pb::Invite,
    pub server: pb::Server,
    pub channel_name: String,
    pub inviter: Option<pb::User>,
}

/// The options of a new server.
#[derive(Debug, Clone, Default)]
pub struct NewServer {
    pub name: String,
    pub description: String,
    pub discoverable: bool,
    pub icon_url: String,
    /// One of the instance's regions, empty for its home one.
    pub region: String,
}

/// What asking about your applications found.
#[derive(Debug, Default)]
pub struct Checked {
    pub let_in: Vec<pb::Server>,
    pub turned_down: Vec<pb::Server>,
}

fn missing() -> Problem {
    Problem::new(tonic::Code::NotFound, "That instance isn't here.")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl Core {
    /// Where an account's applications are kept on this computer.
    fn applied_file(&self, key: &str, user_id: &str) -> PathBuf {
        let name: String = format!("{key}|{user_id}")
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' })
            .collect();
        self.paths.config.join("applied").join(format!("{name}.json"))
    }

    /// Reads the applications you're waiting on into the instance's state,
    /// once you're signed in there. Does nothing once they're read.
    pub fn load_applied(&self, key: &str) {
        let Some(user) =
            self.shared.read(|s| s.instance(key).filter(|i| i.applied.is_none()).and_then(|i| i.me.clone()))
        else {
            return;
        };
        let text = std::fs::read_to_string(self.applied_file(key, &user.id)).unwrap_or_default();
        let applied = parse_applied(&text);
        self.shared.instance(key, |i| {
            if i.applied.is_none() {
                i.applied = Some(applied);
            }
        });
    }

    /// Changes which servers you're waiting on, here and on disk.
    pub fn set_applied(&self, key: &str, server_id: &str, applied: Option<Applied>) {
        let saved = self.shared.instance(key, |i| {
            let list = i.applied.get_or_insert_with(HashMap::new);
            match applied {
                Some(a) => list.insert(server_id.to_owned(), a),
                None => list.remove(server_id),
            };
            i.me.as_ref().map(|m| (m.id.clone(), write_applied(list), list.is_empty()))
        });
        if let Some(Some((user_id, text, empty))) = saved {
            let path = self.applied_file(key, &user_id);
            if empty {
                let _ = std::fs::remove_file(path);
            } else {
                let _ = crate::core::vault::write_file(&path, text.as_bytes());
            }
        }
    }

    /// The servers in the instance's Browse.
    pub async fn discover(&self, key: &str) -> Result<Vec<pb::Server>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        Ok(rpc!(api.servers(), discover_servers(pb::DiscoverServersRequest {})).await?.servers)
    }

    /// Makes a server, with all the web form's choices.
    pub async fn create_server_with(&self, key: &str, new: NewServer) -> Result<pb::Server, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let server = rpc!(
            api.servers(),
            create_server(pb::CreateServerRequest {
                name: new.name,
                description: new.description,
                icon_url: new.icon_url,
                discoverable: new.discoverable,
                region: new.region,
            })
        )
        .await?
        .server
        .unwrap_or_default();
        self.joined(key, server.clone());
        Ok(server)
    }

    /// Joins a server from Browse, or with one of its invites.
    pub async fn join_server(&self, key: &str, server_id: &str, invite_code: &str) -> Result<pb::Server, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        reports::used(if invite_code.is_empty() { "server.join" } else { "server.join_invite" });
        let server = rpc!(
            api.servers(),
            join_server(pb::JoinServerRequest { server_id: server_id.into(), invite_code: invite_code.into() })
        )
        .await?
        .server
        .unwrap_or_default();
        self.joined(key, server.clone());
        Ok(server)
    }

    /// A server is yours now: in the list, and followed.
    fn joined(&self, key: &str, server: pb::Server) {
        let id = server.id.clone();
        self.shared.instance(key, |i| store::add_server(i, server));
        self.follow(key, &id, true);
    }

    /// A server's rules and questions; `invite_code` for one out of Browse.
    pub async fn join_form(&self, key: &str, server_id: &str, invite_code: &str) -> Result<pb::JoinForm, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        Ok(rpc!(
            api.join(),
            get_join_form(pb::GetJoinFormRequest { server_id: server_id.into(), invite_code: invite_code.into() })
        )
        .await?
        .form
        .unwrap_or_default())
    }

    /// Asks to join a server that lets people in by hand, answering its questions as shown.
    pub async fn apply_to_join(
        &self,
        key: &str,
        server: &pb::Server,
        invite_code: &str,
        answers: Vec<(String, String)>,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.join(),
            apply_to_join(pb::ApplyToJoinRequest {
                server_id: server.id.clone(),
                invite_code: invite_code.into(),
                answers: answers
                    .into_iter()
                    .map(|(question, answer)| pb::ApplicationAnswer { question, answer })
                    .collect(),
            })
        )
        .await?;
        self.set_applied(
            key,
            &server.id,
            Some(Applied {
                server: server.clone(),
                status: pb::ApplicationStatus::Pending,
                reason: String::new(),
                applied_at: now_ms(),
                invite_code: invite_code.into(),
            }),
        );
        Ok(())
    }

    /// Takes an application back.
    pub async fn withdraw_application(&self, key: &str, server_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        match rpc!(api.join(), withdraw_application(pb::WithdrawApplicationRequest { server_id: server_id.into() }))
            .await
        {
            // Already gone on the server: forgetting it here is all that's left.
            Ok(_) => {}
            Err(e) if e.code == tonic::Code::NotFound => {}
            Err(e) => return Err(e),
        }
        self.set_applied(key, server_id, None);
        Ok(())
    }

    /// Asks how each waiting application went: the ones that were let in
    /// become servers, turned-down ones say why, and ones that are gone go.
    pub async fn check_applied(&self, key: &str) -> Checked {
        let mut out = Checked::default();
        let Some(api) = self.api(key) else { return out };
        let waiting: Vec<Applied> = self.shared.read(|s| {
            s.instance(key)
                .and_then(|i| i.applied.as_ref())
                .map(|a| a.values().filter(|a| a.waiting()).cloned().collect())
                .unwrap_or_default()
        });
        for a in waiting {
            let id = a.server.id.clone();
            let found =
                match rpc!(api.join(), get_application(pb::GetApplicationRequest { server_id: id.clone() })).await {
                    Ok(r) => Some(r),
                    Err(e) if e.code == tonic::Code::NotFound => None,
                    // Couldn't reach it: asked again next time.
                    Err(_) => continue,
                };
            match found {
                Some(pb::GetApplicationResponse { member: true, server: Some(server), .. }) => {
                    self.set_applied(key, &id, None);
                    self.joined(key, server.clone());
                    out.let_in.push(server);
                }
                Some(pb::GetApplicationResponse { application: Some(app), .. })
                    if app.status == pb::ApplicationStatus::Rejected as i32 =>
                {
                    let server = a.server.clone();
                    self.set_applied(
                        key,
                        &id,
                        Some(Applied { status: pb::ApplicationStatus::Rejected, reason: app.reason, ..a }),
                    );
                    out.turned_down.push(server);
                }
                Some(pb::GetApplicationResponse { application: Some(_), .. }) => {}
                _ => self.set_applied(key, &id, None),
            }
        }
        out
    }

    /// Where an invite leads, asked of its instance without signing in.
    pub async fn look_up_invite(self: &Arc<Self>, address: &str, code: &str) -> Result<Found, Problem> {
        let (url, node) = self.probe(address).await?;
        let api = Api::new(&url, None).map_err(|e| Problem::new(tonic::Code::InvalidArgument, e.to_string()))?;
        let found = rpc!(api.invites(), get_invite(pb::GetInviteRequest { code: code.into() })).await?;
        Ok(Found {
            url,
            node,
            invite: found.invite.unwrap_or_default(),
            server: found.server.unwrap_or_default(),
            channel_name: found.channel_name,
            inviter: found.inviter,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invites_from_codes_and_links() {
        assert_eq!(parse_invite("  hTKzmak ", "here"), Some(("here".into(), "hTKzmak".into())));
        assert_eq!(
            parse_invite("https://chat.example.com/invite/abc123/", "here"),
            Some(("chat.example.com".into(), "abc123".into()))
        );
        assert_eq!(
            parse_invite("http://127.0.0.1:18080/invite/Xy9?ref=1", "here"),
            Some(("127.0.0.1:18080".into(), "Xy9".into()))
        );
        assert_eq!(
            parse_invite("https://example.com/fuwa/invite/abc", "here"),
            Some(("example.com~fuwa".into(), "abc".into()))
        );
        assert_eq!(parse_invite("https://chat.example.com/invite/", "here"), None);
        assert_eq!(parse_invite("not an invite", "here"), None);
        assert_eq!(parse_invite("/invite/abc", "here"), None);
        assert_eq!(parse_invite("https://x.com/invite/a-b", "here"), None);
    }

    #[test]
    fn first_match_wins() {
        let at = At { member: true, waiting: true, applications: true, ..At::default() };
        assert_eq!(kind_of(at), Kind::Open);
        assert_eq!(kind_of(At { joined: true, ..at }), Kind::Joined);
        assert_eq!(kind_of(At { linked_only: true, waiting: true, ..At::default() }), Kind::LinkedOnly);
        assert_eq!(kind_of(At { waiting: true, sso: true, ..At::default() }), Kind::Waiting);
        assert_eq!(kind_of(At { sso: true, applications: true, ..At::default() }), Kind::Sso);
        assert_eq!(kind_of(At { applications: true, ..At::default() }), Kind::Apply);
        assert_eq!(kind_of(At::default()), Kind::Join);
    }

    #[test]
    fn applications_round_trip() {
        let server = pb::Server { id: "s1".into(), name: "Art".into(), ..Default::default() };
        let a = Applied {
            server,
            status: pb::ApplicationStatus::Rejected,
            reason: "too new".into(),
            applied_at: 42,
            invite_code: "abc".into(),
        };
        let list = HashMap::from([("s1".to_owned(), a.clone())]);
        let back = parse_applied(&write_applied(&list));
        assert_eq!(back.get("s1"), Some(&a));
        assert!(parse_applied("not json").is_empty());
    }
}
