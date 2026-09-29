//! The running instance: its state, its HTTP router, and serving it.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use axum::Router;
use axum::routing::get;
use http::{HeaderName, HeaderValue, Method};
use tokio_util::sync::CancellationToken;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::api::Api;
use crate::auth::SignInLimiter;
use crate::config::Config;
use crate::error::Result;
use crate::hub::Hub;
use crate::node::NodeDb;
use crate::pb;
use crate::pb::{
    account_service_server::AccountServiceServer, admin_service_server::AdminServiceServer,
    auth_service_server::AuthServiceServer, channel_service_server::ChannelServiceServer,
    event_service_server::EventServiceServer, message_service_server::MessageServiceServer,
    node_service_server::NodeServiceServer, server_service_server::ServerServiceServer,
};
use crate::servers::Servers;
use crate::settings::Settings;

pub struct App {
    /// How the process was started. Settings admins can change live in `settings`.
    pub config: Config,
    settings: RwLock<Arc<Settings>>,
    /// The banner admins put up, as last set; see [`App::announcement`].
    announcement: RwLock<Option<pb::Announcement>>,
    pub node: NodeDb,
    pub servers: Servers,
    pub hub: Arc<Hub>,
    pub limiter: SignInLimiter,
    pub started: Instant,
    /// Cancelled when the instance shuts down, ending live streams.
    pub shutdown: CancellationToken,
}

impl App {
    /// Opens the data directory: node.db and every server under servers/.
    pub async fn open(config: Config) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&config.data_path)?;
        let key = config.encryption_key.clone();
        let node = NodeDb::open(&config.data_path.join("node.db"), key.as_ref()).await?;
        node.install_id().await?;
        let hub = Arc::new(Hub::default());
        let servers = Servers::open(&config.data_path, key, hub.clone()).await?;
        let settings = Settings::load(&config, &node.settings().await?);
        let announcement = node.announcement().await?;
        // Exports are written here and streamed; one left by a crash is stale.
        let _ = std::fs::remove_dir_all(config.data_path.join("exports"));
        Ok(Arc::new(Self {
            config,
            settings: RwLock::new(Arc::new(settings)),
            announcement: RwLock::new(announcement),
            node,
            servers,
            hub,
            limiter: SignInLimiter::default(),
            started: Instant::now(),
            shutdown: CancellationToken::new(),
        }))
    }

    /// The settings in force right now.
    pub fn settings(&self) -> Arc<Settings> {
        self.settings.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// Puts new settings in force for every request from now on.
    pub fn replace_settings(&self, settings: Settings) {
        *self.settings.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::new(settings);
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
        let settings = self.settings();
        pb::Node {
            name: settings.name.clone(),
            version: crate::VERSION.into(),
            public_url: settings.public_url.clone(),
            auth: Some(pb::AuthMethods {
                local_sign_in: settings.local_accounts.sign_in(),
                local_sign_up: settings.local_accounts.sign_up(),
                linked_sign_in: false,
                linked_issuer: String::new(),
            }),
            server_creation: settings.server_creation as i32,
            telemetry: settings.telemetry,
            announcement: self.announcement(),
        }
    }

    /// Every route: the gRPC services (also reachable as gRPC-Web from
    /// browsers), gRPC health and reflection, and plain HTTP health.
    pub fn router(self: &Arc<Self>) -> Router {
        let api = Api::new(self.clone());
        let (_, health) = tonic_health::server::health_reporter();
        let reflection = tonic_reflection::server::Builder::configure()
            .register_encoded_file_descriptor_set(crate::proto::FILE_DESCRIPTOR_SET)
            .build_v1()
            .expect("the embedded descriptor set is valid");

        let grpc = tonic::service::Routes::new(NodeServiceServer::new(api.clone()))
            .add_service(AuthServiceServer::new(api.clone()))
            .add_service(AccountServiceServer::new(api.clone()))
            .add_service(ServerServiceServer::new(api.clone()))
            .add_service(ChannelServiceServer::new(api.clone()))
            .add_service(MessageServiceServer::new(api.clone()))
            .add_service(EventServiceServer::new(api.clone()))
            .add_service(AdminServiceServer::new(api))
            .add_service(health)
            .add_service(reflection)
            .into_axum_router()
            // Only the gRPC routes: gRPC-Web answers anything else over HTTP/1.1 with a 400.
            .layer(tonic_web::GrpcWebLayer::new());

        grpc.route("/healthz", get(|| async { "ok" }))
            // The web app (when it's on) answers every other GET, so its own addresses work on reload.
            .fallback(crate::web::handler(self.clone()))
            .layer(self.cors())
    }

    /// CORS for browsers on other sites, such as a fuwa app served by another
    /// instance. The allowed origins are read per request, so changes apply at once.
    fn cors(self: &Arc<Self>) -> CorsLayer {
        let app = self.clone();
        let allow_origin =
            AllowOrigin::predicate(move |origin: &HeaderValue, _| app.settings().allows_origin(origin.as_bytes()));
        let headers =
            |names: &[&'static str]| names.iter().map(|name| HeaderName::from_static(name)).collect::<Vec<_>>();
        CorsLayer::new()
            .allow_origin(allow_origin)
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
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
}

/// Opens the instance and serves it until Ctrl-C or SIGTERM.
pub async fn run(config: Config) -> std::result::Result<(), String> {
    let address: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|_| format!("FUWA_HOST {:?} isn't an IP address to listen on", config.host))?;
    let data_path = config.data_path.clone();
    let app = App::open(config)
        .await
        .map_err(|err| format!("couldn't open the data directory {}: {err}", data_path.display()))?;
    let listener =
        tokio::net::TcpListener::bind(address).await.map_err(|err| format!("couldn't listen on {address}: {err}"))?;

    let (servers, _) = app.servers.count();
    tracing::info!(
        version = crate::VERSION,
        %address,
        public_url = %app.settings().public_url,
        data = %data_path.display(),
        servers,
        local_accounts = app.settings().local_accounts.as_str(),
        "fuwa is up"
    );

    crate::telemetry::spawn(app.clone());
    spawn_session_pruning(app.clone());

    let shutdown = app.shutdown.clone();
    tokio::spawn(async move {
        wait_for_signal().await;
        tracing::info!("shutting down");
        shutdown.cancel();
    });

    let shutdown = app.shutdown.clone();
    axum::serve(listener, app.router())
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
        .map_err(|err| format!("server error: {err}"))
}

fn spawn_session_pruning(app: Arc<App>) {
    tokio::spawn(async move {
        let mut every = tokio::time::interval(Duration::from_secs(60 * 60));
        loop {
            tokio::select! {
                _ = app.shutdown.cancelled() => return,
                _ = every.tick() => {
                    if let Err(err) = app.node.prune_sessions().await {
                        tracing::warn!(error = %err, "couldn't prune expired sessions");
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
