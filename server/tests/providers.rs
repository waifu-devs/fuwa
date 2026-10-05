//! Signing in with Google, X and Twitch end to end: a real instance, one
//! fake server playing all three providers (their token and profile
//! endpoints and picture hosts, under the addresses fuwa moves them to for
//! tests), and the browser's part played by plain HTTP requests.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use fuwa_server::app::App;
use fuwa_server::config::Config;
use fuwa_server::pb;
use sha2::{Digest, Sha256};
use tokio::task::JoinHandle;
use tonic::transport::Channel;
use tonic::{Code, Request};

const APP: &str = "http://localhost:5173";
const SECRET: &str = "the app's secret";
const PASSWORD: &str = "correct horse battery";
const CLIENT_SECRET: &str = "provider-client-secret-1234";

struct Instance {
    app: Arc<App>,
    addr: SocketAddr,
    serving: JoinHandle<()>,
}

async fn start(dir: &Path) -> Instance {
    let dir = dir.to_str().unwrap().to_string();
    let config = Config::from_lookup(|key| match key {
        "FUWA_DATA_PATH" => Some(dir.clone()),
        "FUWA_TELEMETRY" => Some("off".into()),
        "FUWA_UPDATE_CHECK" => Some("off".into()),
        _ => None,
    })
    .unwrap();
    let app = App::open(config).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router();
    let shutdown = app.shutdown.clone();
    let serving = tokio::spawn(async move {
        axum::serve(listener, router).with_graceful_shutdown(async move { shutdown.cancelled().await }).await.unwrap();
    });
    Instance { app, addr, serving }
}

impl Instance {
    async fn stop(self) {
        self.app.shutdown.cancel();
        self.serving.await.unwrap();
    }

    /// Makes a session look `ms` older, as if it signed in that long ago.
    async fn age_sessions(&self, ms: i64) {
        fuwa_server::db::write(self.app.node().unwrap().db(), async |conn| {
            conn.execute("UPDATE sessions SET created_at = created_at - ?1", [ms]).await?;
            Ok(())
        })
        .await
        .unwrap();
    }
}

struct Clients {
    auth: pb::auth_service_client::AuthServiceClient<Channel>,
    account: pb::account_service_client::AccountServiceClient<Channel>,
    admin: pb::admin_service_client::AdminServiceClient<Channel>,
    node: pb::node_service_client::NodeServiceClient<Channel>,
}

async fn clients(instance: &Instance) -> Clients {
    let channel = Channel::from_shared(format!("http://{}", instance.addr)).unwrap().connect().await.unwrap();
    Clients {
        auth: pb::auth_service_client::AuthServiceClient::new(channel.clone()),
        account: pb::account_service_client::AccountServiceClient::new(channel.clone()),
        admin: pb::admin_service_client::AdminServiceClient::new(channel.clone()),
        node: pb::node_service_client::NodeServiceClient::new(channel),
    }
}

fn authed<T>(token: &str, message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

async fn sign_up(c: &mut Clients, username: &str) -> String {
    let request = pb::SignUpRequest { username: username.into(), password: PASSWORD.into(), ..Default::default() };
    c.auth.sign_up(request).await.unwrap().into_inner().token
}

fn browser() -> reqwest::Client {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).no_proxy().build().unwrap()
}

fn query(url: &str) -> HashMap<String, String> {
    reqwest::Url::parse(url).unwrap().query_pairs().into_owned().collect()
}

/// Where a provider's answer sent the browser: the done page, with the
/// answer in the fragment.
fn landed(response: &reqwest::Response) -> HashMap<String, String> {
    assert_eq!(response.status(), 303, "the instance sends the browser on");
    let location = reqwest::Url::parse(response.headers()["location"].to_str().unwrap()).unwrap();
    assert_eq!(location.path(), "/auth/provider/done", "{location}");
    assert_eq!(location.query(), None, "{location}");
    url::form_urlencoded::parse(location.fragment().unwrap_or_default().as_bytes()).into_owned().collect()
}

/// A small PNG: a header, a private chunk, and the end.
fn png() -> Vec<u8> {
    let chunk =
        |kind: &[u8; 4], data: &[u8]| [&(data.len() as u32).to_be_bytes()[..], kind, data, &[0, 0, 0, 0]].concat();
    [
        &b"\x89PNG\r\n\x1a\n"[..],
        &chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]),
        &chunk(b"fuWa", &[7; 40]),
        &chunk(b"IEND", &[]),
    ]
    .concat()
}

// ───────────────────── one fake server playing every provider ─────────────────────

#[derive(Default)]
struct Fake {
    base: String,
    /// code → (provider, PKCE challenge, who)
    codes: Mutex<HashMap<String, (String, String, String)>>,
}

/// Started once for every test in this file, on its own thread, since the
/// addresses providers move to are set for the whole process.
fn fake() -> Arc<Fake> {
    static FAKE: OnceLock<Arc<Fake>> = OnceLock::new();
    FAKE.get_or_init(|| {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let fake = Arc::new(Fake { base: base.clone(), ..Default::default() });
        let state = fake.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let router = Router::new().route("/{*path}", any(serve)).with_state(state);
                axum::serve(listener, router).await.unwrap();
            });
        });
        for id in ["google", "x", "twitch"] {
            fuwa_server::sso::providers::override_for_tests(id, &base);
        }
        fake
    })
    .clone()
}

fn who_of(headers: &HeaderMap) -> Option<String> {
    headers.get("authorization")?.to_str().ok()?.strip_prefix("Bearer tok-").map(str::to_string)
}

async fn serve(State(fake): State<Arc<Fake>>, uri: Uri, headers: HeaderMap, body: String) -> Response {
    let path = uri.path();
    let (provider, rest) = path.trim_start_matches('/').split_once('/').unwrap_or_default();
    if let Some(who) = rest.strip_prefix("pic/") {
        return ([("content-type", "image/png")], [png(), who.as_bytes().to_vec()].concat()).into_response();
    }
    if rest.ends_with("token") {
        let form: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes()).into_owned().collect();
        let Some(code) = form.get("code") else { return StatusCode::BAD_REQUEST.into_response() };
        let basic = format!("Basic {}", STANDARD.encode(format!("client-{provider}:{CLIENT_SECRET}")));
        let client_ok = match provider {
            "x" => headers.get("authorization").and_then(|v| v.to_str().ok()) == Some(basic.as_str()),
            _ => form.get("client_secret").map(String::as_str) == Some(CLIENT_SECRET),
        };
        if !client_ok {
            return (StatusCode::UNAUTHORIZED, "bad client").into_response();
        }
        let Some((issued_for, challenge, who)) = fake.codes.lock().unwrap().remove(code) else {
            return (StatusCode::BAD_REQUEST, "bad code").into_response();
        };
        if issued_for != provider {
            return (StatusCode::BAD_REQUEST, "another provider's code").into_response();
        }
        let verifier = form.get("code_verifier").cloned().unwrap_or_default();
        if provider != "twitch" && URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) != challenge {
            return (StatusCode::BAD_REQUEST, "bad verifier").into_response();
        }
        return ([("content-type", "application/json")], format!(r#"{{"access_token":"tok-{who}"}}"#)).into_response();
    }
    let Some(who) = who_of(&headers) else { return StatusCode::UNAUTHORIZED.into_response() };
    let picture = format!("{}/{provider}/pic/{who}", fake.base);
    let body = match provider {
        "google" => serde_json::json!({
            "sub": format!("g-{who}"), "name": format!("{who} G"), "picture": picture, "email": format!("{who}@gmail.com"),
        }),
        "x" => serde_json::json!({
            "data": { "id": format!("x-{who}"), "name": format!("{who} X"), "username": format!("{who}_x"), "profile_image_url": picture },
        }),
        _ => {
            if headers.get("client-id").and_then(|v| v.to_str().ok()) != Some("client-twitch") {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            serde_json::json!({ "data": [{ "id": format!("t-{who}"), "login": who, "display_name": who, "profile_image_url": "" }] })
        }
    };
    ([("content-type", "application/json")], body.to_string()).into_response()
}

impl Fake {
    /// Plays the provider's sign-in page: `who` signs in, and the browser
    /// goes back to the redirect URI with a code.
    async fn sign_in(&self, instance: &Instance, authorize_url: &str, who: &str) -> reqwest::Response {
        let asked = query(authorize_url);
        let provider = reqwest::Url::parse(authorize_url).unwrap().path().split('/').nth(1).unwrap().to_string();
        assert_eq!(asked["response_type"], "code");
        assert_eq!(asked["client_id"], format!("client-{provider}"));
        let code = fuwa_server::auth::new_token();
        let challenge = asked.get("code_challenge").cloned().unwrap_or_default();
        self.codes.lock().unwrap().insert(code.clone(), (provider, challenge, who.into()));
        let redirect = reqwest::Url::parse(&asked["redirect_uri"]).unwrap();
        browser()
            .get(format!("http://{}{}", instance.addr, redirect.path()))
            .query(&[("state", asked["state"].as_str()), ("code", code.as_str())])
            .send()
            .await
            .unwrap()
    }
}

// ───────────────────────────────── helpers ─────────────────────────────────

async fn update_settings(
    c: &mut Clients,
    token: &str,
    settings: pb::InstanceSettings,
    paths: &[&str],
) -> Result<pb::InstanceConfig, tonic::Status> {
    let request = pb::UpdateSettingsRequest {
        settings: Some(settings),
        update_mask: Some(prost_types::FieldMask { paths: paths.iter().map(|p| p.to_string()).collect() }),
        reset_mask: None,
    };
    c.admin.update_settings(authed(token, request)).await.map(|r| r.into_inner().config.unwrap())
}

fn provider(id: &str, enabled: bool) -> pb::SignInProviderSetting {
    pb::SignInProviderSetting {
        id: id.into(),
        enabled,
        client_id: format!("client-{id}"),
        client_secret: CLIENT_SECRET.into(),
        ..Default::default()
    }
}

async fn turn_on(c: &mut Clients, admin: &str, ids: &[&str]) {
    let providers = ["google", "x", "twitch"].iter().map(|id| provider(id, ids.contains(id))).collect();
    let settings = pb::InstanceSettings { sign_in_providers: providers, ..Default::default() };
    update_settings(c, admin, settings, &["sign_in_providers"]).await.unwrap();
}

async fn start_sign_in(c: &mut Clients, id: &str) -> Result<pb::StartProviderSignInResponse, Code> {
    let request = pb::StartProviderSignInRequest {
        provider: id.into(),
        return_origin: APP.into(),
        secret_hash: fuwa_server::linked::secret_hash(SECRET),
    };
    c.auth.start_provider_sign_in(request).await.map(|r| r.into_inner()).map_err(|e| e.code())
}

async fn finish(
    c: &mut Clients,
    back: &HashMap<String, String>,
    secret: &str,
    create: Option<(&str, bool)>,
) -> Result<pb::FinishProviderSignInResponse, tonic::Status> {
    let request = pb::FinishProviderSignInRequest {
        state: back["state"].clone(),
        code: back.get("code").unwrap_or_else(|| panic!("{back:?}")).clone(),
        secret: secret.into(),
        create: create.is_some(),
        username: create.map(|(name, _)| name.to_string()).unwrap_or_default(),
        use_profile: create.is_some_and(|(_, use_profile)| use_profile),
    };
    c.auth.finish_provider_sign_in(request).await.map(|r| r.into_inner())
}

/// Signs `who` in with a provider from start to finish; someone new gets
/// an account as `username` (with their name and picture if `use_profile`).
async fn sign_in(
    c: &mut Clients,
    instance: &Instance,
    id: &str,
    who: &str,
    create: Option<(&str, bool)>,
) -> Result<pb::FinishProviderSignInResponse, tonic::Status> {
    let started = start_sign_in(c, id).await.unwrap();
    let back = landed(&fake().sign_in(instance, &started.authorize_url, who).await);
    let first = finish(c, &back, SECRET, None).await?;
    match (first.new_account.is_some(), create) {
        (true, Some(_)) => finish(c, &back, SECRET, create).await,
        _ => Ok(first),
    }
}

async fn start_link(
    c: &mut Clients,
    token: &str,
    id: &str,
    password: &str,
) -> Result<pb::StartProviderLinkResponse, tonic::Status> {
    let request = pb::StartProviderLinkRequest {
        provider: id.into(),
        return_origin: APP.into(),
        secret_hash: fuwa_server::linked::secret_hash(SECRET),
        password: password.into(),
        code: String::new(),
    };
    c.account.start_provider_link(authed(token, request)).await.map(|r| r.into_inner())
}

async fn finish_link(
    c: &mut Clients,
    token: &str,
    back: &HashMap<String, String>,
) -> Result<pb::FinishProviderLinkResponse, tonic::Status> {
    let request = pb::FinishProviderLinkRequest {
        state: back["state"].clone(),
        code: back.get("code").unwrap_or_else(|| panic!("{back:?}")).clone(),
        secret: SECRET.into(),
    };
    c.account.finish_provider_link(authed(token, request)).await.map(|r| r.into_inner())
}

async fn methods(c: &mut Clients, token: &str) -> pb::ListSignInMethodsResponse {
    c.account.list_sign_in_methods(authed(token, pb::ListSignInMethodsRequest {})).await.unwrap().into_inner()
}

async fn unlink(c: &mut Clients, token: &str, id: &str, password: &str) -> Result<(), Code> {
    let request = pb::UnlinkProviderRequest { provider: id.into(), password: password.into(), code: String::new() };
    c.account.unlink_provider(authed(token, request)).await.map(|_| ()).map_err(|e| e.code())
}

// ────────────────────────────────── tests ──────────────────────────────────

#[tokio::test]
async fn people_sign_up_and_back_in_with_a_provider() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let mut c = clients(&instance).await;
    let fake = fake();
    let admin = sign_up(&mut c, "admin").await;

    // Off until an admin sets one up.
    let auth = c.node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap().auth.unwrap();
    assert!(auth.providers.is_empty());
    assert_eq!(start_sign_in(&mut c, "x").await.unwrap_err(), Code::FailedPrecondition);

    // Turning one on needs its secret, which never comes back.
    let without = pb::SignInProviderSetting { client_secret: String::new(), ..provider("x", true) };
    let refused = pb::InstanceSettings { sign_in_providers: vec![without], ..Default::default() };
    assert_eq!(
        update_settings(&mut c, &admin, refused, &["sign_in_providers"]).await.unwrap_err().code(),
        Code::FailedPrecondition
    );
    turn_on(&mut c, &admin, &["x"]).await;
    let shown = c.admin.get_settings(authed(&admin, pb::GetSettingsRequest {})).await.unwrap().into_inner();
    assert!(!format!("{shown:?}").contains(CLIENT_SECRET), "the secret never leaves the instance");
    let x = &shown.config.unwrap().settings.unwrap().sign_in_providers[1];
    assert_eq!((x.id.as_str(), x.client_secret_set, x.client_secret_hint.as_str()), ("x", true, "1234"));

    let auth = c.node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap().auth.unwrap();
    assert_eq!(auth.providers.len(), 1);
    assert_eq!((auth.providers[0].name.as_str(), auth.providers[0].host.as_str()), ("X", "127.0.0.1"));

    // Someone new is asked for a username first; the sign-in keeps until then.
    let started = start_sign_in(&mut c, "x").await.unwrap();
    let info = c
        .auth
        .get_provider_sign_in(pb::GetProviderSignInRequest { provider: "x".into(), state: started.state.clone() })
        .await
        .unwrap()
        .into_inner();
    assert_eq!((info.return_origin.as_str(), info.provider_name.as_str()), (APP, "X"));
    let back = landed(&fake.sign_in(&instance, &started.authorize_url, "bea").await);
    assert_eq!(back["provider"], "x");
    assert_eq!(finish(&mut c, &back, "someone else's", None).await.unwrap_err().code(), Code::PermissionDenied);
    let asked = finish(&mut c, &back, SECRET, None).await.unwrap();
    assert!(asked.token.is_empty());
    let new = asked.new_account.unwrap();
    assert_eq!((new.suggested_username.as_str(), new.display_name.as_str(), new.has_picture), ("bea_x", "bea X", true));
    assert_eq!(finish(&mut c, &back, SECRET, Some(("admin", true))).await.unwrap_err().code(), Code::AlreadyExists);
    let made = finish(&mut c, &back, SECRET, Some(("bea", true))).await.unwrap();
    assert!(made.created && !made.token.is_empty());
    let user = made.user.unwrap();
    assert_eq!((user.username.as_str(), user.display_name.as_str()), ("bea", "bea X"));
    assert_eq!(user.kind, pb::AccountKind::Provider as i32);
    // The picture is the account's own upload: nothing at X is linked.
    let public = instance.app.settings().public_url.clone();
    assert!(user.avatar_url.starts_with(&format!("{public}/media/")), "{}", user.avatar_url);
    assert!(!user.avatar_url.contains("pic") && !user.avatar_url.contains("url="), "{}", user.avatar_url);
    let path = reqwest::Url::parse(&user.avatar_url).unwrap().path().to_string();
    let served = browser().get(format!("http://{}{path}", instance.addr)).send().await.unwrap();
    assert_eq!(served.status(), 200);
    assert!(served.bytes().await.unwrap().starts_with(b"\x89PNG"));
    // The code works once.
    assert_eq!(finish(&mut c, &back, SECRET, None).await.unwrap_err().code(), Code::FailedPrecondition);

    // The next time, the same account.
    let again = sign_in(&mut c, &instance, "x", "bea", None).await.unwrap();
    assert!(!again.created);
    assert_eq!(again.user.unwrap().id, user.id);

    // Without their profile, the username is all that's copied.
    let plain = sign_in(&mut c, &instance, "x", "cai", Some(("cai", false))).await.unwrap().user.unwrap();
    assert_eq!((plain.display_name.as_str(), plain.avatar_url.as_str()), ("cai", ""));

    // Closed: people already linked still get in; nobody new.
    let closed = pb::InstanceSettings { provider_accounts: pb::ProviderAccounts::Closed as i32, ..Default::default() };
    update_settings(&mut c, &admin, closed, &["provider_accounts"]).await.unwrap();
    assert!(sign_in(&mut c, &instance, "x", "bea", None).await.is_ok());
    let nobody = sign_in(&mut c, &instance, "x", "dee", Some(("dee", false))).await.unwrap_err();
    assert_eq!(nobody.code(), Code::FailedPrecondition);

    // Changing the client stops sign-ins under way.
    let open = pb::InstanceSettings { provider_accounts: pb::ProviderAccounts::Open as i32, ..Default::default() };
    update_settings(&mut c, &admin, open, &["provider_accounts"]).await.unwrap();
    let under_way = start_sign_in(&mut c, "x").await.unwrap();
    let moved = pb::SignInProviderSetting { client_id: "client-x-new".into(), ..provider("x", true) };
    let settings = pb::InstanceSettings { sign_in_providers: vec![moved], ..Default::default() };
    update_settings(&mut c, &admin, settings, &["sign_in_providers"]).await.unwrap();
    let stale =
        browser().get(format!("http://{}/sso/instance/providers/x?state={}&code=x", instance.addr, under_way.state));
    assert_eq!(stale.send().await.unwrap().status(), 400);

    instance.stop().await;
}

#[tokio::test]
async fn links_need_proof_and_the_last_way_in_stays() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let mut c = clients(&instance).await;
    let fake = fake();
    let admin = sign_up(&mut c, "admin").await;
    turn_on(&mut c, &admin, &["google", "twitch"]).await;
    let ana = sign_up(&mut c, "ana").await;

    let listed = methods(&mut c, &ana).await;
    assert!(listed.can_link && listed.needs_password && !listed.needs_fresh_sign_in);
    assert_eq!(listed.methods.iter().map(|m| m.kind.as_str()).collect::<Vec<_>>(), ["password"]);
    assert_eq!(listed.available.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["google", "twitch"]);

    // A session alone can't add a way in.
    assert_eq!(start_link(&mut c, &ana, "google", "wrong").await.unwrap_err().code(), Code::PermissionDenied);
    let started = start_link(&mut c, &ana, "google", PASSWORD).await.unwrap();
    let back = landed(&fake.sign_in(&instance, &started.authorize_url, "ana").await);
    assert_eq!(back.get("link").map(String::as_str), Some("1"));
    // Only from the account that started it, and never as a sign-in.
    assert_eq!(finish(&mut c, &back, SECRET, None).await.unwrap_err().code(), Code::FailedPrecondition);
    assert_eq!(finish_link(&mut c, &admin, &back).await.unwrap_err().code(), Code::PermissionDenied);
    let linked = finish_link(&mut c, &ana, &back).await.unwrap().method.unwrap();
    assert_eq!((linked.kind.as_str(), linked.account_name.as_str()), ("google", "ana G"));

    // Every device hears about it.
    let me = c.auth.get_me(authed(&ana, pb::GetMeRequest {})).await.unwrap().into_inner();
    assert_eq!(me.recent_sign_in_methods.iter().map(|m| m.kind.as_str()).collect::<Vec<_>>(), ["google"]);

    // Google now signs in to ana's account.
    let ana_id = me.user.unwrap().id;
    let signed = sign_in(&mut c, &instance, "google", "ana", None).await.unwrap();
    assert_eq!(signed.user.unwrap().id, ana_id);

    // That Google person can't be linked to a second account.
    let started = start_link(&mut c, &admin, "google", PASSWORD).await.unwrap();
    let back = landed(&fake.sign_in(&instance, &started.authorize_url, "ana").await);
    assert_eq!(finish_link(&mut c, &admin, &back).await.unwrap_err().code(), Code::AlreadyExists);

    // Someone from Twitch with nothing else: no password to ask for, so only
    // a fresh session links or unlinks, and their one way in stays.
    let bea = sign_in(&mut c, &instance, "twitch", "bea", Some(("bea", false))).await.unwrap().token;
    let listed = methods(&mut c, &bea).await;
    assert!(listed.needs_fresh_sign_in && !listed.needs_password);
    assert_eq!(unlink(&mut c, &bea, "twitch", "").await.unwrap_err(), Code::FailedPrecondition, "the last way in");
    instance.age_sessions(11 * 60 * 1000).await;
    let stale = start_link(&mut c, &bea, "google", "").await.unwrap_err();
    assert_eq!(stale.code(), Code::FailedPrecondition, "an old session can't add a way in");
    let bea = sign_in(&mut c, &instance, "twitch", "bea", None).await.unwrap().token;
    let started = start_link(&mut c, &bea, "google", "").await.unwrap();
    let back = landed(&fake.sign_in(&instance, &started.authorize_url, "bea").await);
    finish_link(&mut c, &bea, &back).await.unwrap();
    unlink(&mut c, &bea, "twitch", "").await.unwrap();
    assert_eq!(methods(&mut c, &bea).await.methods.iter().map(|m| m.kind.as_str()).collect::<Vec<_>>(), ["google"]);

    // A provider turned off doesn't count as a way in, and can't be used.
    turn_on(&mut c, &admin, &["twitch"]).await;
    assert_eq!(start_sign_in(&mut c, "google").await.unwrap_err(), Code::FailedPrecondition);
    let listed = methods(&mut c, &ana).await;
    let google = listed.methods.iter().find(|m| m.kind == "google").unwrap();
    assert!(!google.works, "kept, but not working");
    turn_on(&mut c, &admin, &["google", "twitch"]).await;

    // Two-step sign-in still asks for its code after Google.
    let set_up = c
        .account
        .set_up_two_factor(authed(&ana, pb::SetUpTwoFactorRequest { password: PASSWORD.into() }))
        .await
        .unwrap()
        .into_inner();
    let code = fuwa_server::twofactor::code_for(&set_up.secret, fuwa_server::id::now_ms()).unwrap();
    c.account.enable_two_factor(authed(&ana, pb::EnableTwoFactorRequest { code })).await.unwrap();
    let two_step = sign_in(&mut c, &instance, "google", "ana", None).await.unwrap();
    assert!(two_step.token.is_empty() && !two_step.two_factor_ticket.is_empty());
    // Unlinking now needs the code as well as the password.
    assert_eq!(unlink(&mut c, &ana, "google", PASSWORD).await.unwrap_err(), Code::PermissionDenied);

    instance.stop().await;
}
