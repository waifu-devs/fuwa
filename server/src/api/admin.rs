use std::pin::Pin;

use futures::Stream;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use super::{Api, respond, text};
use crate::auth::{self, Viewer};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::node::{AccountFilter, AccountSummary};
use crate::pb::{self, admin_service_server::AdminService};
use crate::servers::effective_limits;
use crate::settings::{self, Settings};

/// How much of an exported file goes in each message.
const EXPORT_CHUNK: usize = 256 * 1024;

impl Api {
    async fn require_instance_admin(&self, metadata: &tonic::metadata::MetadataMap) -> Result<Viewer> {
        let viewer = self.viewer(metadata).await?;
        if !viewer.is_instance_admin() {
            return Err(Error::denied("only this instance's admins can do that"));
        }
        Ok(viewer)
    }

    fn account_summary_pb(&self, summary: AccountSummary) -> pb::AccountSummary {
        let account = &summary.account;
        let servers = self.app.index.joined_ids(&account.id).len();
        pb::AccountSummary {
            user: Some(account.user()),
            admin: account.admin,
            disabled: account.disabled,
            disabled_reason: summary.disabled_reason,
            disabled_at: summary.disabled_at.map(timestamp),
            two_factor: account.two_factor,
            created_at: Some(timestamp(account.created_at)),
            last_seen_at: Some(timestamp(account.last_seen_at)),
            sessions: summary.sessions as i32,
            servers: servers as i32,
            servers_owned: self.app.index.owned_count(&account.id) as i32,
        }
    }
}

/// A file name from a server's name: lowercase letters, digits and dashes.
fn file_slug(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() { "server".into() } else { slug.chars().take(48).collect() }
}

impl Api {
    /// The settings in force, their defaults, which ones were changed here, and
    /// how the process was started.
    async fn instance_config(&self) -> Result<pb::InstanceConfig> {
        let config = &self.app.config;
        let mut overridden: Vec<String> =
            self.app.node()?.settings().await?.into_iter().map(|(field, _)| field).collect();
        overridden.retain(|field| settings::FIELDS.contains(&field.as_str()));
        Ok(pb::InstanceConfig {
            settings: Some(self.app.settings().to_pb()),
            defaults: Some(Settings::defaults(config).to_pb()),
            overridden,
            startup: Some(pb::StartupSettings {
                port: config.port.into(),
                encryption: config.encryption_key.is_some(),
                admin_token: config.admin_token.is_some(),
                web_built_in: crate::web::BUILT_IN,
                telemetry_url: config.telemetry.url.clone(),
                hosted: config.telemetry.hosted,
                version: crate::VERSION.into(),
            }),
        })
    }
}

fn check_limit(field: &str, value: Option<i64>) -> Result<()> {
    if value.is_some_and(|v| v < 0) {
        return Err(Error::invalid(format!("limits.{field} can't be negative")));
    }
    Ok(())
}

type ExportStream = Pin<Box<dyn Stream<Item = Result<pb::ExportServerResponse, Status>> + Send>>;

#[tonic::async_trait]
impl AdminService for Api {
    type ExportServerStream = ExportStream;

    async fn get_settings(
        &self,
        request: Request<pb::GetSettingsRequest>,
    ) -> Result<Response<pb::GetSettingsResponse>, Status> {
        respond(
            async {
                self.require_instance_admin(request.metadata()).await?;
                Ok(pb::GetSettingsResponse { config: Some(self.instance_config().await?) })
            }
            .await,
        )
    }

    async fn update_settings(
        &self,
        request: Request<pb::UpdateSettingsRequest>,
    ) -> Result<Response<pb::UpdateSettingsResponse>, Status> {
        respond(
            async {
                self.require_instance_admin(request.metadata()).await?;
                let req = request.into_inner();
                let update = settings::expand(&req.update_mask.map(|mask| mask.paths).unwrap_or_default())?;
                let reset = settings::expand(&req.reset_mask.map(|mask| mask.paths).unwrap_or_default())?;
                if let Some(field) = update.iter().find(|field| reset.contains(field)) {
                    return Err(Error::invalid(format!("{field} can't be both changed and reset")));
                }
                let from = req.settings.unwrap_or_default();

                // Check every change against the current settings before storing any.
                let mut next = (*self.app.settings()).clone();
                let mut store = Vec::new();
                for field in &update {
                    next.set_from_pb(field, &from)?;
                    store.push((field.clone(), next.get_json(field)?.to_string()));
                }
                let touches = |name: &str| update.iter().chain(&reset).any(|field| field == name);
                let mut after_reset = next.clone();
                for field in &reset {
                    after_reset.set_from_pb(field, &Settings::defaults(&self.app.config).to_pb())?;
                }
                // There's always a way in: standalone accounts, or waifu.dev sign-in that works.
                if (touches("local_accounts") || touches("linked_accounts") || touches("public_url"))
                    && !after_reset.local_accounts.sign_in()
                    && !after_reset.linked_sign_in()
                {
                    return Err(Error::FailedPrecondition(if after_reset.linked_accounts.sign_in() {
                        "with standalone accounts off, signing in with waifu.dev has to work, and it needs an https public URL".into()
                    } else {
                        "turning both kinds of account off would leave no way to sign in to this instance".into()
                    }));
                }

                self.app.node()?.save_settings(&store, &reset).await?;
                self.app.replace_settings(Settings::load(&self.app.config, &self.app.node()?.settings().await?));
                tracing::info!(changed = ?update, reset = ?reset, "instance settings updated");
                Ok(pb::UpdateSettingsResponse { config: Some(self.instance_config().await?) })
            }
            .await,
        )
    }

    async fn get_node_usage(
        &self,
        request: Request<pb::GetNodeUsageRequest>,
    ) -> Result<Response<pb::GetNodeUsageResponse>, Status> {
        respond(
            async {
                self.require_instance_admin(request.metadata()).await?;
                let server_usage: Vec<pb::ServerUsage> =
                    self.app.describe_servers(&[]).await?.into_iter().filter_map(|server| server.usage).collect();
                let defaults = &self.app.settings().limits;
                let (pictures, picture_bytes) = self.app.node()?.picture_totals().await?;
                Ok(pb::GetNodeUsageResponse {
                    accounts: self.app.node()?.account_counts().await?.total,
                    servers: server_usage.len() as i64,
                    server_usage,
                    default_limits: Some(effective_limits(pb::ServerLimits::default(), defaults)),
                    pictures,
                    picture_bytes,
                })
            }
            .await,
        )
    }

    async fn set_server_limits(
        &self,
        request: Request<pb::SetServerLimitsRequest>,
    ) -> Result<Response<pb::SetServerLimitsResponse>, Status> {
        respond(
            async {
                self.require_instance_admin(request.metadata()).await?;
                let req = request.into_inner();
                let limits = req.limits.unwrap_or_default();
                check_limit("members", limits.members)?;
                check_limit("channels", limits.channels)?;
                check_limit("storage_bytes", limits.storage_bytes)?;
                check_limit("attachment_bytes", limits.attachment_bytes)?;
                check_limit("emojis", limits.emojis)?;
                let sdb = self.app.servers.get(&req.server_id).await?;
                sdb.set_limits(&limits).await?;
                Ok(pb::SetServerLimitsResponse { limits: Some(sdb.limits(&self.app.settings().limits).await?) })
            }
            .await,
        )
    }
    async fn list_accounts(
        &self,
        request: Request<pb::ListAccountsRequest>,
    ) -> Result<Response<pb::ListAccountsResponse>, Status> {
        respond(
            async {
                self.require_instance_admin(request.metadata()).await?;
                let req = request.into_inner();
                let filter = match pb::AccountFilter::try_from(req.filter) {
                    Ok(pb::AccountFilter::Admins) => AccountFilter::Admins,
                    Ok(pb::AccountFilter::Disabled) => AccountFilter::Disabled,
                    _ => AccountFilter::All,
                };
                let limit = if req.limit <= 0 { 50 } else { i64::from(req.limit.min(200)) };
                let (found, has_more) =
                    self.app.node()?.list_accounts(&req.query, filter, &req.before_id, limit).await?;
                let totals = self.app.node()?.account_totals().await?;
                Ok(pb::ListAccountsResponse {
                    accounts: found.into_iter().map(|summary| self.account_summary_pb(summary)).collect(),
                    has_more,
                    totals: Some(pb::AccountTotals {
                        all: totals.all,
                        admins: totals.admins,
                        disabled: totals.disabled,
                    }),
                })
            }
            .await,
        )
    }

    async fn update_account(
        &self,
        request: Request<pb::UpdateAccountRequest>,
    ) -> Result<Response<pb::UpdateAccountResponse>, Status> {
        respond(
            async {
                let viewer = self.require_instance_admin(request.metadata()).await?;
                let req = request.into_inner();
                if viewer.account().is_ok_and(|me| me.id == req.account_id) {
                    return Err(Error::FailedPrecondition(
                        "you can't change your own account here; ask another admin".into(),
                    ));
                }
                let reason = text("reason", &req.reason, 0, 512)?;
                let target = self.app.node()?.account(&req.account_id).await?.ok_or(Error::NotFound("account"))?;
                if req.admin == Some(true) && target.kind == pb::AccountKind::Agent {
                    return Err(Error::FailedPrecondition("agents can't be instance admins".into()));
                }
                // Taking admin away comes before turning off, and turning on before making admin.
                if req.admin == Some(false) {
                    self.app.node()?.set_admin(&req.account_id, false).await?;
                }
                if let Some(disabled) = req.disabled {
                    self.app.node()?.set_disabled(&req.account_id, disabled, &reason).await?;
                }
                if req.admin == Some(true) {
                    self.app.node()?.set_admin(&req.account_id, true).await?;
                }
                let by = viewer.account().map(|a| a.id.clone()).unwrap_or_else(|_| "operator".into());
                tracing::info!(account = %req.account_id, admin = ?req.admin, disabled = ?req.disabled, by = %by, "account updated by an admin");
                let summary = self.app.node()?.account_summary(&req.account_id).await?.ok_or(Error::NotFound("account"))?;
                Ok(pb::UpdateAccountResponse { account: Some(self.account_summary_pb(summary)) })
            }
            .await,
        )
    }

    async fn reset_account_password(
        &self,
        request: Request<pb::ResetAccountPasswordRequest>,
    ) -> Result<Response<pb::ResetAccountPasswordResponse>, Status> {
        respond(
            async {
                let viewer = self.require_instance_admin(request.metadata()).await?;
                let req = request.into_inner();
                if viewer.account().is_ok_and(|me| me.id == req.account_id) {
                    return Err(Error::FailedPrecondition("change your own password from your account settings".into()));
                }
                let account = self.app.node()?.account(&req.account_id).await?.ok_or(Error::NotFound("account"))?;
                if !account.has_password() {
                    return Err(Error::FailedPrecondition("only standalone accounts have a password here".into()));
                }
                let password = auth::temporary_password();
                let hash = auth::hash_password(password.clone()).await?;
                self.app.node()?.reset_password(&account.id, &hash, req.turn_off_two_factor).await?;
                tracing::info!(account = %account.id, two_factor_off = req.turn_off_two_factor, "password reset by an admin");
                Ok(pb::ResetAccountPasswordResponse { password })
            }
            .await,
        )
    }

    async fn list_instance_servers(
        &self,
        request: Request<pb::ListInstanceServersRequest>,
    ) -> Result<Response<pb::ListInstanceServersResponse>, Status> {
        respond(
            async {
                let viewer = self.require_instance_admin(request.metadata()).await?;
                let me = viewer.account().map(|a| a.id.clone()).unwrap_or_default();
                let defaults = self.app.settings().limits.clone();
                let mut servers = Vec::new();
                for described in self.app.describe_servers(&[]).await? {
                    let Some(server) = described.server else { continue };
                    let owner = match self.app.node()?.account(&server.owner_id).await? {
                        Some(account) => Some(account.user()),
                        None => described.owner,
                    };
                    servers.push(pb::InstanceServer {
                        owner,
                        usage: described.usage,
                        limits: Some(effective_limits(described.own_limits.unwrap_or_default(), &defaults)),
                        member: !me.is_empty() && self.app.index.is_member(&me, &server.id),
                        server: Some(server),
                    });
                }
                Ok(pb::ListInstanceServersResponse { servers })
            }
            .await,
        )
    }

    async fn export_server(
        &self,
        request: Request<pb::ExportServerRequest>,
    ) -> Result<Response<Self::ExportServerStream>, Status> {
        let viewer = self.require_instance_admin(request.metadata()).await?;
        let sdb = self.app.servers.get(&request.get_ref().server_id).await?;
        let server = sdb.server().await?;
        let dir = self.app.config.data_path.join("exports");
        std::fs::create_dir_all(&dir).map_err(Error::from)?;
        let path = dir.join(format!("{}.db", new_id()));
        if let Err(err) = sdb.export_to(&path).await {
            let _ = std::fs::remove_file(&path);
            return Err(err.into());
        }
        let size = std::fs::metadata(&path).map_err(Error::from)?.len() as i64;
        let by = viewer.account().map(|a| a.id.clone()).unwrap_or_else(|_| "operator".into());
        tracing::info!(server = %sdb.id, bytes = size, by = %by, "server exported");

        let filename = format!("{}.db", file_slug(&server.name));
        let (tx, rx) = mpsc::channel::<Result<pb::ExportServerResponse, Status>>(4);
        tokio::spawn(async move {
            let sent = async {
                let mut file = tokio::fs::File::open(&path).await?;
                let mut first = true;
                loop {
                    let mut chunk = vec![0u8; EXPORT_CHUNK];
                    let mut filled = 0;
                    while filled < EXPORT_CHUNK {
                        let n = file.read(&mut chunk[filled..]).await?;
                        if n == 0 {
                            break;
                        }
                        filled += n;
                    }
                    chunk.truncate(filled);
                    if filled == 0 && !first {
                        break;
                    }
                    let message = pb::ExportServerResponse {
                        chunk,
                        size: if first { size } else { 0 },
                        filename: if first { filename.clone() } else { String::new() },
                    };
                    first = false;
                    if tx.send(Ok(message)).await.is_err() || filled < EXPORT_CHUNK {
                        break;
                    }
                }
                std::io::Result::Ok(())
            }
            .await;
            if let Err(err) = sent {
                let _ = tx.send(Err(Status::internal(format!("couldn't read the export: {err}")))).await;
            }
            let _ = tokio::fs::remove_file(&path).await;
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn set_announcement(
        &self,
        request: Request<pb::SetAnnouncementRequest>,
    ) -> Result<Response<pb::SetAnnouncementResponse>, Status> {
        respond(
            async {
                self.require_instance_admin(request.metadata()).await?;
                let next = request.into_inner().announcement.unwrap_or_default();
                let body = text("announcement.text", &next.text, 0, 300)?;
                if body.is_empty() {
                    self.app.node()?.set_announcement(None).await?;
                    self.app.replace_announcement(None);
                    tracing::info!("announcement taken down");
                    return Ok(pb::SetAnnouncementResponse { announcement: None });
                }
                if next.ends_at.as_ref().is_some_and(|end| crate::id::millis(end) <= now_ms()) {
                    return Err(Error::invalid("announcement.ends_at is already past"));
                }
                let tone = match pb::AnnouncementTone::try_from(next.tone) {
                    Ok(pb::AnnouncementTone::Warning) => pb::AnnouncementTone::Warning,
                    Ok(pb::AnnouncementTone::Critical) => pb::AnnouncementTone::Critical,
                    _ => pb::AnnouncementTone::Info,
                };
                // The same text keeps its id, so people who closed it don't see it again.
                let current = self.app.node()?.announcement().await?;
                let (id, created_at) = match current {
                    Some(current) if current.text == body => (current.id, current.created_at),
                    _ => (new_id(), Some(timestamp(now_ms()))),
                };
                let announcement =
                    pb::Announcement { id, text: body, tone: tone as i32, created_at, ends_at: next.ends_at };
                self.app.node()?.set_announcement(Some(&announcement)).await?;
                self.app.replace_announcement(Some(announcement.clone()));
                tracing::info!(id = %announcement.id, "announcement put up");
                Ok(pb::SetAnnouncementResponse { announcement: Some(announcement) })
            }
            .await,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_file_names() {
        assert_eq!(file_slug("Kai's Corner!"), "kai-s-corner");
        assert_eq!(file_slug("  Waifu   Devs  "), "waifu-devs");
        assert_eq!(file_slug("ふわ"), "server");
        assert_eq!(file_slug(&"a".repeat(80)).len(), 48);
    }

    #[test]
    fn temporary_passwords_read_clearly() {
        let password = auth::temporary_password();
        assert_eq!(password.len(), 19);
        assert!(password.split('-').all(|group| group.len() == 4));
        assert!(!password.chars().any(|c| "01loI".contains(c)));
        assert_ne!(password, auth::temporary_password());
        assert!(auth::validate_password(&password).is_ok());
    }
}
