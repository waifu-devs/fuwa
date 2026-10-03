//! `AgentService`: accounts programs drive, made and owned by a person. An
//! agent's token is its one session (see `NodeDb::create_agent`); with it,
//! the agent uses the same API people do, in the servers it was added to.

use tonic::{Request, Response, Status};

use super::messages::post_join;
use super::{Api, respond, text};
use crate::auth;
use crate::error::{Error, Result};
use crate::id::{now_ms, timestamp};
use crate::node::{Account, AgentRow};
use crate::pb::{self, Permission, agent_service_server::AgentService};
use crate::servers::{self as store, Audit, Payload};

/// Most agents one person may make.
pub const MAX_AGENTS: i64 = 25;

impl Api {
    /// The caller, who must be a person: agents don't make or manage agents.
    async fn person(&self, metadata: &tonic::metadata::MetadataMap) -> Result<Account> {
        let account = self.account(metadata).await?;
        if account.kind == pb::AccountKind::Agent {
            return Err(Error::denied("agents can't make or manage agents"));
        }
        Ok(account)
    }

    /// One of the caller's agents. Someone else's is as good as missing.
    async fn own_agent(&self, owner: &Account, agent_id: &str) -> Result<AgentRow> {
        self.app
            .node()?
            .agent(Some(agent_id), None)
            .await?
            .filter(|agent| agent.owner_id == owner.id)
            .ok_or(Error::NotFound("agent"))
    }

    fn agent_pb(&self, row: AgentRow) -> pb::Agent {
        let servers = self.app.index.joined_ids(&row.account.id).len();
        pb::Agent {
            user: Some(row.account.user()),
            owner_id: row.owner_id,
            public: row.public,
            bio: row.bio,
            created_at: Some(timestamp(row.account.created_at)),
            last_active_at: row.last_active_at.map(timestamp),
            servers: i32::try_from(servers).unwrap_or(i32::MAX),
        }
    }

    async fn update_agent(&self, owner: &Account, req: pb::UpdateAgentRequest) -> Result<pb::Agent> {
        let agent = self.own_agent(owner, &req.agent_id).await?;
        // The owner uploads the picture; it becomes the agent's, so it goes
        // with the agent and is replaced like a person's avatar.
        if let Some(url) = req.avatar_url.as_deref().map(str::trim).filter(|url| *url != agent.account.avatar_url)
            && let Some(id) = self.check_picture(owner, pb::MediaPurpose::Avatar, url).await?
        {
            self.app.node()?.give_media(&id, &agent.account.id).await?;
        }
        let profile = pb::UpdateProfileRequest {
            display_name: req.display_name,
            avatar_url: req.avatar_url,
            bio: req.bio,
            ..Default::default()
        };
        if profile.display_name.is_some() || profile.avatar_url.is_some() || profile.bio.is_some() {
            self.apply_profile(&agent.account, profile).await?;
        }
        if let Some(public) = req.public {
            self.app.node()?.set_agent_public(&agent.account.id, public).await?;
        }
        Ok(self.agent_pb(self.own_agent(owner, &req.agent_id).await?))
    }

    async fn add_agent(&self, account: &Account, req: pb::AddAgentRequest) -> Result<pb::Member> {
        let seat = self.with(account, &req.server_id, Permission::ManageServer).await?;
        let username = auth::validate_username(&req.username)?;
        let agent = self.app.find_agent(&username).await?.ok_or(Error::NotFound("agent"))?;
        if !agent.public && agent.owner_id != account.id {
            return Err(Error::denied("that agent is private: only the person who made it can add it to servers"));
        }
        let sdb = seat.sdb;
        let limits = sdb.limits(&self.app.settings().limits).await?;
        let user = agent.account.user();
        let now = now_ms();
        let member = sdb
            .write(&account.id, async |conn, events| {
                if store::member(conn, &sdb.id, &user.id).await?.is_some() {
                    return Err(Error::AlreadyExists("that agent is already here".into()));
                }
                if crate::db::query_one(conn, "SELECT 1 FROM bans WHERE user_id = ?1", [user.id.as_str()], |r| {
                    r.get::<i64>(0)
                })
                .await?
                .is_some()
                {
                    return Err(Error::denied("that agent is banned from this server; unban it first"));
                }
                if let Some(limit) = limits.members
                    && store::usage_count(conn, "members").await? >= limit
                {
                    return Err(Error::ResourceExhausted(format!("this server is full ({limit} members)")));
                }
                let server = store::load_server(conn).await?;
                // Agents don't agree to rules: whoever adds one answers for it.
                let member = store::add_member(conn, &user, &sdb.id, now, false).await?;
                events.push(Payload::MemberJoined(pb::MemberJoined { member: Some(member.clone()) }));
                post_join(conn, &server, &user.id, now, events).await?;
                store::audit(conn, &account.id, Audit::new(pb::AuditAction::AgentAdd, &user.id)).await?;
                Ok(member)
            })
            .await?;
        self.app.membership_changed(&user.id, &sdb.id, true).await;
        tracing::info!(server = %sdb.id, agent = %user.id, by = %account.id, "agent added");
        Ok(member)
    }
}

#[tonic::async_trait]
impl AgentService for Api {
    async fn list_agents(
        &self,
        request: Request<pb::ListAgentsRequest>,
    ) -> Result<Response<pb::ListAgentsResponse>, Status> {
        respond(
            async {
                let owner = self.person(request.metadata()).await?;
                let agents = self.app.node()?.agents(&owner.id).await?;
                Ok(pb::ListAgentsResponse { agents: agents.into_iter().map(|row| self.agent_pb(row)).collect() })
            }
            .await,
        )
    }

    async fn create_agent(
        &self,
        request: Request<pb::CreateAgentRequest>,
    ) -> Result<Response<pb::CreateAgentResponse>, Status> {
        respond(
            async {
                let owner = self.person(request.metadata()).await?;
                match self.app.settings().agent_creation {
                    pb::AgentCreation::Disabled | pb::AgentCreation::Unspecified => {
                        return Err(Error::FailedPrecondition("this instance doesn't allow making agents".into()));
                    }
                    pb::AgentCreation::Admins if !owner.admin => {
                        return Err(Error::denied("only this instance's admins can make agents"));
                    }
                    _ => {}
                }
                let req = request.into_inner();
                let username = auth::validate_username(&req.username)?;
                let display_name = text("display_name", &req.display_name, 1, 64)?;
                let node = self.app.node()?;
                if node.agent_count(&owner.id).await? >= MAX_AGENTS {
                    return Err(Error::ResourceExhausted(format!("you can have at most {MAX_AGENTS} agents")));
                }
                let token = auth::new_token();
                let account = node.create_agent(&owner.id, &username, &display_name, &auth::hash_token(&token)).await?;
                tracing::info!(agent = %account.id, owner = %owner.id, "agent created");
                let row = self.own_agent(&owner, &account.id).await?;
                Ok(pb::CreateAgentResponse { agent: Some(self.agent_pb(row)), token })
            }
            .await,
        )
    }

    async fn update_agent(
        &self,
        request: Request<pb::UpdateAgentRequest>,
    ) -> Result<Response<pb::UpdateAgentResponse>, Status> {
        respond(
            async {
                let owner = self.person(request.metadata()).await?;
                let agent = Api::update_agent(self, &owner, request.into_inner()).await?;
                Ok(pb::UpdateAgentResponse { agent: Some(agent) })
            }
            .await,
        )
    }

    async fn reset_agent_token(
        &self,
        request: Request<pb::ResetAgentTokenRequest>,
    ) -> Result<Response<pb::ResetAgentTokenResponse>, Status> {
        respond(
            async {
                let owner = self.person(request.metadata()).await?;
                let agent = self.own_agent(&owner, &request.get_ref().agent_id).await?;
                let token = auth::new_token();
                self.app.node()?.reset_agent_token(&agent.account.id, &auth::hash_token(&token)).await?;
                tracing::info!(agent = %agent.account.id, "agent token reset");
                Ok(pb::ResetAgentTokenResponse { token })
            }
            .await,
        )
    }

    async fn delete_agent(
        &self,
        request: Request<pb::DeleteAgentRequest>,
    ) -> Result<Response<pb::DeleteAgentResponse>, Status> {
        respond(
            async {
                let owner = self.person(request.metadata()).await?;
                let agent = self.own_agent(&owner, &request.get_ref().agent_id).await?;
                self.erase_account(&agent.account).await?;
                Ok(pb::DeleteAgentResponse {})
            }
            .await,
        )
    }

    async fn add_agent(&self, request: Request<pb::AddAgentRequest>) -> Result<Response<pb::AddAgentResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let member = Api::add_agent(self, &account, request.into_inner()).await?;
                Ok(pb::AddAgentResponse { member: Some(member) })
            }
            .await,
        )
    }
}
