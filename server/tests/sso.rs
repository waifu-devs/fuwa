//! Single sign-on end to end: a real instance, a fake OpenID Connect
//! provider (signing ID tokens with tests/fixtures/sso-idp.key) and a fake
//! SAML one (signing responses with the same key), with the browser's part
//! played by plain HTTP requests.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Form, State};
use axum::routing::{get, post};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use fuwa_server::app::App;
use fuwa_server::config::Config;
use fuwa_server::pb;
use sha2::{Digest, Sha256};
use tokio::task::JoinHandle;
use tonic::transport::Channel;
use tonic::{Code, Request};

const KEY: &str = include_str!("fixtures/sso-idp.key");
const CERT: &str = include_str!("fixtures/sso-idp.crt");
const JWKS: &str = include_str!("fixtures/sso-idp.jwks.json");
const APP: &str = "http://localhost:5173";
const SECRET: &str = "the app's secret";

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
}

struct Clients {
    auth: pb::auth_service_client::AuthServiceClient<Channel>,
    admin: pb::admin_service_client::AdminServiceClient<Channel>,
    node: pb::node_service_client::NodeServiceClient<Channel>,
    servers: pb::server_service_client::ServerServiceClient<Channel>,
    channels: pb::channel_service_client::ChannelServiceClient<Channel>,
    sso: pb::sso_service_client::SsoServiceClient<Channel>,
    roles: pb::role_service_client::RoleServiceClient<Channel>,
}

async fn clients(instance: &Instance) -> Clients {
    let channel = Channel::from_shared(format!("http://{}", instance.addr)).unwrap().connect().await.unwrap();
    Clients {
        auth: pb::auth_service_client::AuthServiceClient::new(channel.clone()),
        admin: pb::admin_service_client::AdminServiceClient::new(channel.clone()),
        node: pb::node_service_client::NodeServiceClient::new(channel.clone()),
        servers: pb::server_service_client::ServerServiceClient::new(channel.clone()),
        channels: pb::channel_service_client::ChannelServiceClient::new(channel.clone()),
        sso: pb::sso_service_client::SsoServiceClient::new(channel.clone()),
        roles: pb::role_service_client::RoleServiceClient::new(channel),
    }
}

fn authed<T>(token: &str, message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

async fn sign_up(c: &mut Clients, username: &str) -> String {
    let request =
        pb::SignUpRequest { username: username.into(), password: "correct horse battery".into(), ..Default::default() };
    c.auth.sign_up(request).await.unwrap().into_inner().token
}

fn browser() -> reqwest::Client {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).no_proxy().build().unwrap()
}

fn query(url: &str) -> HashMap<String, String> {
    reqwest::Url::parse(url).unwrap().query_pairs().into_owned().collect()
}

/// Where a provider's answer sent the browser: the done page, with the
/// answer in the fragment (never in the query, which servers log).
fn landed(response: &reqwest::Response) -> HashMap<String, String> {
    assert_eq!(response.status(), 303, "the instance sends the browser on");
    let location = reqwest::Url::parse(response.headers()["location"].to_str().unwrap()).unwrap();
    assert_eq!(location.path(), "/auth/sso/done", "{location}");
    assert_eq!(location.query(), None, "{location}");
    url::form_urlencoded::parse(location.fragment().unwrap_or_default().as_bytes()).into_owned().collect()
}

fn key_pair() -> ring::signature::RsaKeyPair {
    let body: String = KEY.lines().filter(|l| !l.starts_with("-----")).collect();
    ring::signature::RsaKeyPair::from_pkcs8(&STANDARD.decode(body).unwrap()).unwrap()
}

fn rsa_sign(message: &[u8]) -> Vec<u8> {
    let pair = key_pair();
    let mut signature = vec![0; pair.public().modulus_len()];
    pair.sign(&ring::signature::RSA_PKCS1_SHA256, &ring::rand::SystemRandom::new(), message, &mut signature).unwrap();
    signature
}

// ───────────────────────── a fake OpenID Connect provider ─────────────────────────

#[derive(Default)]
struct Idp {
    url: String,
    /// code → (nonce, PKCE challenge, who)
    codes: Mutex<HashMap<String, (String, String, String)>>,
    /// Sign ID tokens with a key the provider doesn't publish.
    forge: Mutex<bool>,
}

#[derive(serde::Deserialize)]
struct TokenForm {
    code: String,
    code_verifier: String,
}

async fn fake_oidc() -> Arc<Idp> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let idp = Arc::new(Idp { url: url.clone(), ..Default::default() });
    let router = Router::new()
        .route(
            "/.well-known/openid-configuration",
            get(|State(idp): State<Arc<Idp>>| async move {
                let body = serde_json::json!({
                    "issuer": idp.url,
                    "authorization_endpoint": format!("{}/authorize", idp.url),
                    "token_endpoint": format!("{}/token", idp.url),
                    "jwks_uri": format!("{}/jwks", idp.url),
                });
                ([("content-type", "application/json")], body.to_string())
            }),
        )
        .route("/jwks", get(|| async { ([("content-type", "application/json")], JWKS) }))
        .route(
            "/token",
            post(
                |State(idp): State<Arc<Idp>>, headers: axum::http::HeaderMap, Form(form): Form<TokenForm>| async move {
                    // Basic client authentication, as the provider doesn't say otherwise.
                    let basic = format!("Basic {}", STANDARD.encode("fuwa-client:s3cret"));
                    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(basic.as_str()) {
                        return (axum::http::StatusCode::UNAUTHORIZED, "bad client").boxed();
                    }
                    let Some((nonce, challenge, who)) = idp.codes.lock().unwrap().remove(&form.code) else {
                        return (axum::http::StatusCode::BAD_REQUEST, "bad code").boxed();
                    };
                    if URL_SAFE_NO_PAD.encode(Sha256::digest(form.code_verifier.as_bytes())) != challenge {
                        return (axum::http::StatusCode::BAD_REQUEST, "bad verifier").boxed();
                    }
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
                    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","kid":"test-key"}"#);
                    let claims = URL_SAFE_NO_PAD.encode(
                    serde_json::json!({
                        "iss": idp.url, "aud": "fuwa-client", "sub": format!("sub-{who}"), "exp": now + 300, "iat": now,
                        "nonce": nonce, "email": format!("{who}@acme.com"), "email_verified": true,
                        "name": format!("{who} from Acme"), "preferred_username": who,
                    })
                    .to_string(),
                );
                    let message = format!("{header}.{claims}");
                    let mut signature = rsa_sign(message.as_bytes());
                    if *idp.forge.lock().unwrap() {
                        signature[10] ^= 1;
                    }
                    let body =
                        serde_json::json!({ "id_token": format!("{message}.{}", URL_SAFE_NO_PAD.encode(signature)) });
                    (axum::http::StatusCode::OK, body.to_string()).boxed()
                },
            ),
        )
        .with_state(idp.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    idp
}

trait Boxed {
    fn boxed(self) -> axum::response::Response;
}

impl<T: axum::response::IntoResponse> Boxed for T {
    fn boxed(self) -> axum::response::Response {
        self.into_response()
    }
}

impl Idp {
    /// Plays the provider's sign-in page: someone signs in, and the browser
    /// goes back to the redirect URI with a code.
    async fn sign_in(&self, instance: &Instance, authorize_url: &str, who: &str) -> reqwest::Response {
        let asked = query(authorize_url);
        assert_eq!(asked["code_challenge_method"], "S256");
        assert!(asked["scope"].split(' ').any(|s| s == "openid"));
        let code = fuwa_server::auth::new_token();
        self.codes
            .lock()
            .unwrap()
            .insert(code.clone(), (asked["nonce"].clone(), asked["code_challenge"].clone(), who.into()));
        let redirect = reqwest::Url::parse(&asked["redirect_uri"]).unwrap();
        browser()
            .get(format!("http://{}{}", instance.addr, redirect.path()))
            .query(&[("state", asked["state"].as_str()), ("code", code.as_str())])
            .send()
            .await
            .unwrap()
    }
}

fn oidc_provider(issuer: &str) -> pb::IdentityProvider {
    pb::IdentityProvider {
        protocol: pb::SsoProtocol::Oidc as i32,
        name: "Acme".into(),
        oidc: Some(pb::OidcProvider {
            issuer: issuer.into(),
            client_id: "fuwa-client".into(),
            client_secret: "s3cret".into(),
            ..Default::default()
        }),
        email_domains: vec!["acme.com".into()],
        ..Default::default()
    }
}

fn saml_provider() -> pb::IdentityProvider {
    pb::IdentityProvider {
        protocol: pb::SsoProtocol::Saml as i32,
        name: "Acme SAML".into(),
        saml: Some(pb::SamlProvider {
            entity_id: "https://idp.acme.com".into(),
            sso_url: "https://idp.acme.com/sso".into(),
            certificates: CERT.into(),
        }),
        ..Default::default()
    }
}

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

async fn start_instance_sso(
    c: &mut Clients,
    token: Option<&str>,
    test: bool,
) -> Result<pb::StartSsoSignInResponse, Code> {
    let message = pb::StartSsoSignInRequest {
        return_origin: APP.into(),
        secret_hash: fuwa_server::linked::secret_hash(SECRET),
        test,
    };
    let request = match token {
        Some(token) => authed(token, message),
        None => Request::new(message),
    };
    c.auth.start_sso_sign_in(request).await.map(|r| r.into_inner()).map_err(|e| e.code())
}

async fn finish_instance_sso(
    c: &mut Clients,
    landed: &HashMap<String, String>,
    secret: &str,
) -> Result<pb::FinishSsoSignInResponse, tonic::Status> {
    let request = pb::FinishSsoSignInRequest {
        state: landed["state"].clone(),
        code: landed.get("code").unwrap_or_else(|| panic!("{landed:?}")).clone(),
        secret: secret.into(),
    };
    c.auth.finish_sso_sign_in(request).await.map(|r| r.into_inner())
}

#[tokio::test]
async fn instances_sign_people_in_through_openid_connect() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let mut c = clients(&instance).await;
    let idp = fake_oidc().await;
    let admin = sign_up(&mut c, "admin").await;

    // Off until an admin sets it up.
    let auth = c.node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap().auth.unwrap();
    assert!(!auth.sso_sign_in && auth.sso_name.is_empty());
    assert_eq!(start_instance_sso(&mut c, None, false).await.unwrap_err(), Code::FailedPrecondition);

    // Turning it on needs a working provider.
    let on = pb::InstanceSettings { sso_accounts: pb::SsoAccounts::Open as i32, ..Default::default() };
    let refused = update_settings(&mut c, &admin, on.clone(), &["sso_accounts"]).await.unwrap_err();
    assert_eq!(refused.code(), Code::FailedPrecondition);
    let config = update_settings(
        &mut c,
        &admin,
        pb::InstanceSettings { sso_provider: Some(oidc_provider(&idp.url)), ..Default::default() },
        &["sso_provider"],
    )
    .await
    .unwrap();
    let shown = config.settings.unwrap().sso_provider.unwrap().oidc.unwrap();
    assert!(shown.client_secret.is_empty() && shown.client_secret_set, "the secret never comes back");
    assert!(config.sso_service_provider.unwrap().oidc_redirect_uri.ends_with("/sso/instance/oidc"));

    // An admin tests it while it's still off: nobody is signed in.
    assert_eq!(start_instance_sso(&mut c, None, true).await.unwrap_err(), Code::Unauthenticated);
    let test = start_instance_sso(&mut c, Some(&admin), true).await.unwrap();
    let back = landed(&idp.sign_in(&instance, &test.authorize_url, "ana").await);
    let tested = finish_instance_sso(&mut c, &back, SECRET).await.unwrap();
    assert!(tested.token.is_empty() && tested.user.is_none());
    assert_eq!(tested.identity.unwrap().email, "ana@acme.com");

    update_settings(&mut c, &admin, on, &["sso_accounts"]).await.unwrap();
    let auth = c.node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap().auth.unwrap();
    assert!(auth.sso_sign_in && auth.sso_sign_up);
    assert_eq!(auth.sso_name, "Acme");
    assert_eq!(auth.sso_host, "127.0.0.1", "apps name the site that sees your address");

    // Someone new gets an account; the next time, the same one.
    let started = start_instance_sso(&mut c, None, false).await.unwrap();
    let info =
        c.auth.get_sso_sign_in(pb::GetSsoSignInRequest { state: started.state.clone() }).await.unwrap().into_inner();
    assert_eq!((info.return_origin.as_str(), info.provider_name.as_str()), (APP, "Acme"));
    let back = landed(&idp.sign_in(&instance, &started.authorize_url, "ana").await);
    // Only the app holding the secret can finish.
    assert_eq!(finish_instance_sso(&mut c, &back, "someone else's").await.unwrap_err().code(), Code::PermissionDenied);
    let first = finish_instance_sso(&mut c, &back, SECRET).await.unwrap();
    assert!(first.created && !first.admin);
    // Nothing was kept for the sign-in until the provider answered, and its
    // state works once.
    let again = landed(&idp.sign_in(&instance, &started.authorize_url, "ana").await);
    assert!(again["error"].contains("already used"), "{again:?}");
    let user = first.user.unwrap();
    assert_eq!((user.username.as_str(), user.kind), ("ana", pb::AccountKind::Sso as i32));
    assert_eq!(user.display_name, "ana from Acme");
    assert_eq!(finish_instance_sso(&mut c, &back, SECRET).await.unwrap_err().code(), Code::FailedPrecondition, "once");
    let me = c.auth.get_me(authed(&first.token, pb::GetMeRequest {})).await.unwrap().into_inner();
    assert_eq!(me.user.unwrap().id, user.id);

    let again = start_instance_sso(&mut c, None, false).await.unwrap();
    let back = landed(&idp.sign_in(&instance, &again.authorize_url, "ana").await);
    let second = finish_instance_sso(&mut c, &back, SECRET).await.unwrap();
    assert!(!second.created);
    assert_eq!(second.user.unwrap().id, user.id);

    // A code the provider gave out can't be swapped into another sign-in.
    let one = start_instance_sso(&mut c, None, false).await.unwrap();
    let two = start_instance_sso(&mut c, None, false).await.unwrap();
    let back_one = landed(&idp.sign_in(&instance, &one.authorize_url, "ana").await);
    let mut mixed = back_one.clone();
    mixed.insert("state".into(), two.state.clone());
    assert!(finish_instance_sso(&mut c, &mixed, SECRET).await.is_err());

    // A forged ID token is refused, and the browser hears why.
    *idp.forge.lock().unwrap() = true;
    let forged = start_instance_sso(&mut c, None, false).await.unwrap();
    let back = landed(&idp.sign_in(&instance, &forged.authorize_url, "mallory").await);
    assert!(back["error"].contains("isn't signed by the provider's keys"), "{back:?}");
    assert!(!back.contains_key("code"));
    *idp.forge.lock().unwrap() = false;

    // Closed: people with an account still get in; nobody new.
    let closed = pb::InstanceSettings { sso_accounts: pb::SsoAccounts::Closed as i32, ..Default::default() };
    update_settings(&mut c, &admin, closed, &["sso_accounts"]).await.unwrap();
    let new = start_instance_sso(&mut c, None, false).await.unwrap();
    let back = landed(&idp.sign_in(&instance, &new.authorize_url, "bea").await);
    assert_eq!(finish_instance_sso(&mut c, &back, SECRET).await.unwrap_err().code(), Code::FailedPrecondition);

    // A state nobody started gets a plain page, not a redirect.
    let stray =
        browser().get(format!("http://{}/sso/instance/oidc?state=nope&code=x", instance.addr)).send().await.unwrap();
    assert_eq!(stray.status(), 400);
    instance.stop().await;
}

// ───────────────────────── a fake SAML provider ─────────────────────────

/// The provider's signed answer to an AuthnRequest, as its page would post it.
fn saml_response(authorize_url: &str, who: &str, acs: &str, audience: &str, now_ms: i64) -> (String, String) {
    let asked = query(authorize_url);
    let deflated = STANDARD.decode(&asked["SAMLRequest"]).unwrap();
    let mut request = String::new();
    std::io::Read::read_to_string(&mut flate2::read::DeflateDecoder::new(deflated.as_slice()), &mut request).unwrap();
    let request_id = request.split("ID=\"").nth(1).unwrap().split('"').next().unwrap().to_string();
    let t = |offset_ms: i64| fuwa_server::sso::xml::format_time(now_ms + offset_ms);
    let assertion = format!(
        r#"<saml:Assertion xmlns:saml="urn:oasis:names:tc:SAML:2.0:assertion" ID="_a{who}" Version="2.0" IssueInstant="{now}"><saml:Issuer>https://idp.acme.com</saml:Issuer><saml:Subject><saml:NameID>{who}-id</saml:NameID><saml:SubjectConfirmation Method="urn:oasis:names:tc:SAML:2.0:cm:bearer"><saml:SubjectConfirmationData NotOnOrAfter="{later}" Recipient="{acs}" InResponseTo="{request_id}"/></saml:SubjectConfirmation></saml:Subject><saml:Conditions NotBefore="{now}" NotOnOrAfter="{later}"><saml:AudienceRestriction><saml:Audience>{audience}</saml:Audience></saml:AudienceRestriction></saml:Conditions><saml:AttributeStatement><saml:Attribute Name="mail"><saml:AttributeValue>{who}@acme.com</saml:AttributeValue></saml:Attribute></saml:AttributeStatement></saml:Assertion>"#,
        now = t(0),
        later = t(5 * 60 * 1000),
    );
    // Sign the assertion: digest its canonical form, then sign SignedInfo's.
    let doc = fuwa_server::sso::xml::parse(&assertion).unwrap();
    let canonical = fuwa_server::sso::xml::exc_c14n(&assertion, doc.root_element(), None, &[], false);
    let digest = STANDARD.encode(Sha256::digest(canonical.as_bytes()));
    let signed_info = format!(
        r##"<ds:SignedInfo xmlns:ds="http://www.w3.org/2000/09/xmldsig#"><ds:CanonicalizationMethod Algorithm="http://www.w3.org/2001/10/xml-exc-c14n#"/><ds:SignatureMethod Algorithm="http://www.w3.org/2001/04/xmldsig-more#rsa-sha256"/><ds:Reference URI="#_a{who}"><ds:Transforms><ds:Transform Algorithm="http://www.w3.org/2000/09/xmldsig#enveloped-signature"/><ds:Transform Algorithm="http://www.w3.org/2001/10/xml-exc-c14n#"/></ds:Transforms><ds:DigestMethod Algorithm="http://www.w3.org/2001/04/xmlenc#sha256"/><ds:DigestValue>{digest}</ds:DigestValue></ds:Reference></ds:SignedInfo>"##
    );
    let info = fuwa_server::sso::xml::parse(&signed_info).unwrap();
    let canonical_info = fuwa_server::sso::xml::exc_c14n(&signed_info, info.root_element(), None, &[], false);
    let signature = STANDARD.encode(rsa_sign(canonical_info.as_bytes()));
    let signed = assertion.replacen(
        "<saml:Subject>",
        &format!(
            r#"<ds:Signature xmlns:ds="http://www.w3.org/2000/09/xmldsig#">{signed_info}<ds:SignatureValue>{signature}</ds:SignatureValue></ds:Signature><saml:Subject>"#
        ),
        1,
    );
    let response = format!(
        r#"<samlp:Response xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol" ID="_r{who}" Version="2.0" IssueInstant="{now}" Destination="{acs}" InResponseTo="{request_id}"><samlp:Status><samlp:StatusCode Value="urn:oasis:names:tc:SAML:2.0:status:Success"/></samlp:Status>{signed}</samlp:Response>"#,
        now = t(0),
    );
    (STANDARD.encode(response), asked["RelayState"].clone())
}

async fn post_saml(instance: &Instance, path: &str, response: &str, relay_state: &str) -> reqwest::Response {
    browser()
        .post(format!("http://{}{path}", instance.addr))
        .form(&[("SAMLResponse", response), ("RelayState", relay_state)])
        .send()
        .await
        .unwrap()
}

async fn start_server_sso(
    c: &mut Clients,
    token: &str,
    server_id: &str,
) -> Result<pb::StartServerSsoResponse, tonic::Status> {
    let request = pb::StartServerSsoRequest {
        server_id: server_id.into(),
        return_origin: APP.into(),
        secret_hash: fuwa_server::linked::secret_hash(SECRET),
        invite_code: String::new(),
    };
    c.sso.start_server_sso(authed(token, request)).await.map(|r| r.into_inner())
}

/// Signs `token`'s account in to a server through its SAML provider.
async fn server_saml_sign_in(
    instance: &Instance,
    c: &mut Clients,
    token: &str,
    server_id: &str,
    who: &str,
) -> pb::FinishServerSsoResponse {
    let started = start_server_sso(c, token, server_id).await.unwrap();
    let base = format!("/sso/servers/{server_id}");
    let public = instance.app.settings().public_url.clone();
    let (response, relay) = saml_response(
        &started.authorize_url,
        who,
        &format!("{public}{base}/saml"),
        &format!("{public}{base}/saml/metadata"),
        fuwa_server::id::now_ms(),
    );
    let back = landed(&post_saml(instance, &format!("{base}/saml"), &response, &relay).await);
    assert_eq!(back["server"], server_id);
    assert!(back.contains_key("code"), "{back:?}");
    let request = pb::FinishServerSsoRequest {
        server_id: server_id.into(),
        state: back["state"].clone(),
        code: back["code"].clone(),
        secret: SECRET.into(),
    };
    c.sso.finish_server_sso(authed(token, request)).await.unwrap().into_inner()
}

async fn visible_channels(c: &mut Clients, token: &str, server_id: &str) -> usize {
    let request = pb::ListChannelsRequest { server_id: server_id.into() };
    c.channels.list_channels(authed(token, request)).await.unwrap().into_inner().channels.len()
}

async fn join(c: &mut Clients, token: &str, server_id: &str) -> Result<pb::JoinServerResponse, tonic::Status> {
    let request = pb::JoinServerRequest { server_id: server_id.into(), ..Default::default() };
    c.servers.join_server(authed(token, request)).await.map(|r| r.into_inner())
}

#[tokio::test]
async fn servers_require_their_own_saml_sign_in_to_join_and_stay() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let mut c = clients(&instance).await;
    let owner = sign_up(&mut c, "owner").await;
    let ana = sign_up(&mut c, "ana").await;
    let bea = sign_up(&mut c, "bea").await;
    let request = pb::CreateServerRequest { name: "Acme".into(), discoverable: true, ..Default::default() };
    let server = c.servers.create_server(authed(&owner, request)).await.unwrap().into_inner().server.unwrap();
    let id = server.id.clone();
    assert!(!server.sso_required, "off by default");

    // Bea joins before it's required.
    join(&mut c, &bea, &id).await.unwrap();
    assert!(visible_channels(&mut c, &bea, &id).await > 0);

    // Only the owner sees or changes it, not even an Admin.
    let get = |id: &str| pb::GetServerSsoRequest { server_id: id.into() };
    assert_eq!(c.sso.get_server_sso(authed(&bea, get(&id))).await.unwrap_err().code(), Code::PermissionDenied);
    let update = |provider: Option<pb::IdentityProvider>, required: Option<bool>| pb::UpdateServerSsoRequest {
        server_id: id.clone(),
        provider,
        remove_provider: false,
        required,
        recheck_days: None,
    };
    let roles = c.roles.list_roles(authed(&owner, pb::ListRolesRequest { server_id: id.clone() })).await.unwrap();
    let admin_role = roles.into_inner().roles.into_iter().find(|r| r.name == "Admin").unwrap();
    let bea_id = c.auth.get_me(authed(&bea, pb::GetMeRequest {})).await.unwrap().into_inner().user.unwrap().id;
    let give = pb::AddMemberRoleRequest { server_id: id.clone(), user_id: bea_id.clone(), role_id: admin_role.id };
    c.roles.add_member_role(authed(&owner, give)).await.unwrap();
    let by_admin = c.sso.update_server_sso(authed(&bea, update(Some(saml_provider()), None))).await.unwrap_err();
    assert_eq!(by_admin.code(), Code::PermissionDenied);
    assert!(by_admin.message().contains("owner"), "{by_admin:?}");
    assert_eq!(c.sso.get_server_sso(authed(&bea, get(&id))).await.unwrap_err().code(), Code::PermissionDenied);
    let sso = c
        .sso
        .update_server_sso(authed(&owner, update(Some(saml_provider()), None)))
        .await
        .unwrap()
        .into_inner()
        .sso
        .unwrap();
    assert!(!sso.required);
    assert_eq!(sso.recheck_days, 30);
    let sp = sso.service_provider.unwrap();
    assert!(sp.saml_acs_url.ends_with(&format!("/sso/servers/{id}/saml")));

    // This side's metadata, for the provider.
    let metadata =
        browser().get(format!("http://{}/sso/servers/{id}/saml/metadata", instance.addr)).send().await.unwrap();
    assert_eq!(metadata.status(), 200);
    assert!(metadata.text().await.unwrap().contains(&sp.saml_acs_url));

    // A server's provider can't send the instance to private addresses.
    c.sso.update_server_sso(authed(&owner, update(Some(oidc_provider("https://127.0.0.1:9")), None))).await.unwrap();
    let local = start_server_sso(&mut c, &ana, &id).await.unwrap_err();
    assert_eq!(local.code(), Code::FailedPrecondition);
    assert!(local.message().contains("won't fetch"), "{}", local.message());
    c.sso.update_server_sso(authed(&owner, update(Some(saml_provider()), None))).await.unwrap();

    // The owner can require it without signing in; everyone else must.
    let server = {
        c.sso.update_server_sso(authed(&owner, update(None, Some(true)))).await.unwrap();
        c.servers
            .get_server(authed(&owner, pb::GetServerRequest { server_id: id.clone() }))
            .await
            .unwrap()
            .into_inner()
            .server
            .unwrap()
    };
    assert!(server.sso_required);
    assert_eq!(server.sso_name, "Acme SAML");
    assert_eq!(server.sso_host, "idp.acme.com", "apps name the site that sees your address");
    assert!(visible_channels(&mut c, &owner, &id).await > 0, "the owner is never shut out");
    assert_eq!(visible_channels(&mut c, &bea, &id).await, 0, "members who haven't signed in see nothing");

    // Ana can't join until she signs in through it.
    let refused = join(&mut c, &ana, &id).await.unwrap_err();
    assert_eq!(refused.code(), Code::FailedPrecondition);
    assert!(refused.message().contains("Acme SAML"));
    let finished = server_saml_sign_in(&instance, &mut c, &ana, &id, "ana").await;
    assert_eq!(finished.identity.unwrap().email, "ana@acme.com");
    assert!(finished.member.is_none(), "not a member yet");
    let joined = join(&mut c, &ana, &id).await.unwrap();
    assert!(joined.member.unwrap().sso_signed_in_at.is_some());
    assert!(visible_channels(&mut c, &ana, &id).await > 0);

    // Bea signs in and sees the server again.
    let finished = server_saml_sign_in(&instance, &mut c, &bea, &id, "bea").await;
    assert!(finished.member.is_some());
    assert!(visible_channels(&mut c, &bea, &id).await > 0);

    // One provider identity belongs to one account.
    let started = start_server_sso(&mut c, &owner, &id).await.unwrap();
    let public = instance.app.settings().public_url.clone();
    let base = format!("/sso/servers/{id}");
    let (response, relay) = saml_response(
        &started.authorize_url,
        "ana",
        &format!("{public}{base}/saml"),
        &format!("{public}{base}/saml/metadata"),
        fuwa_server::id::now_ms(),
    );
    let back = landed(&post_saml(&instance, &format!("{base}/saml"), &response, &relay).await);
    // Posting the same answer again doesn't work: it's used up.
    let replayed = landed(&post_saml(&instance, &format!("{base}/saml"), &response, &relay).await);
    assert!(replayed["error"].contains("already used"), "{replayed:?}");
    let request = pb::FinishServerSsoRequest {
        server_id: id.clone(),
        state: back["state"].clone(),
        code: back["code"].clone(),
        secret: SECRET.into(),
    };
    assert_eq!(c.sso.finish_server_sso(authed(&owner, request)).await.unwrap_err().code(), Code::AlreadyExists);

    // An answer for another server's address is refused.
    let started = start_server_sso(&mut c, &ana, &id).await.unwrap();
    let (wrong, relay) = saml_response(
        &started.authorize_url,
        "ana",
        "https://elsewhere/acs",
        "https://elsewhere/meta",
        fuwa_server::id::now_ms(),
    );
    let back = landed(&post_saml(&instance, &format!("{base}/saml"), &wrong, &relay).await);
    assert!(back["error"].contains("refused"), "{back:?}");

    // When someone signed in is theirs and the managers' business alone.
    let members_seen_by = |token: &str| authed(token, pb::ListMembersRequest { server_id: id.clone() });
    let seen = c.servers.list_members(members_seen_by(&ana)).await.unwrap().into_inner().members;
    let signed_in = |members: &[pb::Member], who: &str| {
        members.iter().find(|m| m.user.as_ref().unwrap().username == who).unwrap().sso_signed_in_at.is_some()
    };
    assert!(signed_in(&seen, "ana"), "your own");
    assert!(!signed_in(&seen, "bea"), "not someone else's");
    let seen = c.servers.list_members(members_seen_by(&owner)).await.unwrap().into_inner().members;
    assert!(signed_in(&seen, "bea"), "managers see it");
    // Not moderators who only time people out, even in what they get back.
    let roles = c.roles.list_roles(authed(&owner, pb::ListRolesRequest { server_id: id.clone() })).await.unwrap();
    let admin_role = roles.into_inner().roles.into_iter().find(|r| r.name == "Admin").unwrap();
    let take = pb::RemoveMemberRoleRequest { server_id: id.clone(), user_id: bea_id.clone(), role_id: admin_role.id };
    c.roles.remove_member_role(authed(&owner, take)).await.unwrap();
    let mods = pb::CreateRoleRequest {
        server_id: id.clone(),
        name: "Mods".into(),
        permissions: vec![pb::Permission::TimeOutMembers as i32],
        ..Default::default()
    };
    let mods = c.roles.create_role(authed(&owner, mods)).await.unwrap().into_inner().role.unwrap();
    let ana_id = c.auth.get_me(authed(&ana, pb::GetMeRequest {})).await.unwrap().into_inner().user.unwrap().id;
    let give = pb::AddMemberRoleRequest { server_id: id.clone(), user_id: ana_id, role_id: mods.id };
    c.roles.add_member_role(authed(&owner, give)).await.unwrap();
    let time_out =
        pb::TimeOutMemberRequest { server_id: id.clone(), user_id: bea_id.clone(), seconds: 60, ..Default::default() };
    let timed = c.servers.time_out_member(authed(&ana, time_out)).await.unwrap().into_inner().member.unwrap();
    assert!(timed.sso_signed_in_at.is_none(), "a moderator doesn't learn when they signed in");
    let undo =
        pb::TimeOutMemberRequest { server_id: id.clone(), user_id: bea_id.clone(), seconds: 0, ..Default::default() };
    c.servers.time_out_member(authed(&owner, undo)).await.unwrap();

    // Managers see how many members have signed in.
    let got = c.sso.get_server_sso(authed(&owner, get(&id))).await.unwrap().into_inner();
    assert_eq!(got.sso.unwrap().signed_in_members, 2);

    // A new signing certificate is a new provider too, and the log says which.
    // (Requiring it again first, to see that it stops.)
    c.sso.update_server_sso(authed(&owner, update(None, Some(true)))).await.unwrap();
    let mut rotated = saml_provider();
    rotated.saml.as_mut().unwrap().sso_url = "https://idp.acme.com/sso2".into();
    let sso =
        c.sso.update_server_sso(authed(&owner, update(Some(rotated), None))).await.unwrap().into_inner().sso.unwrap();
    assert!(!sso.required);
    assert_eq!(sso.signed_in_members, 0);
    c.sso.update_server_sso(authed(&owner, update(Some(saml_provider()), None))).await.unwrap();

    // Only https for a server's provider.
    let mut plain = saml_provider();
    plain.saml.as_mut().unwrap().sso_url = "http://localhost:9/sso".into();
    let plain = c.sso.update_server_sso(authed(&owner, update(Some(plain), None))).await.unwrap_err();
    assert_eq!(plain.code(), Code::InvalidArgument);

    // Switching providers forgets every sign-in and stops requiring it.
    let mut other = saml_provider();
    other.saml.as_mut().unwrap().entity_id = "https://idp2.acme.com".into();
    let sso =
        c.sso.update_server_sso(authed(&owner, update(Some(other), None))).await.unwrap().into_inner().sso.unwrap();
    assert!(!sso.required);
    assert_eq!(sso.signed_in_members, 0);
    assert!(visible_channels(&mut c, &ana, &id).await > 0, "not required any more");
    instance.stop().await;
}
