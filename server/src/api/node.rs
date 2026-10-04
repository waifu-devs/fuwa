use tonic::{Request, Response, Status};

use super::{Api, respond};
use crate::pb::{self, node_service_server::NodeService};

#[tonic::async_trait]
impl NodeService for Api {
    async fn get_node(&self, request: Request<pb::GetNodeRequest>) -> Result<Response<pb::GetNodeResponse>, Status> {
        let mut node = self.app.node_info();
        // Only the instance's admins hear that it's behind.
        if let Some(versions) = node.versions.as_mut()
            && versions.newer_release.is_some()
            && !self.viewer(request.metadata()).await.is_ok_and(|viewer| viewer.is_instance_admin())
        {
            versions.newer_release = None;
        }
        respond(Ok(pb::GetNodeResponse { node: Some(node) }))
    }

    async fn send_report(
        &self,
        request: Request<pb::SendReportRequest>,
    ) -> Result<Response<pb::SendReportResponse>, Status> {
        let account = self.account(request.metadata()).await?;
        let report = request.into_inner().report.unwrap_or_default();
        let telemetry = self.app.settings().telemetry;
        respond(crate::reports::add_app_report(&account.id, telemetry, &report).map(|()| pb::SendReportResponse {}))
    }
}
