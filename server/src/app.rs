//! The running instance: its state, its HTTP router, and serving it.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use axum::Router;
use axum::routing::get;
use http::{HeaderName, HeaderValue, Method};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::api::Api;
use crate::auth::SignInLimiter;
use crate::cluster::Role;
use crate::cluster::index::Index;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::hub::Hub;
use crate::node::NodeDb;
use crate::pb;
use crate::pb::{
    account_service_server::AccountServiceServer, admin_service_server::AdminServiceServer,
    auth_service_server::AuthServiceServer, channel_service_server::ChannelServiceServer,
    event_service_server::EventServiceServer, invite_service_server::InviteServiceServer,
    media_service_server::MediaServiceServer, message_service_server::MessageServiceServer,
    node_service_server::NodeServiceServer, role_service_server::RoleServiceServer,
    server_service_server::ServerServiceServer,
};
use crate::servers::Servers;
use crate::settings::Settings;

pub struct App {
    /// How the process was started. Settings admins can change live in `settings`.
    pub config: Config,
    settings: watch::Sender<Arc<Settings>>,
    /// The banner admins put up, as last set; see [`App::announcement`].
    announcement: RwLock<Option<pb::Announcement>>,
    /// Accounts, sessions and settings: kept by a single process, or a split
    /// instance's directory. See [`App::node`].
    node: Option<NodeDb>,
    /// Uploaded pictures, under `<data>/media/`, where `node` is.
    media: Option<crate::media::Store>,
    /// Every server and who's in it, where `node` is.
    pub index: Index,
    /// The servers whose files are here: all of them, or a shard's share.
    pub servers: Servers,
    pub hub: Arc<Hub>,
    pub limiter: SignInLimiter,
    pub started: Instant,
    /// Cancelled when the instance shuts down, ending live streams.
    pub shutdown: CancellationToken,
    /// How this process reaches the other parts of a split instance.
    pub link: Link,
}

/// Where the parts this process doesn't run are.
pub enum Link {
    /// Nowhere: this process runs everything.
    Alone,
    /// This is the directory; server files are on shards.
    Directory(crate::cluster::directory::Shards),
    /// This is a shard; accounts are on the directory.
    Shard(Box<crate::cluster::shard::Link>),
}

impl App {
    /// Opens the data directory: node.db and every server under servers/, or
    /// the part of them this process's role keeps.
    pub async fn open(config: Config) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&config.data_path)?;
        let key = config.encryption_key.clone();
        let role = config.cluster.role;
        let hub = Arc::new(Hub::default());
        // Exports are written here and streamed; one left by a crash is stale.
        let _ = std::fs::remove_dir_all(config.data_path.join("exports"));

        let (node, media, servers, link) = match role {
            Role::Gateway => return Err(Error::internal("a gateway keeps no data")),
            Role::All | Role::Directory => {
                let node = NodeDb::open(&config.data_path.join("node.db"), key.as_ref()).await?;
                node.install_id().await?;
                let media = crate::media::Store::open(&config.data_path)?;
                media.remove_strays(&node.media_ids().await?)?;
                let (servers, link) = if role == Role::All {
                    (Servers::open(&config.data_path, key, hub.clone(), false).await?, Link::Alone)
                } else {
                    let shards = crate::cluster::directory::Shards::load(&config, &node).await?;
                    (Servers::none(hub.clone()), Link::Directory(shards))
                };
                (Some(node), Some(media), servers, link)
            }
            Role::Shard => {
                let servers = Servers::open(&config.data_path, key, hub.clone(), true).await?;
                (None, None, servers, Link::Shard(Box::new(crate::cluster::shard::Link::new(&config)?)))
            }
        };

        let index = Index::default();
        let (settings, announcement) = match &node {
            Some(node) => (Settings::load(&config, &node.settings().await?), node.announcement().await?),
            // A shard follows the directory's settings once it's connected.
            None => (Settings::defaults(&config), None),
        };
        match (&link, &node) {
            (Link::Alone, _) => {
                for entry in servers.entries().await? {
                    if let Some(server) = entry.server {
                        index.insert(server, entry.member_ids, entry.invite_codes, None);
                    }
                }
            }
            (Link::Directory(_), Some(node)) => index.load_placements(node.placements().await?),
            _ => {}
        }

        let app = Arc::new(Self {
            config,
            settings: watch::Sender::new(Arc::new(settings)),
            announcement: RwLock::new(announcement),
            node,
            media,
            index,
            servers,
            hub,
            limiter: SignInLimiter::default(),
            started: Instant::now(),
            shutdown: CancellationToken::new(),
            link,
        });
        if app.node.is_some() {
            app.sweep_media(crate::id::now_ms()).await?;
        }
        crate::cluster::shard::connect(&app).await;
        Ok(app)
    }

    /// Accounts, sessions and settings. Only a single process and a split
    /// instance's directory keep them; the gateways route every call that
    /// needs them there.
    pub fn node(&self) -> Result<&NodeDb> {
        self.node.as_ref().ok_or_else(|| Error::internal("this part of the instance doesn't keep accounts"))
    }

    /// Uploaded pictures' files, where accounts are kept.
    pub fn media(&self) -> Result<&crate::media::Store> {
        self.media.as_ref().ok_or_else(|| Error::internal("this part of the instance doesn't keep pictures"))
    }

    /// Deletes uploads that never arrived and pictures nothing used, as of `now`.
    pub async fn sweep_media(&self, now: i64) -> Result<usize> {
        let ids = self.node()?.sweepable_media(now).await?;
        self.delete_media(&ids).await?;
        Ok(ids.len())
    }

    /// Deletes uploaded files and their rows.
    pub async fn delete_media(&self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        self.node()?.delete_media(ids).await?;
        for id in ids {
            self.media()?.remove(id);
        }
        Ok(())
    }

    /// The settings in force right now.
    pub fn settings(&self) -> Arc<Settings> {
        self.settings.borrow().clone()
    }

    /// Puts new settings in force for every request from now on (and sends
    /// them to the other parts of a split instance).
    pub fn replace_settings(&self, settings: Settings) {
        self.settings.send_if_modified(|current| {
            let changed = **current != settings;
            *current = Arc::new(settings);
            changed
        });
    }

    /// The settings, each time they change.
    pub fn watch_settings(&self) -> watch::Receiver<Arc<Settings>> {
        self.settings.subscribe()
    }

    /// The banner clients should show now: the last one set, unless it ran out.
    pub fn announcement(&self) -> Option<pb::Announcement> {
        let current = self.announcement.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        current.filter(|a| a.ends_at.as_ref().is_none_or(|end| crate::id::millis(end) > crate::id::now_ms()))
    }

    pub fn replace_announcement(&self, announcement: Option<pb::Announcement>) {
        *self.announcement.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = announcement;
    }

    pub fn node_info(&self) -> pb::Node {
        node_info(&self.settings(), self.announcement())
    }

    /// Every route: the gRPC services (also reachable as gRPC-Web from
    /// browsers), gRPC health and reflection, and plain HTTP health. The parts
    /// of a split instance also answer the cluster protocol, and only calls
    /// carrying the cluster key.
    pub fn router(self: &Arc<Self>) -> Router {
        let api = Api::new(self.clone());
        let (_, health) = tonic_health::server::health_reporter();
        let reflection = tonic_reflection::server::Builder::configure()
            .register_encoded_file_descriptor_set(crate::proto::FILE_DESCRIPTOR_SET)
            .build_v1()
            .expect("the embedded descriptor set is valid");

        let mut grpc = tonic::service::Routes::new(NodeServiceServer::new(api.clone()))
            .add_service(AuthServiceServer::new(api.clone()))
            .add_service(AccountServiceServer::new(api.clone()))
            .add_service(ServerServiceServer::new(api.clone()))
            .add_service(ChannelServiceServer::new(api.clone()))
            .add_service(MessageServiceServer::new(api.clone()))
            .add_service(RoleServiceServer::new(api.clone()))
            .add_service(InviteServiceServer::new(api.clone()))
            .add_service(EventServiceServer::new(api.clone()))
            .add_service(MediaServiceServer::new(api.clone()))
            .add_service(AdminServiceServer::new(api))
            .add_service(health)
            .add_service(reflection);
        grpc = match &self.link {
            Link::Alone => grpc,
            Link::Directory(_) => grpc
                .add_service(crate::cluster::directory_server(crate::cluster::directory::Internal::new(self.clone()))),
            Link::Shard(_) => {
                grpc.add_service(crate::cluster::shard_server(crate::cluster::shard::Internal::new(self.clone())))
            }
        };
        // Only the gRPC routes: gRPC-Web answers anything else over HTTP/1.1 with a 400.
        let mut router =
            grpc.into_axum_router().layer(tonic_web::GrpcWebLayer::new()).route("/healthz", get(|| async { "ok" }));
        if self.node.is_some() {
            router = router.merge(crate::media::routes(self.clone()));
        }
        if !self.config.cluster.is_split() {
            // The web app (when it's on) answers every other GET, so its own addresses work on reload.
            return router.fallback(crate::web::handler(self.clone())).layer(cors(self.clone()));
        }
        // Behind gateways, which serve the web app and answer browsers' CORS.
        let key: Arc<str> = self.config.cluster.key.as_deref().unwrap_or_default().into();
        router.layer(axum::middleware::from_fn_with_state(key, crate::cluster::require_key))
    }
}

/// What clients are told about the instance.
pub fn node_info(settings: &Settings, announcement: Option<pb::Announcement>) -> pb::Node {
    pb::Node {
        name: settings.name.clone(),
        version: crate::VERSION.into(),
        public_url: settings.public_url.clone(),
        auth: Some(pb::AuthMethods {
            local_sign_in: settings.local_accounts.sign_in(),
            local_sign_up: settings.local_accounts.sign_up(),
            linked_sign_in: settings.linked_sign_in(),
            linked_sign_up: settings.linked_sign_up(),
            linked_issuer: if settings.linked_sign_in() { settings.linked_issuer.clone() } else { String::new() },
        }),
        server_creation: settings.server_creation as i32,
        telemetry: settings.telemetry,
        announcement,
        build: Some(pb::Build {
            version: crate::VERSION.into(),
            commit: crate::COMMIT.into(),
            source: crate::SOURCE.into(),
        }),
    }
}

/// Anything that has the settings in force.
pub trait HasSettings: Send + Sync + 'static {
    fn settings(&self) -> Arc<Settings>;
}

impl HasSettings for App {
    fn settings(&self) -> Arc<Settings> {
        App::settings(self)
    }
}

/// CORS for browsers on other sites, such as a fuwa app served by another
/// instance. The allowed origins are read per request, so changes apply at once.
pub fn cors(source: Arc<impl HasSettings>) -> CorsLayer {
    let allow_origin =
        AllowOrigin::predicate(move |origin: &HeaderValue, _| source.settings().allows_origin(origin.as_bytes()));
    let headers = |names: &[&'static str]| names.iter().map(|name| HeaderName::from_static(name)).collect::<Vec<_>>();
    CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::OPTIONS])
        .allow_headers(headers(&[
            "authorization",
            "content-type",
            "x-grpc-web",
            "x-user-agent",
            "grpc-timeout",
            "grpc-accept-encoding",
            "connect-protocol-version",
            "connect-timeout-ms",
        ]))
        .expose_headers(headers(&["grpc-status", "grpc-message", "grpc-status-details-bin"]))
        .max_age(Duration::from_secs(2 * 60 * 60))
}

/// Opens the instance (or this process's part of it) and serves it until
/// Ctrl-C or SIGTERM.
pub async fn run(config: Config) -> std::result::Result<(), String> {
    let address: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|_| format!("FUWA_HOST {:?} isn't an IP address to listen on", config.host))?;
    if config.cluster.role == Role::Gateway {
        return crate::cluster::gateway::run(config, address).await;
    }
    let data_path = config.data_path.clone();
    let listener =
        tokio::net::TcpListener::bind(address).await.map_err(|err| format!("couldn't listen on {address}: {err}"))?;
    let app = App::open(config)
        .await
        .map_err(|err| format!("couldn't open the data directory {}: {err}", data_path.display()))?;

    tracing::info!(
        version = crate::VERSION,
        role = app.config.cluster.role.as_str(),
        %address,
        public_url = %app.settings().public_url,
        data = %data_path.display(),
        servers = app.servers.len(),
        local_accounts = app.settings().local_accounts.as_str(),
        linked_accounts = app.settings().linked_accounts.as_str(),
        "fuwa is up"
    );

    if app.node.is_some() {
        crate::telemetry::spawn(app.clone());
        spawn_housekeeping(app.clone());
    }
    spawn_signal_handler(app.shutdown.clone());

    let shutdown = app.shutdown.clone();
    axum::serve(listener, app.router())
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
        .map_err(|err| format!("server error: {err}"))
}

/// Cancels `shutdown` on Ctrl-C or SIGTERM.
pub fn spawn_signal_handler(shutdown: CancellationToken) {
    tokio::spawn(async move {
        wait_for_signal().await;
        tracing::info!("shutting down");
        shutdown.cancel();
    });
}

/// Hourly: drops expired sessions and sweeps uploads nothing uses.
fn spawn_housekeeping(app: Arc<App>) {
    tokio::spawn(async move {
        let mut every = tokio::time::interval(Duration::from_secs(60 * 60));
        loop {
            tokio::select! {
                _ = app.shutdown.cancelled() => return,
                _ = every.tick() => {
                    if let Err(err) = async { app.node()?.prune_sessions().await }.await {
                        tracing::warn!(error = %err, "couldn't prune expired sessions");
                    }
                    if let Err(err) = app.sweep_media(crate::id::now_ms()).await {
                        tracing::warn!(error = %err, "couldn't sweep unused uploads");
                    }
                }
            }
        }
    });
}

async fn wait_for_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
