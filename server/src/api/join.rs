//! Getting into a server past its door: the rules members agree to before
//! they talk, and applications for servers that let people in by hand.

use tonic::{Request, Response, Status};

use super::messages::post_join;
use super::servers::{at_the_door, let_in, use_invite};
use super::{Api, Seat, respond, text};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::pb::{self, Permission, join_service_server::JoinService};
use crate::permissions::{self, Access, Bits, bit};
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
const MAX_STEPS: usize = 6;
const MAX_STEP_TITLE: usize = 80;
const MAX_STEP_DESCRIPTION: usize = 200;
const MAX_OPTIONS: usize = 12;
const MAX_OPTION_LABEL: usize = 50;
const MAX_OPTION_DESCRIPTION: usize = 100;
const MAX_OPTION_ROLES: usize = 5;
const MAX_OPTION_CHANNELS: usize = 5;
const MAX_HELLO: usize = 200;

/// What a role handed out by onboarding may never carry: anyone could pick
/// it, so nothing that moderates or manages the server.
const POWERS: Bits = permissions::ADMIN | bit(Permission::Administrator) | bit(Permission::ManageRoles);

/// Whether anyone may get the role by picking it in onboarding.
fn harmless(role: &pb::Role) -> bool {
    permissions::from_list(&role.permissions).is_ok_and(|bits| bits & POWERS == 0)
}

/// A welcome or onboarding emoji: Unicode, or one of the server's own.
fn checked_emoji(emojis: &[pb::Emoji], emoji: &str, what: &str) -> Result<String> {
    let emoji = emoji.trim().to_string();
    let custom = emoji.strip_prefix("<:").and_then(|rest| rest.strip_suffix('>')).and_then(|r| r.rsplit_once(':'));
    let fine = match custom {
        Some((_, id)) => emojis.iter().any(|e| e.id == id),
        None => emoji.chars().count() <= 16 && !emoji.chars().any(|c| c.is_ascii_alphanumeric() || c == '<'),
    };
    if !fine {
        return Err(Error::invalid(format!("{what} emoji are a Unicode emoji or one of the server's own")));
    }
    Ok(emoji)
}

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
        let emoji = checked_emoji(&emojis, &item.emoji, "welcome")?;
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

/// An onboarding as it may be saved: within its limits, its roles ones the
/// caller ranks above and that hand out no powers, its channels real. Steps
/// and options keep the ids they had in `before` and new ones get theirs.
async fn checked_onboarding(
    conn: &turso::Connection,
    server_id: &str,
    access: &Access,
    draft: pb::Onboarding,
    before: &pb::Onboarding,
) -> Result<pb::Onboarding> {
    if draft.steps.len() > MAX_STEPS {
        return Err(Error::invalid(format!("onboarding has up to {MAX_STEPS} steps")));
    }
    let emojis = store::load_emojis(conn, server_id).await?;
    let roles = permissions::roles(conn, server_id).await?;
    let known_step = |id: &str| before.steps.iter().any(|s| s.id == id);
    let known_option = |id: &str| before.steps.iter().flat_map(|s| &s.options).any(|o| o.id == id);
    let mut steps = Vec::with_capacity(draft.steps.len());
    for step in draft.steps {
        let kind = pb::OnboardingStepKind::try_from(step.kind).unwrap_or(pb::OnboardingStepKind::Unspecified);
        let title = text("a step's title", &step.title, 1, MAX_STEP_TITLE)?;
        let description = text("a step's words", &step.description, 0, MAX_STEP_DESCRIPTION)?;
        let id = if !step.id.is_empty() && known_step(&step.id) { step.id } else { new_id() };
        let mut checked = pb::OnboardingStep {
            id,
            kind: kind as i32,
            title,
            description,
            skippable: step.skippable,
            ..Default::default()
        };
        match kind {
            pb::OnboardingStepKind::Pick => {
                if step.options.is_empty() || step.options.len() > MAX_OPTIONS {
                    return Err(Error::invalid(format!("\"{}\" needs 1 to {MAX_OPTIONS} choices", checked.title)));
                }
                checked.multiple = step.multiple;
                for option in step.options {
                    if option.role_ids.len() > MAX_OPTION_ROLES || option.channel_ids.len() > MAX_OPTION_CHANNELS {
                        return Err(Error::invalid(format!(
                            "a choice hands out up to {MAX_OPTION_ROLES} roles and suggests up to {MAX_OPTION_CHANNELS} channels"
                        )));
                    }
                    let mut role_ids: Vec<String> = Vec::new();
                    for role_id in option.role_ids {
                        let role = roles.iter().find(|r| r.id == role_id).ok_or(Error::NotFound("role"))?;
                        if role.id == server_id {
                            return Err(Error::invalid("everyone has @everyone"));
                        }
                        if !access.above(role.position.into()) {
                            return Err(Error::denied(
                                "onboarding can only hand out roles ranked below your highest role",
                            ));
                        }
                        if !harmless(role) {
                            return Err(Error::invalid(format!(
                                "@{} can moderate or manage the server, so onboarding can't hand it out",
                                role.name
                            )));
                        }
                        if !role_ids.contains(&role.id) {
                            role_ids.push(role.id.clone());
                        }
                    }
                    let mut channel_ids: Vec<String> = Vec::new();
                    for channel_id in option.channel_ids {
                        let channel = store::load_channel(conn, server_id, &channel_id)
                            .await?
                            .ok_or(Error::NotFound("channel"))?;
                        if channel.r#type == pb::ChannelType::Category as i32 {
                            return Err(Error::invalid("suggest channels, not categories"));
                        }
                        if !channel_ids.contains(&channel.id) {
                            channel_ids.push(channel.id);
                        }
                    }
                    let id = if !option.id.is_empty() && known_option(&option.id) { option.id } else { new_id() };
                    checked.options.push(pb::OnboardingOption {
                        id,
                        label: text("a choice", &option.label, 1, MAX_OPTION_LABEL)?,
                        description: text("a choice's words", &option.description, 0, MAX_OPTION_DESCRIPTION)?,
                        emoji: checked_emoji(&emojis, &option.emoji, "onboarding")?,
                        role_ids,
                        channel_ids,
                    });
                }
            }
            pb::OnboardingStepKind::Rules => {
                if steps.iter().any(|s: &pb::OnboardingStep| s.kind == pb::OnboardingStepKind::Rules as i32) {
                    return Err(Error::invalid("onboarding shows the rules once"));
                }
                // Agreeing is never optional.
                checked.skippable = false;
            }
            pb::OnboardingStepKind::Hello => {
                let channel =
                    store::load_channel(conn, server_id, &step.channel_id).await?.ok_or(Error::NotFound("channel"))?;
                if !matches!(
                    pb::ChannelType::try_from(channel.r#type),
                    Ok(pb::ChannelType::Text | pb::ChannelType::Announcement)
                ) {
                    return Err(Error::invalid("people say hello in a text channel"));
                }
                checked.channel_id = channel.id;
                checked.hello = text("the hello", &step.hello, 0, MAX_HELLO)?;
            }
            pb::OnboardingStepKind::Unspecified => {
                return Err(Error::invalid("a step picks, shows the rules or says hello"));
            }
        }
        steps.push(checked);
    }
    if draft.enabled && steps.is_empty() {
        return Err(Error::invalid("add a step before turning onboarding on"));
    }
    Ok(pb::Onboarding { enabled: draft.enabled, steps, set_by: String::new() })
}

#[tonic::async_trait]
impl JoinService for Api {
    async fn get_onboarding(
        &self,
        request: Request<pb::GetOnboardingRequest>,
    ) -> Result<Response<pb::GetOnboardingResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let Seat { sdb, access, .. } = self.membership(&account, &request.get_ref().server_id).await?;
                let mut onboarding = store::load_onboarding(&*sdb.read()?).await?;
                onboarding.set_by.clear();
                if !access.has(Permission::ManageServer) {
                    if !onboarding.enabled {
                        onboarding = pb::Onboarding::default();
                    }
                    for step in &mut onboarding.steps {
                        for option in &mut step.options {
                            option.channel_ids.retain(|id| access.can_see(id));
                        }
                    }
                }
                Ok(pb::GetOnboardingResponse { onboarding: Some(onboarding) })
            }
            .await,
        )
    }

    async fn set_onboarding(
        &self,
        request: Request<pb::SetOnboardingRequest>,
    ) -> Result<Response<pb::SetOnboardingResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::ManageServer).await?;
                let draft = req.onboarding.unwrap_or_default();
                let (onboarding, server) = sdb
                    .write(&account.id, async |conn, events| {
                        let before = store::load_onboarding(conn).await?;
                        let mut onboarding = checked_onboarding(conn, &sdb.id, &access, draft.clone(), &before).await?;
                        // Its roles are handed out only while they rank below whoever saved it.
                        onboarding.set_by = account.id.clone();
                        store::save_onboarding(conn, &onboarding).await?;
                        if (before.enabled, &before.steps) != (onboarding.enabled, &onboarding.steps) {
                            let options = |o: &pb::Onboarding| o.steps.iter().map(|s| s.options.len()).sum::<usize>();
                            let entry = Audit::new(pb::AuditAction::OnboardingUpdate, "")
                                .change("enabled", before.enabled, onboarding.enabled)
                                .change("steps", before.steps.len(), onboarding.steps.len())
                                .change("options", options(&before), options(&onboarding));
                            store::audit(conn, &account.id, entry).await?;
                        }
                        let server = store::load_server(conn).await?;
                        events.push(Payload::ServerUpdated(pb::ServerUpdated { server: Some(server.clone()) }));
                        Ok((onboarding, server))
                    })
                    .await?;
                self.app.server_changed(&server).await;
                let onboarding = pb::Onboarding { set_by: String::new(), ..onboarding };
                Ok(pb::SetOnboardingResponse { onboarding: Some(onboarding) })
            }
            .await,
        )
    }

    async fn finish_onboarding(
        &self,
        request: Request<pb::FinishOnboardingRequest>,
    ) -> Result<Response<pb::FinishOnboardingResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                if req.option_ids.len() > MAX_STEPS * MAX_OPTIONS {
                    return Err(Error::invalid("that's more choices than onboarding has"));
                }
                let Seat { sdb, .. } = self.membership(&account, &req.server_id).await?;
                let member = sdb
                    .write(&account.id, async |conn, events| {
                        let onboarding = store::load_onboarding(conn).await?;
                        let roles = permissions::roles(conn, &sdb.id).await?;
                        // The steps as saved: one pick where only one is allowed,
                        // and at least one where the step can't be skipped.
                        if onboarding.enabled {
                            for step in &onboarding.steps {
                                if pb::OnboardingStepKind::try_from(step.kind) != Ok(pb::OnboardingStepKind::Pick) {
                                    continue;
                                }
                                let picked = step.options.iter().filter(|o| req.option_ids.contains(&o.id)).count();
                                if picked > 1 && !step.multiple {
                                    return Err(Error::invalid(format!("pick one for “{}”", step.title)));
                                }
                                if picked == 0 && !step.skippable {
                                    return Err(Error::invalid(format!("pick at least one for “{}”", step.title)));
                                }
                            }
                        }
                        // Roles go out only while the person who set them up could
                        // still set them up: a member who manages the server, ranked
                        // above the role.
                        let rules = permissions::load(conn, &sdb.id).await?;
                        let setter = match onboarding.set_by.as_str() {
                            "" => None,
                            id if store::member(conn, &sdb.id, id).await?.is_some() => {
                                let held: Vec<String> = crate::db::query_all(
                                    conn,
                                    "SELECT role_id FROM member_roles WHERE user_id = ?1",
                                    [id],
                                    |r| r.get::<String>(0),
                                )
                                .await?;
                                Some(rules.access(id, &held)).filter(|a| a.has(Permission::ManageServer))
                            }
                            _ => None,
                        };
                        let mut give: Vec<&pb::Role> = Vec::new();
                        let mut take: Vec<&pb::Role> = Vec::new();
                        let options = onboarding.steps.iter().flat_map(|s| &s.options);
                        for option in options {
                            let picked = onboarding.enabled && req.option_ids.contains(&option.id);
                            for role in option.role_ids.iter().filter_map(|id| roles.iter().find(|r| r.id == *id)) {
                                if role.id == sdb.id || !harmless(role) {
                                    continue;
                                }
                                if picked && !setter.as_ref().is_some_and(|s| s.above(role.position.into())) {
                                    continue;
                                }
                                if picked {
                                    give.push(role);
                                } else {
                                    take.push(role);
                                }
                            }
                        }
                        // A role two options hand out stays when either is picked.
                        take.retain(|role| !give.iter().any(|g| g.id == role.id));
                        let mut changed = 0;
                        for role in &give {
                            changed += conn
                                .execute(
                                    "INSERT INTO member_roles (user_id, role_id) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
                                    (account.id.as_str(), role.id.as_str()),
                                )
                                .await?;
                        }
                        for role in &take {
                            changed += conn
                                .execute(
                                    "DELETE FROM member_roles WHERE user_id = ?1 AND role_id = ?2",
                                    (account.id.as_str(), role.id.as_str()),
                                )
                                .await?;
                        }
                        conn.execute("UPDATE members SET onboarded_at = ?2 WHERE user_id = ?1", (account.id.as_str(), now_ms()))
                            .await?;
                        let member =
                            store::member(conn, &sdb.id, &account.id).await?.ok_or(Error::NotFound("membership"))?;
                        if changed > 0 {
                            let names = |list: &[&pb::Role]| {
                                let mut names: Vec<&str> = list.iter().map(|r| r.name.as_str()).collect();
                                names.sort_unstable();
                                names.dedup();
                                names.join(", ")
                            };
                            let entry = Audit::new(pb::AuditAction::MemberRolesUpdate, &account.id)
                                .reason("onboarding")
                                .change("roles", names(&take), names(&give));
                            store::audit(conn, &account.id, entry).await?;
                        }
                        events.push(Payload::MemberUpdated(pb::MemberUpdated { member: Some(member.clone()) }));
                        Ok(member)
                    })
                    .await?;
                Ok(pb::FinishOnboardingResponse { member: Some(member) })
            }
            .await,
        )
    }

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
                tracing::info!("applied to join");
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
