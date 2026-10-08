//! Signing in with Google, X or Twitch, for the ones an instance's admins
//! turned on (`AuthMethods.providers`), the desktop's side of the web's
//! `startProviderSignIn` and `pages/ProviderDone.tsx`. It runs like single
//! sign-on (`sso.rs`): the app keeps a secret, opens the provider's page in
//! the browser and listens on this computer; the instance sends the browser
//! to `/auth/provider/done` with a one-time code, which the loopback page
//! hands back. Someone new picks a username first, then the same sign-in
//! finishes with `create`.

use std::sync::Arc;

use crate::core::api::{Api, Problem};
use crate::core::{Core, sso};
use crate::pb;
use crate::rpc;

/// A provider sign-in that came back but isn't finished: someone new choosing their username.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingProvider {
    pub url: String,
    pub provider: String,
    state: String,
    code: String,
    secret: String,
}

/// How a provider sign-in went.
#[derive(Clone, Debug)]
pub enum ProviderAnswer {
    /// Signed in: the instance's key.
    Done { key: String },
    /// The account has two-step sign-in: send the code with this ticket.
    TwoFactor { ticket: String },
    /// Someone new: ask for a username (and whether to bring the provider's name and picture).
    NewAccount { pending: PendingProvider, account: pb::NewProviderAccount },
}

fn api(url: &str) -> Result<Api, Problem> {
    Api::new(url, None).map_err(|e| Problem::new(tonic::Code::InvalidArgument, e.to_string()))
}

impl Core {
    /// Starts signing in with `provider` ("google", "x", "twitch") in the browser.
    pub async fn provider_sign_in(
        self: &Arc<Self>,
        url: &str,
        provider: &str,
        open_page: impl FnOnce(&str) + Send,
    ) -> Result<ProviderAnswer, Problem> {
        let api = api(url)?;
        let callback = sso::listen().await?;
        let (secret, secret_hash) = sso::secret()?;
        let started = rpc!(
            api.auth(),
            start_provider_sign_in(pb::StartProviderSignInRequest {
                provider: provider.into(),
                return_origin: callback.origin.clone(),
                secret_hash,
            })
        )
        .await?;
        let answer = sso::round_trip(callback, &started.authorize_url, &started.state, secret, open_page).await?;
        let pending = PendingProvider {
            url: url.to_owned(),
            provider: provider.to_owned(),
            state: answer.state,
            code: answer.code,
            secret: answer.secret,
        };
        self.finish_provider(pending, None).await
    }

    /// Finishes a provider sign-in; `create` makes the account for someone
    /// new, with a username and whether to use the provider's name and picture.
    pub async fn finish_provider(
        self: &Arc<Self>,
        pending: PendingProvider,
        create: Option<(String, bool)>,
    ) -> Result<ProviderAnswer, Problem> {
        let api = api(&pending.url)?;
        let (username, use_profile) = create.clone().unwrap_or_default();
        let res = rpc!(
            api.auth(),
            finish_provider_sign_in(pb::FinishProviderSignInRequest {
                state: pending.state.clone(),
                code: pending.code.clone(),
                secret: pending.secret.clone(),
                create: create.is_some(),
                username,
                use_profile,
            })
        )
        .await?;
        if let Some(account) = res.new_account {
            return Ok(ProviderAnswer::NewAccount { pending, account });
        }
        if !res.two_factor_ticket.is_empty() {
            return Ok(ProviderAnswer::TwoFactor { ticket: res.two_factor_ticket });
        }
        Ok(ProviderAnswer::Done { key: self.add_instance(&pending.url, Some(res.token)) })
    }
}
