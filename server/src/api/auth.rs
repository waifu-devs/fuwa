use tonic::{Request, Response, Status};

use super::{Api, respond, text, url};
use crate::auth::{self, Viewer};
use crate::error::{Error, Result};
use crate::pb::{self, auth_service_server::AuthService};
use crate::servers::{self as store, Payload};

impl Api {
    async fn create_account(&self, req: pb::SignUpRequest) -> Result<pb::SignUpResponse> {
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
        self.app.node.create_session(&account.id, &auth::hash_token(&token)).await?;
        tracing::info!(account = %account.id, admin = account.admin, "account created");
        Ok(pb::SignUpResponse { token, user: Some(account.user()), admin: account.admin })
    }

    async fn start_session(&self, req: pb::SignInRequest) -> Result<pb::SignInResponse> {
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
        let token = auth::new_token();
        self.app.node.create_session(&account.id, &auth::hash_token(&token)).await?;
        Ok(pb::SignInResponse { token, user: Some(account.user()), admin: account.admin })
    }

    async fn apply_profile(&self, account: &crate::node::Account, req: pb::UpdateProfileRequest) -> Result<pb::User> {
        let display_name = req.display_name.as_deref().map(|name| text("display_name", name, 1, 64)).transpose()?;
        let avatar_url = req.avatar_url.as_deref().map(|value| url("avatar_url", value)).transpose()?;
        let account = self.app.node.update_profile(&account.id, display_name.as_deref(), avatar_url.as_deref()).await?;
        let user = account.user();
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
        Ok(user)
    }
}

#[tonic::async_trait]
impl AuthService for Api {
    async fn sign_up(&self, request: Request<pb::SignUpRequest>) -> Result<Response<pb::SignUpResponse>, Status> {
        respond(self.create_account(request.into_inner()).await)
    }

    async fn sign_in(&self, request: Request<pb::SignInRequest>) -> Result<Response<pb::SignInResponse>, Status> {
        respond(self.start_session(request.into_inner()).await)
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
                let user = self.apply_profile(&account, request.into_inner()).await?;
                Ok(pb::UpdateProfileResponse { user: Some(user) })
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
