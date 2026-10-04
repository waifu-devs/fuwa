use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use super::messages::decode_extras;
use super::{Api, respond};
use crate::app::App;
use crate::auth::{self, Caller};
use crate::cpb;
use crate::db::query_all;
use crate::error::{Error, Result};
use crate::id::{millis, now_ms, timestamp};
use crate::node::Account;
use crate::pb::{self, account_service_server::AccountService};
use crate::servers::{self as store, ServerDb};
use crate::twofactor;

/// Messages read from a server at a time while exporting.
const EXPORT_PAGE: i64 = 500;

/// What one server arrangement may hold, so a row stays a few tens of KB.
const ARRANGED_SERVERS: usize = 1000;
const ARRANGED_FOLDERS: usize = 200;
const FOLDER_ID_CHARS: usize = 32;
const FOLDER_NAME_CHARS: usize = 32;

type ExportStream = Pin<Box<dyn Stream<Item = Result<pb::ExportDataResponse, Status>> + Send>>;

impl Api {
    /// Checks the account's password, for changes that need it again.
    async fn confirm_password(&self, account: &Account, password: &str) -> Result<()> {
        if !account.has_password() {
            return Err(Error::FailedPrecondition(if account.kind == pb::AccountKind::Agent {
                "agents sign in with their token, not a password".into()
            } else {
                "this account signs in through waifu.dev, not with a password".into()
            }));
        }
        let guesses = format!("password:{}", account.id);
        self.app.limiter.attempt(&guesses)?;
        let hash = self.app.node()?.password_hash(&account.id).await?;
        if !auth::verify_password(password.to_string(), hash).await? {
            return Err(Error::denied("that password is wrong"));
        }
        self.app.limiter.succeeded(&guesses);
        Ok(())
    }

    /// Checks a two-step code (from the app, or a backup code, which it uses up).
    async fn confirm_code(&self, account: &Account, code: &str) -> Result<()> {
        let guesses = format!("two-factor:{}", account.id);
        self.app.limiter.attempt(&guesses)?;
        if !twofactor::check(self.app.node()?, &account.id, code).await? {
            return Err(Error::denied("that code didn't work"));
        }
        self.app.limiter.succeeded(&guesses);
        Ok(())
    }

    /// The caller's arrangement, without servers they've left since.
    async fn arrangement(&self, account: &Account) -> Result<(Vec<pb::ServerRailItem>, Option<i64>)> {
        Self::not_an_agent(account)?;
        let (items, updated_at) = self.app.node()?.server_arrangement(&account.id).await?;
        let index = &self.app.index;
        let items = tidy_arrangement(items, |id| index.is_member(&account.id, id))?;
        Ok((items, updated_at))
    }

    /// Agents have no rail to arrange.
    fn not_an_agent(account: &Account) -> Result<()> {
        if account.kind == pb::AccountKind::Agent {
            return Err(Error::denied("agents don't arrange servers"));
        }
        Ok(())
    }

    fn two_factor_on(account: &Account) -> Result<()> {
        if !account.two_factor {
            return Err(Error::FailedPrecondition("two-step sign-in isn't on".into()));
        }
        Ok(())
    }

    async fn change_notifications(
        &self,
        account: &Account,
        req: pb::UpdateNotificationSettingsRequest,
    ) -> Result<pb::NotificationSettings> {
        let wanted = req.settings.ok_or_else(|| Error::invalid("settings are required"))?;
        let paths = req.update_mask.map(|mask| mask.paths).unwrap_or_default();
        if paths.is_empty() {
            return Err(Error::invalid("update_mask names nothing to change"));
        }
        let server = self.app.index.summary(&wanted.server_id).ok_or(Error::NotFound("server"))?;
        if !self.app.index.is_member(&account.id, &server.id) {
            return Err(Error::denied("join this server first"));
        }
        if !wanted.channel_id.is_empty() && !self.app.channel_exists(&server.id, &wanted.channel_id).await? {
            return Err(Error::NotFound("channel"));
        }
        for path in &paths {
            match path.as_str() {
                "level" => {
                    pb::NotificationLevel::try_from(wanted.level)
                        .map_err(|_| Error::invalid("level isn't a notification level"))?;
                }
                "muted" | "muted_until" => {}
                "suppress_everyone" if wanted.channel_id.is_empty() => {}
                "suppress_everyone" => {
                    return Err(Error::invalid("suppress_everyone is set per server, not per channel"));
                }
                other => return Err(Error::invalid(format!("{other} isn't a notification setting"))),
            }
        }
        let has = |name: &str| paths.iter().any(|p| p == name);
        let (level, mute, suppress) = (has("level"), has("muted") || has("muted_until"), has("suppress_everyone"));
        let until = wanted.muted_until.as_ref().map(millis);
        let muted = wanted.muted && until.is_none_or(|until| until > now_ms());
        self.app
            .node()?
            .update_notification_settings(&account.id, &server.id, &wanted.channel_id, move |settings| {
                if level {
                    settings.level = wanted.level;
                }
                if mute {
                    settings.muted = muted;
                    settings.muted_until = if muted { until.map(timestamp) } else { None };
                }
                if suppress {
                    settings.suppress_everyone = wanted.suppress_everyone;
                }
            })
            .await
    }

    async fn delete_account(&self, caller: Caller, req: pb::DeleteAccountRequest) -> Result<()> {
        let account = caller.account;
        if account.kind == pb::AccountKind::Agent {
            return Err(Error::denied("an agent is deleted by the person who made it"));
        }
        if account.has_password() {
            self.confirm_password(&account, &req.password).await?;
            if account.two_factor {
                self.confirm_code(&account, &req.code).await?;
            }
        } else if req.username.trim().to_lowercase() != account.username {
            return Err(Error::denied("type your username to confirm"));
        }

        let owned: Vec<String> = self
            .app
            .index
            .joined(&account.id)
            .into_iter()
            .filter(|server| server.owner_id == account.id)
            .map(|server| server.name)
            .collect();
        if !owned.is_empty() {
            let (names, which) = if owned.len() == 1 { (owned[0].clone(), "it") } else { (owned.join(", "), "them") };
            return Err(Error::FailedPrecondition(format!("you own {names}; delete {which} first")));
        }
        if account.admin
            && self.app.node()?.admin_count().await? == 1
            && self.app.node()?.account_counts().await?.total > 1
        {
            return Err(Error::FailedPrecondition(
                "you're this instance's only admin; make someone else an admin first".into(),
            ));
        }

        // Their agents go with them.
        for agent in self.app.node()?.agents(&account.id).await? {
            self.erase_account(&agent.account).await?;
        }
        self.erase_account(&account).await
    }

    /// Takes an account off the instance: out of every server (which keep its
    /// messages, from "Deleted account"), with its sessions, devices and pictures.
    pub(super) async fn erase_account(&self, account: &Account) -> Result<()> {
        let gone = pb::User {
            id: account.id.clone(),
            username: "deleted".into(),
            display_name: "Deleted account".into(),
            kind: account.kind as i32,
            ..Default::default()
        };
        self.app.forget_account(&account.id, &gone).await?;
        let uploads = self.app.node()?.account_media(&account.id).await?;
        self.app.node()?.delete_account(&account.id).await?;
        self.app.sessions_ended(&account.id);
        // Their conversations stay for the other person in each; their devices go.
        self.app.dms()?.forget_account(&account.id).await?;
        if let Err(err) = self.app.delete_media(&uploads).await {
            tracing::warn!(account = %account.id, error = %err, "couldn't delete a deleted account's pictures");
        }
        tracing::info!(account = %account.id, "account deleted");
        Ok(())
    }
}

#[tonic::async_trait]
impl AccountService for Api {
    async fn list_sessions(
        &self,
        request: Request<pb::ListSessionsRequest>,
    ) -> Result<Response<pb::ListSessionsResponse>, Status> {
        respond(
            async {
                let caller = self.caller(request.metadata()).await?;
                let sessions = self.app.node()?.sessions(&caller.account.id, &caller.token_hash).await?;
                Ok(pb::ListSessionsResponse {
                    sessions: sessions
                        .into_iter()
                        .map(|s| pb::Session {
                            id: s.id,
                            user_agent: s.user_agent,
                            created_at: Some(timestamp(s.created_at)),
                            last_active_at: Some(timestamp(s.last_active_at)),
                            expires_at: Some(timestamp(s.expires_at)),
                            current: s.current,
                        })
                        .collect(),
                })
            }
            .await,
        )
    }

    async fn revoke_session(
        &self,
        request: Request<pb::RevokeSessionRequest>,
    ) -> Result<Response<pb::RevokeSessionResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                if !self.app.node()?.delete_session_by_id(&account.id, &request.get_ref().session_id).await? {
                    return Err(Error::NotFound("session"));
                }
                self.app.sessions_ended(&account.id);
                Ok(pb::RevokeSessionResponse {})
            }
            .await,
        )
    }

    async fn revoke_other_sessions(
        &self,
        request: Request<pb::RevokeOtherSessionsRequest>,
    ) -> Result<Response<pb::RevokeOtherSessionsResponse>, Status> {
        respond(
            async {
                let caller = self.caller(request.metadata()).await?;
                let revoked = self.app.node()?.delete_other_sessions(&caller.account.id, &caller.token_hash).await?;
                self.app.sessions_ended(&caller.account.id);
                Ok(pb::RevokeOtherSessionsResponse { revoked: revoked as i32 })
            }
            .await,
        )
    }

    async fn get_two_factor(
        &self,
        request: Request<pb::GetTwoFactorRequest>,
    ) -> Result<Response<pb::GetTwoFactorResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let left = if account.two_factor { self.app.node()?.backup_codes_left(&account.id).await? } else { 0 };
                Ok(pb::GetTwoFactorResponse { enabled: account.two_factor, backup_codes_left: left as i32 })
            }
            .await,
        )
    }

    async fn set_up_two_factor(
        &self,
        request: Request<pb::SetUpTwoFactorRequest>,
    ) -> Result<Response<pb::SetUpTwoFactorResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                if account.two_factor {
                    return Err(Error::FailedPrecondition("two-step sign-in is already on".into()));
                }
                self.confirm_password(&account, &request.get_ref().password).await?;
                let secret = twofactor::new_secret();
                self.app.node()?.set_totp_pending(&account.id, &secret).await?;
                let uri = twofactor::uri(&secret, &self.app.settings().name, &account.username);
                Ok(pb::SetUpTwoFactorResponse { secret, uri })
            }
            .await,
        )
    }

    async fn enable_two_factor(
        &self,
        request: Request<pb::EnableTwoFactorRequest>,
    ) -> Result<Response<pb::EnableTwoFactorResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let state = self.app.node()?.totp_state(&account.id).await?;
                if state.secret.is_some() {
                    return Err(Error::FailedPrecondition("two-step sign-in is already on".into()));
                }
                let Some(pending) = state.pending else {
                    return Err(Error::FailedPrecondition("set up two-step sign-in first".into()));
                };
                let guesses = format!("two-factor:{}", account.id);
                self.app.limiter.attempt(&guesses)?;
                let code = twofactor::normalize(&request.get_ref().code);
                let Some(step) = twofactor::matching_step(&pending, &code, now_ms()) else {
                    return Err(Error::denied("that code didn't work; check your device's clock"));
                };
                self.app.limiter.succeeded(&guesses);
                let (backup_codes, hashes) = twofactor::new_backup_codes();
                if !self.app.node()?.enable_totp(&account.id, &pending, step, &hashes).await? {
                    return Err(Error::FailedPrecondition("the setup changed meanwhile; start again".into()));
                }
                tracing::info!(account = %account.id, "two-step sign-in turned on");
                Ok(pb::EnableTwoFactorResponse { backup_codes })
            }
            .await,
        )
    }

    async fn disable_two_factor(
        &self,
        request: Request<pb::DisableTwoFactorRequest>,
    ) -> Result<Response<pb::DisableTwoFactorResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                Self::two_factor_on(&account)?;
                let req = request.into_inner();
                self.confirm_password(&account, &req.password).await?;
                self.confirm_code(&account, &req.code).await?;
                self.app.node()?.disable_totp(&account.id).await?;
                tracing::info!(account = %account.id, "two-step sign-in turned off");
                Ok(pb::DisableTwoFactorResponse {})
            }
            .await,
        )
    }

    async fn regenerate_backup_codes(
        &self,
        request: Request<pb::RegenerateBackupCodesRequest>,
    ) -> Result<Response<pb::RegenerateBackupCodesResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                Self::two_factor_on(&account)?;
                self.confirm_password(&account, &request.get_ref().password).await?;
                let (backup_codes, hashes) = twofactor::new_backup_codes();
                self.app.node()?.set_backup_codes(&account.id, &hashes).await?;
                Ok(pb::RegenerateBackupCodesResponse { backup_codes })
            }
            .await,
        )
    }

    async fn get_notification_settings(
        &self,
        request: Request<pb::GetNotificationSettingsRequest>,
    ) -> Result<Response<pb::GetNotificationSettingsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let settings = self.app.node()?.notification_settings(&account.id).await?;
                Ok(pb::GetNotificationSettingsResponse { settings })
            }
            .await,
        )
    }

    async fn update_notification_settings(
        &self,
        request: Request<pb::UpdateNotificationSettingsRequest>,
    ) -> Result<Response<pb::UpdateNotificationSettingsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let settings = self.change_notifications(&account, request.into_inner()).await?;
                Ok(pb::UpdateNotificationSettingsResponse { settings: Some(settings) })
            }
            .await,
        )
    }

    async fn get_server_arrangement(
        &self,
        request: Request<pb::GetServerArrangementRequest>,
    ) -> Result<Response<pb::GetServerArrangementResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let (items, updated_at) = self.arrangement(&account).await?;
                Ok(pb::GetServerArrangementResponse { items, updated_at: updated_at.map(timestamp) })
            }
            .await,
        )
    }

    async fn set_server_arrangement(
        &self,
        request: Request<pb::SetServerArrangementRequest>,
    ) -> Result<Response<pb::SetServerArrangementResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                Self::not_an_agent(&account)?;
                let index = &self.app.index;
                let items = tidy_arrangement(request.into_inner().items, |id| index.is_member(&account.id, id))?;
                let updated_at = self.app.node()?.set_server_arrangement(&account.id, items.clone()).await?;
                Ok(pb::SetServerArrangementResponse { items, updated_at: Some(timestamp(updated_at)) })
            }
            .await,
        )
    }

    type ExportDataStream = ExportStream;

    async fn export_data(&self, request: Request<pb::ExportDataRequest>) -> Result<Response<ExportStream>, Status> {
        let caller = self.caller(request.metadata()).await?;
        let (tx, rx) = mpsc::channel::<Result<pb::ExportDataResponse, Status>>(4);
        let app = self.app.clone();
        tokio::spawn(async move {
            if let Err(err) = export(&app, &caller, &tx).await {
                let _ = tx.send(Err(err.into())).await;
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn delete_account(
        &self,
        request: Request<pb::DeleteAccountRequest>,
    ) -> Result<Response<pb::DeleteAccountResponse>, Status> {
        respond(
            async {
                let caller = self.caller(request.metadata()).await?;
                Api::delete_account(self, caller, request.into_inner()).await?;
                Ok(pb::DeleteAccountResponse {})
            }
            .await,
        )
    }
}

fn time(ms: i64) -> Value {
    Value::String(timestamp(ms).to_string())
}

fn wire_time(t: &Option<prost_types::Timestamp>) -> Value {
    t.as_ref().map_or(Value::Null, |t| Value::String(t.to_string()))
}

fn level_name(level: i32) -> &'static str {
    match pb::NotificationLevel::try_from(level) {
        Ok(pb::NotificationLevel::All) => "all",
        Ok(pb::NotificationLevel::Mentions) => "mentions",
        Ok(pb::NotificationLevel::Nothing) => "nothing",
        _ => "default",
    }
}

type ExportSender = mpsc::Sender<Result<pb::ExportDataResponse, Status>>;

/// Sends the next piece of an export. False once the client has gone.
async fn send(tx: &ExportSender, chunk: Vec<u8>) -> bool {
    tx.send(Ok(pb::ExportDataResponse { chunk })).await.is_ok()
}

/// Writes the export as JSON, sending it a piece at a time so a big history
/// never sits in memory whole. Stops early if the client goes away.
async fn export(app: &Arc<App>, caller: &Caller, tx: &ExportSender) -> Result<()> {
    let account = &caller.account;
    let node = app.node()?;
    let profile = node.profile(&account.id).await?.ok_or(Error::NotFound("account"))?;
    let sessions = node.sessions(&account.id, &caller.token_hash).await?;
    let notifications = node.notification_settings(&account.id).await?;
    let (arrangement, _) = node.server_arrangement(&account.id).await?;
    let arrangement = tidy_arrangement(arrangement, |id| app.index.is_member(&account.id, id))?;
    let settings = app.settings();
    let head = json!({
        "format": "fuwa.export.v1",
        "exported_at": time(now_ms()),
        "instance": { "name": settings.name, "url": settings.public_url },
        "account": {
            "id": account.id,
            "kind": match account.kind {
                pb::AccountKind::Agent => "agent",
                pb::AccountKind::Sso => "single sign-on",
                _ if account.has_password() => "standalone",
                _ => "linked",
            },
            "username": account.username,
            "display_name": account.display_name,
            "avatar_url": account.avatar_url,
            "pronouns": profile.pronouns,
            "bio": profile.bio,
            "banner_url": profile.banner_url,
            "accent_color": profile.accent_color.map(|c| format!("#{c:06x}")),
            "profile_effect": profile.effect,
            "status": account.status,
            "status_expires_at": account.status_expires_at.map_or(Value::Null, time),
            "instance_admin": account.admin,
            "two_step_sign_in": account.two_factor,
            "created_at": time(account.created_at),
            "last_seen_at": time(account.last_seen_at),
        },
        "sessions": sessions.iter().map(|s| json!({
            "id": s.id,
            "user_agent": s.user_agent,
            "created_at": time(s.created_at),
            "last_active_at": time(s.last_active_at),
            "expires_at": time(s.expires_at),
            "this_device": s.current,
        })).collect::<Vec<_>>(),
        "notification_settings": notifications.iter().map(|n| json!({
            "server_id": n.server_id,
            "channel_id": n.channel_id,
            "level": level_name(n.level),
            "muted": n.muted,
            "muted_until": wire_time(&n.muted_until),
            "suppress_everyone": n.suppress_everyone,
        })).collect::<Vec<_>>(),
        "server_arrangement": arrangement.iter().filter_map(|item| match &item.item {
            Some(pb::server_rail_item::Item::ServerId(id)) => Some(json!({ "server_id": id })),
            Some(pb::server_rail_item::Item::Folder(f)) => Some(json!({
                "folder": {
                    "name": f.name,
                    "color": (f.color != 0).then(|| format!("#{:06x}", f.color)),
                    "server_ids": f.server_ids,
                },
            })),
            None => None,
        }).collect::<Vec<_>>(),
    });
    let mut head = serde_json::to_string_pretty(&head).map_err(|err| Error::internal(err.to_string()))?;
    head.truncate(head.trim_end().len() - 1); // the closing brace, reopened for the servers
    head.push_str(",\n  \"servers\": [");
    if !send(tx, head.into_bytes()).await {
        return Ok(());
    }

    // Each server's part comes from wherever its file is.
    let mut pieces = app.export_account(&account.id);
    let mut first_server = true;
    while let Some(piece) = pieces.recv().await {
        let piece = piece?;
        let mut chunk = piece.chunk;
        if piece.starts_server {
            let separator: &[u8] = if first_server { b"\n    " } else { b",\n    " };
            chunk.splice(0..0, separator.iter().copied());
            first_server = false;
        }
        if !send(tx, chunk).await {
            return Ok(());
        }
    }
    send(tx, b"\n  ]\n}\n".to_vec()).await;
    Ok(())
}

/// Pieces of one server's part of an export; the first of each server's
/// starts its JSON object.
pub(crate) type ExportPieces = mpsc::Sender<Result<cpb::ExportAccountResponse>>;

/// A server's part of an account's export, if the account was ever in it:
/// its membership and the messages it wrote, sent as pieces of JSON. False
/// once nobody is listening.
pub(crate) async fn export_server(sdb: &ServerDb, account_id: &str, tx: &ExportPieces) -> Result<bool> {
    let conn = sdb.read()?;
    if store::user(&conn, account_id).await?.is_none() {
        return Ok(true); // never a member
    }
    let server = sdb.server().await?;
    let member = store::member(&conn, &sdb.id, account_id).await?;
    let roles = crate::permissions::roles(&conn, &sdb.id).await?;
    let role_names: Vec<&str> = member
        .as_ref()
        .map_or(vec![], |m| roles.iter().filter(|r| m.role_ids.contains(&r.id)).map(|r| r.name.as_str()).collect());
    let application = store::load_application(&conn, &sdb.id, account_id).await?.map(|a| {
        let status = if a.status == pb::ApplicationStatus::Rejected as i32 { "turned down" } else { "waiting" };
        let answers: Vec<Value> =
            a.answers.iter().map(|x| json!({ "question": x.question, "answer": x.answer })).collect();
        json!({ "status": status, "reason": a.reason, "answers": answers, "applied_at": wire_time(&a.created_at) })
    });
    let about = json!({
        "id": server.id,
        "name": server.name,
        "member": member.is_some(),
        "owner": server.owner_id == account_id,
        "roles": role_names,
        "nickname": member.as_ref().map(|m| m.nickname.clone()).filter(|n| !n.is_empty()),
        "joined_at": member.as_ref().map_or(Value::Null, |m| wire_time(&m.joined_at)),
        "application": application,
    });
    let mut piece = serde_json::to_string(&about).map_err(|err| Error::internal(err.to_string()))?;
    piece.pop();
    piece.push_str(",\"messages\":[");
    let mut starts_server = true;
    let emit = async |piece: String, starts_server: bool| {
        tx.send(Ok(cpb::ExportAccountResponse { chunk: piece.into_bytes(), starts_server })).await.is_ok()
    };

    let mut after = String::new();
    let mut first_message = true;
    loop {
        let page = query_all(
            &conn,
            "SELECT m.id, m.channel_id, coalesce(c.name, ''), m.content, m.extras, m.reply_to_id, m.created_at, m.edited_at
             FROM messages m LEFT JOIN channels c ON c.id = m.channel_id
             WHERE m.author_id = ?1 AND m.kind = 0 AND m.id > ?2 ORDER BY m.id LIMIT ?3",
            (account_id, after.as_str(), EXPORT_PAGE),
            |r| {
                Ok((
                    r.get::<String>(0)?,
                    r.get::<String>(1)?,
                    r.get::<String>(2)?,
                    r.get::<String>(3)?,
                    r.get::<Option<Vec<u8>>>(4)?,
                    r.get::<Option<String>>(5)?,
                    r.get::<i64>(6)?,
                    r.get::<Option<i64>>(7)?,
                ))
            },
        )
        .await?;
        let Some(last) = page.last() else { break };
        after = last.0.clone();
        for (id, channel_id, channel, content, extras, reply_to, created_at, edited_at) in page {
            let attachments = match extras {
                Some(bytes) => decode_extras(&bytes)?.0,
                None => vec![],
            };
            let message = json!({
                "id": id,
                "channel_id": channel_id,
                "channel": channel,
                "content": content,
                "reply_to_id": reply_to,
                "attachments": attachments.iter().map(|a| json!({
                    "filename": a.filename,
                    "content_type": a.content_type,
                    "size": a.size,
                    "url": a.url,
                })).collect::<Vec<_>>(),
                "created_at": time(created_at),
                "edited_at": edited_at.map_or(Value::Null, time),
            });
            if !first_message {
                piece.push(',');
            }
            first_message = false;
            piece.push_str(&message.to_string());
        }
        if !emit(std::mem::take(&mut piece), starts_server).await {
            return Ok(false);
        }
        starts_server = false;
    }
    piece.push_str("]}");
    Ok(emit(piece, starts_server).await)
}

/// Unicode format characters (category Cf): invisible, and some (bidi
/// overrides and isolates) reorder the text around them.
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{ad}'
            | '\u{600}'..='\u{605}'
            | '\u{61c}'
            | '\u{6dd}'
            | '\u{70f}'
            | '\u{890}'..='\u{891}'
            | '\u{8e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}

/// Checks an arrangement and keeps only what applies: servers the person is
/// in (`member`), each once, in folders that still hold one. Refuses one over
/// the caps or with a malformed folder.
fn tidy_arrangement(items: Vec<pb::ServerRailItem>, member: impl Fn(&str) -> bool) -> Result<Vec<pb::ServerRailItem>> {
    use pb::server_rail_item::Item;
    let mut servers = 0;
    let mut folders = std::collections::HashSet::new();
    for item in &items {
        match &item.item {
            Some(Item::ServerId(_)) => servers += 1,
            Some(Item::Folder(f)) => {
                servers += f.server_ids.len();
                let id_ok = (1..=FOLDER_ID_CHARS).contains(&f.id.len())
                    && f.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
                if !id_ok || !folders.insert(f.id.as_str()) {
                    return Err(Error::invalid("a folder's id is missing, malformed or used twice"));
                }
                if f.name.chars().count() > FOLDER_NAME_CHARS || f.name.chars().any(|c| c.is_control() || invisible(c))
                {
                    return Err(Error::invalid("folder names are up to 32 characters"));
                }
                if f.color > 0xff_ffff {
                    return Err(Error::invalid("a folder's color is 0xRRGGBB"));
                }
            }
            None => {}
        }
    }
    if servers > ARRANGED_SERVERS || folders.len() > ARRANGED_FOLDERS {
        return Err(Error::invalid("an arrangement holds up to 1000 servers and 200 folders"));
    }
    let mut seen = std::collections::HashSet::new();
    let mut keep = |id: &String| member(id) && seen.insert(id.clone());
    Ok(items
        .into_iter()
        .filter_map(|item| match item.item? {
            Item::ServerId(id) => keep(&id).then_some(pb::ServerRailItem { item: Some(Item::ServerId(id)) }),
            Item::Folder(mut f) => {
                f.server_ids.retain(|id| keep(id));
                f.name = f.name.trim().to_string();
                (!f.server_ids.is_empty()).then_some(pb::ServerRailItem { item: Some(Item::Folder(f)) })
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pb::server_rail_item::Item;

    fn server(id: &str) -> pb::ServerRailItem {
        pb::ServerRailItem { item: Some(Item::ServerId(id.into())) }
    }

    fn folder(id: &str, servers: &[&str]) -> pb::ServerRailItem {
        pb::ServerRailItem {
            item: Some(Item::Folder(pb::ServerFolder {
                id: id.into(),
                name: " Games ".into(),
                color: 0xff66aa,
                server_ids: servers.iter().map(|s| s.to_string()).collect(),
            })),
        }
    }

    #[test]
    fn arrangements_keep_what_applies() {
        let member = |id: &str| id != "left";
        let tidy = tidy_arrangement(
            vec![
                server("a"),
                folder("f1", &["b", "left", "a", "c"]),
                folder("f2", &["left"]),
                server("left"),
                server("c"),
                server("d"),
                pb::ServerRailItem { item: None },
            ],
            member,
        )
        .unwrap();
        assert_eq!(tidy.len(), 3);
        assert_eq!(tidy[0], server("a"));
        let Some(Item::Folder(f)) = &tidy[1].item else { panic!("a folder") };
        assert_eq!((f.name.as_str(), f.server_ids.clone()), ("Games", vec!["b".to_string(), "c".to_string()]));
        assert_eq!(tidy[2], server("d"));
    }

    #[test]
    fn arrangements_refuse_what_is_malformed() {
        let any = |_: &str| true;
        for bad in [
            vec![folder("", &["a"])],
            vec![folder("no spaces", &["a"])],
            vec![folder(&"x".repeat(33), &["a"])],
            vec![folder("f", &["a"]), folder("f", &["b"])],
        ] {
            assert!(tidy_arrangement(bad, any).is_err());
        }
        let mut long = folder("f", &["a"]);
        if let Some(Item::Folder(f)) = &mut long.item {
            f.name = "ü".repeat(33);
        }
        assert!(tidy_arrangement(vec![long], any).is_err());
        let mut dark = folder("f", &["a"]);
        if let Some(Item::Folder(f)) = &mut dark.item {
            f.color = 0x100_0000;
        }
        assert!(tidy_arrangement(vec![dark], any).is_err());
        let many: Vec<_> = (0..1001).map(|n| server(&n.to_string())).collect();
        assert!(tidy_arrangement(many, any).is_err());
        let folders: Vec<_> = (0..201).map(|n| folder(&format!("f{n}"), &["a"])).collect();
        assert!(tidy_arrangement(folders, any).is_err());
        for sneaky in ["\u{202e}gnp.exe", "a\u{2066}b", "zero\u{200b}width"] {
            let mut f = folder("f", &["a"]);
            if let Some(Item::Folder(x)) = &mut f.item {
                x.name = sneaky.into();
            }
            assert!(tidy_arrangement(vec![f], any).is_err());
        }
        // Exactly 32 characters (not bytes) is fine.
        let mut wide = folder("f", &["a"]);
        if let Some(Item::Folder(f)) = &mut wide.item {
            f.name = "ü".repeat(32);
        }
        assert!(tidy_arrangement(vec![wide], any).is_ok());
    }
}
