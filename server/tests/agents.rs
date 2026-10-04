//! What programs driving an agent lean on: a stream that picks up servers the
//! agent is added to, streams that end the moment its token is reset,
//! mentions as ids, and limits that say how long to wait.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fuwa_server::app::App;
use fuwa_server::config::Config;
use fuwa_server::pb;
use tokio::task::JoinHandle;
use tonic::transport::Channel;
use tonic::{Code, Request, Streaming};

struct Instance {
    app: Arc<App>,
    channel: Channel,
    serving: JoinHandle<()>,
}

async fn start(dir: &Path) -> Instance {
    let dir = dir.to_str().unwrap().to_string();
    let config = Config::from_lookup(|key| match key {
        "FUWA_DATA_PATH" => Some(dir.clone()),
        "FUWA_TELEMETRY" => Some("off".into()),
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

/// A person with a server, and an agent of theirs that isn't in it yet.
struct World {
    owner: String,
    agent: String,
    agent_id: String,
    server_id: String,
    general_id: String,
}

async fn world(channel: &Channel) -> World {
    let mut auth = pb::auth_service_client::AuthServiceClient::new(channel.clone());
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
    let mut servers = pb::server_service_client::ServerServiceClient::new(channel.clone());
    let server = servers
        .create_server(authed(&owner, pb::CreateServerRequest { name: "Waifu Devs".into(), ..Default::default() }))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap();
    let mut channels = pb::channel_service_client::ChannelServiceClient::new(channel.clone());
    let general_id = channels
        .list_channels(authed(&owner, pb::ListChannelsRequest { server_id: server.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels
        .into_iter()
        .find(|c| c.name == "general")
        .unwrap()
        .id;
    let mut agents = pb::agent_service_client::AgentServiceClient::new(channel.clone());
    let made = agents
        .create_agent(authed(
            &owner,
            pb::CreateAgentRequest { username: "helper".into(), display_name: "Helper".into() },
        ))
        .await
        .unwrap()
        .into_inner();
    let agent_id = made.agent.unwrap().user.unwrap().id;
    World { owner, agent: made.token, agent_id, server_id: server.id, general_id }
}

async fn add_agent(channel: &Channel, w: &World) {
    pb::agent_service_client::AgentServiceClient::new(channel.clone())
        .add_agent(authed(&w.owner, pb::AddAgentRequest { server_id: w.server_id.clone(), username: "helper".into() }))
        .await
        .unwrap();
}

/// The next response that isn't a heartbeat, or the stream's error.
async fn next(stream: &mut Streaming<pb::SubscribeResponse>) -> Result<pb::SubscribeResponse, tonic::Status> {
    loop {
        let item = tokio::time::timeout(Duration::from_secs(10), stream.message()).await.expect("nothing came");
        match item? {
            Some(response) if response == pb::SubscribeResponse::default() => continue,
            Some(response) => return Ok(response),
            None => panic!("the stream ended"),
        }
    }
}

fn created(response: &pb::SubscribeResponse) -> Option<&pb::Message> {
    match response.event.as_ref()?.payload.as_ref()? {
        pb::event::Payload::MessageCreated(created) => created.message.as_ref(),
        _ => None,
    }
}

#[tokio::test]
async fn a_stream_follows_servers_its_agent_is_added_to_and_ends_when_its_token_is_reset() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let channel = instance.channel.clone();
    let w = world(&channel).await;
    let mut events = pb::event_service_client::EventServiceClient::new(channel.clone());

    // Following nothing is only for streams that follow new servers.
    let refused = events.subscribe(authed(&w.agent, pb::SubscribeRequest::default())).await.unwrap_err();
    assert_eq!(refused.code(), Code::InvalidArgument);

    let mut stream = events
        .subscribe(authed(&w.agent, pb::SubscribeRequest { follow_new_servers: true, ..Default::default() }))
        .await
        .unwrap()
        .into_inner();
    let ready = next(&mut stream).await.unwrap().ready.expect("ready first");
    assert!(ready.servers.is_empty());

    // Added: followed from then on, without subscribing again.
    add_agent(&channel, &w).await;
    let followed = next(&mut stream).await.unwrap().followed.expect("the server it was added to");
    assert_eq!(followed.server_id, w.server_id);
    let mut messages = pb::message_service_client::MessageServiceClient::new(channel.clone());
    messages
        .send_message(authed(
            &w.owner,
            pb::SendMessageRequest {
                server_id: w.server_id.clone(),
                channel_id: w.general_id.clone(),
                content: "welcome aboard".into(),
                ..Default::default()
            },
        ))
        .await
        .unwrap();
    let message = loop {
        let response = next(&mut stream).await.unwrap();
        if let Some(message) = created(&response) {
            break message.clone();
        }
    };
    assert_eq!(message.content, "welcome aboard");

    // A token reset ends it at once, long before the next heartbeat.
    let mut agents = pb::agent_service_client::AgentServiceClient::new(channel.clone());
    let started = std::time::Instant::now();
    agents
        .reset_agent_token(authed(&w.owner, pb::ResetAgentTokenRequest { agent_id: w.agent_id.clone() }))
        .await
        .unwrap();
    let ended = loop {
        match next(&mut stream).await {
            Ok(_) => continue,
            Err(status) => break status,
        }
    };
    assert_eq!(ended.code(), Code::Unauthenticated);
    assert!(started.elapsed() < Duration::from_secs(5), "ended at the reset, not a heartbeat");

    instance.stop().await;
}

#[tokio::test]
async fn messages_name_their_mentions_and_slow_mode_says_how_long_to_wait() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let channel = instance.channel.clone();
    let w = world(&channel).await;
    add_agent(&channel, &w).await;
    let mut messages = pb::message_service_client::MessageServiceClient::new(channel.clone());
    let send = |token: &str, content: &str| {
        authed(
            token,
            pb::SendMessageRequest {
                server_id: w.server_id.clone(),
                channel_id: w.general_id.clone(),
                content: content.into(),
                ..Default::default()
            },
        )
    };

    // Members named once each, in order; anyone else left out.
    let content = format!("<@{0}> and <@!{0}>, and <@01J0000000000000000000NOPE>", w.agent_id);
    let sent = messages.send_message(send(&w.owner, &content)).await.unwrap().into_inner().message.unwrap();
    assert_eq!(sent.mention_user_ids, [w.agent_id.as_str()]);
    let listed = messages
        .list_messages(authed(
            &w.owner,
            pb::ListMessagesRequest {
                server_id: w.server_id.clone(),
                channel_id: w.general_id.clone(),
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .messages;
    let stored = listed.iter().find(|m| m.id == sent.id).unwrap();
    assert_eq!(stored.mention_user_ids, [w.agent_id.as_str()], "kept with the message");
    let edited = messages
        .update_message(authed(
            &w.owner,
            pb::UpdateMessageRequest {
                server_id: w.server_id.clone(),
                message_id: sent.id.clone(),
                content: "nobody now".into(),
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .message
        .unwrap();
    assert!(edited.mention_user_ids.is_empty(), "an edit names them again");

    // Slow mode: the second message is refused with how long to wait.
    pb::channel_service_client::ChannelServiceClient::new(channel.clone())
        .update_channel(authed(
            &w.owner,
            pb::UpdateChannelRequest {
                server_id: w.server_id.clone(),
                channel_id: w.general_id.clone(),
                slowmode_seconds: Some(30),
                ..Default::default()
            },
        ))
        .await
        .unwrap();
    messages.send_message(send(&w.agent, "one")).await.unwrap();
    let slowed = messages.send_message(send(&w.agent, "two")).await.unwrap_err();
    assert_eq!(slowed.code(), Code::ResourceExhausted);
    let wait: i64 = slowed.metadata().get("fuwa-retry-after-ms").unwrap().to_str().unwrap().parse().unwrap();
    assert!((1..=30_000).contains(&wait), "{wait}");

    instance.stop().await;
}
