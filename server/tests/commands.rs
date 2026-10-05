//! Slash commands and buttons: an agent sets commands, people run them and
//! press its buttons, and only that agent hears about it.

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

/// A person with a server, an agent of theirs in it, and a second person.
struct World {
    owner: String,
    owner_id: String,
    agent: String,
    agent_id: String,
    server_id: String,
    general_id: String,
    other_id: String,
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
    let other_id = channels
        .create_channel(authed(
            &owner,
            pb::CreateChannelRequest {
                server_id: server.id.clone(),
                name: "other".into(),
                r#type: pb::ChannelType::Text as i32,
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .channel
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
        .add_agent(authed(&owner, pb::AddAgentRequest { server_id: server.id.clone(), username: "helper".into() }))
        .await
        .unwrap();
    World { owner, owner_id, agent: made.token, agent_id, server_id: server.id, general_id, other_id }
}

fn roll() -> pb::Command {
    pb::Command {
        name: "roll".into(),
        description: "Rolls dice".into(),
        options: vec![
            pb::CommandOption {
                name: "sides".into(),
                description: "How many sides".into(),
                r#type: pb::CommandOptionType::Integer as i32,
                required: true,
                ..Default::default()
            },
            pb::CommandOption {
                name: "colour".into(),
                description: "The dice's colour".into(),
                choices: vec!["red".into(), "blue".into()],
                ..Default::default()
            },
        ],
    }
}

/// The next event that isn't a heartbeat.
async fn next(stream: &mut Streaming<pb::SubscribeResponse>) -> pb::SubscribeResponse {
    loop {
        let item = tokio::time::timeout(Duration::from_secs(10), stream.message()).await.expect("nothing came");
        match item.unwrap() {
            Some(response) if response == pb::SubscribeResponse::default() => continue,
            Some(response) => return response,
            None => panic!("the stream ended"),
        }
    }
}

fn interaction_of(response: &pb::SubscribeResponse) -> Option<&pb::Interaction> {
    match response.event.as_ref()?.payload.as_ref()? {
        pb::event::Payload::InteractionCreated(created) => created.interaction.as_ref(),
        _ => None,
    }
}

#[tokio::test]
async fn people_run_an_agents_commands_and_only_the_agent_hears() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let channel = instance.channel.clone();
    let w = world(&channel).await;
    let mut commands = pb::command_service_client::CommandServiceClient::new(channel.clone());

    // Only agents set commands.
    let refused = commands
        .set_commands(authed(
            &w.owner,
            pb::SetCommandsRequest { server_id: w.server_id.clone(), commands: vec![roll()] },
        ))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Code::PermissionDenied);
    commands
        .set_commands(authed(
            &w.agent,
            pb::SetCommandsRequest { server_id: w.server_id.clone(), commands: vec![roll()] },
        ))
        .await
        .unwrap();
    let listed = commands
        .list_commands(authed(&w.owner, pb::ListCommandsRequest { server_id: w.server_id.clone() }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(listed.commands.len(), 1);
    assert_eq!(listed.commands[0].agent_id, w.agent_id);
    assert_eq!(listed.agents[0].id, w.agent_id);

    let mut events = pb::event_service_client::EventServiceClient::new(channel.clone());
    let cursor = vec![pb::ServerCursor { server_id: w.server_id.clone(), after_sequence: None }];
    let mut agent_stream = events
        .subscribe(authed(&w.agent, pb::SubscribeRequest { servers: cursor.clone(), ..Default::default() }))
        .await
        .unwrap()
        .into_inner();
    assert!(next(&mut agent_stream).await.ready.is_some());

    // Checked against the options before anything happens.
    let run = |arguments: Vec<(&str, &str)>| {
        authed(
            &w.owner,
            pb::RunCommandRequest {
                server_id: w.server_id.clone(),
                channel_id: w.general_id.clone(),
                agent_id: w.agent_id.clone(),
                command: "roll".into(),
                arguments: arguments
                    .into_iter()
                    .map(|(name, value)| pb::CommandArgument { name: name.into(), value: value.into() })
                    .collect(),
            },
        )
    };
    for bad in
        [vec![], vec![("sides", "six")], vec![("sides", "6"), ("colour", "green")], vec![("sides", "6"), ("x", "1")]]
    {
        assert_eq!(commands.run_command(run(bad)).await.unwrap_err().code(), Code::InvalidArgument);
    }
    let id =
        commands.run_command(run(vec![("sides", "6"), ("colour", "red")])).await.unwrap().into_inner().interaction_id;
    let got = loop {
        let response = next(&mut agent_stream).await;
        if let Some(interaction) = interaction_of(&response) {
            break interaction.clone();
        }
    };
    assert_eq!(got.id, id);
    assert_eq!(got.user_id, w.owner_id);
    assert_eq!(got.command, "roll");
    assert_eq!(got.arguments.len(), 2);

    // Nobody else ever sees it, live or catching up.
    let owner_log = events
        .list_events(authed(&w.owner, pb::ListEventsRequest { server_id: w.server_id.clone(), ..Default::default() }))
        .await
        .unwrap()
        .into_inner()
        .events;
    assert!(owner_log.iter().all(|e| !matches!(e.payload, Some(pb::event::Payload::InteractionCreated(_)))));
    let agent_log = events
        .list_events(authed(&w.agent, pb::ListEventsRequest { server_id: w.server_id.clone(), ..Default::default() }))
        .await
        .unwrap()
        .into_inner()
        .events;
    let typed = |log: &[pb::Event]| {
        log.iter()
            .find_map(|e| match &e.payload {
                Some(pb::event::Payload::InteractionCreated(c)) => c.interaction.as_ref().map(|i| i.arguments.len()),
                _ => None,
            })
            .expect("the agent's interaction")
    };
    // While it can be answered, catching up brings what was typed too.
    assert_eq!(typed(&agent_log), 2);

    // Answered only by its agent, only in its own channel.
    let mut messages = pb::message_service_client::MessageServiceClient::new(channel.clone());
    let answer = |token: &str, channel_id: &str| {
        authed(
            token,
            pb::SendMessageRequest {
                server_id: w.server_id.clone(),
                channel_id: channel_id.into(),
                content: "You rolled a 4".into(),
                interaction_id: id.clone(),
                ..Default::default()
            },
        )
    };
    assert_eq!(
        messages.send_message(answer(&w.owner, &w.general_id)).await.unwrap_err().code(),
        Code::PermissionDenied
    );
    assert_eq!(messages.send_message(answer(&w.agent, &w.other_id)).await.unwrap_err().code(), Code::InvalidArgument);
    let sent = messages.send_message(answer(&w.agent, &w.general_id)).await.unwrap().into_inner().message.unwrap();
    let shown = sent.interaction.unwrap();
    assert_eq!((shown.command.as_str(), shown.user_id.as_str()), ("roll", w.owner_id.as_str()));

    // What was typed lives only with the interaction: once it's gone (here
    // with the agent, kicked and added back), the logged event has none.
    let mut servers = pb::server_service_client::ServerServiceClient::new(channel.clone());
    servers
        .kick_member(authed(
            &w.owner,
            pb::KickMemberRequest {
                server_id: w.server_id.clone(),
                user_id: w.agent_id.clone(),
                reason: String::new(),
            },
        ))
        .await
        .unwrap();
    let mut agents = pb::agent_service_client::AgentServiceClient::new(channel.clone());
    agents
        .add_agent(authed(&w.owner, pb::AddAgentRequest { server_id: w.server_id.clone(), username: "helper".into() }))
        .await
        .unwrap();
    let replayed = events
        .list_events(authed(&w.agent, pb::ListEventsRequest { server_id: w.server_id.clone(), ..Default::default() }))
        .await
        .unwrap()
        .into_inner()
        .events;
    assert_eq!(typed(&replayed), 0);

    instance.stop().await;
}

#[tokio::test]
async fn pressing_an_agents_button_tells_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[("FUWA_LIMIT_COMMANDS_PER_MINUTE", "2")]).await;
    let channel = instance.channel.clone();
    let w = world(&channel).await;
    let mut messages = pb::message_service_client::MessageServiceClient::new(channel.clone());
    let buttons = vec![pb::ComponentRow {
        buttons: vec![
            pb::Button { custom_id: "yes".into(), label: "Yes".into(), ..Default::default() },
            pb::Button {
                label: "Docs".into(),
                style: pb::ButtonStyle::Link as i32,
                url: "https://waifu.dev".into(),
                ..Default::default()
            },
        ],
    }];
    let send = |token: &str| {
        authed(
            token,
            pb::SendMessageRequest {
                server_id: w.server_id.clone(),
                channel_id: w.general_id.clone(),
                content: "Ready?".into(),
                components: buttons.clone(),
                ..Default::default()
            },
        )
    };
    // Only agents send buttons.
    assert_eq!(messages.send_message(send(&w.owner)).await.unwrap_err().code(), Code::PermissionDenied);
    let sent = messages.send_message(send(&w.agent)).await.unwrap().into_inner().message.unwrap();
    assert_eq!(sent.components[0].buttons.len(), 2);

    let mut commands = pb::command_service_client::CommandServiceClient::new(channel.clone());
    let press = |custom_id: &str| {
        authed(
            &w.owner,
            pb::PressButtonRequest {
                server_id: w.server_id.clone(),
                message_id: sent.id.clone(),
                custom_id: custom_id.into(),
            },
        )
    };
    assert_eq!(commands.press_button(press("no")).await.unwrap_err().code(), Code::NotFound);
    commands.press_button(press("yes")).await.unwrap();
    let agent_log = pb::event_service_client::EventServiceClient::new(channel.clone())
        .list_events(authed(&w.agent, pb::ListEventsRequest { server_id: w.server_id.clone(), ..Default::default() }))
        .await
        .unwrap()
        .into_inner()
        .events;
    let pressed = agent_log
        .iter()
        .find_map(|e| match &e.payload {
            Some(pb::event::Payload::InteractionCreated(c)) => c.interaction.clone(),
            _ => None,
        })
        .unwrap();
    assert_eq!((pressed.message_id.as_str(), pressed.custom_id.as_str()), (sent.id.as_str(), "yes"));

    // The instance's cap (2 here, counting the miss) is per account, and says how long to wait.
    let limited = commands.press_button(press("yes")).await.unwrap_err();
    assert_eq!(limited.code(), Code::ResourceExhausted);
    assert!(limited.metadata().get("fuwa-retry-after-ms").is_some());

    instance.stop().await;
}
