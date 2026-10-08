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
