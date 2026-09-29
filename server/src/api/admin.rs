use tonic::{Request, Response, Status};

use super::{Api, respond};
use crate::error::{Error, Result};
use crate::pb::{self, admin_service_server::AdminService};
use crate::servers::effective_limits;

impl Api {
    async fn require_instance_admin(&self, metadata: &tonic::metadata::MetadataMap) -> Result<()> {
        if !self.viewer(metadata).await?.is_instance_admin() {
            return Err(Error::denied("only this instance's admins can do that"));
        }
        Ok(())
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
                let defaults = &self.app.config.limits;
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
                Ok(pb::SetServerLimitsResponse { limits: Some(sdb.limits(&self.app.config.limits).await?) })
            }
            .await,
        )
    }
}
