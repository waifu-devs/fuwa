//! The running instance: its state, its HTTP router, and serving it.

use std::net::{IpAddr, SocketAddr};
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
use crate::dms::DmDb;
use crate::error::{Error, Result};
use crate::hub::Hub;
use crate::node::NodeDb;
use crate::pb;
use crate::pb::{
    account_service_server::AccountServiceServer, admin_service_server::AdminServiceServer,
    agent_service_server::AgentServiceServer, auth_service_server::AuthServiceServer,
    auto_mod_service_server::AutoModServiceServer, channel_service_server::ChannelServiceServer,
    direct_message_service_server::DirectMessageServiceServer, emoji_service_server::EmojiServiceServer,
    event_service_server::EventServiceServer, invite_service_server::InviteServiceServer,
    join_service_server::JoinServiceServer, media_service_server::MediaServiceServer,
    message_service_server::MessageServiceServer, node_service_server::NodeServiceServer,
    role_service_server::RoleServiceServer, server_service_server::ServerServiceServer,
    webhook_service_server::WebhookServiceServer,
};
use crate::replica::Replica;
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
    /// Direct messages' devices, conversations and ciphertext, where `node` is.
    dms: Option<DmDb>,
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
    /// Where this process's databases and pictures are continuously copied.
    pub replica: Option<Arc<Replica>>,
    /// Who's in the calls this part keeps: its servers' voice channels, and
    /// direct-message calls where accounts are kept.
    pub voice: crate::voice::Voice,
    /// The media part calls' sound goes through.
    pub media_link: crate::voice::MediaLink,
    /// The voice channels being recorded on the server, where `voice` keeps them.
    pub recordings: crate::recordings::Recordings,
    /// What links to pictures from other sites are signed with.
    picture_key: crate::outside::Key,
    /// How this instance talks to other fuwa instances (docs/federation.md).
    pub federation: crate::federation::Federation,
}

/// Where the parts this process doesn't run are.
pub enum Link {
    /// Nowhere: this process runs everything.
    Alone,
    /// This is the directory; server files are on shards.
    Directory(Box<crate::cluster::directory::Shards>),
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

        // A shard takes the single process's servers first, so a shard that
        // just took them never looks empty to the replica's restore check.
        if role == Role::Shard {
            crate::cluster::shard::take_servers(&config).await?;
        }
        // Only a split instance's directory and shards replicate (see config.rs).
        let replica = match &config.replica {
            Some(replica_config) if matches!(role, Role::Directory | Role::Shard) => {
                let store = replica_config.store()?;
                let shard = match role {
                    Role::Shard => Some(crate::cluster::shard_id(&config.cluster, &config.data_path)?),
                    _ => None,
                };
                let part = match &shard {
                    Some(shard) => crate::replica::Part::Shard(shard),
                    None => crate::replica::Part::Directory,
                };
                crate::replica::prepare(replica_config, &store, &config.data_path, key.as_ref(), part).await?;
                Some(Replica::new(store, replica_config.interval, &config.data_path, shard)?)
            }
            _ => None,
        };

        let (node, dms, media, servers, link) = match role {
            Role::Gateway | Role::Media => return Err(Error::internal("this part keeps no data")),
            Role::All | Role::Directory => {
                let node = NodeDb::open(&config.data_path.join("node.db"), key.as_ref()).await?;
                if let Some(replica) = &replica {
                    replica.track("node", node.db().clone()).await?;
                    replica.track_media(config.data_path.join("media"));
                }
                node.install_id().await?;
                let dms = DmDb::open(&config.data_path.join("dms.db"), key.as_ref()).await?;
                if let Some(replica) = &replica {
                    replica.track("dms", dms.db().clone()).await?;
                }
                let media = crate::media::Store::open(&config.data_path)?;
                let media_ids = node.media_ids().await?;
                media.remove_strays(&media_ids)?;
                if let Some(replica) = &replica {
                    match replica.prune_media(&media_ids).await {
                        Ok(0) => {}
                        Ok(pruned) => tracing::info!(pruned, "deleted pictures from the replica that nothing knows"),
                        Err(err) => tracing::warn!(error = %err, "couldn't tidy the pictures in the replica"),
                    }
                }
                let (servers, link) = if role == Role::All {
                    (
                        Servers::open(&config.data_path, key, hub.clone(), false, None, &config.cluster.region).await?,
                        Link::Alone,
                    )
                } else {
                    let shards = crate::cluster::directory::Shards::load(&config, &node).await?;
                    (Servers::none(hub.clone()), Link::Directory(Box::new(shards)))
                };
                (Some(node), Some(dms), Some(media), servers, link)
            }
            Role::Shard => {
                let servers =
                    Servers::open(&config.data_path, key, hub.clone(), true, replica.clone(), &config.cluster.region)
                        .await?;
                (None, None, None, servers, Link::Shard(Box::new(crate::cluster::shard::Link::new(&config)?)))
            }
        };

        // Every part of a split instance signs picture links alike; a single
        // process keeps its own key.
        let picture_key = match (&config.cluster.key, &node) {
            (Some(cluster_key), _) => crate::outside::Key::from_cluster_key(cluster_key),
            (None, Some(node)) => node.picture_key().await?,
            (None, None) => return Err(Error::internal("a shard needs FUWA_CLUSTER_KEY")),
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

        let federation = crate::federation::Federation::new(config.federation_allow_private);
        let shutdown = CancellationToken::new();
        let media_link = media_link(&config, &shutdown).await;

        let app = Arc::new(Self {
            config,
            settings: watch::Sender::new(Arc::new(settings)),
            announcement: RwLock::new(announcement),
            node,
            dms,
            media,
            index,
            servers,
            hub,
            limiter: SignInLimiter::default(),
            started: Instant::now(),
            shutdown,
            link,
            replica,
            voice: crate::voice::Voice::default(),
            recordings: crate::recordings::Recordings::default(),
            media_link,
            picture_key,
            federation,
        });
        if app.node.is_some() {
            app.sweep_media(crate::id::now_ms()).await?;
        }
        crate::cluster::shard::connect(&app).await;
        // Shared channels' messages reach the servers showing them from the start.
        if matches!(app.link, Link::Alone | Link::Shard(_)) {
            crate::api::spawn_shared_fanout(app.clone());
        }
        Ok(app)
    }

    /// Accounts, sessions and settings. Only a single process and a split
    /// instance's directory keep them; the gateways route every call that
    /// needs them there.
    pub fn node(&self) -> Result<&NodeDb> {
        self.node.as_ref().ok_or_else(|| Error::internal("this part of the instance doesn't keep accounts"))
    }

    /// What links to pictures from other sites are signed with.
    pub fn picture_key(&self) -> &crate::outside::Key {
        &self.picture_key
    }

    /// The link to store for a picture someone gave: see [`crate::outside::link`].
    pub fn picture_link(&self, url: &str) -> String {
        crate::outside::link(&self.picture_key, &self.settings().public_url, url)
    }

    /// Direct messages, where accounts are kept.
    pub fn dms(&self) -> Result<&DmDb> {
        self.dms.as_ref().ok_or_else(|| Error::internal("this part of the instance doesn't keep direct messages"))
    }

    /// Forgets the direct-message devices of sessions that ended.
    pub async fn sweep_devices(&self) -> Result<usize> {
        let live = self.node()?.live_session_ids(None).await?;
        self.dms()?.sweep(&live).await
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
        if let Some(replica) = &self.replica {
            replica.drop_media(ids).await;
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
        pb::Node { regions: self.regions(), ..node_info(&self.settings(), self.announcement()) }
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
            .add_service(JoinServiceServer::new(api.clone()))
            .add_service(AutoModServiceServer::new(api.clone()))
            .add_service(EmojiServiceServer::new(api.clone()))
            .add_service(WebhookServiceServer::new(api.clone()))
            .add_service(AgentServiceServer::new(api.clone()))
            .add_service(crate::pb::sso_service_server::SsoServiceServer::new(api.clone()))
            .add_service(EventServiceServer::new(api.clone()))
            .add_service(MediaServiceServer::new(api.clone()))
            .add_service(DirectMessageServiceServer::new(api.clone()))
            .add_service(crate::pb::call_service_server::CallServiceServer::new(api.clone()))
            .add_service(crate::pb::secure_channel_service_server::SecureChannelServiceServer::new(api.clone()))
            .add_service(crate::pb::shared_channel_service_server::SharedChannelServiceServer::new(api.clone()))
            .add_service(AdminServiceServer::new(api))
            .add_service(health)
            .add_service(reflection);
        // Other instances' calls, where node.db is (docs/federation.md).
        if self.node.is_some() {
            grpc = grpc.add_service(crate::federation::server(self.clone()));
        }
        grpc = match &self.link {
            Link::Alone => grpc,
            Link::Directory(_) => grpc
                .add_service(crate::cluster::directory_server(crate::cluster::directory::Internal::new(self.clone()))),
            Link::Shard(_) => {
                grpc.add_service(crate::cluster::shard_server(crate::cluster::shard::Internal::new(self.clone())))
            }
        };
        // Only the gRPC routes: gRPC-Web answers anything else over HTTP/1.1 with a 400.
        let mut router = grpc
            .into_axum_router()
            .layer(axum::middleware::from_fn(crate::reports::time_calls))
            .layer(tonic_web::GrpcWebLayer::new())
            .route("/healthz", get(|| async { "ok" }));
        if matches!(self.link, Link::Alone | Link::Directory(_)) {
            // Which parts are up, for a status page. A directory's is behind the cluster key
            // (only gateways ask it); a single process answers anyone.
            let app = self.clone();
            router = router.route(
                "/healthz/parts",
                get(move || {
                    let app = app.clone();
                    async move { crate::cluster::status::parts(&app).await }
                }),
            );
        }
        if self.node.is_some() {
            router = router.merge(crate::media::routes(self.clone())).merge(crate::outside::routes(self.clone()));
        }
        if self.node.is_some() {
            router = router.merge(crate::sso::http::instance_routes(self.clone()));
        }
        if let Link::Shard(_) = &self.link {
            router = router.merge(crate::cluster::pictures::routes(self.clone()));
        }
        if matches!(self.link, Link::Alone | Link::Shard(_)) {
            router = router.merge(crate::webhooks::routes(self.clone()));
            router = router.merge(crate::sso::http::server_routes(self.clone()));
        }
        if !self.config.cluster.is_split() {
            // The web app (when it's on) answers every other GET, so its own addresses work on reload.
            return router
                .fallback(crate::web::handler(self.clone()))
                .layer(cors(self.clone()))
                .layer(axum::middleware::from_fn(crate::web::no_store_by_default))
                .layer(axum::middleware::from_fn(crate::probes::turn_away));
        }
        if let Link::Directory(_) = &self.link {
            let wait = crate::cluster::directory::wait_for_shards;
            router = router.layer(axum::middleware::from_fn_with_state(self.clone(), wait));
        }
        // Behind gateways, which serve the web app and answer browsers' CORS.
        let key: Arc<str> = self.config.cluster.key.as_deref().unwrap_or_default().into();
        router.layer(axum::middleware::from_fn_with_state(key, crate::cluster::require_key))
    }
}

/// Where calls' sound goes: a media part in this process (when it runs
/// everything), or the split instance's media parts.
async fn media_link(config: &Config, shutdown: &CancellationToken) -> crate::voice::MediaLink {
    use crate::voice::MediaLink;
    if config.cluster.is_split() {
        if config.media_urls.is_empty() {
            return MediaLink::Off("no media part is set up (FUWA_MEDIA_URL)".into());
        }
        return match config.cluster.media_key_value().and_then(|key| MediaLink::remote(&config.media_urls, key)) {
            Ok(link) => link,
            Err(err) => MediaLink::Off(err.to_string()),
        };
    }
    let Some(media) = config.media.clone() else {
        return MediaLink::Off("FUWA_MEDIA_PORT is off".into());
    };
    // Calls stop with the rest of the process, after telling apps to join again.
    match crate::rtc::Sfu::start(media, shutdown.child_token()).await {
        Ok(sfu) => MediaLink::Local(sfu),
        Err(err) => {
            tracing::warn!(error = %err, "calls are off: the media part couldn't start (see FUWA_MEDIA_PORT)");
            MediaLink::Off(err.to_string())
        }
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
            sso_sign_in: settings.sso_sign_in(),
            sso_sign_up: settings.sso_sign_up(),
            sso_name: if settings.sso_sign_in() { settings.sso_provider.name.clone() } else { String::new() },
            sso_host: if settings.sso_sign_in() { settings.sso_provider.host() } else { String::new() },
            linked_issuer: if settings.linked_sign_in() { settings.linked_issuer.clone() } else { String::new() },
        }),
        server_creation: settings.server_creation as i32,
        agent_creation: settings.agent_creation as i32,
        telemetry: settings.telemetry,
        shared_channels: settings.shared_channels,
        federation: settings.shared_channels && settings.federation && !settings.public_url.is_empty(),
        profile_effects: settings.profile_effects,
        announcement,
        build: Some(pb::Build {
            version: crate::VERSION.into(),
            commit: crate::COMMIT.into(),
            source: crate::SOURCE.into(),
        }),
        regions: vec![],
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
    let host = config.host.trim().trim_start_matches('[').trim_end_matches(']');
    let address = host
        .parse::<IpAddr>()
        .map(|ip| SocketAddr::new(ip, config.port))
        .map_err(|_| format!("FUWA_HOST {:?} isn't an IP address to listen on", config.host))?;
    match config.cluster.role {
        Role::Gateway => return crate::cluster::gateway::run(config, address).await,
        Role::Media => return crate::cluster::media::run(config, address).await,
        _ => {}
    }
    let data_path = config.data_path.clone();
    let opening = std::time::Instant::now();
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
        replica = app.replica.as_ref().map(|r| r.store().describe()).unwrap_or_else(|| "off".into()),
        calls = %match &app.media_link {
            crate::voice::MediaLink::Off(why) => format!("off ({why})"),
            crate::voice::MediaLink::Local(sfu) => format!("port {} at {}", sfu.config().port, sfu.describe().join(", ")),
            crate::voice::MediaLink::Remote(parts) => format!("{} media part(s)", parts.len()),
        },
        encrypted = app.config.encryption_key.is_some(),
        servers = app.servers.len(),
        local_accounts = app.settings().local_accounts.as_str(),
        linked_accounts = app.settings().linked_accounts.as_str(),
        "fuwa is up"
    );

    crate::reports::server_timing("startup", opening.elapsed());

    let install_id = match &app.node {
        Some(node) => node.install_id().await.ok(),
        None => None,
    };
    let reports = crate::reports::spawn(app.clone(), &app.config, install_id, app.shutdown.clone());
    if let Link::Shard(_) = &app.link {
        crate::cluster::pictures::spawn_sweep(app.clone());
    }
    if app.node.is_some() || matches!(app.link, Link::Shard(_)) {
        crate::media::backfill::spawn(app.clone());
    }
    if app.node.is_some() {
        crate::telemetry::spawn(app.clone());
        spawn_housekeeping(app.clone());
    }
    if matches!(app.link, Link::Alone | Link::Shard(_)) {
        spawn_sso_rechecks(app.clone());
        spawn_poll_closings(app.clone());
    }
    spawn_signal_handler(app.shutdown.clone());
    crate::api::spawn_voice_sweeper(app.clone());
    crate::api::spawn_voice_guard(app.clone());
    crate::recordings::Recordings::spawn(app.clone());
    if let Some(replica) = &app.replica {
        replica.start();
    }

    let shutdown = app.shutdown.clone();
    let served = axum::serve(listener, app.router())
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
        .map_err(|err| format!("server error: {err}"));
    // Recordings finish their files, and the last commits go out, before the process does.
    app.recordings.finish_all().await;
    if let Some(replica) = &app.replica {
        replica.close().await;
    }
    crate::reports::finish(reports).await;
    served
}

/// Cancels `shutdown` on Ctrl-C or SIGTERM.
pub fn spawn_signal_handler(shutdown: CancellationToken) {
    tokio::spawn(async move {
        wait_for_signal().await;
        tracing::info!("shutting down");
        shutdown.cancel();
    });
}

/// Every few minutes: members whose single sign-on to a server ran out get
/// a MemberUpdated, so open streams stop showing them its channels.
fn spawn_sso_rechecks(app: Arc<App>) {
    tokio::spawn(async move {
        let mut every = tokio::time::interval(Duration::from_secs(5 * 60));
        // Streams opened before a restart worked out what members see then.
        let mut since = crate::id::now_ms() - 10 * 60 * 1000;
        loop {
            tokio::select! {
                _ = app.shutdown.cancelled() => return,
                _ = every.tick() => {
                    let now = crate::id::now_ms();
                    for sdb in app.servers.all() {
                        if let Err(err) = crate::api::note_lapses(&sdb, since, now).await {
                            tracing::warn!(server = %sdb.id, error = %err, "couldn't check for single sign-ons that ran out");
                        }
                    }
                    since = now;
                }
            }
        }
    });
}

/// Every minute: anonymous polls whose time ran out lose their votes (only
/// the counts stay) and show everyone the counts.
fn spawn_poll_closings(app: Arc<App>) {
    tokio::spawn(async move {
        let mut every = tokio::time::interval(Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = app.shutdown.cancelled() => return,
                _ = every.tick() => {
                    let now = crate::id::now_ms();
                    for sdb in app.servers.all() {
                        if crate::api::close_due_polls(&sdb, now).await.is_err() {
                            tracing::warn!(server = %sdb.id, "couldn't close the anonymous polls whose time ran out");
                        }
                    }
                }
            }
        }
    });
}

/// Hourly: drops expired sessions, the direct-message devices they had, and
/// uploads nothing uses.
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
                    if let Err(err) = app.sweep_devices().await {
                        tracing::warn!(error = %err, "couldn't forget the devices of ended sessions");
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
