use tonic::{Request, Response, Status};

use super::{Api, respond};
use crate::config::LocalAccounts;
use crate::error::{Error, Result};
use crate::pb::{self, admin_service_server::AdminService};
use crate::servers::effective_limits;
use crate::settings::{self, Settings};

impl Api {
    async fn require_instance_admin(&self, metadata: &tonic::metadata::MetadataMap) -> Result<()> {
        if !self.viewer(metadata).await?.is_instance_admin() {
            return Err(Error::denied("only this instance's admins can do that"));
        }
        Ok(())
    }
}

impl Api {
    /// The settings in force, their defaults, which ones were changed here, and
    /// how the process was started.
    async fn instance_config(&self) -> Result<pb::InstanceConfig> {
        let config = &self.app.config;
        let mut overridden: Vec<String> = self.app.node.settings().await?.into_iter().map(|(field, _)| field).collect();
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

#[tonic::async_trait]
impl AdminService for Api {
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
                if update.iter().any(|field| field == "local_accounts") && next.local_accounts == LocalAccounts::Off {
                    return Err(Error::FailedPrecondition(
                        "turning standalone accounts off would leave no way to sign in to this instance".into(),
                    ));
                }

                self.app.node.save_settings(&store, &reset).await?;
                self.app.replace_settings(Settings::load(&self.app.config, &self.app.node.settings().await?));
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
                let mut server_usage = Vec::new();
                for id in self.app.servers.ids() {
                    let sdb = self.app.servers.get(&id).await?;
                    server_usage.push(sdb.usage().await?);
                }
                let defaults = &self.app.settings().limits;
                Ok(pb::GetNodeUsageResponse {
                    accounts: self.app.node.account_counts().await?.total,
                    servers: server_usage.len() as i64,
                    server_usage,
                    default_limits: Some(effective_limits(pb::ServerLimits::default(), defaults)),
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
                let sdb = self.app.servers.get(&req.server_id).await?;
                sdb.set_limits(&limits).await?;
                Ok(pb::SetServerLimitsResponse { limits: Some(sdb.limits(&self.app.settings().limits).await?) })
            }
            .await,
        )
    }
}
