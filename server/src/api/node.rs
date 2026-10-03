use tonic::{Request, Response, Status};

use super::{Api, respond};
use crate::pb::{self, node_service_server::NodeService};

#[tonic::async_trait]
impl NodeService for Api {
    async fn get_node(&self, _: Request<pb::GetNodeRequest>) -> Result<Response<pb::GetNodeResponse>, Status> {
        respond(Ok(pb::GetNodeResponse { node: Some(self.app.node_info()) }))
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
