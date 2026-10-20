//! Ephemeral messages: an agent sends a message only one person sees, which
//! reaches only them, live, and is never kept.

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

/// A person with a server, an agent of theirs in it.
struct World {
    owner: String,
    owner_id: String,
    agent: String,
    agent_id: String,
    server_id: String,
    general_id: String,
}

async fn world(channel: &Channel) -> World {
    let mut auth = pb::auth_service_client::AuthServiceClient::new(channel.clone());
    let signed_up = auth
        .sign_up(pb::SignUpRequest {
            username: "juan".into(),
            password: "correct horse battery".into(),
            display_name: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    let owner = signed_up.token;
    let owner_id = signed_up.user.unwrap().id;
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
    agents
        .add_agent(authed(
            &owner,
            pb::AddAgentRequest { server_id: server.id.clone(), username: "helper".into(), ..Default::default() },
        ))
        .await
        .unwrap();
    World { owner, owner_id, agent: made.token, agent_id, server_id: server.id, general_id }
}

/// The next event that isn't a heartbeat, or none within a second.
async fn next_within(stream: &mut Streaming<pb::SubscribeResponse>, wait: Duration) -> Option<pb::SubscribeResponse> {
    loop {
        let item = tokio::time::timeout(wait, stream.message()).await.ok()?;
        match item.unwrap() {
            Some(response) if response == pb::SubscribeResponse::default() => continue,
            Some(response) => return Some(response),
            None => panic!("the stream ended"),
        }
    }
}

async fn next(stream: &mut Streaming<pb::SubscribeResponse>) -> pb::SubscribeResponse {
    next_within(stream, Duration::from_secs(10)).await.expect("nothing came")
}

fn payload_of(response: &pb::SubscribeResponse) -> Option<&pb::event::Payload> {
    response.event.as_ref()?.payload.as_ref()
}

#[tokio::test]
async fn an_agents_ephemeral_message_reaches_only_its_person_and_is_never_kept() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let channel = instance.channel.clone();
    let w = world(&channel).await;

    // A second person, in through an invite.
    let mut auth = pb::auth_service_client::AuthServiceClient::new(channel.clone());
    let ana = auth
        .sign_up(pb::SignUpRequest {
            username: "ana".into(),
            password: "correct horse battery".into(),
            display_name: String::new(),
        })
        .await
        .unwrap()
        .into_inner()
        .token;
    let code = pb::invite_service_client::InviteServiceClient::new(channel.clone())
        .create_invite(authed(
            &w.owner,
            pb::CreateInviteRequest { server_id: w.server_id.clone(), ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .invite
        .unwrap()
        .code;
    pb::server_service_client::ServerServiceClient::new(channel.clone())
        .join_server(authed(&ana, pb::JoinServerRequest { server_id: w.server_id.clone(), invite_code: code }))
        .await
        .unwrap();

    let mut events = pb::event_service_client::EventServiceClient::new(channel.clone());
    let cursor = vec![pb::ServerCursor { server_id: w.server_id.clone(), after_sequence: None }];
    let mut subscribe = async |token: &str| {
        let mut stream = events
            .subscribe(authed(token, pb::SubscribeRequest { servers: cursor.clone(), ..Default::default() }))
            .await
            .unwrap()
            .into_inner();
        assert!(next(&mut stream).await.ready.is_some());
        stream
    };
    let mut owner_stream = subscribe(&w.owner).await;
    let mut ana_stream = subscribe(&ana).await;

    let mut messages = pb::message_service_client::MessageServiceClient::new(channel.clone());
    let ephemeral = |token: &str, user_id: &str, seconds: u32| {
        authed(
            token,
            pb::SendEphemeralMessageRequest {
                server_id: w.server_id.clone(),
                channel_id: w.general_id.clone(),
                user_id: user_id.into(),
                content: "Only for you".into(),
                expires_in_seconds: seconds,
                ..Default::default()
            },
        )
    };
    // Only agents send them, only to people, only for an hour at most.
    let refused = messages.send_ephemeral_message(ephemeral(&w.owner, &w.owner_id, 0)).await.unwrap_err();
    assert_eq!(refused.code(), Code::PermissionDenied);
    let refused = messages.send_ephemeral_message(ephemeral(&w.agent, &w.agent_id, 0)).await.unwrap_err();
    assert_eq!(refused.code(), Code::InvalidArgument);
    let refused = messages.send_ephemeral_message(ephemeral(&w.agent, "nobody", 0)).await.unwrap_err();
    assert_eq!(refused.code(), Code::NotFound);
    let refused = messages.send_ephemeral_message(ephemeral(&w.agent, &w.owner_id, 3601)).await.unwrap_err();
    assert_eq!(refused.code(), Code::InvalidArgument);

    let sent = messages
        .send_ephemeral_message(ephemeral(&w.agent, &w.owner_id, 0))
        .await
        .unwrap()
        .into_inner()
        .message
        .unwrap();
    let info = sent.ephemeral.clone().unwrap();
    assert_eq!(info.user_id, w.owner_id);
    let lifetime = info.expires_at.unwrap().seconds - sent.created_at.unwrap().seconds;
    assert!((899..=901).contains(&lifetime), "15 minutes unless asked: {lifetime}");

    // The owner gets it, not stored (sequence 0).
    let got = loop {
        let response = next(&mut owner_stream).await;
        if let Some(pb::event::Payload::EphemeralMessageCreated(created)) = payload_of(&response) {
            assert_eq!(response.event.as_ref().unwrap().sequence, 0);
            break created.message.clone().unwrap();
        }
    };
    assert_eq!(got, sent);

    // Ana gets the agent's next ordinary message, and never the ephemeral one.
    let plain = messages
        .send_message(authed(
            &w.agent,
            pb::SendMessageRequest {
                server_id: w.server_id.clone(),
                channel_id: w.general_id.clone(),
                content: "For everyone".into(),
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .message
        .unwrap();
    loop {
        let response = next(&mut ana_stream).await;
        match payload_of(&response) {
            Some(pb::event::Payload::EphemeralMessageCreated(_)) => panic!("ana saw someone else's ephemeral message"),
            Some(pb::event::Payload::MessageCreated(created)) if created.message.as_ref().unwrap().id == plain.id => {
                break;
            }
            _ => {}
        }
    }

    // It's in no list and no log.
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
    assert!(listed.iter().all(|m| m.id != sent.id && m.ephemeral.is_none()));
    let log = events
        .list_events(authed(&w.owner, pb::ListEventsRequest { server_id: w.server_id.clone(), ..Default::default() }))
        .await
        .unwrap()
        .into_inner()
        .events;
    assert!(log.iter().all(|e| !matches!(e.payload, Some(pb::event::Payload::EphemeralMessageCreated(_)))));

    // An answer to a command goes to whoever ran it, and to nobody else.
    let mut commands = pb::command_service_client::CommandServiceClient::new(channel.clone());
    commands
        .set_commands(authed(
            &w.agent,
            pb::SetCommandsRequest {
                server_id: w.server_id.clone(),
                commands: vec![pb::Command {
                    name: "secret".into(),
                    description: "Tells you a secret".into(),
                    ..Default::default()
                }],
            },
        ))
        .await
        .unwrap();
    let interaction_id = commands
        .run_command(authed(
            &ana,
            pb::RunCommandRequest {
                server_id: w.server_id.clone(),
                channel_id: w.general_id.clone(),
                agent_id: w.agent_id.clone(),
                command: "secret".into(),
                arguments: vec![],
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .interaction_id;
    let answer = |user_id: &str| {
        authed(
            &w.agent,
            pb::SendEphemeralMessageRequest {
                server_id: w.server_id.clone(),
                channel_id: w.general_id.clone(),
                user_id: user_id.into(),
                content: "psst".into(),
                interaction_id: interaction_id.clone(),
                expires_in_seconds: 60,
                ..Default::default()
            },
        )
    };
    let refused = messages.send_ephemeral_message(answer(&w.owner_id)).await.unwrap_err();
    assert_eq!(refused.code(), Code::InvalidArgument);
    let answered = messages.send_ephemeral_message(answer("")).await.unwrap().into_inner().message.unwrap();
    let ana_id = answered.ephemeral.as_ref().unwrap().user_id.clone();
    assert_ne!(ana_id, w.owner_id);
    assert_eq!(answered.interaction.as_ref().unwrap().user_id, ana_id);
    assert_eq!(answered.interaction.as_ref().unwrap().command, "secret");
    loop {
        let response = next(&mut ana_stream).await;
        if let Some(pb::event::Payload::EphemeralMessageCreated(created)) = payload_of(&response) {
            assert_eq!(created.message.as_ref().unwrap().id, answered.id);
            break;
        }
    }
    while let Some(response) = next_within(&mut owner_stream, Duration::from_millis(500)).await {
        if let Some(pb::event::Payload::EphemeralMessageCreated(created)) = payload_of(&response) {
            assert_ne!(created.message.as_ref().unwrap().id, answered.id, "the owner saw ana's answer");
        }
    }

    instance.stop().await;
}
