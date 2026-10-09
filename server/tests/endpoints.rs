//! Agent endpoints: an agent with no stream gets its events posted to a URL,
//! signed, in order, tried again when its endpoint fails, and answers
//! interactions in its answer.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use fuwa_server::app::App;
use fuwa_server::config::Config;
use fuwa_server::pb;
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tonic::transport::Channel;
use tonic::{Code, Request};

struct Instance {
    app: Arc<App>,
    channel: Channel,
    serving: JoinHandle<()>,
}

async fn start(dir: &Path, env: &[(&str, &str)]) -> Instance {
    let dir = dir.to_str().unwrap().to_string();
    let env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let config = Config::from_lookup(|key| match key {
        "FUWA_DATA_PATH" => Some(dir.clone()),
        "FUWA_TELEMETRY" => Some("off".into()),
        _ => env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()),
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
    let channel = Channel::from_shared(format!("http://{addr}")).unwrap().connect().await.unwrap();
    Instance { app, channel, serving }
}

impl Instance {
    async fn stop(self) {
        self.app.shutdown.cancel();
        self.serving.await.unwrap();
    }
}

fn authed<T>(token: &str, message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

/// A stateless agent: checks each delivery's signature, answers the check,
/// and answers every interaction with "rolled 4". Down, it answers 500.
/// Slow, it takes a while over each delivery, noting the most it had at once.
/// It can serve other agents too, signed with their secrets.
struct Endpoint {
    url: String,
    secret: Arc<Mutex<String>>,
    others: Arc<Mutex<Vec<String>>>,
    down: Arc<AtomicBool>,
    slow: Arc<AtomicBool>,
    most_at_once: Arc<AtomicUsize>,
    deliveries: mpsc::UnboundedReceiver<Value>,
}

fn signed(secret: &str, headers: &HeaderMap, body: &[u8]) -> bool {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    let Some(key) = secret.strip_prefix("whsec_") else { return false };
    let key = STANDARD.decode(key).unwrap();
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).unwrap();
    mac.update(format!("{}.{}.", header("webhook-id"), header("webhook-timestamp")).as_bytes());
    mac.update(body);
    let expected = format!("v1,{}", STANDARD.encode(mac.finalize().into_bytes()));
    header("webhook-signature") == expected
}

async fn endpoint() -> Endpoint {
    let secret = Arc::new(Mutex::new(String::new()));
    let others: Arc<Mutex<Vec<String>>> = Arc::default();
    let down = Arc::new(AtomicBool::new(false));
    let slow = Arc::new(AtomicBool::new(false));
    let (at_once, most_at_once) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (tx, deliveries) = mpsc::unbounded_channel();
    let handler = {
        let (secret, others, down, slow, at_once, most) =
            (secret.clone(), others.clone(), down.clone(), slow.clone(), at_once, most_at_once.clone());
        move |headers: HeaderMap, body: Bytes| {
            let (secret, others, down, tx) = (secret.clone(), others.clone(), down.clone(), tx.clone());
            let (slow, at_once, most) = (slow.clone(), at_once.clone(), most.clone());
            async move {
                let mine = signed(&secret.lock().unwrap(), &headers, &body);
                if !mine && !others.lock().unwrap().iter().any(|other| signed(other, &headers, &body)) {
                    return (StatusCode::UNAUTHORIZED, String::new());
                }
                if down.load(Ordering::SeqCst) {
                    return (StatusCode::INTERNAL_SERVER_ERROR, String::new());
                }
                let delivery: Value = serde_json::from_slice(&body).unwrap();
                if let Some(challenge) = delivery["challenge"].as_str() {
                    return (StatusCode::OK, json!({ "challenge": challenge }).to_string());
                }
                if slow.load(Ordering::SeqCst) {
                    let now = at_once.fetch_add(1, Ordering::SeqCst) + 1;
                    most.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    at_once.fetch_sub(1, Ordering::SeqCst);
                }
                let replies: Vec<Value> = delivery["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|event| event["interactionCreated"]["interaction"]["id"].as_str())
                    .map(|id| json!({ "interactionId": id, "content": "rolled 4" }))
                    .collect();
                tx.send(delivery).unwrap();
                (StatusCode::OK, json!({ "replies": replies }).to_string())
            }
        }
    };
    let router = axum::Router::new().route("/fuwa", axum::routing::post(handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Endpoint { url: format!("http://{addr}/fuwa"), secret, others, down, slow, most_at_once, deliveries }
}

impl Endpoint {
    /// The next event delivered whose payload is `kind`.
    async fn next(&mut self, kind: &str) -> Value {
        loop {
            let delivery = tokio::time::timeout(Duration::from_secs(15), self.deliveries.recv())
                .await
                .expect("nothing came")
                .unwrap();
            for event in delivery["events"].as_array().unwrap() {
                if event.get(kind).is_some() {
                    return event.clone();
                }
            }
        }
    }
}

#[tokio::test]
async fn an_agent_hears_through_its_endpoint_and_answers_there() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[("FUWA_AGENT_ENDPOINTS", "any")]).await;
    let channel = instance.channel.clone();
    let mut auth = pb::auth_service_client::AuthServiceClient::new(channel.clone());
    let mut servers = pb::server_service_client::ServerServiceClient::new(channel.clone());
    let mut channels = pb::channel_service_client::ChannelServiceClient::new(channel.clone());
    let mut messages = pb::message_service_client::MessageServiceClient::new(channel.clone());
    let mut agents = pb::agent_service_client::AgentServiceClient::new(channel.clone());
    let mut commands = pb::command_service_client::CommandServiceClient::new(channel.clone());

    let owner = auth
        .sign_up(pb::SignUpRequest {
            username: "juan".into(),
            password: "correct horse battery".into(),
            display_name: String::new(),
        })
        .await
        .unwrap()
        .into_inner()
        .token;
    let server_id = servers
        .create_server(authed(&owner, pb::CreateServerRequest { name: "Waifu Devs".into(), ..Default::default() }))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap()
        .id;
    let general_id = channels
        .list_channels(authed(&owner, pb::ListChannelsRequest { server_id: server_id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels
        .into_iter()
        .find(|c| c.name == "general")
        .unwrap()
        .id;
    let made = agents
        .create_agent(authed(&owner, pb::CreateAgentRequest { username: "dice".into(), display_name: "Dice".into() }))
        .await
        .unwrap()
        .into_inner();
    let agent_id = made.agent.unwrap().user.unwrap().id;
    agents
        .add_agent(authed(&owner, pb::AddAgentRequest { server_id: server_id.clone(), username: "dice".into() }))
        .await
        .unwrap();
    commands
        .set_commands(authed(
            &made.token,
            pb::SetCommandsRequest {
                server_id: server_id.clone(),
                commands: vec![pb::Command { name: "roll".into(), description: "Rolls a die".into(), options: vec![] }],
            },
        ))
        .await
        .unwrap();

    // The secret comes first, so the agent can check the check.
    let got = agents
        .get_agent_endpoint(authed(&owner, pb::GetAgentEndpointRequest { agent_id: agent_id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .endpoint
        .unwrap();
    assert!(got.secret.starts_with("whsec_"));
    assert!(got.url.is_empty());
    // Agents don't manage endpoints, not even their own.
    let refused = agents
        .get_agent_endpoint(authed(&made.token, pb::GetAgentEndpointRequest { agent_id: agent_id.clone() }))
        .await;
    assert_eq!(refused.unwrap_err().code(), Code::PermissionDenied);

    let mut endpoint = endpoint().await;
    *endpoint.secret.lock().unwrap() = got.secret.clone();
    let set = |url: String, events: &[&str]| {
        authed(
            &owner,
            pb::SetAgentEndpointRequest {
                agent_id: agent_id.clone(),
                url,
                events: events.iter().map(|e| e.to_string()).collect(),
            },
        )
    };
    // A URL that doesn't answer the check isn't saved, nor events that don't exist.
    let missing = agents.set_agent_endpoint(set(endpoint.url.replace("/fuwa", "/elsewhere"), &[])).await;
    assert_eq!(missing.unwrap_err().code(), Code::FailedPrecondition);
    let unknown = agents.set_agent_endpoint(set(endpoint.url.clone(), &["message_eaten"])).await;
    assert_eq!(unknown.unwrap_err().code(), Code::InvalidArgument);
    let saved = agents
        .set_agent_endpoint(set(endpoint.url.clone(), &["message_created", "interaction_created"]))
        .await
        .unwrap()
        .into_inner()
        .endpoint
        .unwrap();
    assert_eq!(saved.url, endpoint.url);

    // Events come signed, in proto3 JSON.
    let send = |content: &str| {
        authed(
            &owner,
            pb::SendMessageRequest {
                server_id: server_id.clone(),
                channel_id: general_id.clone(),
                content: content.into(),
                ..Default::default()
            },
        )
    };
    messages.send_message(send("hello dice")).await.unwrap();
    let event = endpoint.next("messageCreated").await;
    assert_eq!(event["serverId"], server_id.as_str());
    assert_eq!(event["messageCreated"]["message"]["content"], "hello dice");

    // A run comes with its arguments, and the answer posts the reply.
    let interaction_id = commands
        .run_command(authed(
            &owner,
            pb::RunCommandRequest {
                server_id: server_id.clone(),
                channel_id: general_id.clone(),
                agent_id: agent_id.clone(),
                command: "roll".into(),
                arguments: vec![],
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .interaction_id;
    let event = endpoint.next("interactionCreated").await;
    assert_eq!(event["interactionCreated"]["interaction"]["id"], interaction_id.as_str());
    let reply = endpoint.next("messageCreated").await;
    assert_eq!(reply["messageCreated"]["message"]["content"], "rolled 4");
    assert_eq!(reply["messageCreated"]["message"]["authorId"], agent_id.as_str());
    assert_eq!(reply["messageCreated"]["message"]["interaction"]["id"], interaction_id.as_str());

    // Down, the endpoint's owner sees why; back up, nothing was lost.
    endpoint.down.store(true, Ordering::SeqCst);
    messages.send_message(send("while you were out")).await.unwrap();
    let failing = loop {
        let got = agents
            .get_agent_endpoint(authed(&owner, pb::GetAgentEndpointRequest { agent_id: agent_id.clone() }))
            .await
            .unwrap()
            .into_inner()
            .endpoint
            .unwrap();
        if got.failing_since.is_some() {
            break got;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(failing.last_error, "answered 500");
    assert!(failing.last_delivered_at.is_some());
    endpoint.down.store(false, Ordering::SeqCst);
    let event = endpoint.next("messageCreated").await;
    assert_eq!(event["messageCreated"]["message"]["content"], "while you were out");

    // A new secret is used at once.
    let reset = agents
        .reset_agent_endpoint_secret(authed(&owner, pb::ResetAgentEndpointSecretRequest { agent_id: agent_id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .endpoint
        .unwrap();
    assert_ne!(reset.secret, got.secret);
    *endpoint.secret.lock().unwrap() = reset.secret;
    messages.send_message(send("new secret")).await.unwrap();
    let event = endpoint.next("messageCreated").await;
    assert_eq!(event["messageCreated"]["message"]["content"], "new secret");

    // Turned off, nothing more comes.
    agents.set_agent_endpoint(set(String::new(), &[])).await.unwrap();
    messages.send_message(send("anyone there?")).await.unwrap();
    let quiet = tokio::time::timeout(Duration::from_secs(2), endpoint.deliveries.recv()).await;
    assert!(quiet.is_err(), "a delivery came after the endpoint was turned off");

    // Back on, it starts from what comes after: not what it missed.
    agents.set_agent_endpoint(set(endpoint.url.clone(), &["message_created"])).await.unwrap();
    messages.send_message(send("welcome back")).await.unwrap();
    let event = endpoint.next("messageCreated").await;
    assert_eq!(event["messageCreated"]["message"]["content"], "welcome back");

    instance.stop().await;
}

#[tokio::test]
async fn endpoints_stay_on_public_addresses_unless_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let channel = instance.channel.clone();
    let mut auth = pb::auth_service_client::AuthServiceClient::new(channel.clone());
    let mut agents = pb::agent_service_client::AgentServiceClient::new(channel.clone());
    let owner = auth
        .sign_up(pb::SignUpRequest {
            username: "juan".into(),
            password: "correct horse battery".into(),
            display_name: String::new(),
        })
        .await
        .unwrap()
        .into_inner()
        .token;
    let made = agents
        .create_agent(authed(&owner, pb::CreateAgentRequest { username: "dice".into(), display_name: "Dice".into() }))
        .await
        .unwrap()
        .into_inner();
    let agent_id = made.agent.unwrap().user.unwrap().id;
    let endpoint = endpoint().await;
    let set = agents
        .set_agent_endpoint(authed(
            &owner,
            pb::SetAgentEndpointRequest { agent_id, url: endpoint.url.clone(), events: vec![] },
        ))
        .await;
    assert_eq!(set.unwrap_err().code(), Code::InvalidArgument);
    instance.stop().await;
}

/// Message contents the endpoint hears, until it has `count` of them.
async fn heard(endpoint: &mut Endpoint, count: usize) {
    let mut heard = 0;
    while heard < count {
        let delivery = tokio::time::timeout(Duration::from_secs(20), endpoint.deliveries.recv())
            .await
            .expect("nothing came")
            .unwrap();
        heard += delivery["events"].as_array().unwrap().iter().filter(|e| e.get("messageCreated").is_some()).count();
    }
}

#[tokio::test]
async fn deliveries_in_flight_are_bounded_by_agent_and_by_owner() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[("FUWA_AGENT_ENDPOINTS", "any")]).await;
    let channel = instance.channel.clone();
    let mut auth = pb::auth_service_client::AuthServiceClient::new(channel.clone());
    let mut servers = pb::server_service_client::ServerServiceClient::new(channel.clone());
    let mut channels = pb::channel_service_client::ChannelServiceClient::new(channel.clone());
    let mut messages = pb::message_service_client::MessageServiceClient::new(channel.clone());
    let mut agents = pb::agent_service_client::AgentServiceClient::new(channel.clone());
    let owner = auth
        .sign_up(pb::SignUpRequest {
            username: "juan".into(),
            password: "correct horse battery".into(),
            display_name: String::new(),
        })
        .await
        .unwrap()
        .into_inner()
        .token;
    let mut places = Vec::new();
    for name in ["Waifu Devs", "Rin's Room"] {
        let server_id = servers
            .create_server(authed(&owner, pb::CreateServerRequest { name: name.into(), ..Default::default() }))
            .await
            .unwrap()
            .into_inner()
            .server
            .unwrap()
            .id;
        let general_id = channels
            .list_channels(authed(&owner, pb::ListChannelsRequest { server_id: server_id.clone() }))
            .await
            .unwrap()
            .into_inner()
            .channels
            .into_iter()
            .find(|c| c.name == "general")
            .unwrap()
            .id;
        places.push((server_id, general_id));
    }
    // Six agents of one owner, each in both servers.
    let mut endpoint = endpoint().await;
    endpoint.slow.store(true, Ordering::SeqCst);
    let mut ids = Vec::new();
    for n in 0..6 {
        let username = format!("dice{n}");
        let made = agents
            .create_agent(authed(
                &owner,
                pb::CreateAgentRequest { username: username.clone(), display_name: username.clone() },
            ))
            .await
            .unwrap()
            .into_inner();
        let agent_id = made.agent.unwrap().user.unwrap().id;
        for (server_id, _) in &places {
            let add = pb::AddAgentRequest { server_id: server_id.clone(), username: username.clone() };
            agents.add_agent(authed(&owner, add)).await.unwrap();
        }
        let secret = agents
            .get_agent_endpoint(authed(&owner, pb::GetAgentEndpointRequest { agent_id: agent_id.clone() }))
            .await
            .unwrap()
            .into_inner()
            .endpoint
            .unwrap()
            .secret;
        endpoint.others.lock().unwrap().push(secret);
        ids.push(agent_id);
    }
    let url = endpoint.url.clone();
    let point = |agent_id: &str| {
        authed(
            &owner,
            pb::SetAgentEndpointRequest {
                agent_id: agent_id.to_string(),
                url: url.clone(),
                events: vec!["message_created".into()],
            },
        )
    };
    let mut send_round = async |round: usize| {
        for (server_id, general_id) in &places {
            let request = pb::SendMessageRequest {
                server_id: server_id.clone(),
                channel_id: general_id.clone(),
                content: format!("round {round}"),
                ..Default::default()
            };
            messages.send_message(authed(&owner, request)).await.unwrap();
        }
    };

    // One agent pointed at a slow endpoint gets its servers' deliveries one
    // after another, never two at once.
    agents.set_agent_endpoint(point(&ids[0])).await.unwrap();
    send_round(0).await;
    send_round(1).await;
    heard(&mut endpoint, 4).await;
    assert_eq!(endpoint.most_at_once.load(Ordering::SeqCst), 1);

    // All six of the owner's agents there have at most four at once.
    for agent_id in &ids[1..] {
        agents.set_agent_endpoint(point(agent_id)).await.unwrap();
    }
    endpoint.most_at_once.store(0, Ordering::SeqCst);
    send_round(2).await;
    send_round(3).await;
    heard(&mut endpoint, 6 * 4).await;
    assert_eq!(endpoint.most_at_once.load(Ordering::SeqCst), 4);

    instance.stop().await;
}
