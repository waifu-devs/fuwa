//! Getting into a server past its door: the rules members agree to before
//! they talk, and applications for servers that let people in by hand.

use tonic::{Request, Response, Status};

use super::messages::post_join;
use super::servers::{at_the_door, let_in, use_invite};
use super::{Api, Seat, respond, text};
use crate::error::{Error, Result};
use crate::id::{now_ms, timestamp};
use crate::pb::{self, Permission, join_service_server::JoinService};
use crate::servers::{self as store, Audit, Payload};

/// As many rules as Discord's rules screening takes.
const MAX_RULES: usize = 16;
const MAX_RULE: usize = 300;
const MAX_QUESTIONS: usize = 5;
const MAX_PROMPT: usize = 200;
const MAX_LINE_ANSWER: usize = 300;
const MAX_PARAGRAPH_ANSWER: usize = 1000;
/// As Discord's welcome screen has it.
const MAX_WELCOME_CHANNELS: usize = 5;
const MAX_WELCOME: usize = 300;
const MAX_WELCOME_CHANNEL: usize = 60;

/// A form as it may be saved: trimmed, within its limits, nothing blank.
fn checked_form(form: pb::JoinForm) -> Result<pb::JoinForm> {
    if form.rules.len() > MAX_RULES {
        return Err(Error::invalid(format!("a server can have up to {MAX_RULES} rules")));
    }
    if form.questions.len() > MAX_QUESTIONS {
        return Err(Error::invalid(format!("a server can ask up to {MAX_QUESTIONS} questions")));
    }
    let rules = form.rules.iter().map(|rule| text("a rule", rule, 1, MAX_RULE)).collect::<Result<Vec<_>>>()?;
    let questions = form
        .questions
        .into_iter()
        .map(|q| Ok(pb::JoinQuestion { prompt: text("a question", &q.prompt, 1, MAX_PROMPT)?, ..q }))
        .collect::<Result<Vec<_>>>()?;
    Ok(pb::JoinForm { rules, questions })
}

/// Answers matched to the questions as they stand: one each, in order, naming
/// the question that was shown.
fn checked_answers(
    questions: &[pb::JoinQuestion],
    answers: Vec<pb::ApplicationAnswer>,
) -> Result<Vec<pb::ApplicationAnswer>> {
    let changed = || Error::FailedPrecondition("this server's questions just changed; look them over again".into());
    if answers.len() != questions.len() {
        return Err(changed());
    }
    questions
        .iter()
        .zip(answers)
        .map(|(question, answer)| {
            if answer.question.trim() != question.prompt {
                return Err(changed());
            }
            let max = if question.paragraph { MAX_PARAGRAPH_ANSWER } else { MAX_LINE_ANSWER };
            let min = usize::from(question.required);
            let value = text("an answer", &answer.answer, min, max).map_err(|_| {
                Error::invalid(if min == 1 && answer.answer.trim().is_empty() {
                    format!("\"{}\" needs an answer", question.prompt)
                } else {
                    format!("answers to \"{}\" can be at most {max} characters", question.prompt)
                })
            })?;
            Ok(pb::ApplicationAnswer { question: question.prompt.clone(), answer: value })
        })
        .collect()
}

/// A welcome screen as it may be saved: its channels real and once each,
/// its emoji Unicode or the server's own.
async fn checked_welcome(
    conn: &turso::Connection,
    server_id: &str,
    welcome: pb::WelcomeScreen,
) -> Result<pb::WelcomeScreen> {
    let description = text("the welcome", &welcome.description, 0, MAX_WELCOME)?;
    if welcome.channels.len() > MAX_WELCOME_CHANNELS {
        return Err(Error::invalid(format!("a welcome screen suggests up to {MAX_WELCOME_CHANNELS} channels")));
    }
    let emojis = store::load_emojis(conn, server_id).await?;
    let mut channels: Vec<pb::WelcomeChannel> = Vec::new();
    for item in welcome.channels {
        let channel =
            store::load_channel(conn, server_id, &item.channel_id).await?.ok_or(Error::NotFound("channel"))?;
        if channel.r#type == pb::ChannelType::Category as i32 {
            return Err(Error::invalid("suggest channels, not categories"));
        }
        if channels.iter().any(|c| c.channel_id == channel.id) {
            return Err(Error::invalid(format!("#{} is on the welcome screen twice", channel.name)));
        }
        let emoji = item.emoji.trim().to_string();
        let custom = emoji.strip_prefix("<:").and_then(|rest| rest.strip_suffix('>')).and_then(|r| r.rsplit_once(':'));
        let fine = match custom {
            Some((_, id)) => emojis.iter().any(|e| e.id == id),
            None => emoji.chars().count() <= 16 && !emoji.chars().any(|c| c.is_ascii_alphanumeric() || c == '<'),
        };
        if !fine {
            return Err(Error::invalid("welcome emoji are a Unicode emoji or one of the server's own"));
        }
        channels.push(pb::WelcomeChannel {
            channel_id: channel.id,
            description: text("a channel's note", &item.description, 0, MAX_WELCOME_CHANNEL)?,
            emoji,
        });
    }
    if welcome.enabled && description.is_empty() && channels.is_empty() {
        return Err(Error::invalid("say hello or suggest a channel before turning the welcome screen on"));
    }
    Ok(pb::WelcomeScreen { enabled: welcome.enabled, description, channels })
}

#[tonic::async_trait]
impl JoinService for Api {
    async fn get_welcome_screen(
        &self,
        request: Request<pb::GetWelcomeScreenRequest>,
    ) -> Result<Response<pb::GetWelcomeScreenResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let Seat { sdb, access, .. } = self.membership(&account, &request.get_ref().server_id).await?;
                let mut welcome = store::load_welcome(&*sdb.read()?).await?;
                if !access.has(Permission::ManageServer) {
                    if !welcome.enabled {
                        welcome = pb::WelcomeScreen::default();
                    }
                    welcome.channels.retain(|c| access.can_see(&c.channel_id));
                }
                Ok(pb::GetWelcomeScreenResponse { welcome_screen: Some(welcome) })
            }
            .await,
        )
    }

    async fn set_welcome_screen(
        &self,
        request: Request<pb::SetWelcomeScreenRequest>,
    ) -> Result<Response<pb::SetWelcomeScreenResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageServer).await?.sdb;
                let draft = req.welcome_screen.unwrap_or_default();
                let (welcome, server) = sdb
                    .write(&account.id, async |conn, events| {
                        let welcome = checked_welcome(conn, &sdb.id, draft.clone()).await?;
                        let before = store::load_welcome(conn).await?;
                        store::save_welcome(conn, &welcome).await?;
                        if before != welcome {
                            let entry = Audit::new(pb::AuditAction::WelcomeScreenUpdate, "")
                                .change("enabled", before.enabled, welcome.enabled)
                                .change("channels", before.channels.len(), welcome.channels.len());
                            store::audit(conn, &account.id, entry).await?;
                        }
                        let server = store::load_server(conn).await?;
                        events.push(Payload::ServerUpdated(pb::ServerUpdated { server: Some(server.clone()) }));
                        Ok((welcome, server))
                    })
                    .await?;
                self.app.server_changed(&server).await;
                Ok(pb::SetWelcomeScreenResponse { welcome_screen: Some(welcome) })
            }
            .await,
        )
    }

    async fn get_join_form(
        &self,
        request: Request<pb::GetJoinFormRequest>,
    ) -> Result<Response<pb::GetJoinFormResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.app.servers.get(&req.server_id).await?;
                let conn = sdb.read()?;
                let server = store::load_server(&conn).await?;
                let code = req.invite_code.trim();
                let allowed = server.discoverable
                    || account.admin
                    || store::member(&conn, &sdb.id, &account.id).await?.is_some()
                    || store::load_application(&conn, &sdb.id, &account.id).await?.is_some()
                    || (!code.is_empty()
                        && store::load_invite(&conn, &sdb.id, code)
                            .await?
                            .is_some_and(|invite| store::invite_works(&invite, now_ms())));
                if !allowed {
                    return Err(Error::NotFound("server"));
                }
                Ok(pb::GetJoinFormResponse { form: Some(store::load_join_form(&conn).await?) })
            }
            .await,
        )
    }

    async fn set_join_form(
        &self,
        request: Request<pb::SetJoinFormRequest>,
    ) -> Result<Response<pb::SetJoinFormResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageServer).await?.sdb;
                let form = checked_form(req.form.unwrap_or_default())?;
                let server = sdb
                    .write(&account.id, async |conn, events| {
                        let before = store::load_join_form(conn).await?;
                        store::save_join_form(conn, &form).await?;
                        // Without rules there's nothing left to agree to.
                        if form.rules.is_empty() {
                            conn.execute("UPDATE members SET pending = 0 WHERE pending = 1", ()).await?;
                        }
                        if before != form {
                            // The counts, when they changed; a reworded rule is an entry with no changes.
                            let entry = Audit::new(pb::AuditAction::JoinFormUpdate, "")
                                .change("rules", before.rules.len(), form.rules.len())
                                .change("questions", before.questions.len(), form.questions.len());
                            store::audit(conn, &account.id, entry).await?;
                        }
                        let server = store::load_server(conn).await?;
                        events.push(Payload::ServerUpdated(pb::ServerUpdated { server: Some(server.clone()) }));
                        Ok(server)
                    })
                    .await?;
                self.app.server_changed(&server).await;
                Ok(pb::SetJoinFormResponse { form: Some(form) })
            }
            .await,
        )
    }

    async fn agree_to_rules(
        &self,
        request: Request<pb::AgreeToRulesRequest>,
    ) -> Result<Response<pb::AgreeToRulesResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let Seat { sdb, member, .. } = self.membership(&account, &request.get_ref().server_id).await?;
                if !member.pending {
                    return Ok(pb::AgreeToRulesResponse { member: Some(member) });
                }
                let member = sdb
                    .write(&account.id, async |conn, events| {
                        conn.execute("UPDATE members SET pending = 0 WHERE user_id = ?1", [account.id.as_str()])
                            .await?;
                        let member =
                            store::member(conn, &sdb.id, &account.id).await?.ok_or(Error::NotFound("membership"))?;
                        events.push(Payload::MemberUpdated(pb::MemberUpdated { member: Some(member.clone()) }));
                        Ok(member)
                    })
                    .await?;
                Ok(pb::AgreeToRulesResponse { member: Some(member) })
            }
            .await,
        )
    }

    async fn apply_to_join(
        &self,
        request: Request<pb::ApplyToJoinRequest>,
    ) -> Result<Response<pb::ApplyToJoinResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let code = req.invite_code.trim();
                let sdb = self.app.servers.get(&req.server_id).await?;
                let server = sdb.server().await?;
                let now = now_ms();
                at_the_door(&account, &server, code, now)?;
                if !server.applications {
                    return Err(Error::FailedPrecondition("this server lets people join straight away".into()));
                }
                let user = account.user();
                let (application, used_up) = sdb
                    .write(&account.id, async |conn, events| {
                        let invite = let_in(conn, &sdb.id, &user.id, code, now).await?;
                        if store::load_application(conn, &sdb.id, &user.id)
                            .await?
                            .is_some_and(|a| a.status == pb::ApplicationStatus::Pending as i32)
                        {
                            return Err(Error::AlreadyExists("you've applied already; it's waiting for review".into()));
                        }
                        let form = store::load_join_form(conn).await?;
                        let application = pb::Application {
                            server_id: sdb.id.clone(),
                            user: Some(user.clone()),
                            answers: checked_answers(&form.questions, req.answers.clone())?,
                            status: pb::ApplicationStatus::Pending as i32,
                            account_created_at: Some(timestamp(account.created_at)),
                            created_at: Some(timestamp(now)),
                            ..Default::default()
                        };
                        store::save_application(conn, &application).await?;
                        events.push(Payload::ApplicationUpdated(pb::ApplicationUpdated {
                            application: Some(application.clone()),
                        }));
                        Ok((application, use_invite(conn, invite.as_ref()).await?))
                    })
                    .await?;
                if used_up {
                    self.app.index_invite(&sdb.id, code, false).await;
                }
                tracing::info!(server = %sdb.id, account = %account.id, "applied to join");
                Ok(pb::ApplyToJoinResponse { application: Some(application) })
            }
            .await,
        )
    }

    async fn get_application(
        &self,
        request: Request<pb::GetApplicationRequest>,
    ) -> Result<Response<pb::GetApplicationResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let sdb = self.app.servers.get(&request.get_ref().server_id).await?;
                let conn = sdb.read()?;
                if store::member(&conn, &sdb.id, &account.id).await?.is_some() {
                    let server = store::load_server(&conn).await?;
                    return Ok(pb::GetApplicationResponse { application: None, member: true, server: Some(server) });
                }
                let application = store::load_application(&conn, &sdb.id, &account.id).await?;
                if application.is_none() && !store::load_server(&conn).await?.discoverable {
                    return Err(Error::NotFound("server"));
                }
                Ok(pb::GetApplicationResponse { application, member: false, server: None })
            }
            .await,
        )
    }

    async fn withdraw_application(
        &self,
        request: Request<pb::WithdrawApplicationRequest>,
    ) -> Result<Response<pb::WithdrawApplicationResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let sdb = self.app.servers.get(&request.get_ref().server_id).await?;
                sdb.write(&account.id, async |conn, events| {
                    if !store::drop_application(conn, &sdb.id, &account.id, &account.id, events).await? {
                        return Err(Error::NotFound("application"));
                    }
                    Ok(())
                })
                .await?;
                Ok(pb::WithdrawApplicationResponse {})
            }
            .await,
        )
    }

    async fn list_applications(
        &self,
        request: Request<pb::ListApplicationsRequest>,
    ) -> Result<Response<pb::ListApplicationsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let sdb = self.with(&account, &request.get_ref().server_id, Permission::KickMembers).await?.sdb;
                let applications =
                    store::load_applications(&*sdb.read()?, &sdb.id, pb::ApplicationStatus::Pending).await?;
                Ok(pb::ListApplicationsResponse { applications })
            }
            .await,
        )
    }

    async fn review_application(
        &self,
        request: Request<pb::ReviewApplicationRequest>,
    ) -> Result<Response<pb::ReviewApplicationResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::KickMembers).await?;
                let reason = text("reason", &req.reason, 0, 512)?;
                let limits = sdb.limits(&self.app.settings().limits).await?;
                let member = sdb
                    .write(&account.id, async |conn, events| {
                        let application = store::load_application(conn, &sdb.id, &req.user_id)
                            .await?
                            .filter(|a| a.status == pb::ApplicationStatus::Pending as i32)
                            .ok_or(Error::NotFound("application"))?;
                        let user = application.user.clone().unwrap_or_default();
                        let now = now_ms();
                        if !req.approve {
                            conn.execute(
                                "UPDATE applications SET status = ?2, reason = ?3, reviewed_by = ?4, reviewed_at = ?5
                                 WHERE user_id = ?1",
                                (
                                    user.id.as_str(),
                                    pb::ApplicationStatus::Rejected as i64,
                                    reason.as_str(),
                                    account.id.as_str(),
                                    now,
                                ),
                            )
                            .await?;
                            let entry = Audit::new(pb::AuditAction::ApplicationReject, &user.id).reason(&reason);
                            store::audit(conn, &account.id, entry).await?;
                            events.push(store::closed_application(
                                &sdb.id,
                                user,
                                pb::ApplicationStatus::Rejected,
                                &account.id,
                            ));
                            return Ok(None);
                        }
                        if store::member(conn, &sdb.id, &user.id).await?.is_some() {
                            return Err(Error::AlreadyExists("they're a member already".into()));
                        }
                        if let Some(limit) = limits.members
                            && store::usage_count(conn, "members").await? >= limit
                        {
                            return Err(Error::ResourceExhausted(format!("this server is full ({limit} members)")));
                        }
                        conn.execute("DELETE FROM applications WHERE user_id = ?1", [user.id.as_str()]).await?;
                        // They agreed to the rules when they applied.
                        let member = store::add_member(conn, &user, &sdb.id, now, false).await?;
                        events.push(Payload::MemberJoined(pb::MemberJoined { member: Some(member.clone()) }));
                        post_join(conn, &store::load_server(conn).await?, &user.id, now, events).await?;
                        let entry = Audit::new(pb::AuditAction::ApplicationApprove, &user.id).reason(&reason);
                        store::audit(conn, &account.id, entry).await?;
                        events.push(store::closed_application(
                            &sdb.id,
                            user,
                            pb::ApplicationStatus::Approved,
                            &account.id,
                        ));
                        Ok(Some(member))
                    })
                    .await?;
                if member.is_some() {
                    self.app.membership_changed(&req.user_id, &sdb.id, true).await;
                }
                let mut member = member;
                if let Some(member) = &mut member {
                    store::scrub_sso(member, &account.id, access.has(Permission::ManageServer));
                }
                Ok(pb::ReviewApplicationResponse { member })
            }
            .await,
        )
    }
}
