use tonic::{Request, Response, Status};

use super::media::PictureOwner;
use super::{Api, respond, text, url};
use crate::auth::{self, Viewer};
use crate::error::{Error, Result};
use crate::id::millis;
use crate::node::{Account, ProfileChange};
use crate::pb::{self, auth_service_server::AuthService};
use crate::servers::{self as store, Payload};
use crate::twofactor;

/// What someone whose account was turned off hears when they sign in.
const DISABLED: &str = "this account was turned off by the instance's admins";

impl Api {
    async fn create_account(&self, req: pb::SignUpRequest, user_agent: &str) -> Result<pb::SignUpResponse> {
        if !self.app.settings().local_accounts.sign_up() {
            return Err(Error::FailedPrecondition("this instance isn't taking new sign-ups".into()));
        }
        let username = auth::validate_username(&req.username)?;
        auth::validate_password(&req.password)?;
        let display_name = if req.display_name.trim().is_empty() {
            username.clone()
        } else {
            text("display_name", &req.display_name, 1, 64)?
        };
        let hash = auth::hash_password(req.password).await?;
        let account = self.app.node.create_local_account(&username, &display_name, &hash).await?;
        let token = auth::new_token();
        self.app.node.create_session(&account.id, &auth::hash_token(&token), user_agent).await?;
        tracing::info!(account = %account.id, admin = account.admin, "account created");
        Ok(pb::SignUpResponse { token, user: Some(account.user()), admin: account.admin })
    }

    async fn start_session(&self, req: pb::SignInRequest, user_agent: &str) -> Result<pb::SignInResponse> {
        if !self.app.settings().local_accounts.sign_in() {
            return Err(Error::FailedPrecondition("this instance doesn't use standalone accounts".into()));
        }
        let username = req.username.trim().to_lowercase();
        self.app.limiter.check(&username)?;
        let found = self.app.node.local_account_by_username(&username).await?;
        let (account, hash) = match found {
            Some((account, hash)) => (Some(account), Some(hash)),
            None => (None, None),
        };
        let valid = auth::verify_password(req.password, hash).await?;
        let Some(account) = account.filter(|_| valid) else {
            self.app.limiter.failed(&username);
            return Err(Error::Unauthenticated);
        };
        self.app.limiter.succeeded(&username);
        if account.disabled {
            return Err(Error::denied(DISABLED));
        }
        if account.two_factor {
            let ticket = auth::new_token();
            self.app.node.create_ticket(&auth::hash_token(&ticket), &account.id).await?;
            return Ok(pb::SignInResponse { two_factor_ticket: ticket, ..Default::default() });
        }
        let token = auth::new_token();
        self.app.node.create_session(&account.id, &auth::hash_token(&token), user_agent).await?;
        Ok(pb::SignInResponse {
            token,
            user: Some(account.user()),
            admin: account.admin,
            two_factor_ticket: String::new(),
        })
    }

    /// The second step of signing in: a code for a sign-in whose password was right.
    async fn finish_two_factor(
        &self,
        req: pb::VerifyTwoFactorRequest,
        user_agent: &str,
    ) -> Result<pb::VerifyTwoFactorResponse> {
        let ticket_hash = auth::hash_token(req.ticket.trim());
        let Some(account_id) = self.app.node.ticket_account(&ticket_hash).await? else {
            return Err(Error::FailedPrecondition("this sign-in ran out; enter your password again".into()));
        };
        let guesses = format!("two-factor:{account_id}");
        self.app.limiter.check(&guesses)?;
        if !twofactor::check(&self.app.node, &account_id, &req.code).await? {
            self.app.limiter.failed(&guesses);
            self.app.node.ticket_failed(&ticket_hash).await?;
            return Err(Error::denied("that code didn't work"));
        }
        self.app.limiter.succeeded(&guesses);
        if !self.app.node.take_ticket(&ticket_hash).await? {
            return Err(Error::FailedPrecondition("this sign-in ran out; enter your password again".into()));
        }
        let account = self.app.node.account(&account_id).await?.ok_or(Error::Unauthenticated)?;
        if account.disabled {
            return Err(Error::denied(DISABLED));
        }
        let token = auth::new_token();
        self.app.node.create_session(&account.id, &auth::hash_token(&token), user_agent).await?;
        Ok(pb::VerifyTwoFactorResponse { token, user: Some(account.user()), admin: account.admin })
    }

    async fn apply_profile(&self, account: &Account, req: pb::UpdateProfileRequest) -> Result<(pb::User, pb::Profile)> {
        let status = match req.status.as_deref() {
            Some(status) => {
                let status = text("status", status, 0, 128)?;
                let expires = if status.is_empty() { None } else { req.status_expires_at.as_ref().map(millis) };
                Some((status, expires))
            }
            None => None,
        };
        let avatar_url = req.avatar_url.as_deref().map(|value| url("avatar_url", value)).transpose()?;
        let banner_url = req.banner_url.as_deref().map(|value| url("banner_url", value)).transpose()?;
        let old_banner = match &banner_url {
            Some(_) => self.app.node.profile(&account.id).await?.map(|profile| profile.banner_url).unwrap_or_default(),
            None => String::new(),
        };
        let new_avatar = match avatar_url.as_deref().filter(|url| *url != account.avatar_url) {
            Some(url) => self.check_picture(account, pb::MediaPurpose::Avatar, url).await?,
            None => None,
        };
        let new_banner = match banner_url.as_deref().filter(|url| *url != old_banner) {
            Some(url) => self.check_picture(account, pb::MediaPurpose::Banner, url).await?,
            None => None,
        };
        let change = ProfileChange {
            display_name: req.display_name.as_deref().map(|name| text("display_name", name, 1, 64)).transpose()?,
            avatar_url: avatar_url.clone(),
            pronouns: req.pronouns.as_deref().map(|value| text("pronouns", value, 0, 40)).transpose()?,
            bio: req.bio.as_deref().map(|value| text("bio", value, 0, 2000)).transpose()?,
            banner_url: banner_url.clone(),
            accent_color: match req.accent_color {
                None => None,
                Some(color) if color < 0 => Some(None),
                Some(color) if color <= 0xFF_FFFF => Some(Some(color)),
                Some(_) => return Err(Error::invalid("accent_color is a 0xRRGGBB color")),
            },
            status,
        };
        let shows_everywhere = change.display_name.is_some() || change.avatar_url.is_some() || change.status.is_some();
        let old_avatar = account.avatar_url.clone();
        self.keep_picture(new_avatar.as_deref(), None).await;
        self.keep_picture(new_banner.as_deref(), None).await;
        let account = self.app.node.update_profile(&account.id, &change).await?;
        if let Some(avatar) = &avatar_url {
            self.drop_picture(&old_avatar, avatar, PictureOwner::Account(&account.id, pb::MediaPurpose::Avatar)).await;
        }
        if let Some(banner) = &banner_url {
            self.drop_picture(&old_banner, banner, PictureOwner::Account(&account.id, pb::MediaPurpose::Banner)).await;
        }
        let user = account.user();
        let profile = self.app.node.profile(&account.id).await?.ok_or(Error::NotFound("account"))?;
        if !shows_everywhere {
            return Ok((user, profile));
        }
        // Every server the account belongs to keeps its own copy of the profile.
        for server_id in self.app.servers.joined_ids(&account.id) {
            let Ok(sdb) = self.app.servers.get(&server_id).await else { continue };
            let updated = sdb
                .write(&account.id, async |conn, events| {
                    store::upsert_user(conn, &user).await?;
                    if let Some(member) = store::member(conn, &sdb.id, &user.id).await? {
                        events.push(Payload::MemberUpdated(pb::MemberUpdated { member: Some(member) }));
                    }
                    Ok(())
                })
                .await;
            if let Err(err) = updated {
                tracing::warn!(server = %server_id, error = %err, "couldn't update a member's profile");
            }
        }
        Ok((user, profile))
    }

    /// Whether `viewer` may see `user_id`'s profile: their own, someone they
    /// share a server with, or anyone for an instance admin.
    fn can_see_profile(&self, viewer: &Account, user_id: &str) -> bool {
        viewer.id == user_id
            || viewer.admin
            || self.app.servers.joined_ids(&viewer.id).iter().any(|server| self.app.servers.is_member(user_id, server))
    }
}

#[tonic::async_trait]
impl AuthService for Api {
    async fn sign_up(&self, request: Request<pb::SignUpRequest>) -> Result<Response<pb::SignUpResponse>, Status> {
        let user_agent = auth::user_agent(request.metadata());
        respond(self.create_account(request.into_inner(), &user_agent).await)
    }

    async fn sign_in(&self, request: Request<pb::SignInRequest>) -> Result<Response<pb::SignInResponse>, Status> {
        let user_agent = auth::user_agent(request.metadata());
        respond(self.start_session(request.into_inner(), &user_agent).await)
    }

    async fn verify_two_factor(
        &self,
        request: Request<pb::VerifyTwoFactorRequest>,
    ) -> Result<Response<pb::VerifyTwoFactorResponse>, Status> {
        let user_agent = auth::user_agent(request.metadata());
        respond(self.finish_two_factor(request.into_inner(), &user_agent).await)
    }

    async fn sign_out(&self, request: Request<pb::SignOutRequest>) -> Result<Response<pb::SignOutResponse>, Status> {
        respond(
            async {
                if let Viewer::Account { token_hash, .. } = self.viewer(request.metadata()).await? {
                    self.app.node.delete_session(&token_hash).await?;
                }
                Ok(pb::SignOutResponse {})
            }
            .await,
        )
    }

    async fn get_me(&self, request: Request<pb::GetMeRequest>) -> Result<Response<pb::GetMeResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                Ok(pb::GetMeResponse { user: Some(account.user()), admin: account.admin })
            }
            .await,
        )
    }

    async fn update_profile(
        &self,
        request: Request<pb::UpdateProfileRequest>,
    ) -> Result<Response<pb::UpdateProfileResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let (user, profile) = self.apply_profile(&account, request.into_inner()).await?;
                Ok(pb::UpdateProfileResponse { user: Some(user), profile: Some(profile) })
            }
            .await,
        )
    }

    async fn get_profile(
        &self,
        request: Request<pb::GetProfileRequest>,
    ) -> Result<Response<pb::GetProfileResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let user_id = &request.get_ref().user_id;
                if !self.can_see_profile(&account, user_id) {
                    return Err(Error::NotFound("profile"));
                }
                let profile = self.app.node.profile(user_id).await?.ok_or(Error::NotFound("profile"))?;
                Ok(pb::GetProfileResponse { profile: Some(profile) })
            }
            .await,
        )
    }

    async fn change_password(
        &self,
        request: Request<pb::ChangePasswordRequest>,
    ) -> Result<Response<pb::ChangePasswordResponse>, Status> {
        respond(
            async {
                let viewer = self.viewer(request.metadata()).await?;
                let Viewer::Account { account, token_hash } = viewer else {
                    return Err(Error::denied("sign in to change a password"));
                };
                let req = request.into_inner();
                let current = self.app.node.password_hash(&account.id).await?;
                if current.is_none() {
                    return Err(Error::FailedPrecondition(
                        "this account signs in through waifu.dev, not with a password".into(),
                    ));
                }
                if !auth::verify_password(req.current_password, current).await? {
                    return Err(Error::denied("the current password is wrong"));
                }
                auth::validate_password(&req.new_password)?;
                let hash = auth::hash_password(req.new_password).await?;
                self.app.node.set_password(&account.id, &hash, &token_hash).await?;
                Ok(pb::ChangePasswordResponse {})
            }
            .await,
        )
    }
}
