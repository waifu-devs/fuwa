//! Signing in with a code sent by email (`AuthMethods.email_sign_in`), the
//! desktop's side of the web's `components/EmailSignIn.tsx`: the instance
//! emails six digits and returns a ticket, and the ticket with the digits
//! signs in. Someone new picks a username first, then the same ticket and
//! code finish with `create`. Nothing leaves the app for the browser.

use std::sync::Arc;

use crate::core::Core;
use crate::core::api::{Api, Problem};
use crate::pb;
use crate::rpc;

/// How a code went.
#[derive(Clone, Debug)]
pub enum EmailAnswer {
    /// Signed in: the instance's key.
    Done { key: String },
    /// The account has two-step sign-in: send that code with this ticket.
    TwoFactor { ticket: String },
    /// Someone new: ask for a username, then finish again with it.
    NewAccount { account: pb::NewProviderAccount },
}

fn api(url: &str) -> Result<Api, Problem> {
    Api::new(url, None).map_err(|e| Problem::new(tonic::Code::InvalidArgument, e.to_string()))
}

impl Core {
    /// Emails a code to `email`; returns the ticket it goes back with.
    pub async fn start_email_sign_in(&self, url: &str, email: &str) -> Result<String, Problem> {
        let api = api(url)?;
        let res = rpc!(api.auth(), start_email_sign_in(pb::StartEmailSignInRequest { email: email.into() })).await?;
        Ok(res.ticket)
    }

    /// Finishes with the code; `create` makes the account for someone new, with that username.
    pub async fn finish_email_sign_in(
        self: &Arc<Self>,
        url: &str,
        ticket: &str,
        code: &str,
        create: Option<String>,
    ) -> Result<EmailAnswer, Problem> {
        let api = api(url)?;
        let res = rpc!(
            api.auth(),
            finish_email_sign_in(pb::FinishEmailSignInRequest {
                ticket: ticket.into(),
                code: code.into(),
                create: create.is_some(),
                username: create.unwrap_or_default(),
            })
        )
        .await?;
        if let Some(account) = res.new_account {
            return Ok(EmailAnswer::NewAccount { account });
        }
        if !res.two_factor_ticket.is_empty() {
            return Ok(EmailAnswer::TwoFactor { ticket: res.two_factor_ticket });
        }
        Ok(EmailAnswer::Done { key: self.add_instance(url, Some(res.token)) })
    }
}
