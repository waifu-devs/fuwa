use tonic::{Request, Response, Status};

use super::{Api, can_manage, respond, text, url};
use crate::db::query_all;
use crate::error::{Error, Result};
use crate::id::now_ms;
use crate::pb::{self, server_service_server::ServerService};
use crate::servers::{self as store, MEMBER_COLUMNS, NewServer, Payload, effective_limits, member_row};

#[tonic::async_trait]
impl ServerService for Api {
    async fn create_server(
        &self,
        request: Request<pb::CreateServerRequest>,
    ) -> Result<Response<pb::CreateServerResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                match self.app.settings().server_creation {
                    pb::ServerCreation::Disabled | pb::ServerCreation::Unspecified => {
                        return Err(Error::FailedPrecondition("this instance doesn't allow creating servers".into()));
                    }
                    pb::ServerCreation::Admins if !account.admin => {
                        return Err(Error::denied("only this instance's admins can create servers"));
                    }
                    _ => {}
                }
                if let Some(limit) = self.app.settings().limits.servers_per_account
                    && self.app.servers.owned_count(&account.id) >= limit
                {
                    return Err(Error::ResourceExhausted(format!("an account can own at most {limit} servers here")));
                }
                let req = request.into_inner();
                let new = NewServer {
                    name: text("name", &req.name, 1, 100)?,
                    description: text("description", &req.description, 0, 1000)?,
                    icon_url: url("icon_url", &req.icon_url)?,
                    discoverable: req.discoverable,
                };
                let server = self.app.servers.create(&account.user(), new).await?;
                tracing::info!(server = %server.id, owner = %account.id, "server created");
                Ok(pb::CreateServerResponse { server: Some(server) })
            }
            .await,
        )
    }

    async fn get_server(
        &self,
        request: Request<pb::GetServerRequest>,
    ) -> Result<Response<pb::GetServerResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let sdb = self.app.servers.get(&request.get_ref().server_id).await?;
                let server = sdb.server().await?;
                if !server.discoverable && !self.app.servers.is_member(&account.id, &sdb.id) && !account.admin {
                    return Err(Error::NotFound("server"));
                }
                Ok(pb::GetServerResponse { server: Some(server) })
            }
            .await,
        )
    }

    async fn list_servers(
        &self,
        request: Request<pb::ListServersRequest>,
    ) -> Result<Response<pb::ListServersResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                Ok(pb::ListServersResponse { servers: self.app.servers.joined(&account.id) })
            }
            .await,
        )
    }

    async fn discover_servers(
        &self,
        request: Request<pb::DiscoverServersRequest>,
    ) -> Result<Response<pb::DiscoverServersResponse>, Status> {
        respond(
            async {
                self.account(request.metadata()).await?;
                Ok(pb::DiscoverServersResponse { servers: self.app.servers.discoverable() })
            }
            .await,
        )
    }

    async fn update_server(
        &self,
        request: Request<pb::UpdateServerRequest>,
    ) -> Result<Response<pb::UpdateServerResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let (sdb, _) = self.manager(&account, &req.server_id).await?;
                let name = req.name.as_deref().map(|v| text("name", v, 1, 100)).transpose()?;
                let description = req.description.as_deref().map(|v| text("description", v, 0, 1000)).transpose()?;
                let icon_url = req.icon_url.as_deref().map(|v| url("icon_url", v)).transpose()?;
                let server = sdb
                    .write(&account.id, async |conn, events| {
                        conn.execute(
                            "UPDATE server SET name = coalesce(?1, name), description = coalesce(?2, description),
                         icon_url = coalesce(?3, icon_url), discoverable = coalesce(?4, discoverable), updated_at = ?5",
                            (name, description, icon_url, req.discoverable, now_ms()),
                        )
                        .await?;
                        let server = store::load_server(conn).await?;
                        events.push(Payload::ServerUpdated(pb::ServerUpdated { server: Some(server.clone()) }));
                        Ok(server)
                    })
                    .await?;
                self.app.servers.index_server(server.clone());
                Ok(pb::UpdateServerResponse { server: Some(server) })
            }
            .await,
        )
    }

    async fn delete_server(
        &self,
        request: Request<pb::DeleteServerRequest>,
    ) -> Result<Response<pb::DeleteServerResponse>, Status> {
        respond(
            async {
                let viewer = self.viewer(request.metadata()).await?;
                let server_id = &request.get_ref().server_id;
                let sdb = self.app.servers.get(server_id).await?;
                let actor = match viewer.account() {
                    Ok(account) => account.id.clone(),
                    Err(_) => String::new(),
                };
                let owner = sdb.server().await?.owner_id;
                if !viewer.is_instance_admin() && owner != actor {
                    return Err(Error::denied("only the server's owner can delete it"));
                }
                self.app.servers.delete(&sdb.id, &actor).await?;
                tracing::info!(server = %sdb.id, by = %actor, "server deleted");
                Ok(pb::DeleteServerResponse {})
            }
            .await,
        )
    }

    async fn join_server(
        &self,
        request: Request<pb::JoinServerRequest>,
    ) -> Result<Response<pb::JoinServerResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let sdb = self.app.servers.get(&request.get_ref().server_id).await?;
                let server = sdb.server().await?;
                if !server.discoverable {
                    return Err(Error::NotFound("server"));
                }
                let limits = sdb.limits(&self.app.settings().limits).await?;
                let user = account.user();
                let member = sdb
                    .write(&account.id, async |conn, events| {
                        if store::member(conn, &sdb.id, &user.id).await?.is_some() {
                            return Err(Error::AlreadyExists("you're already a member".into()));
                        }
                        if let Some(limit) = limits.members
                            && store::usage_count(conn, "members").await? >= limit
                        {
                            return Err(Error::ResourceExhausted(format!("this server is full ({limit} members)")));
                        }
                        let member = store::add_member(conn, &user, pb::MemberRole::Member, &sdb.id, now_ms()).await?;
                        events.push(Payload::MemberJoined(pb::MemberJoined { member: Some(member.clone()) }));
                        Ok(member)
                    })
                    .await?;
                self.app.servers.index_join(&account.id, &sdb.id);
                Ok(pb::JoinServerResponse { server: self.app.servers.summary(&sdb.id), member: Some(member) })
            }
            .await,
        )
    }

    async fn leave_server(
        &self,
        request: Request<pb::LeaveServerRequest>,
    ) -> Result<Response<pb::LeaveServerResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let (sdb, member) = self.membership(&account, &request.get_ref().server_id).await?;
                if member.role == pb::MemberRole::Owner as i32 {
                    return Err(Error::FailedPrecondition("the owner can't leave; delete the server instead".into()));
                }
                sdb.write(&account.id, async |conn, events| {
                    // Leaving twice at once: the second finds nothing to take away.
                    if conn.execute("DELETE FROM members WHERE user_id = ?1", [account.id.as_str()]).await? == 0 {
                        return Err(Error::NotFound("membership"));
                    }
                    conn.execute("UPDATE usage SET members = members - 1, updated_at = ?1 WHERE id = 1", [now_ms()])
                        .await?;
                    events.push(Payload::MemberLeft(pb::MemberLeft { user_id: account.id.clone() }));
                    Ok(())
                })
                .await?;
                self.app.servers.index_leave(&account.id, &sdb.id);
                Ok(pb::LeaveServerResponse {})
            }
            .await,
        )
    }

    async fn list_members(
        &self,
        request: Request<pb::ListMembersRequest>,
    ) -> Result<Response<pb::ListMembersResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let (sdb, _) = self.membership(&account, &request.get_ref().server_id).await?;
                let conn = sdb.read()?;
                let members = query_all(
                    &conn,
                    &format!(
                        "SELECT {MEMBER_COLUMNS} FROM members JOIN users ON users.id = members.user_id
                     ORDER BY members.role DESC, users.display_name"
                    ),
                    (),
                    member_row(&sdb.id),
                )
                .await?;
                Ok(pb::ListMembersResponse { members })
            }
            .await,
        )
    }

    async fn get_server_usage(
        &self,
        request: Request<pb::GetServerUsageRequest>,
    ) -> Result<Response<pb::GetServerUsageResponse>, Status> {
        respond(
            async {
                let viewer = self.viewer(request.metadata()).await?;
                let server_id = &request.get_ref().server_id;
                let sdb = if viewer.is_instance_admin() {
                    self.app.servers.get(server_id).await?
                } else {
                    let (sdb, member) = self.membership(viewer.account()?, server_id).await?;
                    if !can_manage(&member) {
                        return Err(Error::denied("only the server's owner and admins can see its usage"));
                    }
                    sdb
                };
                let own = sdb.own_limits().await?;
                Ok(pb::GetServerUsageResponse {
                    usage: Some(sdb.usage().await?),
                    limits: Some(effective_limits(own, &self.app.settings().limits)),
                    own_limits: Some(own),
                })
            }
            .await,
        )
    }
}
