//! Signing in with Google, X and Twitch, and linking them to an account
//! (`crate::sso::providers` has the providers themselves). Sign-ins start
//! signed out on a ticket, which costs nothing until the provider answers;
//! links start signed in, with proof beyond the session, on a kept row tied
//! to the account. Both come back through `/sso/instance/providers/<id>` (in
//! `crate::sso::http`) and finish with the one-time code and the app's secret.

use axum::body::Body;

use super::Api;
use crate::auth::{self, Caller};
use crate::error::{Error, Result};
use crate::id::{now_ms, timestamp};
use crate::node::{Account, LinkedProvider, NewProviderAccount};
use crate::pb;
use crate::sso::{self, providers};

/// How recent a session must be to link or unlink, for an account with
/// neither a password nor two-step sign-in to ask for.
pub const FRESH_SESSION_MS: i64 = 10 * 60 * 1000;

/// How long a new way in is pointed out on every device.
pub const RECENT_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// Links one account may start in [`sso::TTL_MS`]: each keeps a row until it runs out.
const MAX_LINK_STARTS: usize = 10;

/// The provider a sign-in's trust key is for ("provider <id> client …").
pub fn provider_of(provider_key: &str) -> Option<&'static providers::Spec> {
    let mut words = provider_key.split(' ');
    (words.next() == Some("provider")).then(|| words.next()).flatten().and_then(providers::spec)
}

fn ran_out() -> Error {
    Error::FailedPrecondition("this sign-in ran out; start again".into())
}

fn unknown(id: &str) -> Error {
    Error::FailedPrecondition(match providers::spec(id) {
        Some(spec) => format!("signing in with {} is off on this instance", spec.name),
        None => "fuwa doesn't know that sign-in provider".into(),
    })
}

fn method(linked: &LinkedProvider, works: bool) -> pb::SignInMethod {
    pb::SignInMethod {
        kind: linked.provider.clone(),
        name: providers::spec(&linked.provider).map(|spec| spec.name.to_string()).unwrap_or_default(),
        account_name: linked.name.clone(),
        linked_at: Some(timestamp(linked.linked_at)),
        works,
    }
}

impl Api {
    /// Every way an account signs in, and whether each works right now.
    async fn sign_in_methods(&self, account: &Account) -> Result<Vec<pb::SignInMethod>> {
        let settings = self.app.settings();
        let node = self.app.node()?;
        let mut methods = Vec::new();
        if account.has_password() {
            methods.push(pb::SignInMethod {
                kind: "password".into(),
                works: settings.local_accounts.sign_in(),
                ..Default::default()
            });
        }
        if let Some(issuer) = node.linked_issuer(&account.id).await? {
            let (kind, name, works) = match account.kind {
                pb::AccountKind::Sso => (
                    "sso",
                    settings.sso_provider.name.clone(),
                    settings.sso_sign_in() && settings.sso_provider.key() == issuer,
                ),
                _ => ("waifu", "waifu.dev".to_string(), settings.linked_sign_in() && settings.linked_issuer == issuer),
            };
            methods.push(pb::SignInMethod { kind: kind.into(), name, works, ..Default::default() });
        }
        for linked in node.account_providers(&account.id).await? {
            let works = settings.sign_in_provider(&linked.provider).is_some();
            methods.push(method(&linked, works));
        }
        Ok(methods)
    }

    /// Whether an account may link providers: its organization decides how
    /// single sign-on accounts sign in, and agents use their token.
    fn can_link(account: &Account) -> bool {
        matches!(account.kind, pb::AccountKind::Local | pb::AccountKind::Linked | pb::AccountKind::Provider)
    }

    /// Proof beyond the session that it's the account's person: a stolen
    /// session mustn't add (or take away) a way in. The password and two-step
    /// code where the account has them; otherwise a session from the last
    /// ten minutes.
    async fn confirm_owner(&self, caller: &Caller, password: &str, code: &str) -> Result<()> {
        let account = &caller.account;
        if account.has_password() {
            self.confirm_password(account, password).await?;
        }
        if account.two_factor {
            return self.confirm_code(account, code).await;
        }
        if account.has_password() {
            return Ok(());
        }
        let started = self.app.node()?.session_started(&caller.token_hash).await?.unwrap_or(0);
        if now_ms() - started > FRESH_SESSION_MS {
            return Err(Error::FailedPrecondition(
                "sign out and back in first: changing how you sign in needs a fresh sign-in".into(),
            ));
        }
        Ok(())
    }

    pub(super) async fn list_methods(&self, caller: Caller) -> Result<pb::ListSignInMethodsResponse> {
        let account = &caller.account;
        let methods = self.sign_in_methods(account).await?;
        let can_link = Self::can_link(account) && self.app.settings().provider_accounts.sign_in();
        let available = if can_link {
            self.app
                .settings()
                .sign_in_provider_options()
                .into_iter()
                .filter(|option| !methods.iter().any(|m| m.kind == option.id))
                .collect()
        } else {
            Vec::new()
        };
        Ok(pb::ListSignInMethodsResponse {
            methods,
            available,
            can_link,
            needs_password: account.has_password(),
            needs_code: account.two_factor,
            needs_fresh_sign_in: !account.has_password() && !account.two_factor,
        })
    }

    pub(super) async fn start_link(
        &self,
        caller: Caller,
        req: pb::StartProviderLinkRequest,
    ) -> Result<pb::StartProviderLinkResponse> {
        let account = &caller.account;
        if !Self::can_link(account) {
            return Err(Error::FailedPrecondition(if account.kind == pb::AccountKind::Sso {
                "your organization's sign-in is the only way into this account".into()
            } else {
                "this account can't link a sign-in provider".into()
            }));
        }
        let settings = self.app.settings();
        let id = req.provider.trim();
        let setting = settings.sign_in_provider(id).cloned().ok_or_else(|| unknown(id))?;
        let origin = crate::linked::return_origin(&req.return_origin, &settings.public_url, &settings.allowed_origins)?;
        let secret_hash = req.secret_hash.trim().to_ascii_lowercase();
        if secret_hash.len() != 64 || !secret_hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::invalid("secret_hash must be a SHA-256 in hex"));
        }
        self.confirm_owner(&caller, &req.password, &req.code).await?;
        sso::starts().start(&[(&format!("link {}", account.id), MAX_LINK_STARTS)])?;
        let sign_in = sso::SignIn {
            state: auth::new_token(),
            secret_hash,
            return_origin: origin,
            account_id: account.id.clone(),
            provider_key: setting.trust_key(),
            verifier: auth::new_token(),
            expires_at: now_ms() + sso::TTL_MS,
            ..Default::default()
        };
        let redirect = providers::redirect_uri(&settings.public_url, id);
        let authorize_url = providers::authorize_url(&setting, &redirect, &sign_in.state, &sign_in.verifier)?;
        self.app.node()?.save_sso_sign_in(&sign_in).await?;
        Ok(pb::StartProviderLinkResponse { authorize_url, state: sign_in.state })
    }

    pub(super) async fn finish_link(
        &self,
        caller: Caller,
        req: pb::FinishProviderLinkRequest,
    ) -> Result<pb::FinishProviderLinkResponse> {
        let node = self.app.node()?;
        let sign_in = node.sso_sign_in(req.state.trim()).await?.ok_or_else(ran_out)?;
        let spec = provider_of(&sign_in.provider_key).ok_or_else(ran_out)?;
        if sign_in.account_id != caller.account.id {
            return Err(Error::denied("another account started this link"));
        }
        let identity = sso::check_finish(&sign_in, &req.code, &req.secret)?;
        if !node.take_sso_sign_in(&sign_in.state).await? {
            return Err(ran_out());
        }
        let settings = self.app.settings();
        let setting = settings.sign_in_provider(spec.id).ok_or_else(|| unknown(spec.id))?;
        if setting.trust_key() != sign_in.provider_key {
            return Err(Error::FailedPrecondition(format!(
                "{} changed while you were linking it; start again",
                spec.name
            )));
        }
        let shown = if identity.username.is_empty() { &identity.name } else { &identity.username };
        let linked = node.link_provider(&caller.account.id, spec.id, &identity.subject, shown).await.map_err(
            |err| match err {
                Error::AlreadyExists(message) => Error::AlreadyExists(format!("{}: {message}", spec.name)),
                err => err,
            },
        )?;
        tracing::info!("linked a sign-in provider");
        Ok(pb::FinishProviderLinkResponse { method: Some(method(&linked, true)) })
    }

    pub(super) async fn unlink(&self, caller: Caller, req: pb::UnlinkProviderRequest) -> Result<()> {
        let id = req.provider.trim();
        let spec = providers::spec(id).ok_or_else(|| unknown(id))?;
        let methods = self.sign_in_methods(&caller.account).await?;
        if !methods.iter().any(|m| m.kind == id) {
            return Err(Error::NotFound("linked provider"));
        }
        if !methods.iter().any(|m| m.kind != id && m.works) {
            return Err(Error::FailedPrecondition(format!(
                "{} is the only way you can sign in right now; add another first",
                spec.name
            )));
        }
        self.confirm_owner(&caller, &req.password, &req.code).await?;
        if !self.app.node()?.unlink_provider(&caller.account.id, id).await? {
            return Err(Error::NotFound("linked provider"));
        }
        tracing::info!("unlinked a sign-in provider");
        Ok(())
    }

    /// Ways in added lately, newest first, for every device to point out.
    pub(super) async fn recent_sign_in_methods(&self, account: &Account) -> Result<Vec<pb::SignInMethod>> {
        let settings = self.app.settings();
        let since = now_ms() - RECENT_MS;
        let mut recent: Vec<pb::SignInMethod> = self
            .app
            .node()?
            .account_providers(&account.id)
            .await?
            .iter()
            // Not the provider the account was made with, linked as it was made.
            .filter(|linked| linked.linked_at >= since && linked.linked_at != account.created_at)
            .map(|linked| method(linked, settings.sign_in_provider(&linked.provider).is_some()))
            .collect();
        recent.reverse();
        Ok(recent)
    }

    pub(super) async fn start_provider(
        &self,
        req: pb::StartProviderSignInRequest,
    ) -> Result<pb::StartProviderSignInResponse> {
        let settings = self.app.settings();
        let id = req.provider.trim();
        let setting = settings.sign_in_provider(id).ok_or_else(|| unknown(id))?;
        let origin = crate::linked::return_origin(&req.return_origin, &settings.public_url, &settings.allowed_origins)?;
        // Nothing is kept until the provider answers: the state carries it.
        let sign_in = sso::ticket::issue(
            self.app.picture_key(),
            &setting.trust_key(),
            &origin,
            &req.secret_hash,
            false,
            now_ms(),
            &settings.public_url,
            &settings.allowed_origins,
        )?;
        let redirect = providers::redirect_uri(&settings.public_url, id);
        let authorize_url = providers::authorize_url(setting, &redirect, &sign_in.state, &sign_in.verifier)?;
        Ok(pb::StartProviderSignInResponse { authorize_url, state: sign_in.state })
    }

    pub(super) async fn read_provider_sign_in(
        &self,
        req: pb::GetProviderSignInRequest,
    ) -> Result<pb::GetProviderSignInResponse> {
        let settings = self.app.settings();
        let id = req.provider.trim();
        let spec = providers::spec(id).ok_or_else(|| unknown(id))?;
        let setting = settings.sign_in_provider(id).ok_or_else(|| unknown(id))?;
        let state = req.state.trim();
        let sign_in = match self.app.node()?.sso_sign_in(state).await? {
            Some(sign_in) if sign_in.provider_key == setting.trust_key() => sign_in,
            Some(_) => return Err(ran_out()),
            None => sso::ticket::read(
                self.app.picture_key(),
                &setting.trust_key(),
                state,
                now_ms(),
                &settings.public_url,
                &settings.allowed_origins,
            )?,
        };
        Ok(pb::GetProviderSignInResponse { return_origin: sign_in.return_origin, provider_name: spec.name.into() })
    }

    pub(super) async fn finish_provider(
        &self,
        req: pb::FinishProviderSignInRequest,
        user_agent: &str,
    ) -> Result<pb::FinishProviderSignInResponse> {
        let node = self.app.node()?;
        let sign_in = node.sso_sign_in(req.state.trim()).await?.ok_or_else(ran_out)?;
        // A link finishes signed in, through FinishProviderLink.
        if !sign_in.account_id.is_empty() {
            return Err(ran_out());
        }
        let spec = provider_of(&sign_in.provider_key).ok_or_else(ran_out)?;
        let identity = sso::check_finish(&sign_in, &req.code, &req.secret)?;
        let settings = self.app.settings();
        let setting = settings.sign_in_provider(spec.id).ok_or_else(|| unknown(spec.id))?;
        if setting.trust_key() != sign_in.provider_key {
            return Err(Error::FailedPrecondition(format!(
                "{} changed while you were signing in; start again",
                spec.name
            )));
        }
        let take = async || if node.take_sso_sign_in(&sign_in.state).await? { Ok(()) } else { Err(ran_out()) };
        let (account, created) = match node.provider_account(spec.id, &identity.subject).await? {
            Some(account) => {
                take().await?;
                (account, false)
            }
            None if !settings.provider_accounts.sign_up() => {
                take().await?;
                return Err(Error::FailedPrecondition(format!(
                    "no account here is linked to that {} account, and this instance isn't taking new sign-ups",
                    spec.name
                )));
            }
            None if !req.create => {
                // Asked first, without using the sign-in up: which username,
                // and whether to bring their name and picture along.
                let base = crate::linked::username_base(if identity.username.is_empty() {
                    &identity.name
                } else {
                    &identity.username
                });
                let mut suggested = String::new();
                for attempt in 1..=20 {
                    let candidate = crate::linked::username_candidate(&base, attempt).to_lowercase();
                    if auth::validate_username(&candidate).is_ok() && node.username_free(&candidate).await? {
                        suggested = candidate;
                        break;
                    }
                }
                return Ok(pb::FinishProviderSignInResponse {
                    new_account: Some(pb::NewProviderAccount {
                        provider_name: spec.name.into(),
                        suggested_username: suggested,
                        display_name: identity.name.clone(),
                        has_picture: identity.picture.is_some(),
                    }),
                    ..Default::default()
                });
            }
            None => {
                let username = auth::validate_username(&req.username)?;
                if !node.username_free(&username).await? {
                    return Err(Error::AlreadyExists("that username is taken".into()));
                }
                take().await?;
                let display_name: String = match identity.name.trim() {
                    name if req.use_profile && !name.is_empty() => name.chars().take(64).collect(),
                    _ => username.clone(),
                };
                let shown = if identity.username.is_empty() { &identity.name } else { &identity.username };
                let (account, created) = node
                    .create_provider_account(&NewProviderAccount {
                        provider: spec.id,
                        subject: &identity.subject,
                        name: shown,
                        username: &username,
                        display_name: &display_name,
                    })
                    .await?;
                let account = match (&identity.picture, req.use_profile && created) {
                    (Some(picture), true) => self.take_provider_picture(spec, account, picture).await,
                    _ => account,
                };
                (account, created)
            }
        };
        if account.disabled {
            return Err(Error::denied(super::auth::DISABLED));
        }
        if account.two_factor {
            let ticket = auth::new_token();
            node.create_ticket(&auth::hash_token(&ticket), &account.id).await?;
            return Ok(pb::FinishProviderSignInResponse { two_factor_ticket: ticket, ..Default::default() });
        }
        let token = auth::new_token();
        node.create_session(&account.id, &auth::hash_token(&token), user_agent).await?;
        tracing::info!(created, "signed in with a sign-in provider");
        Ok(pb::FinishProviderSignInResponse {
            token,
            user: Some(account.user()),
            admin: account.admin,
            created,
            ..Default::default()
        })
    }

    /// Copies the provider's picture into the new account as its own upload,
    /// so no address at the provider is ever kept or shown. Signing in goes
    /// on without it when it can't be had.
    async fn take_provider_picture(&self, spec: &providers::Spec, account: Account, picture: &str) -> Account {
        let stored = async {
            let bytes = providers::fetch_picture(spec, picture).await?;
            let url = self.store_picture(&account.id, bytes).await?;
            let req = pb::UpdateProfileRequest { avatar_url: Some(url), ..Default::default() };
            self.apply_profile(&account, req).await
        }
        .await;
        match stored {
            Ok((user, _)) => Account { avatar_url: user.avatar_url, ..account },
            Err(_) => {
                tracing::info!("couldn't copy a sign-in provider's picture");
                account
            }
        }
    }

    /// Stores picture bytes the instance fetched as the account's upload,
    /// checked like any upload (a picture fuwa takes, its metadata taken out,
    /// within the instance's picture caps). Returns its address.
    async fn store_picture(&self, account_id: &str, bytes: Vec<u8>) -> Result<String> {
        let settings = self.app.settings();
        let size = bytes.len() as i64;
        if settings.limits.picture_upload_bytes.is_some_and(|cap| size > cap) {
            return Err(Error::ResourceExhausted("that picture is bigger than this instance takes".into()));
        }
        let (node, store) = (self.app.node()?, self.app.media()?);
        let row = crate::media::MediaRow {
            id: crate::media::new_id(),
            account_id: account_id.to_string(),
            purpose: pb::MediaPurpose::Avatar,
            content_type: String::new(),
            size,
            stored: false,
            used: false,
            server_id: None,
        };
        let expires_at = now_ms() + crate::media::UPLOAD_TTL_MS;
        node.reserve_media(
            &row,
            &auth::hash_token(&auth::new_token()),
            expires_at,
            settings.limits.picture_upload_bytes_per_day,
        )
        .await?;
        let temp = store.incoming(&row.id);
        let received = crate::media::receive_file(pb::MediaPurpose::Avatar, size, &temp, Body::from(bytes)).await;
        let stored = match received {
            Ok((content_type, kept)) => match tokio::fs::rename(&temp, store.path(&row.id)).await {
                Ok(()) => node.finish_upload(&row.id, content_type, kept, now_ms()).await,
                Err(_) => Err(Error::internal("couldn't store a picture")),
            },
            Err((_, message)) => Err(Error::FailedPrecondition(message)),
        };
        if let Err(err) = stored {
            let _ = tokio::fs::remove_file(&temp).await;
            store.remove(&row.id);
            let _ = node.delete_media(std::slice::from_ref(&row.id)).await;
            return Err(err);
        }
        Ok(format!("{}/media/{}", settings.public_url.trim_end_matches('/'), row.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_ins_name_their_provider_by_trust_key() {
        assert_eq!(provider_of("provider x client abc").map(|s| s.id), Some("x"));
        assert!(provider_of("oidc https://login.acme.com client provider").is_none(), "single sign-on");
        assert!(provider_of("provider myspace client abc").is_none());
    }
}
