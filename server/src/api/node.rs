use tonic::{Request, Response, Status};

use super::{Api, respond};
use crate::pb::{self, node_service_server::NodeService};

#[tonic::async_trait]
impl NodeService for Api {
    async fn get_node(&self, _: Request<pb::GetNodeRequest>) -> Result<Response<pb::GetNodeResponse>, Status> {
        respond(Ok(pb::GetNodeResponse { node: Some(self.app.node_info()) }))
    }
}
