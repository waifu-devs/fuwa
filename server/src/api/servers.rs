use tonic::{Request, Response, Status};

use super::media::PictureOwner;
use super::messages::post_join;
use super::{Api, Seat, respond, text, url, users};
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{millis, now_ms, span, timestamp};
use crate::node::Account;
use crate::pb::{self, Permission, server_service_server::ServerService};
use crate::permissions::{self, Access};
use crate::servers::{
    self as store, Audit, MEMBER_COLUMNS, NewServer, Payload, USER_COLUMNS, UsageChange, audit_changes,
    effective_limits, load_channel, member_row,
};

/// The longest time-out, as Discord has it: 28 days.
const MAX_TIME_OUT_SECONDS: i64 = 28 * 24 * 60 * 60;

/// The longest minimum account age a server can ask for: a year.
const MAX_ACCOUNT_AGE: i32 = 365 * 24 * 60 * 60;
/// The longest a quiet thread stays open: a year, in hours.
const MAX_THREAD_ARCHIVE_HOURS: i32 = 365 * 24;
/// How far back a ban can take someone's messages with them: seven days.
const MAX_DELETE_MESSAGE_SECONDS: i64 = 7 * 24 * 60 * 60;

/// Checks someone can act on another member: someone else, ranked below them.
fn outranks(my_id: &str, me: &Access, target_id: &str, target: &Access) -> Result<()> {
    if my_id == target_id {
        return Err(Error::invalid("you can't do that to yourself"));
    }
    if !me.outranks(target) {
        return Err(Error::denied("you can only moderate people ranked below you"));
    }
    Ok(())
}

/// A member and what they can do, inside a write.
async fn target(conn: &turso::Connection, server_id: &str, user_id: &str) -> Result<(pb::Member, Access)> {
    store::member_access(conn, server_id, user_id).await?.ok_or(Error::NotFound("member"))
}

/// A reason kept in the audit log.
fn reason(value: &str) -> Result<String> {
    text("reason", value, 0, 512)
}

/// A time as the audit log keeps it: unix milliseconds, or empty for none.
fn audit_time(t: Option<&prost_types::Timestamp>) -> String {
    t.map(|t| millis(t).to_string()).unwrap_or_default()
}

/// Takes a member out of the server, inside a write, for leaving, kicks and bans.
async fn remove_member(
    conn: &turso::Connection,
    server_id: &str,
    user_id: &str,
    reason: pb::LeaveReason,
    events: &mut Vec<Payload>,
) -> Result<bool> {
    if !store::remove_member(conn, server_id, user_id, events).await? {
        return Ok(false);
    }
    events.push(Payload::MemberLeft(pb::MemberLeft { user_id: user_id.to_string(), reason: reason as i32 }));
    Ok(true)
}

/// Whether someone may come in, before touching the server's file: they need
/// it in Browse or an invite, a waifu.dev account if it asks for one, and an
/// account old enough.
pub(super) fn at_the_door(account: &Account, server: &pb::Server, code: &str, now: i64) -> Result<()> {
    if account.kind == pb::AccountKind::Agent {
        return Err(Error::FailedPrecondition(
            "agents don't join by themselves; someone who manages the server adds them".into(),
        ));
    }
    if code.is_empty() && !server.discoverable {
        return Err(Error::NotFound("server"));
    }
    if server.linked_only && account.kind != pb::AccountKind::Linked {
        return Err(Error::FailedPrecondition("only people who sign in with waifu.dev can join this server".into()));
    }
    let wait = i64::from(server.min_account_age_seconds) * 1000 - (now - account.created_at);
    if wait > 0 {
        return Err(Error::FailedPrecondition(format!(
            "this server lets in accounts once they're {} old; yours can join in {}",
            span(i64::from(server.min_account_age_seconds) * 1000),
            span(wait),
        )));
    }
    Ok(())
}

/// The rest of the checks, inside a write: the invite still works, and they
/// aren't a member already or banned. Returns the invite, if they came with one.
pub(super) async fn let_in(
    conn: &turso::Connection,
    server_id: &str,
    user_id: &str,
    code: &str,
    now: i64,
) -> Result<Option<pb::Invite>> {
    let invite = match code {
        "" => None,
        code => Some(
            store::load_invite(conn, server_id, code)
                .await?
                .filter(|invite| store::invite_works(invite, now))
                .ok_or(Error::NotFound("invite"))?,
        ),
    };
    if store::member(conn, server_id, user_id).await?.is_some() {
        return Err(Error::AlreadyExists("you're already a member".into()));
    }
    if query_one(conn, "SELECT 1 FROM bans WHERE user_id = ?1", [user_id], |r| r.get::<i64>(0)).await?.is_some() {
        return Err(Error::denied("you're banned from this server"));
    }
    let sso = store::load_sso(conn).await?;
    if sso.required && !sso.fresh(store::sso_signed_in_at(conn, user_id).await?, now) {
        return Err(Error::FailedPrecondition(format!("sign in with {} to join this server", sso.provider.name)));
    }
    Ok(invite)
}

/// Counts a use of the invite someone came in with, inside a write. Its last
/// use deletes it, so a used-up invite isn't kept around; true when it did.
pub(super) async fn use_invite(conn: &turso::Connection, invite: Option<&pb::Invite>) -> Result<bool> {
    let Some(invite) = invite else { return Ok(false) };
    let used_up = invite.max_uses > 0 && invite.uses + 1 >= invite.max_uses;
    if used_up {
        conn.execute("DELETE FROM invites WHERE code = ?1", [invite.code.as_str()]).await?;
    } else {
        conn.execute("UPDATE invites SET uses = uses + 1 WHERE code = ?1", [invite.code.as_str()]).await?;
    }
    Ok(used_up)
}

#[tonic::async_trait]
impl ServerService for Api {
    async fn create_server(
        &self,
        request: Request<pb::CreateServerRequest>,
    ) -> Result<Response<pb::CreateServerResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                if account.kind == pb::AccountKind::Agent {
                    return Err(Error::denied("agents can't own servers"));
                }
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
                    && self.app.owned_count(&account.id).await? >= limit
                {
                    return Err(Error::ResourceExhausted(format!("an account can own at most {limit} servers here")));
                }
                let req = request.into_inner();
                let new = NewServer {
                    name: text("name", &req.name, 1, 100)?,
                    description: text("description", &req.description, 0, 1000)?,
                    icon_url: self.app.picture_link(&url("icon_url", &req.icon_url)?),
                    discoverable: req.discoverable,
                };
                let icon = self.check_picture(&account, pb::MediaPurpose::ServerIcon, &new.icon_url, None).await?;
                let server = self.app.create_server(&account.user(), new, req.region.trim()).await?;
                self.keep_picture(icon.as_deref(), Some(&server.id)).await;
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
                if !server.discoverable
                    && !account.admin
                    && store::member(&*sdb.read()?, &sdb.id, &account.id).await?.is_none()
                {
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
                Ok(pb::ListServersResponse { servers: self.app.index.joined(&account.id) })
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
                Ok(pb::DiscoverServersResponse { servers: self.app.index.discoverable() })
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
                let sdb = self.with(&account, &req.server_id, Permission::ManageServer).await?.sdb;
                let name = req.name.as_deref().map(|v| text("name", v, 1, 100)).transpose()?;
                let description = req.description.as_deref().map(|v| text("description", v, 0, 1000)).transpose()?;
                let icon_url = req.icon_url.as_deref().map(|v| url("icon_url", v)).transpose()?.map(|v| self.app.picture_link(&v));
                let old_icon = sdb.server().await?.icon_url;
                let new_icon = match icon_url.as_deref().filter(|url| *url != old_icon) {
                    Some(url) => self.check_picture(&account, pb::MediaPurpose::ServerIcon, url, Some(&sdb.id)).await?,
                    None => None,
                };
                self.keep_picture(new_icon.as_deref(), Some(&sdb.id)).await;
                if let Some(level) = req.default_notifications
                    && !matches!(
                        pb::NotificationLevel::try_from(level),
                        Ok(pb::NotificationLevel::Unspecified | pb::NotificationLevel::All | pb::NotificationLevel::Mentions)
                    )
                {
                    return Err(Error::invalid("servers can default to all messages or only @mentions"));
                }
                if req.min_account_age_seconds.is_some_and(|age| !(0..=MAX_ACCOUNT_AGE).contains(&age)) {
                    return Err(Error::invalid("the minimum account age is up to a year"));
                }
                if req.thread_archive_hours.is_some_and(|hours| !(0..=MAX_THREAD_ARCHIVE_HOURS).contains(&hours)) {
                    return Err(Error::invalid("threads are archived after at most a year"));
                }
                let server = sdb
                    .write(&account.id, async |conn, events| {
                        let before = store::load_server(conn).await?;
                        if let Some(channel_id) = req.system_channel_id.as_deref().filter(|id| !id.is_empty()) {
                            let channel = load_channel(conn, &sdb.id, channel_id)
                                .await?
                                .ok_or(Error::NotFound("system channel"))?;
                            if !matches!(
                                pb::ChannelType::try_from(channel.r#type),
                                Ok(pb::ChannelType::Text | pb::ChannelType::Announcement)
                            ) {
                                return Err(Error::invalid("system messages can only go in a text channel"));
                            }
                            if channel.shared.as_ref().is_some_and(|s| !s.home) {
                                return Err(Error::invalid("system messages can't go in a channel shared from another server"));
                            }
                        }
                        conn.execute(
                            "UPDATE server SET name = coalesce(?1, name), description = coalesce(?2, description),
                         icon_url = coalesce(?3, icon_url), discoverable = coalesce(?4, discoverable),
                         default_notifications = coalesce(?5, default_notifications),
                         system_channel_id = CASE WHEN ?6 IS NULL THEN system_channel_id WHEN ?6 = '' THEN NULL ELSE ?6 END,
                         min_account_age_seconds = coalesce(?8, min_account_age_seconds),
                         applications = coalesce(?9, applications), linked_only = coalesce(?10, linked_only),
                         thread_archive_hours = coalesce(?11, thread_archive_hours), updated_at = ?7",
                            (
                                name,
                                description,
                                icon_url,
                                req.discoverable,
                                req.default_notifications,
                                req.system_channel_id.as_deref(),
                                now_ms(),
                                req.min_account_age_seconds,
                                req.applications,
                                req.linked_only,
                                req.thread_archive_hours,
                            ),
                        )
                        .await?;
                        let server = store::load_server(conn).await?;
                        let entry = Audit::new(pb::AuditAction::ServerUpdate, "")
                            .change("name", &before.name, &server.name)
                            .change("description", &before.description, &server.description)
                            .change("icon_url", &before.icon_url, &server.icon_url)
                            .change("discoverable", before.discoverable, server.discoverable)
                            .change("default_notifications", before.default_notifications, server.default_notifications)
                            .change("system_channel_id", &before.system_channel_id, &server.system_channel_id)
                            .change(
                                "min_account_age_seconds",
                                before.min_account_age_seconds,
                                server.min_account_age_seconds,
                            )
                            .change("applications", before.applications, server.applications)
                            .change("linked_only", before.linked_only, server.linked_only)
                            .change("thread_archive_hours", before.thread_archive_hours, server.thread_archive_hours);
                        if !entry.changes.is_empty() {
                            store::audit(conn, &account.id, entry).await?;
                        }
                        events.push(Payload::ServerUpdated(pb::ServerUpdated { server: Some(server.clone()) }));
                        Ok(server)
                    })
                    .await?;
                self.app.server_changed(&server).await;
                self.drop_picture(&old_icon, &server.icon_url, PictureOwner::Server(&server.id)).await;
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
                // Servers its channels are shown in, and that show its own, let go too.
                let ended = super::shared::take_server(&*sdb.read()?).await?;
                let files = crate::attachments::all(&*sdb.read()?).await?;
                self.app.servers.delete(&sdb.id, &actor).await?;
                self.app.server_gone(&sdb.id).await;
                // Its pictures and files go with it, here and wherever its uploads are kept.
                crate::cluster::pictures::drop_all(&self.app, &sdb.id).await;
                crate::attachments::drop_soon(&self.app, &sdb.id, files);
                super::shared::tell_ended(&self.app, &sdb.id, &actor, ended).await;
                tracing::info!(server = %sdb.id, "server deleted");
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
                let req = request.into_inner();
                let code = req.invite_code.trim();
                let sdb = self.app.servers.get(&req.server_id).await?;
                let server = sdb.server().await?;
                let now = now_ms();
                at_the_door(&account, &server, code, now)?;
                if server.applications {
                    return Err(Error::FailedPrecondition("this server takes applications: apply to join".into()));
                }
                let limits = sdb.limits(&self.app.settings().limits).await?;
                let user = account.user();
                let (member, used_up) = sdb
                    .write(&account.id, async |conn, events| {
                        let invite = let_in(conn, &sdb.id, &user.id, code, now).await?;
                        if let Some(limit) = limits.members
                            && store::usage_count(conn, "members").await? >= limit
                        {
                            return Err(Error::ResourceExhausted(format!("this server is full ({limit} members)")));
                        }
                        let server = store::load_server(conn).await?;
                        let member = store::add_member(conn, &user, &sdb.id, now, server.has_rules).await?;
                        events.push(Payload::MemberJoined(pb::MemberJoined { member: Some(member.clone()) }));
                        post_join(conn, &server, &user.id, now, events).await?;
                        // An application left over from when the server took them.
                        store::drop_application(conn, &sdb.id, &user.id, &user.id, events).await?;
                        Ok((member, use_invite(conn, invite.as_ref()).await?))
                    })
                    .await?;
                self.app.membership_changed(&account.id, &sdb.id, true).await;
                if used_up {
                    self.app.index_invite(&sdb.id, code, false).await;
                }
                Ok(pb::JoinServerResponse { server: Some(sdb.server().await?), member: Some(member) })
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
                let seat = self.membership(&account, &request.get_ref().server_id).await?;
                if seat.access.owner {
                    return Err(Error::FailedPrecondition("the owner can't leave; delete the server instead".into()));
                }
                let sdb = seat.sdb;
                sdb.write(&account.id, async |conn, events| {
                    // Leaving twice at once: the second finds nothing to take away.
                    if !remove_member(conn, &sdb.id, &account.id, pb::LeaveReason::Left, events).await? {
                        return Err(Error::NotFound("membership"));
                    }
                    Ok(())
                })
                .await?;
                self.app.membership_changed(&account.id, &sdb.id, false).await;
                self.forget_notifications(&sdb.id, None, Some(&account.id)).await;
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
                let Seat { sdb, access, .. } = self.membership(&account, &request.get_ref().server_id).await?;
                let conn = sdb.read()?;
                let mut members = query_all(
                    &conn,
                    &format!(
                        "SELECT {MEMBER_COLUMNS} FROM members JOIN users ON users.id = members.user_id
                         ORDER BY users.display_name, users.id"
                    ),
                    (),
                    member_row(&sdb.id),
                )
                .await?;
                permissions::attach_roles(&conn, &mut members).await?;
                let rules = permissions::load(&conn, &sdb.id).await?;
                members.sort_by_cached_key(|m| {
                    let id = m.user.as_ref().map(|u| u.id.as_str()).unwrap_or_default();
                    std::cmp::Reverse(rules.access(id, &m.role_ids).rank)
                });
                let manager = access.has(Permission::ManageServer);
                for member in &mut members {
                    store::scrub_sso(member, &account.id, manager);
                }
                Ok(pb::ListMembersResponse { members })
            }
            .await,
        )
    }

    async fn update_member(
        &self,
        request: Request<pb::UpdateMemberRequest>,
    ) -> Result<Response<pb::UpdateMemberResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access: me, .. } = self.membership(&account, &req.server_id).await?;
                let target_id = if req.user_id.is_empty() { account.id.clone() } else { req.user_id.clone() };
                let nickname = req.nickname.as_deref().map(|v| text("nickname", v, 0, 32)).transpose()?;
                if nickname.is_some() {
                    me.require(if target_id == account.id {
                        Permission::ChangeNickname
                    } else {
                        Permission::ManageNicknames
                    })?;
                }
                let member = sdb
                    .write(&account.id, async |conn, events| {
                        let (target, theirs) = target(conn, &sdb.id, &target_id).await?;
                        if target_id != account.id && !me.outranks(&theirs) {
                            return Err(Error::denied("you can only change people ranked below you"));
                        }
                        conn.execute(
                            "UPDATE members SET nickname = coalesce(?2, nickname) WHERE user_id = ?1",
                            (target_id.as_str(), nickname.as_deref()),
                        )
                        .await?;
                        let member =
                            store::member(conn, &sdb.id, &target_id).await?.ok_or(Error::NotFound("member"))?;
                        if target_id != account.id {
                            let entry = Audit::new(pb::AuditAction::MemberUpdate, &target_id).change(
                                "nickname",
                                &target.nickname,
                                &member.nickname,
                            );
                            if !entry.changes.is_empty() {
                                store::audit(conn, &account.id, entry).await?;
                            }
                        }
                        events.push(Payload::MemberUpdated(pb::MemberUpdated { member: Some(member.clone()) }));
                        Ok(member)
                    })
                    .await?;
                let mut member = member;
                store::scrub_sso(&mut member, &account.id, me.has(Permission::ManageServer));
                Ok(pb::UpdateMemberResponse { member: Some(member) })
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
                    self.with(viewer.account()?, server_id, Permission::ManageServer).await?.sdb
                };
                let own = sdb.own_limits().await?;
                let usage = pb::ServerUsage {
                    automod_checks_today: super::automod::checks_today(&sdb.id),
                    ..sdb.usage().await?
                };
                Ok(pb::GetServerUsageResponse {
                    usage: Some(usage),
                    limits: Some(effective_limits(own, &self.app.settings().limits)),
                    own_limits: Some(own),
                })
            }
            .await,
        )
    }

    async fn time_out_member(
        &self,
        request: Request<pb::TimeOutMemberRequest>,
    ) -> Result<Response<pb::TimeOutMemberResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access: me, .. } =
                    self.with(&account, &req.server_id, Permission::TimeOutMembers).await?;
                if !(0..=MAX_TIME_OUT_SECONDS).contains(&req.seconds) {
                    return Err(Error::invalid("a time-out can last up to 28 days"));
                }
                let reason = reason(&req.reason)?;
                let member = sdb
                    .write(&account.id, async |conn, events| {
                        let (target, theirs) = target(conn, &sdb.id, &req.user_id).await?;
                        outranks(&account.id, &me, &req.user_id, &theirs)?;
                        let until = (req.seconds > 0).then(|| now_ms() + req.seconds * 1000);
                        conn.execute(
                            "UPDATE members SET timed_out_until = ?2 WHERE user_id = ?1",
                            (req.user_id.as_str(), until),
                        )
                        .await?;
                        let member =
                            store::member(conn, &sdb.id, &req.user_id).await?.ok_or(Error::NotFound("member"))?;
                        let entry = Audit::new(pb::AuditAction::MemberTimeOut, &req.user_id).reason(&reason).change(
                            "timed_out_until",
                            audit_time(target.timed_out_until.as_ref()),
                            until.map(|t| t.to_string()).unwrap_or_default(),
                        );
                        store::audit(conn, &account.id, entry).await?;
                        events.push(Payload::MemberUpdated(pb::MemberUpdated { member: Some(member.clone()) }));
                        Ok(member)
                    })
                    .await?;
                let mut member = member;
                store::scrub_sso(&mut member, &account.id, me.has(Permission::ManageServer));
                Ok(pb::TimeOutMemberResponse { member: Some(member) })
            }
            .await,
        )
    }

    async fn kick_member(
        &self,
        request: Request<pb::KickMemberRequest>,
    ) -> Result<Response<pb::KickMemberResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access: me, .. } = self.with(&account, &req.server_id, Permission::KickMembers).await?;
                let reason = reason(&req.reason)?;
                sdb.write(&account.id, async |conn, events| {
                    let (_, theirs) = target(conn, &sdb.id, &req.user_id).await?;
                    outranks(&account.id, &me, &req.user_id, &theirs)?;
                    remove_member(conn, &sdb.id, &req.user_id, pb::LeaveReason::Kicked, events).await?;
                    store::audit(
                        conn,
                        &account.id,
                        Audit::new(pb::AuditAction::MemberKick, &req.user_id).reason(&reason),
                    )
                    .await?;
                    Ok(())
                })
                .await?;
                self.app.membership_changed(&req.user_id, &sdb.id, false).await;
                self.forget_notifications(&sdb.id, None, Some(&req.user_id)).await;
                tracing::info!(server = %sdb.id, user = %req.user_id, by = %account.id, "member kicked");
                Ok(pb::KickMemberResponse {})
            }
            .await,
        )
    }

    async fn ban_member(
        &self,
        request: Request<pb::BanMemberRequest>,
    ) -> Result<Response<pb::BanMemberResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access: me, .. } = self.with(&account, &req.server_id, Permission::BanMembers).await?;
                if !(0..=MAX_DELETE_MESSAGE_SECONDS).contains(&req.delete_message_seconds) {
                    return Err(Error::invalid("a ban can take up to seven days of messages with it"));
                }
                let reason = reason(&req.reason)?;
                let ban = async |conn: &turso::Connection, events: &mut Vec<Payload>| {
                    let user = store::user(conn, &req.user_id).await?.ok_or(Error::NotFound("member"))?;
                    match store::member_access(conn, &sdb.id, &req.user_id).await? {
                        Some((_, theirs)) => outranks(&account.id, &me, &req.user_id, &theirs)?,
                        None if req.user_id == account.id => {
                            return Err(Error::invalid("you can't do that to yourself"));
                        }
                        None => {}
                    }
                    let now = now_ms();
                    match conn
                        .execute(
                            "INSERT INTO bans (user_id, reason, banned_by, created_at) VALUES (?1, ?2, ?3, ?4)",
                            (req.user_id.as_str(), reason.as_str(), account.id.as_str(), now),
                        )
                        .await
                        .map_err(Error::from)
                    {
                        Err(err) if crate::db::is_unique_violation(&err) => {
                            return Err(Error::AlreadyExists("they're already banned".into()));
                        }
                        other => other?,
                    };
                    let was_member =
                        remove_member(conn, &sdb.id, &req.user_id, pb::LeaveReason::Banned, events).await?;
                    store::drop_application(conn, &sdb.id, &req.user_id, &account.id, events).await?;
                    let mut deleted = 0;
                    let mut files = Vec::new();
                    if req.delete_message_seconds > 0 {
                        let messages = query_all(
                            conn,
                            "SELECT id, channel_id, size, attachment_count, coalesce(thread_id, '') FROM messages
                             WHERE author_id = ?1 AND created_at >= ?2",
                            (req.user_id.as_str(), now - req.delete_message_seconds * 1000),
                            |r| {
                                Ok((
                                    r.get::<String>(0)?,
                                    r.get::<String>(1)?,
                                    r.get::<i64>(2)?,
                                    r.get::<i64>(3)?,
                                    r.get::<String>(4)?,
                                ))
                            },
                        )
                        .await?;
                        let mut change = UsageChange::default();
                        for (id, channel_id, size, attachments, thread_id) in messages {
                            // A reply already gone with the thread it was in is counted there.
                            if conn.execute("DELETE FROM messages WHERE id = ?1", [id.as_str()]).await? == 0 {
                                continue;
                            }
                            files.extend(crate::attachments::forget_message(conn, &id).await?);
                            change.messages -= 1;
                            change.message_bytes -= size;
                            change.attachments -= attachments;
                            files.extend(
                                super::threads::after_delete(conn, &channel_id, &id, &thread_id, events).await?,
                            );
                            events.push(Payload::MessageDeleted(pb::MessageDeleted { channel_id, message_id: id }));
                            deleted += 1;
                        }
                        if deleted > 0 {
                            store::add_usage(conn, change).await?;
                        }
                    }
                    let entry = Audit::new(pb::AuditAction::MemberBan, &req.user_id).reason(&reason).change(
                        "deleted_messages",
                        0,
                        deleted,
                    );
                    store::audit(conn, &account.id, entry).await?;
                    let ban = pb::Ban {
                        user: Some(user),
                        reason: reason.clone(),
                        banned_by_id: account.id.clone(),
                        created_at: Some(timestamp(now)),
                    };
                    Ok((ban, deleted, was_member, files))
                };
                // Taking their messages sweeps rows they could still be adding to.
                let (ban, deleted, was_member, files) = if req.delete_message_seconds > 0 {
                    sdb.write_alone(&account.id, ban).await?
                } else {
                    sdb.write(&account.id, ban).await?
                };
                crate::attachments::drop_soon(&self.app, &sdb.id, files);
                if was_member {
                    self.app.membership_changed(&req.user_id, &sdb.id, false).await;
                    self.forget_notifications(&sdb.id, None, Some(&req.user_id)).await;
                }
                tracing::info!(server = %sdb.id, user = %req.user_id, by = %account.id, deleted, "member banned");
                Ok(pb::BanMemberResponse { ban: Some(ban), deleted_messages: deleted })
            }
            .await,
        )
    }

    async fn unban_member(
        &self,
        request: Request<pb::UnbanMemberRequest>,
    ) -> Result<Response<pb::UnbanMemberResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::BanMembers).await?.sdb;
                sdb.write(&account.id, async |conn, _| {
                    if conn.execute("DELETE FROM bans WHERE user_id = ?1", [req.user_id.as_str()]).await? == 0 {
                        return Err(Error::NotFound("ban"));
                    }
                    store::audit(conn, &account.id, Audit::new(pb::AuditAction::MemberUnban, &req.user_id)).await
                })
                .await?;
                Ok(pb::UnbanMemberResponse {})
            }
            .await,
        )
    }

    async fn list_bans(&self, request: Request<pb::ListBansRequest>) -> Result<Response<pb::ListBansResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let sdb = self.with(&account, &request.get_ref().server_id, Permission::BanMembers).await?.sdb;
                let conn = sdb.read()?;
                let bans = query_all(
                    &conn,
                    &format!(
                        "SELECT {USER_COLUMNS}, bans.reason, bans.banned_by, bans.created_at
                         FROM bans JOIN users ON users.id = bans.user_id ORDER BY bans.created_at DESC"
                    ),
                    (),
                    |r| {
                        Ok(pb::Ban {
                            user: Some(store::user_row(r)?),
                            reason: r.get(7)?,
                            banned_by_id: r.get(8)?,
                            created_at: Some(timestamp(r.get(9)?)),
                        })
                    },
                )
                .await?;
                let ids: Vec<&str> = bans.iter().map(|b| b.banned_by_id.as_str()).collect();
                let moderators = users(&conn, &ids).await?;
                Ok(pb::ListBansResponse { bans, moderators })
            }
            .await,
        )
    }

    async fn list_audit_log(
        &self,
        request: Request<pb::ListAuditLogRequest>,
    ) -> Result<Response<pb::ListAuditLogResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ViewAuditLog).await?.sdb;
                let conn = sdb.read()?;
                let limit = if req.limit <= 0 { 50 } else { req.limit.min(100) } as i64;
                let rows = query_all(
                    &conn,
                    "SELECT id, actor_id, action, target_id, channel_name, reason, changes, created_at, role_name FROM audit
                     WHERE (?1 = '' OR id < ?1) AND (?2 = '' OR actor_id = ?2) AND (?3 = 0 OR action = ?3)
                     ORDER BY id DESC LIMIT ?4",
                    (req.before_id.as_str(), req.actor_id.as_str(), req.action as i64, limit + 1),
                    |r| {
                        Ok((
                            pb::AuditEntry {
                                id: r.get(0)?,
                                actor_id: r.get(1)?,
                                action: r.get(2)?,
                                target_id: r.get(3)?,
                                channel_name: r.get(4)?,
                                reason: r.get(5)?,
                                changes: vec![],
                                created_at: Some(timestamp(r.get(7)?)),
                                role_name: r.get(8)?,
                            },
                            r.get::<Option<Vec<u8>>>(6)?,
                        ))
                    },
                )
                .await?;
                let has_more = rows.len() as i64 > limit;
                let entries = rows
                    .into_iter()
                    .take(limit as usize)
                    .map(|(mut entry, changes)| {
                        entry.changes = audit_changes(changes)?;
                        Ok(entry)
                    })
                    .collect::<Result<Vec<_>>>()?;
                let ids: Vec<&str> = entries.iter().flat_map(|e| [e.actor_id.as_str(), e.target_id.as_str()]).collect();
                let users = users(&conn, &ids).await?;
                Ok(pb::ListAuditLogResponse { entries, users, has_more })
            }
            .await,
        )
    }

    async fn transfer_ownership(
        &self,
        request: Request<pb::TransferOwnershipRequest>,
    ) -> Result<Response<pb::TransferOwnershipResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let seat = self.membership(&account, &req.server_id).await?;
                if !seat.access.owner {
                    return Err(Error::denied("only the server's owner can hand it on"));
                }
                let sdb = seat.sdb;
                if req.user_id == account.id {
                    return Err(Error::invalid("you already own it"));
                }
                if let Some(limit) = self.app.settings().limits.servers_per_account
                    && self.app.owned_count(&req.user_id).await? >= limit
                {
                    return Err(Error::ResourceExhausted(format!(
                        "they already own {limit} servers, the most an account can here"
                    )));
                }
                let server = sdb
                    .write(&account.id, async |conn, events| {
                        let member =
                            store::member(conn, &sdb.id, &req.user_id).await?.ok_or(Error::NotFound("member"))?;
                        // Agents never own servers; someone has to answer for one.
                        if member.user.is_some_and(|user| user.kind == pb::AccountKind::Agent as i32) {
                            return Err(Error::FailedPrecondition("agents can't own servers".into()));
                        }
                        let now = now_ms();
                        conn.execute(
                            "UPDATE server SET owner_id = ?1, updated_at = ?2 WHERE owner_id = ?3",
                            (req.user_id.as_str(), now, account.id.as_str()),
                        )
                        .await?;
                        let entry = Audit::new(pb::AuditAction::OwnershipTransfer, &req.user_id).change(
                            "owner_id",
                            &account.id,
                            &req.user_id,
                        );
                        store::audit(conn, &account.id, entry).await?;
                        let server = store::load_server(conn).await?;
                        events.push(Payload::ServerUpdated(pb::ServerUpdated { server: Some(server.clone()) }));
                        Ok(server)
                    })
                    .await?;
                self.app.server_changed(&server).await;
                tracing::info!(server = %sdb.id, from = %account.id, to = %req.user_id, "ownership handed on");
                Ok(pb::TransferOwnershipResponse { server: Some(server) })
            }
            .await,
        )
    }
}
