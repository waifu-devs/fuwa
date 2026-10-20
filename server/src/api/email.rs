//! Signing in with a code sent by email, and adding an address to an
//! account to sign in with (docs/email-sign-in.md). An address is a sign-in
//! method like Google or X: a row in node.db's `account_providers` with
//! provider [`EMAIL`] and the address, lowercase, as the subject, so
//! listing, unlinking and the last-way-in check in `providers.rs` cover it.
//! The codes waiting to be typed are node.db's `email_codes`, sent through
//! whichever provider the instance was started with (`crate::mail`).

use super::Api;
use crate::auth::{self, Caller};
use crate::error::{Error, Result};
use crate::id::{now_ms, timestamp};
use crate::mail::{Mail, normalize_address};
use crate::node::{EmailCode, NewProviderAccount};
use crate::pb;

/// The provider id an address is kept under in `account_providers`.
pub const EMAIL: &str = "email";

/// What apps call it.
pub const EMAIL_NAME: &str = "Email";

/// How long a code works.
pub const CODE_TTL_MS: i64 = 10 * 60 * 1000;

/// Wrong codes before a ticket ends.
const CODE_TRIES: i64 = 5;

/// Codes sent to one address in [`crate::sso::TTL_MS`]: enough to ask again
/// a few times, too few to fill someone's inbox.
const SENDS_PER_ADDRESS: usize = 5;

/// Codes the whole instance sends in [`crate::sso::TTL_MS`], so nobody runs
/// up its mail bill or its standing with the provider.
const SENDS_PER_INSTANCE: usize = 1_000;

fn ran_out() -> Error {
    Error::FailedPrecondition("this code ran out; ask for a new one".into())
}

fn off() -> Error {
    Error::FailedPrecondition("this instance doesn't sign in with email codes".into())
}

/// Six digits, every one as likely as the next.
fn new_code() -> String {
    loop {
        let mut bytes = [0u8; 4];
        getrandom::fill(&mut bytes).expect("the OS random number generator failed");
        let n = u32::from_le_bytes(bytes);
        // The top of the range would favour low codes; draw again there.
        if n < u32::MAX - u32::MAX % 1_000_000 {
            return format!("{:06}", n % 1_000_000);
        }
    }
}

/// What's kept for a code: a hash over the ticket too, so the table alone
/// can't be turned back into codes (a million guesses would do it otherwise).
fn code_hash(ticket: &str, code: &str) -> String {
    auth::hash_token(&format!("{ticket}:{code}"))
}

/// The digits of a typed code, however it was spaced or pasted.
fn digits(code: &str) -> String {
    code.chars().filter(char::is_ascii_digit).collect()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The mail with a code in it: for signing in, or for adding the address.
fn code_mail(to: &str, code: &str, instance: &str, public_url: &str, adding: bool) -> Mail {
    let (subject, what) = if adding {
        (format!("{code} is your code to add this address on {instance}"), "add this address to your account on")
    } else {
        (format!("{code} is your sign-in code for {instance}"), "sign in to")
    };
    let text = format!(
        "Your code to {what} {instance} is:\n\n    {code}\n\nIt works for 10 minutes. If you didn't ask for it, \
         you can ignore this email: nobody gets in without the code.\n\n{public_url}\n"
    );
    let html = format!(
        "<!doctype html><html><body style=\"font-family:system-ui,sans-serif;color:#222;max-width:480px;margin:0 auto;padding:24px\">\
         <p>Your code to {what} <b>{instance}</b> is:</p>\
         <p style=\"font-size:32px;font-weight:800;letter-spacing:8px;margin:24px 0\">{code}</p>\
         <p>It works for 10 minutes. If you didn't ask for it, you can ignore this email: nobody gets in without the code.</p>\
         <p style=\"color:#888;font-size:12px\">{url}</p></body></html>",
        instance = escape(instance),
        url = escape(public_url),
    );
    Mail { to: to.to_string(), subject, text, html }
}

impl Api {
    /// Emails a new code to `email`, kept for `account_id` (empty for a
    /// sign-in). Returns the ticket and when the code runs out.
    async fn send_code(&self, email: &str, account_id: &str) -> Result<(String, i64)> {
        let mailer = self.app.mailer.as_ref().ok_or_else(off)?;
        let address_key = format!("email {}", auth::hash_token(email));
        crate::sso::starts()
            .start(&[(&address_key, SENDS_PER_ADDRESS), ("email *", SENDS_PER_INSTANCE)])
            .map_err(|_| Error::ResourceExhausted("too many codes sent just now; try again in a few minutes".into()))?;
        let ticket = auth::new_token();
        let code = new_code();
        let expires_at = now_ms() + CODE_TTL_MS;
        let ticket_hash = auth::hash_token(&ticket);
        let node = self.app.node()?;
        node.create_email_code(&EmailCode {
            ticket_hash: ticket_hash.clone(),
            email: email.to_string(),
            code_hash: code_hash(&ticket, &code),
            account_id: account_id.to_string(),
            expires_at,
        })
        .await?;
        let settings = self.app.settings();
        let mail = code_mail(email, &code, &settings.name, &settings.public_url, !account_id.is_empty());
        if let Err(err) = mailer.send(&mail).await {
            // What the provider said, for the admins; never the address.
            tracing::warn!(error = %err, "couldn't send an email code");
            let _ = node.take_email_code(&ticket_hash).await;
            return Err(Error::Unavailable("the email couldn't be sent; try again in a moment".into()));
        }
        Ok((ticket, expires_at))
    }

    /// Checks a typed code against its ticket, counting the try. A wrong
    /// code says so; the last wrong one ends the ticket.
    async fn check_code(&self, ticket: &str, code: &str) -> Result<EmailCode> {
        let node = self.app.node()?;
        let ticket = ticket.trim();
        let ticket_hash = auth::hash_token(ticket);
        let pending = node.try_email_code(&ticket_hash, CODE_TRIES).await?.ok_or_else(ran_out)?;
        let typed = digits(code);
        if typed.len() != 6
            || !auth::constant_time_eq(code_hash(ticket, &typed).as_bytes(), pending.code_hash.as_bytes())
        {
            return Err(Error::denied("that code didn't work"));
        }
        node.email_code_right(&ticket_hash).await?;
        Ok(pending)
    }

    pub(super) async fn start_email(&self, req: pb::StartEmailSignInRequest) -> Result<pb::StartEmailSignInResponse> {
        if !self.app.settings().email_sign_in() {
            return Err(off());
        }
        let email = normalize_address(&req.email)?;
        // The same answer whether or not an account has the address: only
        // whoever reads the mail learns that, once they type the code.
        let (ticket, expires_at) = self.send_code(&email, "").await?;
        Ok(pb::StartEmailSignInResponse { ticket, expires_at: Some(timestamp(expires_at)) })
    }

    pub(super) async fn finish_email(
        &self,
        req: pb::FinishEmailSignInRequest,
        user_agent: &str,
    ) -> Result<pb::FinishEmailSignInResponse> {
        let settings = self.app.settings();
        if !settings.email_sign_in() {
            return Err(off());
        }
        let pending = self.check_code(&req.ticket, &req.code).await?;
        // A code for adding an address finishes signed in, through FinishEmailLink.
        if !pending.account_id.is_empty() {
            return Err(ran_out());
        }
        let node = self.app.node()?;
        let take = async || if node.take_email_code(&pending.ticket_hash).await? { Ok(()) } else { Err(ran_out()) };
        let (account, created) = match node.provider_account(EMAIL, &pending.email).await? {
            Some(account) => {
                take().await?;
                (account, false)
            }
            None if !settings.email_sign_up() => {
                take().await?;
                return Err(Error::FailedPrecondition(
                    "no account here has that address, and this instance isn't taking new sign-ups".into(),
                ));
            }
            None if !req.create => {
                // Asked first, without using the code up: which username.
                let base = crate::linked::username_base(pending.email.split('@').next().unwrap_or_default());
                let mut suggested = String::new();
                for attempt in 1..=20 {
                    let candidate = crate::linked::username_candidate(&base, attempt).to_lowercase();
                    if auth::validate_username(&candidate).is_ok() && node.username_free(&candidate).await? {
                        suggested = candidate;
                        break;
                    }
                }
                return Ok(pb::FinishEmailSignInResponse {
                    new_account: Some(pb::NewProviderAccount {
                        provider_name: EMAIL_NAME.into(),
                        suggested_username: suggested,
                        display_name: String::new(),
                        has_picture: false,
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
                node.create_provider_account(&NewProviderAccount {
                    provider: EMAIL,
                    subject: &pending.email,
                    name: &pending.email,
                    username: &username,
                    display_name: &username,
                })
                .await?
            }
        };
        if account.disabled {
            return Err(Error::denied(super::auth::DISABLED));
        }
        if account.two_factor {
            let ticket = auth::new_token();
            node.create_ticket(&auth::hash_token(&ticket), &account.id).await?;
            return Ok(pb::FinishEmailSignInResponse { two_factor_ticket: ticket, ..Default::default() });
        }
        let token = auth::new_token();
        node.create_session(&account.id, &auth::hash_token(&token), user_agent).await?;
        tracing::info!(created, "signed in with an email code");
        Ok(pb::FinishEmailSignInResponse {
            token,
            user: Some(account.user()),
            admin: account.admin,
            created,
            ..Default::default()
        })
    }

    pub(super) async fn start_email_link(
        &self,
        caller: Caller,
        req: pb::StartEmailLinkRequest,
    ) -> Result<pb::StartEmailLinkResponse> {
        let account = &caller.account;
        if !Self::can_link(account) {
            return Err(Error::FailedPrecondition(if account.kind == pb::AccountKind::Sso {
                "your organization's sign-in is the only way into this account".into()
            } else {
                "this account can't add an email address".into()
            }));
        }
        if !self.app.settings().email_sign_in() {
            return Err(off());
        }
        let email = normalize_address(&req.email)?;
        self.confirm_owner(&caller, &req.password, &req.code, false).await?;
        // Whether the address is someone else's is only said once its code
        // is typed, so this doesn't tell anyone which addresses are here.
        let (ticket, expires_at) = self.send_code(&email, &account.id).await?;
        Ok(pb::StartEmailLinkResponse { ticket, expires_at: Some(timestamp(expires_at)) })
    }

    pub(super) async fn finish_email_link(
        &self,
        caller: Caller,
        req: pb::FinishEmailLinkRequest,
    ) -> Result<pb::FinishEmailLinkResponse> {
        if !self.app.settings().email_sign_in() {
            return Err(off());
        }
        let pending = self.check_code(&req.ticket, &req.code).await?;
        if pending.account_id != caller.account.id {
            return Err(Error::denied("another account asked for this code"));
        }
        let node = self.app.node()?;
        if !node.take_email_code(&pending.ticket_hash).await? {
            return Err(ran_out());
        }
        match node.provider_account(EMAIL, &pending.email).await? {
            Some(owner) if owner.id == caller.account.id => {
                return Err(Error::AlreadyExists("that address is already on your account".into()));
            }
            Some(_) => return Err(Error::AlreadyExists("that address belongs to another account here".into())),
            None => {}
        }
        if node.account_providers(&caller.account.id).await?.iter().any(|linked| linked.provider == EMAIL) {
            return Err(Error::AlreadyExists("your account has an address already; remove it first".into()));
        }
        // Checked again as it's written, for two at once.
        let linked = node.link_provider(&caller.account.id, EMAIL, &pending.email, &pending.email).await?;
        tracing::info!("added an email address to sign in with");
        Ok(pb::FinishEmailLinkResponse { method: Some(super::providers::method(&linked, true)) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_six_digits() {
        for _ in 0..200 {
            let code = new_code();
            assert_eq!(code.len(), 6);
            assert!(code.chars().all(|c| c.is_ascii_digit()));
        }
        assert_eq!(digits(" 123 456\n"), "123456");
        assert_eq!(digits("123-456"), "123456");
    }

    #[test]
    fn codes_are_kept_over_their_ticket() {
        assert_ne!(code_hash("ticket-a", "123456"), code_hash("ticket-b", "123456"));
        assert_eq!(code_hash("ticket-a", "123456"), code_hash("ticket-a", "123456"));
    }

    #[test]
    fn the_mail_names_the_instance_safely() {
        let mail = code_mail("ana@example.com", "042517", "<Acme & co>", "https://chat.acme.com", false);
        assert_eq!(mail.subject, "042517 is your sign-in code for <Acme & co>");
        assert!(mail.text.contains("    042517\n"), "{}", mail.text);
        assert!(mail.html.contains("&lt;Acme &amp; co&gt;"), "{}", mail.html);
        assert!(!mail.html.contains("<Acme"), "{}", mail.html);
        let adding = code_mail("ana@example.com", "042517", "Acme", "https://chat.acme.com", true);
        assert!(adding.subject.contains("add this address"), "{}", adding.subject);
    }
}
