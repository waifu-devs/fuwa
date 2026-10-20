//! Signing in with a code sent by email end to end: a real instance sending
//! through Resend's API, with a fake Resend keeping what it was sent.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use fuwa_server::app::App;
use fuwa_server::config::Config;
use fuwa_server::pb;
use tokio::task::JoinHandle;
use tonic::transport::Channel;
use tonic::{Code, Request};

const PASSWORD: &str = "correct horse battery";
const KEY: &str = "re_test_key";

/// The mail the fake Resend was sent, as JSON.
#[derive(Clone, Default)]
struct Outbox(Arc<Mutex<Vec<serde_json::Value>>>);

impl Outbox {
    fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    /// The code in the last mail to `to`, from its subject.
    fn code_for(&self, to: &str) -> String {
        let sent = self.0.lock().unwrap();
        let mail = sent.iter().rev().find(|mail| mail["to"][0] == to).expect("a mail to that address");
        mail["subject"].as_str().unwrap().split(' ').next().unwrap().to_string()
    }
}

async fn fake_resend() -> (String, Outbox) {
    async fn emails(State(outbox): State<Outbox>, headers: HeaderMap, body: axum::body::Bytes) -> (StatusCode, String) {
        if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(&format!("Bearer {KEY}")) {
            return (StatusCode::UNAUTHORIZED, r#"{"statusCode":401,"message":"API key is invalid"}"#.into());
        }
        outbox.0.lock().unwrap().push(serde_json::from_slice(&body).unwrap());
        (StatusCode::OK, r#"{"id":"49a3999c-0ce1-4ea6-ab68-afcd6dc2e794"}"#.into())
    }
    let outbox = Outbox::default();
    let router = Router::new().route("/emails", post(emails)).with_state(outbox.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (format!("http://{addr}"), outbox)
}

struct Instance {
    app: Arc<App>,
    addr: SocketAddr,
    serving: JoinHandle<()>,
}

async fn start(dir: &Path, resend: Option<&str>) -> Instance {
    let dir = dir.to_str().unwrap().to_string();
    let resend = resend.map(str::to_string);
    let config = Config::from_lookup(|key| match key {
        "FUWA_DATA_PATH" => Some(dir.clone()),
        "FUWA_TELEMETRY" => Some("off".into()),
        "FUWA_UPDATE_CHECK" => Some("off".into()),
        "FUWA_NODE_NAME" => Some("Acme Chat".into()),
        "FUWA_MAIL_PROVIDER" => resend.as_ref().map(|_| "resend".into()),
        "FUWA_MAIL_FROM" => Some("Acme Chat <no-reply@acme.example>".into()),
        "FUWA_RESEND_API_KEY" => Some(KEY.into()),
        "FUWA_RESEND_URL" => resend.clone(),
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
    account: pb::account_service_client::AccountServiceClient<Channel>,
    node: pb::node_service_client::NodeServiceClient<Channel>,
}

async fn clients(instance: &Instance) -> Clients {
    let channel = Channel::from_shared(format!("http://{}", instance.addr)).unwrap().connect().await.unwrap();
    Clients {
        auth: pb::auth_service_client::AuthServiceClient::new(channel.clone()),
        account: pb::account_service_client::AccountServiceClient::new(channel.clone()),
        node: pb::node_service_client::NodeServiceClient::new(channel),
    }
}

fn authed<T>(token: &str, message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

async fn start_code(c: &mut Clients, email: &str) -> String {
    c.auth.start_email_sign_in(pb::StartEmailSignInRequest { email: email.into() }).await.unwrap().into_inner().ticket
}

async fn finish(c: &mut Clients, ticket: &str, code: &str) -> Result<pb::FinishEmailSignInResponse, tonic::Status> {
    let request = pb::FinishEmailSignInRequest { ticket: ticket.into(), code: code.into(), ..Default::default() };
    c.auth.finish_email_sign_in(request).await.map(|r| r.into_inner())
}

/// A code that isn't `code`.
fn wrong(code: &str) -> String {
    format!("{:06}", (code.parse::<u32>().unwrap() + 1) % 1_000_000)
}

#[tokio::test]
async fn someone_new_signs_up_and_back_in_with_codes() {
    let dir = tempfile::tempdir().unwrap();
    let (resend, outbox) = fake_resend().await;
    let instance = start(dir.path(), Some(&resend)).await;
    let mut c = clients(&instance).await;

    let node = c.node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap();
    let auth = node.auth.unwrap();
    assert!(auth.email_sign_in && auth.email_sign_up);

    let ticket = start_code(&mut c, "  Ana@Example.com ").await;
    assert_eq!(outbox.len(), 1);
    let code = outbox.code_for("ana@example.com");
    {
        let sent = outbox.0.lock().unwrap();
        assert_eq!(sent[0]["from"], "Acme Chat <no-reply@acme.example>");
        assert!(sent[0]["subject"].as_str().unwrap().ends_with("is your sign-in code for Acme Chat"));
        assert!(sent[0]["text"].as_str().unwrap().contains(&code));
    }

    let err = finish(&mut c, &ticket, &wrong(&code)).await.unwrap_err();
    assert_eq!((err.code(), err.message()), (Code::PermissionDenied, "that code didn't work"));

    // Someone new picks a username first, without using the code up.
    let asked = finish(&mut c, &ticket, &code).await.unwrap();
    assert!(asked.token.is_empty());
    let new = asked.new_account.unwrap();
    assert_eq!((new.provider_name.as_str(), new.suggested_username.as_str()), ("Email", "ana"));

    let made = c
        .auth
        .finish_email_sign_in(pb::FinishEmailSignInRequest {
            ticket: ticket.clone(),
            // However it's spaced.
            code: format!("{} {}", &code[..3], &code[3..]),
            create: true,
            username: "ana".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(made.created && made.admin, "the first account is the admin");
    let ana = made.user.unwrap();
    assert_eq!((ana.username.as_str(), ana.kind()), ("ana", pb::AccountKind::Provider));

    // The code worked once.
    let err = finish(&mut c, &ticket, &code).await.unwrap_err();
    assert_eq!(err.code(), Code::FailedPrecondition, "{}", err.message());

    // Back in with a new code: the same account.
    let ticket = start_code(&mut c, "ana@example.com").await;
    let again = finish(&mut c, &ticket, &outbox.code_for("ana@example.com")).await.unwrap();
    assert!(!again.created && !again.token.is_empty());
    assert_eq!(again.user.unwrap().id, ana.id);

    let methods = c
        .account
        .list_sign_in_methods(authed(&again.token, pb::ListSignInMethodsRequest {}))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(methods.methods.len(), 1);
    let email = &methods.methods[0];
    assert_eq!(
        (email.kind.as_str(), email.name.as_str(), email.account_name.as_str()),
        ("email", "Email", "ana@example.com")
    );
    assert!(email.works && !methods.can_add_email);

    // The only way in can't be removed.
    let err = c
        .account
        .unlink_provider(authed(
            &again.token,
            pb::UnlinkProviderRequest { provider: "email".into(), ..Default::default() },
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::FailedPrecondition, "{}", err.message());

    instance.stop().await;
}

#[tokio::test]
async fn five_wrong_codes_end_the_ticket() {
    let dir = tempfile::tempdir().unwrap();
    let (resend, outbox) = fake_resend().await;
    let instance = start(dir.path(), Some(&resend)).await;
    let mut c = clients(&instance).await;

    let ticket = start_code(&mut c, "eve@example.com").await;
    let code = outbox.code_for("eve@example.com");
    for _ in 0..5 {
        let err = finish(&mut c, &ticket, &wrong(&code)).await.unwrap_err();
        assert_eq!(err.code(), Code::PermissionDenied);
    }
    let err = finish(&mut c, &ticket, &code).await.unwrap_err();
    assert_eq!((err.code(), err.message()), (Code::FailedPrecondition, "this code ran out; ask for a new one"));

    let err = c.auth.start_email_sign_in(pb::StartEmailSignInRequest { email: "eve".into() }).await.unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);

    instance.stop().await;
}

#[tokio::test]
async fn an_account_adds_an_address_to_sign_in_with() {
    let dir = tempfile::tempdir().unwrap();
    let (resend, outbox) = fake_resend().await;
    let instance = start(dir.path(), Some(&resend)).await;
    let mut c = clients(&instance).await;

    let bob = c
        .auth
        .sign_up(pb::SignUpRequest { username: "bob".into(), password: PASSWORD.into(), ..Default::default() })
        .await
        .unwrap()
        .into_inner();
    let methods =
        c.account.list_sign_in_methods(authed(&bob.token, pb::ListSignInMethodsRequest {})).await.unwrap().into_inner();
    assert!(methods.can_add_email);

    // It takes the password, like linking a provider.
    let start = |password: &str| pb::StartEmailLinkRequest {
        email: "Bob@Example.com".into(),
        password: password.into(),
        ..Default::default()
    };
    let err = c.account.start_email_link(authed(&bob.token, start("wrong password"))).await.unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied);
    let ticket = c.account.start_email_link(authed(&bob.token, start(PASSWORD))).await.unwrap().into_inner().ticket;
    let code = outbox.code_for("bob@example.com");
    assert!(outbox.0.lock().unwrap().last().unwrap()["subject"].as_str().unwrap().contains("add this address"));

    // A code for adding an address doesn't sign anyone in.
    let err = finish(&mut c, &ticket, &code).await.unwrap_err();
    assert_eq!(err.code(), Code::FailedPrecondition);
    let ticket = c.account.start_email_link(authed(&bob.token, start(PASSWORD))).await.unwrap().into_inner().ticket;
    let code = outbox.code_for("bob@example.com");
    let added = c
        .account
        .finish_email_link(authed(&bob.token, pb::FinishEmailLinkRequest { ticket, code }))
        .await
        .unwrap()
        .into_inner()
        .method
        .unwrap();
    assert_eq!((added.kind.as_str(), added.account_name.as_str()), ("email", "bob@example.com"));

    // Now the address signs in to bob.
    let ticket = start_code(&mut c, "bob@example.com").await;
    let signed_in = finish(&mut c, &ticket, &outbox.code_for("bob@example.com")).await.unwrap();
    assert_eq!(signed_in.user.unwrap().id, bob.user.as_ref().unwrap().id);

    // Someone else can't take it.
    let carol = c
        .auth
        .sign_up(pb::SignUpRequest { username: "carol".into(), password: PASSWORD.into(), ..Default::default() })
        .await
        .unwrap()
        .into_inner();
    let ticket = c
        .account
        .start_email_link(authed(
            &carol.token,
            pb::StartEmailLinkRequest {
                email: "bob@example.com".into(),
                password: PASSWORD.into(),
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .ticket;
    let code = outbox.code_for("bob@example.com");
    // Even with its code, the address stays bob's.
    let err = c
        .account
        .finish_email_link(authed(
            &carol.token,
            pb::FinishEmailLinkRequest { ticket: ticket.clone(), code: code.clone() },
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::AlreadyExists, "{}", err.message());

    // With a password still there, bob can remove the address.
    c.account
        .unlink_provider(authed(
            &bob.token,
            pb::UnlinkProviderRequest { provider: "email".into(), password: PASSWORD.into(), ..Default::default() },
        ))
        .await
        .unwrap();
    let methods =
        c.account.list_sign_in_methods(authed(&bob.token, pb::ListSignInMethodsRequest {})).await.unwrap().into_inner();
    assert!(methods.methods.iter().all(|m| m.kind != "email") && methods.can_add_email);

    instance.stop().await;
}

#[tokio::test]
async fn without_a_mail_provider_there_are_no_codes() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), None).await;
    let mut c = clients(&instance).await;
    let node = c.node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap();
    assert!(!node.auth.unwrap().email_sign_in);
    let err =
        c.auth.start_email_sign_in(pb::StartEmailSignInRequest { email: "yan@example.com".into() }).await.unwrap_err();
    assert_eq!(err.code(), Code::FailedPrecondition);
    instance.stop().await;
}

#[tokio::test]
async fn a_provider_that_refuses_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let (resend, outbox) = fake_resend().await;
    // A key the fake Resend turns down.
    let instance = {
        let dir = dir.path().to_str().unwrap().to_string();
        let config = Config::from_lookup(|key| match key {
            "FUWA_DATA_PATH" => Some(dir.clone()),
            "FUWA_TELEMETRY" => Some("off".into()),
            "FUWA_UPDATE_CHECK" => Some("off".into()),
            "FUWA_MAIL_PROVIDER" => Some("resend".into()),
            "FUWA_MAIL_FROM" => Some("no-reply@acme.example".into()),
            "FUWA_RESEND_API_KEY" => Some("re_wrong".into()),
            "FUWA_RESEND_URL" => Some(resend.clone()),
            _ => None,
        })
        .unwrap();
        let app = App::open(config).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = app.router();
        let shutdown = app.shutdown.clone();
        let serving = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move { shutdown.cancelled().await })
                .await
                .unwrap();
        });
        Instance { app, addr, serving }
    };
    let mut c = clients(&instance).await;
    let err =
        c.auth.start_email_sign_in(pb::StartEmailSignInRequest { email: "zoe@example.com".into() }).await.unwrap_err();
    assert_eq!((err.code(), err.message()), (Code::Unavailable, "the email couldn't be sent; try again in a moment"));
    assert_eq!(outbox.len(), 0);
    instance.stop().await;
}
