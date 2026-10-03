//! The directory: keeps node.db and the index of every server, knows the
//! shards, and answers what shards and gateways ask of it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

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
    /// Held while server files left from a single process are promised to a
    /// shard or let go of.
    handover: tokio::sync::Mutex<()>,
    /// The servers in those files, until the shard that takes them registers:
    /// meanwhile they're on a shard that's down, not gone.
    left: RwLock<HashSet<String>>,
    /// Shards known before this start that haven't registered since. Until
    /// they do, the index is missing their servers and who's in them.
    missing: RwLock<HashSet<String>>,
    started: Instant,
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
        let left = leftovers(&config.data_path)?.len();
        if left == 0 && promised(&config.data_path)?.is_some() {
            // Left by a stop right after a handover.
            std::fs::remove_file(config.data_path.join(PROMISED))?;
        }
        match promised(&config.data_path)? {
            _ if left == 0 => {}
            None => tracing::info!(
                files = left,
                "servers/ still has the servers from when this instance ran as one process; the first shard to start takes them"
            ),
            Some(shard) => tracing::info!(
                files = left,
                %shard,
                "servers/ still has the servers from when this instance ran as one process, promised to a shard \
                 (delete servers-promised-to to offer them to whichever shard starts first)"
            ),
        }
        let left = leftovers(&config.data_path)?
            .iter()
            .filter_map(|name| name.strip_suffix(".db"))
            .map(str::to_string)
            .collect();
        let missing = RwLock::new(known.keys().cloned().collect());
        Ok(Self {
            key,
            known: RwLock::new(known),
            handover: tokio::sync::Mutex::new(()),
            left: RwLock::new(left),
            missing,
            started: Instant::now(),
        })
    }

    /// Whether every shard known before this start has registered again, and
    /// a shard has taken any servers left from a single process, so the index
    /// has every server and member. A shard that stays down is given up on
    /// after `within`; its servers are missing until it's back.
    pub fn caught_up(&self, within: Duration) -> bool {
        self.started.elapsed() >= within
            || (self.missing.read().unwrap_or_else(|p| p.into_inner()).is_empty()
                && self.left.read().unwrap_or_else(|p| p.into_inner()).is_empty())
    }

    /// Whether a server is in the files left from a single process, not yet
    /// on a shard.
    fn is_left(&self, server_id: &str) -> bool {
        self.left.read().unwrap_or_else(|p| p.into_inner()).contains(server_id)
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, Shard>> {
        self.known.read().unwrap_or_else(|p| p.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, Shard>> {
        self.known.write().unwrap_or_else(|p| p.into_inner())
    }

    /// Notes where a shard is, as it registers.
    pub fn register(&self, id: &str, url: &str) -> Result<()> {
        {
            let mut missing = self.missing.write().unwrap_or_else(|p| p.into_inner());
            if missing.remove(id) && missing.is_empty() {
                tracing::info!("every shard has registered again");
            }
        }
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

/// Turns clients' calls away while a directory that just started waits for
/// its shards to register again (see [`Shards::caught_up`]), so nobody sees a
/// server list with servers missing. Gateways try them again shortly.
pub async fn wait_for_shards(
    axum::extract::State(app): axum::extract::State<Arc<App>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    if let Link::Directory(shards) = &app.link
        && request.uri().path().starts_with("/fuwa.v1.")
        && !shards.caught_up(app.config.cluster.ride_out)
    {
        let mut status = Status::unavailable("this instance is starting; try again soon");
        status.metadata_mut().insert(crate::error::NOT_READY, "1".parse().expect("a valid header value"));
        return status.into_http::<axum::body::Body>().into_response();
    }
    next.run(request).await
}

/// The server files in a directory's servers/ folder: left there from when
/// the instance ran as one process, waiting for a shard to take them.
fn leftovers(data_path: &Path) -> Result<Vec<String>> {
    let mut names: Vec<String> = match std::fs::read_dir(data_path.join("servers")) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| super::is_server_file(name))
            .collect(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => return Err(err.into()),
    };
    names.sort();
    Ok(names)
}

/// Which shard the leftover server files are promised to, once one has asked.
const PROMISED: &str = "servers-promised-to";

fn promised(data_path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(data_path.join(PROMISED)) {
        Ok(id) => Ok(Some(id.trim().to_string())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// How much of a file goes in one message.
const PIECE: usize = 1024 * 1024;

/// Sends each file, piece by piece, ending each with its SHA-256.
async fn send_files(dir: PathBuf, names: Vec<String>, tx: mpsc::Sender<Result<cpb::TakeServersResponse, Status>>) {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncReadExt;

    for name in names {
        let sent = async {
            let mut file = tokio::fs::File::open(dir.join(&name)).await?;
            let mut hash = Sha256::new();
            let mut buffer = vec![0; PIECE];
            loop {
                let read = file.read(&mut buffer).await?;
                let piece = if read == 0 {
                    let sha256 = std::mem::take(&mut hash).finalize().to_vec();
                    cpb::TakeServersResponse { name: name.clone(), data: Vec::new(), sha256 }
                } else {
                    hash.update(&buffer[..read]);
                    cpb::TakeServersResponse { name: name.clone(), data: buffer[..read].to_vec(), sha256: Vec::new() }
                };
                if tx.send(Ok(piece)).await.is_err() {
                    return Err(std::io::Error::other("the shard stopped listening"));
                }
                if read == 0 {
                    return Ok::<_, std::io::Error>(());
                }
            }
        }
        .await;
        if let Err(err) = sent {
            let _ = tx.send(Err(Status::internal(format!("couldn't read servers/{name}: {err}")))).await;
            return;
        }
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
                tracing::info!(shard = %self.shard, "a shard went away; waiting for it to register again");
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

    /// The leftover server files a shard is to take: all of them for the
    /// first shard to ask (and again for it, until it has them), none for
    /// any other.
    async fn promise(&self, shard: &str) -> Result<Vec<String>> {
        let data_path = &self.app.config.data_path;
        let _promising = self.shards()?.handover.lock().await;
        let names = leftovers(data_path)?;
        Ok(match promised(data_path)? {
            _ if names.is_empty() => Vec::new(),
            Some(other) if other != shard => Vec::new(),
            Some(_) => names,
            None => {
                std::fs::write(data_path.join(PROMISED), format!("{shard}\n"))?;
                tracing::info!(%shard, files = names.len(), "handing this instance's servers to a shard");
                names
            }
        })
    }
}

fn respond<T>(result: Result<T>) -> Result<Response<T>, Status> {
    result.map(Response::new).map_err(Into::into)
}

type WatchStream = Pin<Box<dyn Stream<Item = Result<cpb::WatchResponse, Status>> + Send>>;
type TakeServersStream = Pin<Box<dyn Stream<Item = Result<cpb::TakeServersResponse, Status>> + Send>>;

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
        let is_shard = !shard.is_empty();
        tokio::spawn(async move {
            let _following = is_shard.then(|| Following::new(app.clone(), shard));
            loop {
                let (current, automod_providers) = {
                    let settings = settings.borrow_and_update();
                    // Only shards check messages, so only they get the keys.
                    let providers = match is_shard {
                        false => Vec::new(),
                        true => settings.automod_providers.iter().map(|setup| setup.to_pb(true)).collect(),
                    };
                    (settings.to_pb(), providers)
                };
                let response = cpb::WatchResponse { settings: Some(current), automod_providers };
                if tx.send(Ok(response)).await.is_err() {
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
                let shards = self.shards()?;
                if !shards.left.read().unwrap_or_else(|p| p.into_inner()).is_empty() {
                    let index = &self.app.index;
                    shards.left.write().unwrap_or_else(|p| p.into_inner()).retain(|id| index.placement(id).is_none());
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
                    .filter_map(|server_id| match self.app.index.placement(&server_id) {
                        Some(shard_id) => {
                            let url = shards.url_if_up(&shard_id).unwrap_or_default();
                            Some(cpb::Placement { server_id, shard_id, url })
                        }
                        // Still to be handed to a shard: one that isn't up yet.
                        None if shards.is_left(&server_id) => {
                            Some(cpb::Placement { server_id, shard_id: String::new(), url: String::new() })
                        }
                        None => None,
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
        respond(self.app.check_upload(&req.account_id, purpose, &req.url).await.map(|upload| {
            let upload = upload.unwrap_or_default();
            cpb::CheckPictureResponse { media_id: upload.id, size: upload.size, content_type: upload.content_type }
        }))
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

    async fn find_agent(
        &self,
        request: Request<cpb::FindAgentRequest>,
    ) -> Result<Response<cpb::FindAgentResponse>, Status> {
        let found = self.app.find_agent(&request.into_inner().username).await?;
        Ok(Response::new(match found {
            Some(agent) => cpb::FindAgentResponse {
                agent: Some(account_to_pb(&agent.account)),
                owner_id: agent.owner_id,
                public: agent.public,
            },
            None => cpb::FindAgentResponse::default(),
        }))
    }

    type TakeServersStream = TakeServersStream;

    async fn take_servers(
        &self,
        request: Request<cpb::TakeServersRequest>,
    ) -> Result<Response<TakeServersStream>, Status> {
        let shard = request.into_inner().shard_id;
        super::check_shard_id(&shard).map_err(Error::invalid)?;
        let data_path = self.app.config.data_path.clone();
        let names = self.promise(&shard).await?;
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(send_files(data_path.join("servers"), names, tx));
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn handed_over(
        &self,
        request: Request<cpb::HandedOverRequest>,
    ) -> Result<Response<cpb::HandedOverResponse>, Status> {
        respond(
            async {
                let cpb::HandedOverRequest { shard_id, mut names } = request.into_inner();
                let data_path = &self.app.config.data_path;
                let _promising = self.shards()?.handover.lock().await;
                if promised(data_path)?.as_deref() != Some(shard_id.as_str()) {
                    return Err(Error::FailedPrecondition(format!(
                        "the servers here weren't handed to shard {shard_id}"
                    )));
                }
                names.sort();
                if names != leftovers(data_path)? {
                    return Err(Error::FailedPrecondition("the shard has other files than the ones here".into()));
                }
                // The whole folder moves at once, so the directory never holds
                // part of a server.
                let mut moved = data_path.join("handed-over");
                if moved.exists() {
                    moved = data_path.join(format!("handed-over-{}", crate::id::now_ms()));
                }
                std::fs::rename(data_path.join("servers"), &moved)?;
                std::fs::create_dir_all(data_path.join("servers"))?;
                std::fs::remove_file(data_path.join(PROMISED))?;
                tracing::info!(
                    shard = %shard_id,
                    files = names.len(),
                    kept = %moved.display(),
                    "handed this instance's servers to a shard; delete the copies kept here once everything looks right"
                );
                Ok(cpb::HandedOverResponse {})
            }
            .await,
        )
    }
}
