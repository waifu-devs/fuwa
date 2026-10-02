//! The directory: keeps node.db and the index of every server, knows the
//! shards, and answers what shards and gateways ask of it.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use futures::Stream;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::AsciiMetadataValue;
use tonic::{Request, Response, Status};

use super::{ShardClient, account_to_pb, shard_client};
use crate::api::PictureOwner;
use crate::app::{App, Link};
use crate::auth::{self, Viewer};
use crate::config::Config;
use crate::cpb::{self, directory_service_server::DirectoryService};
use crate::error::{Error, Result};
use crate::node::NodeDb;
use crate::pb;

/// The shards a directory knows of: every one that has registered, and
/// which of them are up (following its settings) right now.
pub struct Shards {
    key: AsciiMetadataValue,
    known: RwLock<HashMap<String, Shard>>,
}

struct Shard {
    url: String,
    /// None until it says where it is.
    client: Option<ShardClient>,
    /// Open settings streams from it: up while there's one.
    watching: usize,
}

impl Shard {
    fn up(&self) -> Option<&ShardClient> {
        self.client.as_ref().filter(|_| self.watching > 0)
    }
}

impl Shards {
    /// The shards that registered before, from node.db, all down until they
    /// follow the directory again.
    pub async fn load(config: &Config, node: &NodeDb) -> Result<Self> {
        let key = config.cluster.key_value()?;
        let mut known = HashMap::new();
        for (id, url) in node.shards().await? {
            let client = Some(shard_client(&url, key.clone())?);
            known.insert(id, Shard { url, client, watching: 0 });
        }
        Ok(Self { key, known: RwLock::new(known) })
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, Shard>> {
        self.known.read().unwrap_or_else(|p| p.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, Shard>> {
        self.known.write().unwrap_or_else(|p| p.into_inner())
    }

    /// Notes where a shard is, as it registers.
    pub fn register(&self, id: &str, url: &str) -> Result<()> {
        let mut known = self.write();
        match known.get_mut(id) {
            Some(shard) if shard.url == url && shard.client.is_some() => {}
            Some(shard) => {
                shard.client = Some(shard_client(url, self.key.clone())?);
                shard.url = url.to_string();
            }
            None => {
                let client = Some(shard_client(url, self.key.clone())?);
                known.insert(id.to_string(), Shard { url: url.to_string(), client, watching: 0 });
            }
        }
        Ok(())
    }

    fn watch(&self, id: &str, change: isize) {
        let mut known = self.write();
        if let Some(shard) = known.get_mut(id) {
            shard.watching = shard.watching.saturating_add_signed(change);
        } else if change > 0 {
            // Following before its first registration: up once it says where it is.
            known.insert(id.to_string(), Shard { url: String::new(), client: None, watching: change as usize });
        }
    }

    /// Where a shard is, while it's up.
    pub fn url_if_up(&self, id: &str) -> Option<String> {
        self.read().get(id).filter(|shard| shard.up().is_some()).map(|shard| shard.url.clone())
    }

    /// A client for a shard that's up.
    pub fn client(&self, id: &str) -> Result<ShardClient> {
        self.read()
            .get(id)
            .and_then(Shard::up)
            .cloned()
            .ok_or_else(|| Error::Unavailable(format!("shard {id} is down right now; try again soon")))
    }

    /// Every shard ever registered, as (id, client if it's up).
    pub fn all(&self) -> Vec<(String, Option<ShardClient>)> {
        let mut all: Vec<_> = self
            .read()
            .iter()
            .filter(|(_, shard)| shard.client.is_some())
            .map(|(id, shard)| (id.clone(), shard.up().cloned()))
            .collect();
        all.sort_by(|a, b| a.0.cmp(&b.0));
        all
    }

    /// The shards up now.
    pub fn up(&self) -> Vec<(String, ShardClient)> {
        self.all().into_iter().filter_map(|(id, client)| client.map(|client| (id, client))).collect()
    }
}

/// Counts a shard as up for as long as its settings stream is open.
struct Following {
    app: Arc<App>,
    shard: String,
}

impl Following {
    fn new(app: Arc<App>, shard: String) -> Self {
        if let Link::Directory(shards) = &app.link {
            shards.watch(&shard, 1);
        }
        Self { app, shard }
    }
}

impl Drop for Following {
    fn drop(&mut self) {
        if let Link::Directory(shards) = &self.app.link {
            shards.watch(&self.shard, -1);
            if shards.url_if_up(&self.shard).is_none() {
                tracing::warn!(shard = %self.shard, "a shard went away");
            }
        }
    }
}

/// The cluster calls a directory answers.
#[derive(Clone)]
pub struct Internal {
    app: Arc<App>,
}

impl Internal {
    pub fn new(app: Arc<App>) -> Self {
        Self { app }
    }

    fn shards(&self) -> Result<&Shards> {
        match &self.app.link {
            Link::Directory(shards) => Ok(shards),
            _ => Err(Error::internal("only a directory keeps track of shards")),
        }
    }
}

fn respond<T>(result: Result<T>) -> Result<Response<T>, Status> {
    result.map(Response::new).map_err(Into::into)
}

type WatchStream = Pin<Box<dyn Stream<Item = Result<cpb::WatchResponse, Status>> + Send>>;

#[tonic::async_trait]
impl DirectoryService for Internal {
    async fn authenticate(
        &self,
        request: Request<cpb::AuthenticateRequest>,
    ) -> Result<Response<cpb::AuthenticateResponse>, Status> {
        respond(
            async {
                let token = request.into_inner().token;
                let admin_token = self.app.config.admin_token.as_deref();
                Ok(match auth::authenticate_token(self.app.node()?, admin_token, &token).await? {
                    Viewer::Account { account, token_hash } => {
                        cpb::AuthenticateResponse { account: Some(account_to_pb(&account)), token_hash }
                    }
                    Viewer::Operator => cpb::AuthenticateResponse::default(),
                })
            }
            .await,
        )
    }

    async fn session_live(
        &self,
        request: Request<cpb::SessionLiveRequest>,
    ) -> Result<Response<cpb::SessionLiveResponse>, Status> {
        let token_hash = request.into_inner().token_hash;
        respond(self.app.session_live(&token_hash).await.map(|live| cpb::SessionLiveResponse { live }))
    }

    type WatchStream = WatchStream;

    async fn watch(&self, request: Request<cpb::WatchRequest>) -> Result<Response<WatchStream>, Status> {
        let shard = request.into_inner().shard_id;
        let app = self.app.clone();
        let mut settings = app.watch_settings();
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(async move {
            let _following = (!shard.is_empty()).then(|| Following::new(app.clone(), shard));
            loop {
                let current = settings.borrow_and_update().to_pb();
                if tx.send(Ok(cpb::WatchResponse { settings: Some(current) })).await.is_err() {
                    return;
                }
                tokio::select! {
                    _ = app.shutdown.cancelled() => return,
                    _ = tx.closed() => return,
                    changed = settings.changed() => if changed.is_err() { return },
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn register_shard(
        &self,
        request: Request<cpb::RegisterShardRequest>,
    ) -> Result<Response<cpb::RegisterShardResponse>, Status> {
        respond(
            async {
                let req = request.into_inner();
                super::check_shard_id(&req.shard_id).map_err(Error::invalid)?;
                let ids: Vec<String> =
                    req.servers.iter().filter_map(|entry| entry.server.as_ref().map(|s| s.id.clone())).collect();
                self.app.node()?.register_shard(&req.shard_id, &req.url, &ids).await?;
                self.shards()?.register(&req.shard_id, &req.url)?;
                // Servers gone from the shard keep their notification settings: the
                // file may only be moving to another shard.
                let gone = self.app.index.register(&req.shard_id, req.servers);
                if !gone.is_empty() {
                    tracing::info!(shard = %req.shard_id, servers = ?gone, "servers no longer on this shard");
                }
                Ok(cpb::RegisterShardResponse {})
            }
            .await,
        )
    }

    async fn placements(
        &self,
        request: Request<cpb::PlacementsRequest>,
    ) -> Result<Response<cpb::PlacementsResponse>, Status> {
        respond(
            async {
                let shards = self.shards()?;
                let placements = request
                    .into_inner()
                    .server_ids
                    .into_iter()
                    .filter_map(|server_id| {
                        let shard_id = self.app.index.placement(&server_id)?;
                        let url = shards.url_if_up(&shard_id).unwrap_or_default();
                        Some(cpb::Placement { server_id, shard_id, url })
                    })
                    .collect();
                Ok(cpb::PlacementsResponse { placements })
            }
            .await,
        )
    }

    async fn index_server(
        &self,
        request: Request<cpb::IndexServerRequest>,
    ) -> Result<Response<cpb::IndexServerResponse>, Status> {
        if let Some(server) = request.into_inner().server {
            self.app.index.update(server);
        }
        Ok(Response::new(cpb::IndexServerResponse {}))
    }

    async fn index_membership(
        &self,
        request: Request<cpb::IndexMembershipRequest>,
    ) -> Result<Response<cpb::IndexMembershipResponse>, Status> {
        let req = request.into_inner();
        if req.joined {
            self.app.index.join(&req.account_id, &req.server_id);
        } else {
            self.app.index.leave(&req.account_id, &req.server_id);
        }
        Ok(Response::new(cpb::IndexMembershipResponse {}))
    }

    async fn index_invite(
        &self,
        request: Request<cpb::IndexInviteRequest>,
    ) -> Result<Response<cpb::IndexInviteResponse>, Status> {
        let req = request.into_inner();
        self.app.index.index_invite(&req.server_id, &req.code, req.exists);
        Ok(Response::new(cpb::IndexInviteResponse {}))
    }

    async fn drop_server(
        &self,
        request: Request<cpb::DropServerRequest>,
    ) -> Result<Response<cpb::DropServerResponse>, Status> {
        self.app.server_gone(&request.into_inner().server_id).await;
        Ok(Response::new(cpb::DropServerResponse {}))
    }

    async fn forget_notifications(
        &self,
        request: Request<cpb::ForgetNotificationsRequest>,
    ) -> Result<Response<cpb::ForgetNotificationsResponse>, Status> {
        let req = request.into_inner();
        let channel = Some(req.channel_id.as_str()).filter(|id| !id.is_empty());
        let account = Some(req.account_id.as_str()).filter(|id| !id.is_empty());
        self.app.forget_notifications(&req.server_id, channel, account).await;
        Ok(Response::new(cpb::ForgetNotificationsResponse {}))
    }

    async fn check_picture(
        &self,
        request: Request<cpb::CheckPictureRequest>,
    ) -> Result<Response<cpb::CheckPictureResponse>, Status> {
        let req = request.into_inner();
        let purpose = pb::MediaPurpose::try_from(req.purpose).unwrap_or(pb::MediaPurpose::Unspecified);
        respond(
            self.app
                .check_picture(&req.account_id, purpose, &req.url)
                .await
                .map(|id| cpb::CheckPictureResponse { media_id: id.unwrap_or_default() }),
        )
    }

    async fn keep_picture(
        &self,
        request: Request<cpb::KeepPictureRequest>,
    ) -> Result<Response<cpb::KeepPictureResponse>, Status> {
        let req = request.into_inner();
        let server = Some(req.server_id.as_str()).filter(|id| !id.is_empty());
        self.app.keep_picture(Some(&req.media_id), server).await;
        Ok(Response::new(cpb::KeepPictureResponse {}))
    }

    async fn drop_picture(
        &self,
        request: Request<cpb::DropPictureRequest>,
    ) -> Result<Response<cpb::DropPictureResponse>, Status> {
        let req = request.into_inner();
        self.app.drop_picture(&req.old_url, &req.new_url, PictureOwner::Server(&req.server_id)).await;
        Ok(Response::new(cpb::DropPictureResponse {}))
    }

    async fn count_owned_servers(
        &self,
        request: Request<cpb::CountOwnedServersRequest>,
    ) -> Result<Response<cpb::CountOwnedServersResponse>, Status> {
        let count = self.app.index.owned_count(&request.into_inner().account_id);
        Ok(Response::new(cpb::CountOwnedServersResponse { count }))
    }
}
