//! A shard: keeps some community servers' files and their live events, and
//! asks the directory about accounts, sessions and settings.
//!
//! The work on server files that accounts need done across servers (a new
//! profile, a deleted account, a data export) is here too, as functions over
//! the servers this process keeps: a single process calls them directly, the
//! directory of a split instance through [`Internal`].

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::Stream;
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};

use super::{Backoff, DirectoryClient, account_from_pb};
use crate::app::{App, Link as AppLink};
use crate::auth::{self, Viewer};
use crate::config::Config;
use crate::cpb::{self, shard_service_server::ShardService};
use crate::db::query_one;
use crate::error::{Error, Result};
use crate::pb;
use crate::servers::{self as store, NewServer, Payload, Servers};
use crate::settings::Settings;

/// How long a shard trusts what the directory said about a session.
const SESSION_CACHE: Duration = Duration::from_secs(5);
/// Sessions remembered at most; the cache starts over past this.
const SESSION_CACHE_SIZE: usize = 10_000;
/// How often a shard checks whether the directory needs its servers again.
const RECHECK: Duration = Duration::from_secs(5);

/// A shard's line to the directory.
pub struct Link {
    pub id: String,
    /// Where the directory and gateways reach this shard.
    pub url: String,
    pub directory_url: String,
    directory: DirectoryClient,
    sessions: Mutex<HashMap<String, (Viewer, Instant)>>,
    /// Bumped by every change sent to the directory's index, so a
    /// registration that raced one runs again.
    changes: AtomicU64,
    /// The directory may have missed a change: register again.
    dirty: AtomicBool,
    registered: watch::Sender<bool>,
}

impl Link {
    pub fn new(config: &Config) -> Result<Self> {
        let cluster = &config.cluster;
        let directory_url = cluster.directory_url.clone().ok_or_else(|| Error::internal("no directory URL"))?;
        Ok(Self {
            id: super::shard_id(cluster, &config.data_path)?,
            url: cluster.internal_url.clone().ok_or_else(|| Error::internal("no internal URL"))?,
            directory: super::directory_client(&directory_url, cluster.key_value()?)?,
            directory_url,
            sessions: Mutex::new(HashMap::new()),
            changes: AtomicU64::new(0),
            dirty: AtomicBool::new(false),
            registered: watch::Sender::new(false),
        })
    }

    pub fn directory(&self) -> DirectoryClient {
        self.directory.clone()
    }

    /// Who's calling, as the directory says (remembered for a few seconds).
    pub async fn authenticate(&self, metadata: &MetadataMap) -> Result<Viewer> {
        let token = auth::bearer(metadata).ok_or(Error::Unauthenticated)?;
        let key = auth::hash_token(token);
        {
            let sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
            if let Some((viewer, at)) = sessions.get(&key)
                && at.elapsed() < SESSION_CACHE
            {
                return Ok(viewer.clone());
            }
        }
        let found = self.directory().authenticate(cpb::AuthenticateRequest { token: token.to_string() }).await?;
        let found = found.into_inner();
        let viewer = match found.account {
            Some(account) => Viewer::Account { account: account_from_pb(account), token_hash: found.token_hash },
            None => Viewer::Operator,
        };
        let mut sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        if sessions.len() >= SESSION_CACHE_SIZE {
            sessions.clear();
        }
        sessions.insert(key, (viewer.clone(), Instant::now()));
        Ok(viewer)
    }

    /// Tells the directory's index something changed. A failure means it may
    /// have missed it, so the shard registers again soon.
    pub async fn tell<T>(&self, what: &str, call: impl AsyncFnOnce(DirectoryClient) -> Result<T, Status>) {
        self.changes.fetch_add(1, Ordering::SeqCst);
        if let Err(err) = call(self.directory()).await {
            self.dirty.store(true, Ordering::SeqCst);
            tracing::warn!(error = %err, "couldn't tell the directory {what}; will register again");
        }
    }
}

/// Starts following the directory, and waits until it knows this shard's
/// servers: answering before then would be for servers clients can't find.
pub async fn connect(app: &Arc<App>) {
    let AppLink::Shard(link) = &app.link else { return };
    let mut registered = link.registered.subscribe();
    tokio::spawn(stay_in_touch(app.clone()));
    loop {
        tokio::select! {
            _ = app.shutdown.cancelled() => return,
            done = tokio::time::timeout(Duration::from_secs(5), registered.wait_for(|done| *done)) => match done {
                Ok(_) => return,
                Err(_) => tracing::info!(directory = %link.directory_url, "waiting for the directory"),
            },
        }
    }
}

/// Follows the directory's settings, and registers this shard's servers each
/// time the connection starts (the directory may have restarted) and whenever
/// a change may not have reached it.
async fn stay_in_touch(app: Arc<App>) {
    let AppLink::Shard(link) = &app.link else { return };
    let mut backoff = Backoff::default();
    loop {
        let mut directory = link.directory();
        let watch = directory.watch(cpb::WatchRequest { shard_id: link.id.clone() });
        let watching = tokio::select! {
            _ = app.shutdown.cancelled() => return,
            watching = watch => watching,
        };
        match watching {
            Ok(response) => {
                let mut stream = response.into_inner();
                let mut first = true;
                let mut recheck = tokio::time::interval(RECHECK);
                loop {
                    tokio::select! {
                        _ = app.shutdown.cancelled() => return,
                        message = stream.message() => match message {
                            Ok(Some(message)) => {
                                if let Some(settings) = message.settings {
                                    app.replace_settings(Settings::from_pb(&app.config, &settings));
                                }
                                if first {
                                    first = false;
                                    backoff.reset();
                                    link.dirty.store(true, Ordering::SeqCst);
                                    register(&app, link).await;
                                }
                            }
                            Ok(None) => {
                                tracing::warn!("the directory closed its connection");
                                break;
                            }
                            Err(err) => {
                                tracing::warn!(error = %err.message(), "lost the directory");
                                break;
                            }
                        },
                        _ = recheck.tick() => if link.dirty.load(Ordering::SeqCst) {
                            register(&app, link).await;
                        },
                    }
                }
            }
            Err(err) => {
                tracing::warn!(directory = %link.directory_url, error = %err.message(), "can't reach the directory")
            }
        }
        tokio::select! {
            _ = app.shutdown.cancelled() => return,
            _ = tokio::time::sleep(backoff.wait()) => {}
        }
    }
}

/// Tells the directory every server here and who's in each. Runs again if a
/// change went out meanwhile, so the directory ends up with the latest.
async fn register(app: &App, link: &Link) {
    for _ in 0..5 {
        let before = link.changes.load(Ordering::SeqCst);
        link.dirty.store(false, Ordering::SeqCst);
        let registered = async {
            let servers = app.servers.entries().await?;
            let count = servers.len();
            let request = cpb::RegisterShardRequest { shard_id: link.id.clone(), url: link.url.clone(), servers };
            link.directory().register_shard(request).await?;
            Ok::<_, Error>(count)
        }
        .await;
        match registered {
            Err(err) => {
                link.dirty.store(true, Ordering::SeqCst);
                tracing::warn!(error = %err, "couldn't register with the directory");
                return;
            }
            Ok(count) if link.changes.load(Ordering::SeqCst) == before => {
                if !link.registered.send_replace(true) {
                    tracing::info!(shard = %link.id, servers = count, "registered with the directory");
                }
                return;
            }
            Ok(_) => continue,
        }
    }
    link.dirty.store(true, Ordering::SeqCst);
}

// ─────────────── Work across the servers kept here ───────────────

/// Copies someone's new profile into the servers here they're in, for others
/// to see.
pub async fn update_user(servers: &Servers, user: &pb::User, server_ids: &[String]) {
    for server_id in server_ids {
        let Ok(sdb) = servers.get(server_id).await else { continue };
        let updated = sdb
            .write(&user.id, async |conn, events| {
                store::upsert_user(conn, user).await?;
                if let Some(member) = store::member(conn, &sdb.id, &user.id).await? {
                    events.push(Payload::MemberUpdated(pb::MemberUpdated { member: Some(member) }));
                }
                Ok(())
            })
            .await;
        if let Err(err) = updated {
            tracing::warn!(server = %server_id, error = %err, "couldn't update a member's profile");
        }
    }
}

/// Takes a deleted account out of every server here it was ever in, leaving
/// `placeholder` as its name on what it wrote. Returns the servers it was
/// still a member of.
pub async fn forget_account(servers: &Servers, account_id: &str, placeholder: &pb::User) -> Result<Vec<String>> {
    let mut left_servers = Vec::new();
    for sdb in servers.all() {
        if store::user(&sdb.read()?, account_id).await?.is_none() {
            continue;
        }
        let left = sdb
            .write(account_id, async |conn, events| {
                let left = store::remove_member(conn, &sdb.id, account_id, events).await?;
                if left {
                    events.push(Payload::MemberLeft(pb::MemberLeft {
                        user_id: account_id.to_string(),
                        reason: pb::LeaveReason::Left as i32,
                    }));
                }
                store::upsert_user(conn, placeholder).await?;
                events.push(Payload::UserUpdated(pb::UserUpdated { user: Some(placeholder.clone()) }));
                Ok(left)
            })
            .await?;
        if left {
            left_servers.push(sdb.id.clone());
        }
    }
    Ok(left_servers)
}

/// An account's part of a data export from the servers here.
pub fn export_account(app: Arc<App>, account_id: String) -> mpsc::Receiver<Result<cpb::ExportAccountResponse>> {
    let (tx, rx) = mpsc::channel(4);
    tokio::spawn(async move {
        for sdb in app.servers.all() {
            match crate::api::export_server(&sdb, &account_id, &tx).await {
                Ok(true) => {}
                Ok(false) => return,
                Err(err) => {
                    let _ = tx.send(Err(err)).await;
                    return;
                }
            }
        }
    });
    rx
}

/// What admins see about the servers here with these ids (every one when empty).
pub async fn describe_servers(servers: &Servers, ids: &[String]) -> Result<Vec<cpb::ServerDescription>> {
    let chosen = if ids.is_empty() {
        servers.all()
    } else {
        let mut chosen = Vec::new();
        for id in ids {
            // A server deleted meanwhile is simply left out.
            if let Ok(sdb) = servers.get(id).await {
                chosen.push(sdb);
            }
        }
        chosen
    };
    let mut described = Vec::with_capacity(chosen.len());
    for sdb in chosen {
        let server = sdb.server().await?;
        let owner = store::user(&sdb.read()?, &server.owner_id).await?;
        described.push(cpb::ServerDescription {
            usage: Some(sdb.usage().await?),
            own_limits: Some(sdb.own_limits().await?),
            owner,
            server: Some(server),
        });
    }
    Ok(described)
}

pub async fn channel_exists(servers: &Servers, server_id: &str, channel_id: &str) -> Result<bool> {
    let sdb = servers.get(server_id).await?;
    let conn = sdb.read()?;
    Ok(query_one(&conn, "SELECT 1 FROM channels WHERE id = ?1", [channel_id], |r| r.get::<i64>(0)).await?.is_some())
}

// ─────────────── What the directory asks of a shard ───────────────

/// The cluster calls a shard answers.
#[derive(Clone)]
pub struct Internal {
    app: Arc<App>,
}

impl Internal {
    pub fn new(app: Arc<App>) -> Self {
        Self { app }
    }
}

fn respond<T>(result: Result<T>) -> Result<Response<T>, Status> {
    result.map(Response::new).map_err(Into::into)
}

type ExportStream = Pin<Box<dyn Stream<Item = Result<cpb::ExportAccountResponse, Status>> + Send>>;

#[tonic::async_trait]
impl ShardService for Internal {
    async fn create_server(
        &self,
        request: Request<cpb::CreateServerRequest>,
    ) -> Result<Response<cpb::CreateServerResponse>, Status> {
        let req = request.into_inner();
        let owner = req.owner.ok_or_else(|| Status::invalid_argument("owner is required"))?;
        let new = NewServer {
            name: req.name,
            description: req.description,
            icon_url: req.icon_url,
            discoverable: req.discoverable,
        };
        respond(
            self.app.servers.create(&owner, new).await.map(|server| cpb::CreateServerResponse { server: Some(server) }),
        )
    }

    async fn update_user(
        &self,
        request: Request<cpb::UpdateUserRequest>,
    ) -> Result<Response<cpb::UpdateUserResponse>, Status> {
        let req = request.into_inner();
        let user = req.user.ok_or_else(|| Status::invalid_argument("user is required"))?;
        update_user(&self.app.servers, &user, &req.server_ids).await;
        Ok(Response::new(cpb::UpdateUserResponse {}))
    }

    async fn forget_account(
        &self,
        request: Request<cpb::ForgetAccountRequest>,
    ) -> Result<Response<cpb::ForgetAccountResponse>, Status> {
        let req = request.into_inner();
        let placeholder = req.placeholder.ok_or_else(|| Status::invalid_argument("placeholder is required"))?;
        respond(
            forget_account(&self.app.servers, &req.account_id, &placeholder)
                .await
                .map(|left_server_ids| cpb::ForgetAccountResponse { left_server_ids }),
        )
    }

    type ExportAccountStream = ExportStream;

    async fn export_account(
        &self,
        request: Request<cpb::ExportAccountRequest>,
    ) -> Result<Response<ExportStream>, Status> {
        let pieces = export_account(self.app.clone(), request.into_inner().account_id);
        let stream = futures::StreamExt::map(ReceiverStream::new(pieces), |piece| piece.map_err(Status::from));
        Ok(Response::new(Box::pin(stream)))
    }

    async fn describe_servers(
        &self,
        request: Request<cpb::DescribeServersRequest>,
    ) -> Result<Response<cpb::DescribeServersResponse>, Status> {
        let ids = request.into_inner().server_ids;
        respond(describe_servers(&self.app.servers, &ids).await.map(|servers| cpb::DescribeServersResponse { servers }))
    }

    async fn channel_exists(
        &self,
        request: Request<cpb::ChannelExistsRequest>,
    ) -> Result<Response<cpb::ChannelExistsResponse>, Status> {
        let req = request.into_inner();
        respond(
            channel_exists(&self.app.servers, &req.server_id, &req.channel_id)
                .await
                .map(|exists| cpb::ChannelExistsResponse { exists }),
        )
    }
}
