//! Everything the app does that isn't drawing. The window (and later the
//! tray and the game overlay) reads the [`Store`] and calls [`Core`]'s
//! methods; nothing here knows about GPUI.
//!
//! The core runs on its own Tokio runtime, since the gRPC clients need one
//! and GPUI has its own executor. It changes the store under a lock and bumps
//! a version the window watches, so the window redraws after every change.

pub mod account;
pub mod api;
pub mod arrange;
pub mod backgrounds;
pub mod calls;
pub mod config;
pub mod dms;
pub mod linked;
pub mod moderation;
pub mod notifications;
pub mod permissions;
pub mod secrets;
pub mod server_admin;
pub mod sso;
pub mod store;
mod sync;
pub mod themes;
pub mod vault;

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::{mpsc, watch};

use crate::core::api::{Api, Problem, instance_key, normalize_url};
use crate::core::config::{Paths, Prefs, SavedInstance};
use crate::core::dms::{Content, DmEngine, DmError, DmStatus};
use crate::core::store::{ChannelMessages, Focus, InstanceState, PendingMessage, Store, upsert_message};
use crate::pb;
use crate::rpc;

/// Something to tell the person about, outside the screen they're on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// A message (or direct message) arrived somewhere they aren't looking.
    /// `mention` when it pings them (direct messages always do).
    Message {
        instance: String,
        server_id: Option<String>,
        channel_id: String,
        title: String,
        body: String,
        mention: bool,
    },
    /// They were removed from a server, or it was deleted.
    Removed { server: String },
    /// A session ended on its own.
    SignedOut { instance: String },
}

/// The store, and telling the window it changed.
#[derive(Clone)]
pub struct Shared {
    store: Arc<Mutex<Store>>,
    version: Arc<watch::Sender<u64>>,
    notices: mpsc::UnboundedSender<Notice>,
}

impl Shared {
    /// Reads the store.
    pub fn read<R>(&self, f: impl FnOnce(&Store) -> R) -> R {
        f(&self.store.lock())
    }

    /// Changes the store and tells the window.
    pub fn update<R>(&self, f: impl FnOnce(&mut Store) -> R) -> R {
        let out = f(&mut self.store.lock());
        self.version.send_modify(|v| *v = v.wrapping_add(1));
        out
    }

    /// Changes one instance; nothing if it was removed meanwhile.
    pub fn instance<R>(&self, key: &str, f: impl FnOnce(&mut InstanceState) -> R) -> Option<R> {
        self.update(|s| s.instances.get_mut(key).map(f))
    }

    pub fn is_focused(&self, key: &str, channel: &str) -> bool {
        self.read(|s| s.focus.as_ref().is_some_and(|f| f.instance == key && f.channel == channel))
    }

    fn notice(&self, notice: Notice) {
        let _ = self.notices.send(notice);
    }

    /// A direct message from someone else arrived.
    pub(crate) fn notify_dm(&self, key: &str, c: &pb::Conversation, item: &vault::Item) {
        // Catching up on what came in while the app was closed stays quiet.
        if self.is_focused(key, &c.id) || dms::now_ms() - item.at > 30_000 {
            return;
        }
        let title =
            c.users.iter().find(|u| u.id == item.sender_id).map(store::user_name).unwrap_or_else(|| "Someone".into());
        self.notice(Notice::Message {
            instance: key.to_owned(),
            server_id: None,
            channel_id: c.id.clone(),
            title,
            body: item.content.chars().take(160).collect(),
            mention: true,
        });
    }
}

/// One instance the app follows.
struct Engine {
    api: Api,
    task: tokio::task::JoinHandle<()>,
    /// The servers its event stream follows. Changing it resubscribes.
    followed: watch::Sender<Vec<String>>,
    dms: Arc<Mutex<Option<Arc<DmEngine>>>>,
}

impl Engine {
    fn stop(&self) {
        self.task.abort();
        if let Some(dms) = self.dms.lock().take() {
            dms.stop();
        }
    }
}

pub struct Core {
    pub shared: Shared,
    pub paths: Paths,
    secrets: secrets::Secrets,
    /// Encrypts the vaults on disk; kept in the system keychain.
    pub(crate) vault_key: [u8; 32],
    runtime: tokio::runtime::Runtime,
    engines: Mutex<HashMap<String, Engine>>,
    prefs: Mutex<Prefs>,
    version: watch::Receiver<u64>,
    notices: Mutex<Option<mpsc::UnboundedReceiver<Notice>>>,
}

/// Messages per page, as the web app reads them.
const PAGE: i32 = 50;

impl Core {
    /// Starts the core with the instances and settings kept in `paths`.
    pub fn start(paths: Paths) -> anyhow::Result<Arc<Self>> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("fuwa-core")
            .enable_all()
            .build()?;
        let (version_tx, version) = watch::channel(0u64);
        let (notices_tx, notices) = mpsc::unbounded_channel();
        let shared = Shared {
            store: Arc::new(Mutex::new(Store::default())),
            version: Arc::new(version_tx),
            notices: notices_tx,
        };
        // Only you can open the app's folders.
        vault::private_dir(&paths.config)?;
        vault::private_dir(&paths.vaults)?;
        let secrets = secrets::Secrets::open(&paths.config);
        let vault_key = secrets.vault_key();
        let prefs = config::load_prefs(&paths);
        let core = Arc::new(Self {
            shared,
            paths,
            secrets,
            vault_key,
            runtime,
            engines: Mutex::new(HashMap::new()),
            prefs: Mutex::new(prefs),
            version,
            notices: Mutex::new(Some(notices)),
        });
        for saved in config::load_instances(&core.paths, &core.secrets) {
            core.add_instance(&saved.url, saved.token);
        }
        Ok(core)
    }

    /// Runs a future on the core's runtime; the answer comes back on any executor.
    pub fn spawn<T: Send + 'static>(
        self: &Arc<Self>,
        future: impl Future<Output = T> + Send + 'static,
    ) -> futures::channel::oneshot::Receiver<T> {
        let (tx, rx) = futures::channel::oneshot::channel();
        self.runtime.spawn(async move {
            let _ = tx.send(future.await);
        });
        rx
    }

    /// The core's runtime, for work the window hands it (fetching pictures).
    pub fn handle(&self) -> tokio::runtime::Handle {
        self.runtime.handle().clone()
    }

    /// Changes whenever the store does.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.version.clone()
    }

    /// Notices for the window; only the first caller gets them.
    pub fn take_notices(&self) -> Option<mpsc::UnboundedReceiver<Notice>> {
        self.notices.lock().take()
    }

    // ───────────────────────── Settings ─────────────────────────

    pub fn prefs(&self) -> Prefs {
        self.prefs.lock().clone()
    }

    pub fn set_prefs(&self, f: impl FnOnce(&mut Prefs)) {
        let prefs = {
            let mut prefs = self.prefs.lock();
            f(&mut prefs);
            prefs.clone()
        };
        config::store_prefs(&self.paths, &prefs);
        self.shared.update(|_| {});
    }

    // ───────────────────────── Instances ─────────────────────────

    fn persist(&self) {
        let list: Vec<SavedInstance> = {
            let engines = self.engines.lock();
            self.shared.read(|s| {
                s.order
                    .iter()
                    .filter_map(|key| {
                        engines.get(key).map(|e| SavedInstance { url: e.api.url.clone(), token: e.api.token() })
                    })
                    .collect()
            })
        };
        config::store_instances(&self.paths, &self.secrets, &list);
    }

    /// The addresses of the instances you added.
    /// The addresses of the instances you added, and the public address each
    /// says it has: the only places pictures load from.
    pub fn instance_urls(&self) -> Vec<String> {
        let mut urls: Vec<String> = self.engines.lock().values().map(|e| e.api.url.clone()).collect();
        self.shared.read(|s| {
            for key in &s.order {
                if let Some(url) = s.instance(key).and_then(|i| i.node.as_ref()).map(|n| n.public_url.clone())
                    && !url.is_empty()
                {
                    urls.push(url);
                }
            }
        });
        urls
    }

    pub fn api(&self, key: &str) -> Option<Api> {
        self.engines.lock().get(key).map(|e| e.api.clone())
    }

    /// Adds an instance (or changes its token) and (re)starts following it. Returns its key.
    pub fn add_instance(self: &Arc<Self>, url: &str, token: Option<String>) -> String {
        let key = instance_key(url);
        if let Some(old) = self.engines.lock().remove(&key) {
            old.stop();
        }
        let Ok(api) = Api::new(url, token) else { return key };
        self.shared.update(|s| {
            s.instances.insert(key.clone(), InstanceState::new(&key, url));
            if !s.order.contains(&key) {
                s.order.push(key.clone());
            }
        });
        let (followed, followed_rx) = watch::channel(Vec::new());
        let dms = Arc::new(Mutex::new(None));
        let task = self.runtime.spawn(sync::run(self.clone(), key.clone(), api.clone(), followed_rx, dms.clone()));
        self.engines.lock().insert(key.clone(), Engine { api, task, followed, dms });
        self.persist();
        key
    }

    /// Forgets an instance: stops following it and drops its token from this computer.
    pub fn remove_instance(&self, key: &str) {
        if let Some(engine) = self.engines.lock().remove(key) {
            engine.stop();
        }
        self.shared.update(|s| {
            s.instances.remove(key);
            s.order.retain(|k| k != key);
            if s.focus.as_ref().is_some_and(|f| f.instance == key) {
                s.focus = None;
            }
        });
        self.persist();
    }

    /// Starts or stops following a server after joining, creating or leaving it.
    fn follow(&self, key: &str, server_id: &str, on: bool) {
        if let Some(engine) = self.engines.lock().get(key) {
            engine.followed.send_if_modified(|ids| {
                let has = ids.iter().any(|id| id == server_id);
                match (on, has) {
                    (true, false) => ids.push(server_id.to_owned()),
                    (false, true) => ids.retain(|id| id != server_id),
                    _ => return false,
                }
                true
            });
        }
    }

    /// What an instance at an address says about itself, before signing in.
    pub async fn probe(&self, input: &str) -> Result<(String, pb::Node), Problem> {
        let url = normalize_url(input).map_err(|m| Problem::new(tonic::Code::InvalidArgument, m))?;
        let api = Api::new(&url, None).map_err(|e| Problem::new(tonic::Code::InvalidArgument, e.to_string()))?;
        let node = rpc!(api.node(), get_node(pb::GetNodeRequest {})).await?.node.unwrap_or_default();
        Ok((url, node))
    }

    // ───────────────────────── Signing in ─────────────────────────

    pub async fn sign_in(self: &Arc<Self>, url: &str, username: &str, password: &str) -> Result<SignIn, Problem> {
        let api = Api::new(url, None).map_err(|e| Problem::new(tonic::Code::InvalidArgument, e.to_string()))?;
        let res = rpc!(api.auth(), sign_in(pb::SignInRequest { username: username.into(), password: password.into() }))
            .await?;
        if !res.two_factor_ticket.is_empty() {
            return Ok(SignIn::TwoFactor { ticket: res.two_factor_ticket });
        }
        Ok(SignIn::Done { key: self.add_instance(url, Some(res.token)) })
    }

    pub async fn verify_two_factor(self: &Arc<Self>, url: &str, ticket: &str, code: &str) -> Result<String, Problem> {
        let api = Api::new(url, None).map_err(|e| Problem::new(tonic::Code::InvalidArgument, e.to_string()))?;
        let res = rpc!(
            api.auth(),
            verify_two_factor(pb::VerifyTwoFactorRequest { ticket: ticket.into(), code: code.into() })
        )
        .await?;
        Ok(self.add_instance(url, Some(res.token)))
    }

    pub async fn sign_up(
        self: &Arc<Self>,
        url: &str,
        username: &str,
        password: &str,
        display_name: &str,
    ) -> Result<String, Problem> {
        let api = Api::new(url, None).map_err(|e| Problem::new(tonic::Code::InvalidArgument, e.to_string()))?;
        let res = rpc!(
            api.auth(),
            sign_up(pb::SignUpRequest {
                username: username.into(),
                password: password.into(),
                display_name: display_name.into(),
            })
        )
        .await?;
        Ok(self.add_instance(url, Some(res.token)))
    }

    /// Signs in with waifu.dev (or the instance's issuer) in the browser.
    /// `opened` gets the sign-in page's address once it's ready to open.
    pub async fn linked_sign_in(
        self: &Arc<Self>,
        url: &str,
        open_page: impl FnOnce(&str) + Send,
    ) -> Result<(String, bool), Problem> {
        let api = Api::new(url, None).map_err(|e| Problem::new(tonic::Code::InvalidArgument, e.to_string()))?;
        let callback =
            linked::Callback::listen().await.map_err(|e| Problem::new(tonic::Code::Internal, e.to_string()))?;
        let mut secret_bytes = [0u8; 32];
        getrandom::fill(&mut secret_bytes).map_err(|e| Problem::new(tonic::Code::Internal, e.to_string()))?;
        let secret = vault::sha256_hex(&secret_bytes);
        let started = rpc!(
            api.auth(),
            start_linked_sign_in(pb::StartLinkedSignInRequest {
                return_origin: callback.origin.clone(),
                secret_hash: vault::sha256_hex(secret.as_bytes()),
            })
        )
        .await?;
        // The page comes from the instance: open it only if it's a real web page.
        if !linked::safe_sign_in_page(&started.authorize_url) {
            return Err(Problem::new(
                tonic::Code::PermissionDenied,
                "The instance gave a sign-in page that isn't https, so fuwa won't open it.",
            ));
        }
        open_page(&started.authorize_url);
        match callback.wait(&started.state).await {
            linked::Returned::Code { code, state } => {
                let res =
                    rpc!(api.auth(), finish_linked_sign_in(pb::FinishLinkedSignInRequest { state, code, secret }))
                        .await?;
                Ok((self.add_instance(url, Some(res.token)), res.created))
            }
            linked::Returned::Failed(message) => Err(Problem::new(tonic::Code::Cancelled, message)),
        }
    }

    /// Signs out of an instance: ends the session there, forgets it here, and
    /// wipes what its encrypted messages kept on this computer.
    pub async fn sign_out(self: &Arc<Self>, key: &str) {
        let Some(api) = self.api(key) else { return };
        let me = self.shared.read(|s| s.instance(key).and_then(|i| i.me.clone()));
        let _ = rpc!(api.auth(), sign_out(pb::SignOutRequest {})).await;
        let url = api.url.clone();
        self.add_instance(&url, None);
        if let Some(me) = me {
            let _ = vault::wipe(&vault::Vault::dir_for(&self.paths.vaults, key, &me.id));
        }
    }

    /// The session ended on its own (revoked, expired): forget the token and the device.
    fn signed_out(self: &Arc<Self>, key: &str) {
        let me = self.shared.read(|s| s.instance(key).and_then(|i| i.me.clone()));
        if let Some(engine) = self.engines.lock().get(key) {
            engine.api.set_token(None);
            if let Some(dms) = engine.dms.lock().take() {
                dms.stop();
            }
        }
        self.persist();
        if let Some(me) = me {
            let _ = vault::wipe(&vault::Vault::dir_for(&self.paths.vaults, key, &me.id));
        }
        self.shared.instance(key, |i| {
            i.connection = store::Connection::SignedOut;
            i.problem = Some("Your session ended. Sign in again.".into());
            i.dms = dms::DmState::default();
        });
        self.shared.notice(Notice::SignedOut { instance: key.to_owned() });
    }

    // ───────────────────────── Looking at things ─────────────────────────

    /// What's on screen, so it doesn't collect unread counts.
    pub fn set_focus(self: &Arc<Self>, focus: Option<Focus>) {
        let dm = self.shared.update(|s| {
            s.focus = focus.clone();
            let f = focus.as_ref()?;
            let i = s.instances.get_mut(&f.instance)?;
            i.unread.remove(&f.channel);
            i.dms.conversations.iter().any(|c| c.id == f.channel).then(|| (f.instance.clone(), f.channel.clone()))
        });
        if let Some((key, id)) = dm
            && let Some(engine) = self.dm_engine(&key)
        {
            self.runtime.spawn(async move { engine.mark_read(&id).await });
        }
    }

    /// Loads the latest messages of a channel the first time it's opened, or older ones.
    pub async fn load_messages(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        older: bool,
    ) -> Result<(), Problem> {
        let Some(api) = self.api(key) else { return Ok(()) };
        let current = self.shared.read(|s| s.instance(key).and_then(|i| i.messages.get(channel_id).cloned()));
        if current.as_ref().is_some_and(|c| c.loading || !older)
            || (older && !current.as_ref().is_some_and(|c| c.has_more))
        {
            return Ok(());
        }
        let before_id = if older { current.as_ref().and_then(|c| c.items.first()).map(|m| m.id.clone()) } else { None };
        self.shared.instance(key, |i| i.messages.entry(channel_id.to_owned()).or_default().loading = true);
        let res = rpc!(
            api.messages(),
            list_messages(pb::ListMessagesRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                limit: PAGE,
                before_id: before_id.unwrap_or_default(),
                after_id: String::new(),
            })
        )
        .await;
        self.shared.instance(key, |i| {
            let res = match res {
                Ok(res) => res,
                Err(_) => {
                    match &current {
                        Some(c) => {
                            i.messages.insert(channel_id.to_owned(), ChannelMessages { loading: false, ..c.clone() })
                        }
                        None => i.messages.remove(channel_id),
                    };
                    return;
                }
            };
            for user in res.authors {
                i.users.insert(user.id.clone(), user);
            }
            let entry = i.messages.entry(channel_id.to_owned()).or_default();
            for m in res.messages {
                upsert_message(&mut entry.items, m);
            }
            entry.has_more = if older || current.is_none() { res.has_more } else { entry.has_more };
            entry.loading = false;
        });
        Ok(())
    }

    /// Sends a message. It shows up right away, dimmed until the instance confirms it.
    pub async fn send_message(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        content: &str,
    ) -> Result<(), Problem> {
        let Some(api) = self.api(key) else { return Ok(()) };
        static NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let nonce = NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let pending = PendingMessage { nonce, content: content.into(), created_at_ms: dms::now_ms(), failed: None };
        self.shared.instance(key, |i| i.pending.entry(channel_id.to_owned()).or_default().push(pending));
        let res = rpc!(
            api.messages(),
            send_message(pb::SendMessageRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                content: content.into(),
                ..Default::default()
            })
        )
        .await;
        self.shared.instance(key, |i| match &res {
            Ok(sent) => {
                if let Some(list) = i.pending.get_mut(channel_id) {
                    list.retain(|p| p.nonce != nonce);
                }
                if let (Some(loaded), Some(message)) = (i.messages.get_mut(channel_id), sent.message.clone()) {
                    upsert_message(&mut loaded.items, message);
                }
            }
            Err(err) => {
                if let Some(p) = i.pending.get_mut(channel_id).and_then(|l| l.iter_mut().find(|p| p.nonce == nonce)) {
                    p.failed = Some(err.message.clone());
                }
            }
        });
        res.map(|_| ())
    }

    pub fn dismiss_pending(&self, key: &str, channel_id: &str, nonce: u64) {
        self.shared.instance(key, |i| {
            if let Some(list) = i.pending.get_mut(channel_id) {
                list.retain(|p| p.nonce != nonce);
            }
        });
    }

    pub async fn edit_message(
        &self,
        key: &str,
        server_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<(), Problem> {
        let Some(api) = self.api(key) else { return Ok(()) };
        let res = rpc!(
            api.messages(),
            update_message(pb::UpdateMessageRequest {
                server_id: server_id.into(),
                message_id: message_id.into(),
                content: content.into(),
            })
        )
        .await?;
        if let Some(message) = res.message {
            self.shared.instance(key, |i| {
                if let Some(loaded) = i.messages.get_mut(&message.channel_id) {
                    upsert_message(&mut loaded.items, message);
                }
            });
        }
        Ok(())
    }

    pub async fn delete_message(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        message_id: &str,
    ) -> Result<(), Problem> {
        let Some(api) = self.api(key) else { return Ok(()) };
        rpc!(
            api.messages(),
            delete_message(pb::DeleteMessageRequest { server_id: server_id.into(), message_id: message_id.into() })
        )
        .await?;
        self.shared.instance(key, |i| {
            if let Some(loaded) = i.messages.get_mut(channel_id) {
                loaded.items.retain(|m| m.id != message_id);
            }
        });
        Ok(())
    }

    // ───────────────────────── Servers ─────────────────────────

    pub async fn create_server(&self, key: &str, name: &str) -> Result<pb::Server, Problem> {
        let api = self.api(key).ok_or_else(|| Problem::new(tonic::Code::NotFound, "That instance isn't here."))?;
        let server =
            rpc!(api.servers(), create_server(pb::CreateServerRequest { name: name.into(), ..Default::default() }))
                .await?
                .server
                .unwrap_or_default();
        self.shared.instance(key, |i| store::add_server(i, server.clone()));
        self.follow(key, &server.id, true);
        Ok(server)
    }

    /// Joins a server by an invite code or link (`https://…/invite/CODE`, or the code alone).
    pub async fn join_by_invite(&self, key: &str, invite: &str) -> Result<pb::Server, Problem> {
        let (code, server) = self.open_invite(key, invite).await?;
        self.join_with_invite(key, &server.id, &code).await
    }

    /// The server an invite (a code or a link) leads to, and its code.
    pub async fn open_invite(&self, key: &str, invite: &str) -> Result<(String, pb::Server), Problem> {
        let api = self.api(key).ok_or_else(|| Problem::new(tonic::Code::NotFound, "That instance isn't here."))?;
        let code = invite.trim().trim_end_matches('/').rsplit('/').next().unwrap_or_default().to_owned();
        if code.is_empty() {
            return Err(Problem::new(tonic::Code::InvalidArgument, "Paste an invite link or code."));
        }
        let found = rpc!(api.invites(), get_invite(pb::GetInviteRequest { code: code.clone() })).await?;
        Ok((code, found.server.unwrap_or_default()))
    }

    /// Joins a server with an invite's code (after its single sign-on, if it has one).
    pub async fn join_with_invite(&self, key: &str, server_id: &str, code: &str) -> Result<pb::Server, Problem> {
        let api = self.api(key).ok_or_else(|| Problem::new(tonic::Code::NotFound, "That instance isn't here."))?;
        let joined = rpc!(
            api.servers(),
            join_server(pb::JoinServerRequest { server_id: server_id.into(), invite_code: code.into() })
        )
        .await?;
        let server = joined.server.unwrap_or_default();
        self.shared.instance(key, |i| store::add_server(i, server.clone()));
        self.follow(key, &server.id, true);
        Ok(server)
    }

    /// Makes an invite to a server that never runs out, and gives its link.
    pub async fn create_invite(&self, key: &str, server_id: &str) -> Result<String, Problem> {
        let api = self.api(key).ok_or_else(|| Problem::new(tonic::Code::NotFound, "That instance isn't here."))?;
        let res = rpc!(
            api.invites(),
            create_invite(pb::CreateInviteRequest { server_id: server_id.into(), ..Default::default() })
        )
        .await?;
        let code = res.invite.map(|i| i.code).unwrap_or_default();
        Ok(format!("{}/invite/{code}", api.url))
    }

    pub async fn leave_server(&self, key: &str, server_id: &str) -> Result<(), Problem> {
        let Some(api) = self.api(key) else { return Ok(()) };
        rpc!(api.servers(), leave_server(pb::LeaveServerRequest { server_id: server_id.into() })).await?;
        self.shared.instance(key, |i| store::remove_server(i, server_id));
        self.follow(key, server_id, false);
        Ok(())
    }

    // ───────────────────────── Direct messages ─────────────────────────

    pub fn dm_engine(&self, key: &str) -> Option<Arc<DmEngine>> {
        self.engines.lock().get(key).and_then(|e| e.dms.lock().clone())
    }

    /// Opens (or finds) the conversation with someone. Returns its id.
    pub async fn open_conversation(&self, key: &str, user_id: &str) -> Result<String, DmError> {
        let api = self.api(key).ok_or_else(|| DmError("That instance isn't here.".into()))?;
        let engine = self.dm_engine(key).ok_or_else(|| DmError("Encrypted messages aren't ready yet.".into()))?;
        let res = rpc!(api.dms(), open_conversation(pb::OpenConversationRequest { user_id: user_id.into() })).await?;
        let conversation = res.conversation.ok_or_else(|| DmError("The instance didn't open it.".into()))?;
        let id = conversation.id.clone();
        engine.add(conversation);
        Ok(id)
    }

    /// Gets a conversation ready to write in (adds devices, says why it can't if it can't).
    pub async fn prepare_conversation(&self, key: &str, id: &str) -> Result<(), DmError> {
        let engine = self.dm_engine(key).ok_or_else(|| DmError("Encrypted messages aren't ready yet.".into()))?;
        engine.prepare(id).await
    }

    pub async fn send_dm(&self, key: &str, id: &str, content: Content) -> Result<(), DmError> {
        let engine = self.dm_engine(key).ok_or_else(|| DmError("Encrypted messages aren't ready yet.".into()))?;
        // A new message shows dimmed until it's sent; an edit changes the one already there.
        let text = match &content {
            Content::Text { text, .. } => Some(text.clone()),
            Content::Edit { .. } => None,
        };
        if let Some(text) = &text {
            self.shared.instance(key, |i| i.dms.sending.entry(id.to_owned()).or_default().push(text.clone()));
        }
        let result = engine.send(id, content).await;
        if let Some(text) = text {
            self.shared.instance(key, |i| {
                if let Some(list) = i.dms.sending.get_mut(id)
                    && let Some(at) = list.iter().position(|t| *t == text)
                {
                    list.remove(at);
                }
            });
        }
        result
    }

    pub async fn delete_dm(&self, key: &str, id: &str, seq: i64) -> Result<(), DmError> {
        let engine = self.dm_engine(key).ok_or_else(|| DmError("Encrypted messages aren't ready yet.".into()))?;
        engine.remove(id, seq).await
    }

    pub async fn verify_conversation(&self, key: &str, id: &str, safety: &str) {
        if let Some(engine) = self.dm_engine(key) {
            engine.verify(id, safety).await;
        }
    }
}

/// How a sign-in went.
#[derive(Debug, Clone)]
pub enum SignIn {
    Done {
        key: String,
    },
    /// Two-step sign-in is on: a code finishes it.
    TwoFactor {
        ticket: String,
    },
}

impl DmStatus {
    pub fn is_ready(self) -> bool {
        self == DmStatus::Ready
    }
}
