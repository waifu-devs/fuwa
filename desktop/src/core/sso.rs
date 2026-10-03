//! Single sign-on through an organization's identity provider (OpenID
//! Connect or SAML), for an instance's own sign-in or one community
//! server's, the desktop's side of `web/src/lib/sso.ts`.
//!
//! It runs like signing in with waifu.dev ([`linked`]): the app keeps a
//! secret, opens the provider's page in the browser and listens on this
//! computer; the instance checks who signed in and sends the browser to its
//! `/auth/sso/done` page, which hands the one-time code on to the app's
//! loopback address, and the app finishes with the secret.

use std::sync::Arc;

use crate::core::api::{Api, Problem};
use crate::core::store::sort_members;
use crate::core::{Core, linked, vault};
use crate::pb;
use crate::rpc;

/// What the browser hands back, once the provider and the instance are done.
struct Answer {
    state: String,
    code: String,
    secret: String,
}

/// A secret to keep and the hash the instance is told.
fn secret() -> Result<(String, String), Problem> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| Problem::new(tonic::Code::Internal, e.to_string()))?;
    let secret = vault::sha256_hex(&bytes);
    let hash = vault::sha256_hex(secret.as_bytes());
    Ok((secret, hash))
}

/// Opens the provider's page (only a real web page) and waits for the browser.
async fn round_trip(
    callback: linked::Callback,
    authorize_url: &str,
    state: &str,
    secret: String,
    open_page: impl FnOnce(&str) + Send,
) -> Result<Answer, Problem> {
    if !linked::safe_sign_in_page(authorize_url) {
        return Err(Problem::new(
            tonic::Code::PermissionDenied,
            "The instance gave a sign-in page that isn't https, so fuwa won't open it.",
        ));
    }
    open_page(authorize_url);
    match callback.wait(state).await {
        linked::Returned::Code { code, state } => Ok(Answer { state, code, secret }),
        linked::Returned::Failed(message) => Err(Problem::new(tonic::Code::Cancelled, message)),
    }
}

async fn listen() -> Result<linked::Callback, Problem> {
    linked::Callback::listen().await.map_err(|e| Problem::new(tonic::Code::Internal, e.to_string()))
}

impl Core {
    /// Signs in to an instance through its identity provider, in the
    /// browser. Returns the instance's key, and whether the account is new.
    pub async fn sso_sign_in(
        self: &Arc<Self>,
        url: &str,
        open_page: impl FnOnce(&str) + Send,
    ) -> Result<(String, bool), Problem> {
        let api = Api::new(url, None).map_err(|e| Problem::new(tonic::Code::InvalidArgument, e.to_string()))?;
        let callback = listen().await?;
        let (secret, secret_hash) = secret()?;
        let started = rpc!(
            api.auth(),
            start_sso_sign_in(pb::StartSsoSignInRequest {
                return_origin: callback.origin.clone(),
                secret_hash,
                test: false,
            })
        )
        .await?;
        let answer = round_trip(callback, &started.authorize_url, &started.state, secret, open_page).await?;
        let res = rpc!(
            api.auth(),
            finish_sso_sign_in(pb::FinishSsoSignInRequest {
                state: answer.state,
                code: answer.code,
                secret: answer.secret,
            })
        )
        .await?;
        Ok((self.add_instance(url, Some(res.token)), res.created))
    }

    /// Signs in through a community server's identity provider, in the
    /// browser: to join it (with the invite, for one out of Browse) or to
    /// see its channels again once a sign-in ran out. Returns your
    /// membership, if you're a member.
    pub async fn server_sso(
        self: &Arc<Self>,
        key: &str,
        server_id: &str,
        invite_code: Option<String>,
        open_page: impl FnOnce(&str) + Send,
    ) -> Result<Option<pb::Member>, Problem> {
        let api = self.api(key).ok_or_else(|| Problem::new(tonic::Code::NotFound, "That instance isn't here."))?;
        let callback = listen().await?;
        let (secret, secret_hash) = secret()?;
        let started = rpc!(
            api.sso(),
            start_server_sso(pb::StartServerSsoRequest {
                server_id: server_id.into(),
                return_origin: callback.origin.clone(),
                secret_hash,
                invite_code: invite_code.unwrap_or_default(),
            })
        )
        .await?;
        let answer = round_trip(callback, &started.authorize_url, &started.state, secret, open_page).await?;
        let res = rpc!(
            api.sso(),
            finish_server_sso(pb::FinishServerSsoRequest {
                server_id: server_id.into(),
                state: answer.state,
                code: answer.code,
                secret: answer.secret,
            })
        )
        .await?;
        // The channels come back through the event stream; the membership now.
        if let Some(member) = &res.member {
            self.shared.instance(key, |i| {
                let members = i.members.entry(server_id.to_owned()).or_default();
                let id = member.user.as_ref().map(|u| u.id.as_str());
                match members.iter_mut().find(|m| m.user.as_ref().map(|u| u.id.as_str()) == id) {
                    Some(m) => *m = member.clone(),
                    None => members.push(member.clone()),
                }
                sort_members(members);
            });
        }
        Ok(res.member)
    }
}

/// Days in milliseconds, for sign-ins that run out.
const DAY_MS: i64 = 86_400_000;

/// Whether you're kept out of a server's channels until you sign in through
/// its provider: it requires one, you aren't its owner or an agent, and your
/// sign-in is missing or older than its recheck. The instance decides; this
/// only says why the channels went away (`ssoLocked` in `web/src/lib/sso.ts`).
pub fn locked(server: &pb::Server, me: Option<&pb::Member>, now_ms: i64) -> bool {
    let Some(member) = me else { return false };
    let Some(user) = member.user.as_ref() else { return false };
    if !server.sso_required || user.id == server.owner_id || user.kind == pb::AccountKind::Agent as i32 {
        return false;
    }
    let at = member.sso_signed_in_at.as_ref().map(|t| t.seconds * 1000).unwrap_or(0);
    if at == 0 {
        return true;
    }
    server.sso_recheck_days > 0 && now_ms - at >= i64::from(server.sso_recheck_days) * DAY_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_only_without_a_fresh_sign_in() {
        let server = |required, days| pb::Server {
            owner_id: "owner".into(),
            sso_required: required,
            sso_recheck_days: days,
            ..Default::default()
        };
        let member = |id: &str, kind: pb::AccountKind, at_days_ago: Option<i64>| pb::Member {
            user: Some(pb::User { id: id.into(), kind: kind as i32, ..Default::default() }),
            sso_signed_in_at: at_days_ago
                .map(|d| prost_types::Timestamp { seconds: (100 * DAY_MS - d * DAY_MS) / 1000, nanos: 0 }),
            ..Default::default()
        };
        let now = 100 * DAY_MS;
        let person = pb::AccountKind::Local;
        assert!(!locked(&server(false, 0), Some(&member("a", person, None)), now));
        assert!(locked(&server(true, 0), Some(&member("a", person, None)), now));
        assert!(!locked(&server(true, 0), Some(&member("a", person, Some(400))), now), "0 days is forever");
        assert!(!locked(&server(true, 30), Some(&member("a", person, Some(29))), now));
        assert!(locked(&server(true, 30), Some(&member("a", person, Some(30))), now));
        assert!(!locked(&server(true, 30), Some(&member("owner", person, None)), now));
        assert!(!locked(&server(true, 30), Some(&member("bot", pb::AccountKind::Agent, None)), now));
        assert!(!locked(&server(true, 30), None, now), "not a member: the join button asks instead");
    }
}
