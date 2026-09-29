//! End-to-end tests: a real instance on a local port, driven through the
//! generated gRPC clients (and raw gRPC-Web, as a browser would).

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fuwa_server::app::App;
use fuwa_server::config::Config;
use fuwa_server::pb;
use fuwa_server::pb::event::Payload;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;
use tonic::transport::Channel;
use tonic::{Code, Request};

const ADMIN_TOKEN: &str = "test-admin-token-0123456789abcdef0123456789";

struct Instance {
    app: Arc<App>,
    addr: SocketAddr,
    serving: JoinHandle<()>,
}

async fn start(dir: &Path, vars: &[(&str, &str)]) -> Instance {
    let dir = dir.to_str().unwrap().to_string();
    let vars: Vec<(String, String)> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let config = Config::from_lookup(|key| match key {
        "FUWA_DATA_PATH" => Some(dir.clone()),
        "FUWA_TELEMETRY" => Some("off".into()),
        "FUWA_ADMIN_TOKEN" => Some(ADMIN_TOKEN.into()),
        _ => vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()),
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
    async fn channel(&self) -> Channel {
        Channel::from_shared(format!("http://{}", self.addr)).unwrap().connect().await.unwrap()
    }

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

struct Clients {
    auth: pb::auth_service_client::AuthServiceClient<Channel>,
    account: pb::account_service_client::AccountServiceClient<Channel>,
    servers: pb::server_service_client::ServerServiceClient<Channel>,
    channels: pb::channel_service_client::ChannelServiceClient<Channel>,
    messages: pb::message_service_client::MessageServiceClient<Channel>,
    events: pb::event_service_client::EventServiceClient<Channel>,
    admin: pb::admin_service_client::AdminServiceClient<Channel>,
    node: pb::node_service_client::NodeServiceClient<Channel>,
}

async fn clients(instance: &Instance) -> Clients {
    let channel = instance.channel().await;
    Clients {
        auth: pb::auth_service_client::AuthServiceClient::new(channel.clone()),
        account: pb::account_service_client::AccountServiceClient::new(channel.clone()),
        servers: pb::server_service_client::ServerServiceClient::new(channel.clone()),
        channels: pb::channel_service_client::ChannelServiceClient::new(channel.clone()),
        messages: pb::message_service_client::MessageServiceClient::new(channel.clone()),
        events: pb::event_service_client::EventServiceClient::new(channel.clone()),
        admin: pb::admin_service_client::AdminServiceClient::new(channel.clone()),
        node: pb::node_service_client::NodeServiceClient::new(channel),
    }
}

async fn sign_up(c: &mut Clients, username: &str) -> (String, pb::User, bool) {
    let res = c
        .auth
        .sign_up(pb::SignUpRequest {
            username: username.into(),
            password: "correct horse battery".into(),
            display_name: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    (res.token, res.user.unwrap(), res.admin)
}

async fn usage(c: &mut Clients, token: &str, server_id: &str) -> pb::ServerUsage {
    c.servers
        .get_server_usage(authed(token, pb::GetServerUsageRequest { server_id: server_id.into() }))
        .await
        .unwrap()
        .into_inner()
        .usage
        .unwrap()
}

#[tokio::test]
async fn a_community_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[("FUWA_NODE_NAME", "Test instance")]).await;
    let mut c = clients(&instance).await;

    let node = c.node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap();
    assert_eq!(node.name, "Test instance");
    let auth = node.auth.unwrap();
    assert!(auth.local_sign_in && auth.local_sign_up && !auth.linked_sign_in);

    // Accounts: the first one is the instance admin.
    let (juan, juan_user, juan_admin) = sign_up(&mut c, "Juan").await;
    let (mika, mika_user, mika_admin) = sign_up(&mut c, "mika").await;
    assert!(juan_admin && !mika_admin);
    assert_eq!(juan_user.username, "juan");
    let taken = c
        .auth
        .sign_up(pb::SignUpRequest {
            username: "JUAN".into(),
            password: "another password".into(),
            display_name: String::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(taken.code(), Code::AlreadyExists);
    let wrong = c
        .auth
        .sign_in(pb::SignInRequest { username: "juan".into(), password: "wrong password".into() })
        .await
        .unwrap_err();
    assert_eq!(wrong.code(), Code::Unauthenticated);
    let signed_in = c
        .auth
        .sign_in(pb::SignInRequest { username: "juan".into(), password: "correct horse battery".into() })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(signed_in.user.unwrap().id, juan_user.id);
    assert_eq!(c.servers.list_servers(pb::ListServersRequest {}).await.unwrap_err().code(), Code::Unauthenticated);

    // A server, private at first.
    let server = c
        .servers
        .create_server(authed(&juan, pb::CreateServerRequest { name: "Waifu Devs".into(), ..Default::default() }))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap();
    assert_eq!(server.member_count, 1);
    let sid = server.id.clone();
    let channels = c
        .channels
        .list_channels(authed(&juan, pb::ListChannelsRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels;
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].name, "general");
    let general = channels[0].id.clone();

    let hidden =
        c.servers.discover_servers(authed(&mika, pb::DiscoverServersRequest {})).await.unwrap().into_inner().servers;
    assert!(hidden.is_empty());
    let not_yet =
        c.servers.join_server(authed(&mika, pb::JoinServerRequest { server_id: sid.clone() })).await.unwrap_err();
    assert_eq!(not_yet.code(), Code::NotFound);
    let reading = c
        .messages
        .list_messages(authed(
            &mika,
            pb::ListMessagesRequest { server_id: sid.clone(), channel_id: general.clone(), ..Default::default() },
        ))
        .await
        .unwrap_err();
    assert_eq!(reading.code(), Code::PermissionDenied);

    c.servers
        .update_server(authed(
            &juan,
            pb::UpdateServerRequest { server_id: sid.clone(), discoverable: Some(true), ..Default::default() },
        ))
        .await
        .unwrap();
    let found =
        c.servers.discover_servers(authed(&mika, pb::DiscoverServersRequest {})).await.unwrap().into_inner().servers;
    assert_eq!(found.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), [sid.as_str()]);

    // Mika follows the whole log from the start, then joins.
    let mut juan_stream = c
        .events
        .subscribe(authed(
            &juan,
            pb::SubscribeRequest {
                servers: vec![pb::ServerCursor { server_id: sid.clone(), after_sequence: Some(0) }],
            },
        ))
        .await
        .unwrap()
        .into_inner();
    let joined = c
        .servers
        .join_server(authed(&mika, pb::JoinServerRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(joined.server.unwrap().member_count, 2);
    let again =
        c.servers.join_server(authed(&mika, pb::JoinServerRequest { server_id: sid.clone() })).await.unwrap_err();
    assert_eq!(again.code(), Code::AlreadyExists);

    // Only managers make channels.
    let denied = c
        .channels
        .create_channel(authed(
            &mika,
            pb::CreateChannelRequest { server_id: sid.clone(), name: "mine".into(), ..Default::default() },
        ))
        .await
        .unwrap_err();
    assert_eq!(denied.code(), Code::PermissionDenied);
    let memes = c
        .channels
        .create_channel(authed(
            &juan,
            pb::CreateChannelRequest { server_id: sid.clone(), name: "Memes And Stuff".into(), ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .channel
        .unwrap();
    assert_eq!(memes.name, "memes-and-stuff");
    assert_eq!(memes.position, 1);

    // Messages, with paging both ways.
    let mut sent = Vec::new();
    for i in 0..7 {
        let token = if i % 2 == 0 { &juan } else { &mika };
        let message = c
            .messages
            .send_message(authed(
                token,
                pb::SendMessageRequest {
                    server_id: sid.clone(),
                    channel_id: general.clone(),
                    content: format!("message {i}"),
                    ..Default::default()
                },
            ))
            .await
            .unwrap()
            .into_inner()
            .message
            .unwrap();
        sent.push(message);
    }
    let latest = c
        .messages
        .list_messages(authed(
            &mika,
            pb::ListMessagesRequest {
                server_id: sid.clone(),
                channel_id: general.clone(),
                limit: 3,
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        latest.messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(),
        ["message 4", "message 5", "message 6"]
    );
    assert!(latest.has_more);
    let mut authors: Vec<_> = latest.authors.iter().map(|u| u.username.as_str()).collect();
    authors.sort();
    assert_eq!(authors, ["juan", "mika"]);
    let older = c
        .messages
        .list_messages(authed(
            &mika,
            pb::ListMessagesRequest {
                server_id: sid.clone(),
                channel_id: general.clone(),
                limit: 3,
                before_id: latest.messages[0].id.clone(),
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        older.messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(),
        ["message 1", "message 2", "message 3"]
    );
    let newer = c
        .messages
        .list_messages(authed(
            &mika,
            pb::ListMessagesRequest {
                server_id: sid.clone(),
                channel_id: general.clone(),
                limit: 10,
                after_id: sent[4].id.clone(),
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(newer.messages.len(), 2);
    assert!(!newer.has_more);

    let not_yours = c
        .messages
        .update_message(authed(
            &mika,
            pb::UpdateMessageRequest {
                server_id: sid.clone(),
                message_id: sent[0].id.clone(),
                content: "hijacked".into(),
            },
        ))
        .await
        .unwrap_err();
    assert_eq!(not_yours.code(), Code::PermissionDenied);
    let edited = c
        .messages
        .update_message(authed(
            &juan,
            pb::UpdateMessageRequest {
                server_id: sid.clone(),
                message_id: sent[0].id.clone(),
                content: "message zero".into(),
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .message
        .unwrap();
    assert!(edited.edited_at.is_some());
    // The owner can delete anyone's message.
    c.messages
        .delete_message(authed(
            &juan,
            pb::DeleteMessageRequest { server_id: sid.clone(), message_id: sent[1].id.clone() },
        ))
        .await
        .unwrap();

    // Usage follows along.
    let u = usage(&mut c, &juan, &sid).await;
    assert_eq!((u.members, u.channels, u.messages, u.messages_sent), (2, 2, 6, 7));
    let expected_bytes: i64 = ["message zero", "message 2", "message 3", "message 4", "message 5", "message 6"]
        .iter()
        .map(|s| s.len() as i64)
        .sum();
    assert_eq!(u.message_bytes, expected_bytes);
    assert!(u.storage_bytes > 0);
    assert_eq!(
        c.servers
            .get_server_usage(authed(&mika, pb::GetServerUsageRequest { server_id: sid.clone() }))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );

    // Juan's stream replayed everything from the start and kept going live, in order.
    let mut seen = Vec::new();
    let mut ready_after = None;
    while seen.len() < u.events as usize {
        let item = tokio::time::timeout(Duration::from_secs(5), juan_stream.next())
            .await
            .expect("event arrives")
            .unwrap()
            .unwrap();
        if let Some(event) = item.event {
            seen.push(event);
        }
        if let Some(ready) = item.ready {
            ready_after = Some((seen.len() as i64, ready.servers[0].sequence));
        }
    }
    let sequences: Vec<i64> = seen.iter().map(|e| e.sequence).collect();
    assert_eq!(sequences, (1..=u.events).collect::<Vec<_>>());
    // `ready` came right after the replay and names the last replayed event.
    let (replayed, head) = ready_after.expect("ready arrives");
    assert!(replayed >= 1 && replayed == head, "replayed {replayed}, head {head}");
    assert!(matches!(seen[0].payload, Some(Payload::MemberJoined(_))));
    assert!(matches!(seen.last().unwrap().payload, Some(Payload::MessageDeleted(_))));
    assert!(seen.iter().any(|e| matches!(&e.payload, Some(Payload::MemberJoined(j)) if j.member.as_ref().unwrap().user.as_ref().unwrap().id == mika_user.id)));

    // A later subscriber resuming from a sequence gets only what came after it.
    let events = c
        .events
        .list_events(authed(&mika, pb::ListEventsRequest { server_id: sid.clone(), after_sequence: 3, limit: 2 }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(events.events.iter().map(|e| e.sequence).collect::<Vec<_>>(), [4, 5]);
    assert!(events.has_more);

    // Deleting a channel takes its messages off the totals.
    let in_memes = c
        .messages
        .send_message(authed(
            &mika,
            pb::SendMessageRequest {
                server_id: sid.clone(),
                channel_id: memes.id.clone(),
                content: "lol".into(),
                ..Default::default()
            },
        ))
        .await
        .unwrap();
    drop(in_memes);
    c.channels
        .delete_channel(authed(
            &juan,
            pb::DeleteChannelRequest { server_id: sid.clone(), channel_id: memes.id.clone() },
        ))
        .await
        .unwrap();
    let u = usage(&mut c, &juan, &sid).await;
    assert_eq!((u.channels, u.messages, u.messages_sent, u.message_bytes), (1, 6, 8, expected_bytes));

    // Profiles propagate to the servers you're in.
    c.auth
        .update_profile(authed(
            &mika,
            pb::UpdateProfileRequest { display_name: Some("Mika ✨".into()), ..Default::default() },
        ))
        .await
        .unwrap();
    let members = c
        .servers
        .list_members(authed(&juan, pb::ListMembersRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner()
        .members;
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].role, pb::MemberRole::Owner as i32);
    assert_eq!(members[1].user.as_ref().unwrap().display_name, "Mika ✨");

    // Mika leaves; the owner can't.
    c.servers.leave_server(authed(&mika, pb::LeaveServerRequest { server_id: sid.clone() })).await.unwrap();
    assert!(
        c.servers.list_servers(authed(&mika, pb::ListServersRequest {})).await.unwrap().into_inner().servers.is_empty()
    );
    assert_eq!(
        c.servers
            .leave_server(authed(&juan, pb::LeaveServerRequest { server_id: sid.clone() }))
            .await
            .unwrap_err()
            .code(),
        Code::FailedPrecondition
    );

    // Everything survives a restart.
    drop(c);
    drop(juan_stream);
    instance.stop().await;
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let servers = c.servers.list_servers(authed(&juan, pb::ListServersRequest {})).await.unwrap().into_inner().servers;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].member_count, 1);
    let latest = c
        .messages
        .list_messages(authed(
            &juan,
            pb::ListMessagesRequest {
                server_id: sid.clone(),
                channel_id: general.clone(),
                limit: 1,
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(latest.messages[0].content, "message 6");
    let me = c.auth.get_me(authed(&juan, pb::GetMeRequest {})).await.unwrap().into_inner();
    assert!(me.admin);

    // Deleting a server ends its streams and parks the file in deleted/.
    let mut stream = c
        .events
        .subscribe(authed(
            &juan,
            pb::SubscribeRequest { servers: vec![pb::ServerCursor { server_id: sid.clone(), after_sequence: None }] },
        ))
        .await
        .unwrap()
        .into_inner();
    let ready = tokio::time::timeout(Duration::from_secs(5), stream.next()).await.unwrap().unwrap().unwrap();
    assert!(ready.ready.is_some());
    c.servers.delete_server(authed(&juan, pb::DeleteServerRequest { server_id: sid.clone() })).await.unwrap();
    let last = tokio::time::timeout(Duration::from_secs(5), stream.next()).await.unwrap().unwrap().unwrap();
    assert!(matches!(last.event.unwrap().payload, Some(Payload::ServerDeleted(_))));
    assert!(tokio::time::timeout(Duration::from_secs(5), stream.next()).await.unwrap().is_none());
    assert!(!dir.path().join("servers").join(format!("{sid}.db")).exists());
    let parked: Vec<_> = std::fs::read_dir(dir.path().join("deleted")).unwrap().filter_map(|e| e.ok()).collect();
    assert!(parked.iter().any(|e| e.file_name().to_string_lossy().starts_with(&sid)));
    assert_eq!(
        c.servers.get_server(authed(&juan, pb::GetServerRequest { server_id: sid.clone() })).await.unwrap_err().code(),
        Code::NotFound
    );
    drop(mika);
    instance.stop().await;
}

#[tokio::test]
async fn limits_are_unlimited_by_default_and_configurable() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[("FUWA_LIMIT_SERVERS_PER_ACCOUNT", "1"), ("FUWA_LIMIT_CHANNELS", "2")]).await;
    let mut c = clients(&instance).await;
    let (owner, _, _) = sign_up(&mut c, "owner").await;
    let (guest, _, _) = sign_up(&mut c, "guest").await;
    let (late, _, _) = sign_up(&mut c, "late").await;

    let server = c
        .servers
        .create_server(authed(
            &owner,
            pb::CreateServerRequest { name: "Capped".into(), discoverable: true, ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap();
    let second = c
        .servers
        .create_server(authed(&owner, pb::CreateServerRequest { name: "One too many".into(), ..Default::default() }))
        .await
        .unwrap_err();
    assert_eq!(second.code(), Code::ResourceExhausted);

    // Instance default: 2 channels (#general is one).
    let create =
        |name: &str| pb::CreateChannelRequest { server_id: server.id.clone(), name: name.into(), ..Default::default() };
    c.channels.create_channel(authed(&owner, create("two"))).await.unwrap();
    assert_eq!(
        c.channels.create_channel(authed(&owner, create("three"))).await.unwrap_err().code(),
        Code::ResourceExhausted
    );

    // A per-server cap set by the operator (as a hosted control plane would).
    let limits = c
        .admin
        .set_server_limits(authed(
            ADMIN_TOKEN,
            pb::SetServerLimitsRequest {
                server_id: server.id.clone(),
                limits: Some(pb::ServerLimits { members: Some(2), channels: Some(5), ..Default::default() }),
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .limits
        .unwrap();
    assert_eq!((limits.members, limits.channels, limits.storage_bytes), (Some(2), Some(5), None));
    c.channels.create_channel(authed(&owner, create("three"))).await.unwrap();
    c.servers.join_server(authed(&guest, pb::JoinServerRequest { server_id: server.id.clone() })).await.unwrap();
    let full =
        c.servers.join_server(authed(&late, pb::JoinServerRequest { server_id: server.id.clone() })).await.unwrap_err();
    assert_eq!(full.code(), Code::ResourceExhausted);

    // Non-admins can't touch limits; the admin token can read every server's usage.
    assert_eq!(
        c.admin
            .set_server_limits(authed(
                &guest,
                pb::SetServerLimitsRequest { server_id: server.id.clone(), limits: None }
            ))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );
    let node = c.admin.get_node_usage(authed(ADMIN_TOKEN, pb::GetNodeUsageRequest {})).await.unwrap().into_inner();
    assert_eq!((node.accounts, node.servers), (3, 1));
    assert_eq!(node.server_usage[0].members, 2);
    assert_eq!(node.default_limits.unwrap().channels, Some(2));
    assert_eq!(
        c.servers
            .create_server(authed(ADMIN_TOKEN, pb::CreateServerRequest { name: "x".into(), ..Default::default() }))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );

    let signal = fuwa_server::telemetry::collect(&instance.app).await.unwrap();
    assert_eq!(signal.schema, "fuwa.signal.v1");
    assert_eq!(
        (signal.totals.accounts, signal.totals.servers, signal.totals.members, signal.totals.channels),
        (3, 1, 2, 3)
    );
    assert!(signal.config.limits_configured);
    assert_eq!(signal.hosting, "self_hosted");
    let json = serde_json::to_string(&signal).unwrap();
    // The fields the analytics service (site repo, apps/analytics/src/Signals.ts) reads.
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let keys = |v: &serde_json::Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(
        keys(&value),
        ["arch", "config", "hosting", "install_id", "os", "schema", "sent_at", "totals", "uptime_seconds", "version"]
    );
    assert_eq!(
        keys(&value["config"]),
        ["encryption", "limits_configured", "linked_accounts", "local_accounts", "server_creation"]
    );
    assert_eq!(value["totals"].as_object().unwrap().len(), 14);
    assert_eq!(value["install_id"].as_str().unwrap().len(), 26);
    for private in ["owner", "guest", "Capped", server.id.as_str()] {
        assert!(!json.contains(private), "the usage signal must not contain {private:?}");
    }
    instance.stop().await;
}

#[tokio::test]
async fn operators_choose_which_accounts_exist() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[("FUWA_LOCAL_ACCOUNTS", "closed"), ("FUWA_SERVER_CREATION", "admins")]).await;
    let mut c = clients(&instance).await;
    let closed = c
        .auth
        .sign_up(pb::SignUpRequest {
            username: "someone".into(),
            password: "a fine password".into(),
            display_name: String::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(closed.code(), Code::FailedPrecondition);
    let node = c.node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap();
    let auth = node.auth.unwrap();
    assert!(auth.local_sign_in && !auth.local_sign_up);
    assert_eq!(node.server_creation, pb::ServerCreation::Admins as i32);
    instance.stop().await;
}

fn settings_update(settings: pb::InstanceSettings, update: &[&str], reset: &[&str]) -> pb::UpdateSettingsRequest {
    let mask = |paths: &[&str]| Some(prost_types::FieldMask { paths: paths.iter().map(|p| p.to_string()).collect() });
    pb::UpdateSettingsRequest { settings: Some(settings), update_mask: mask(update), reset_mask: mask(reset) }
}

async fn preflight_origin(instance: &Instance, origin: &str) -> Option<String> {
    let response = reqwest::Client::new()
        .request(reqwest::Method::OPTIONS, format!("http://{}/fuwa.v1.NodeService/GetNode", instance.addr))
        .header("origin", origin)
        .header("access-control-request-method", "POST")
        .send()
        .await
        .unwrap();
    response.headers().get("access-control-allow-origin").map(|v| v.to_str().unwrap().to_string())
}

#[tokio::test]
async fn admins_change_settings_from_a_client() {
    let dir = tempfile::tempdir().unwrap();
    let env = [("FUWA_NODE_NAME", "From env"), ("FUWA_LIMIT_MEMBERS", "10")];
    let instance = start(dir.path(), &env).await;
    let mut c = clients(&instance).await;
    let (admin, _, _) = sign_up(&mut c, "admin").await;
    let (member, _, _) = sign_up(&mut c, "member").await;

    // Only instance admins see or change settings.
    let denied = c.admin.get_settings(authed(&member, pb::GetSettingsRequest {})).await.unwrap_err();
    assert_eq!(denied.code(), Code::PermissionDenied);
    let config = c.admin.get_settings(authed(&admin, pb::GetSettingsRequest {})).await.unwrap().into_inner();
    let config = config.config.unwrap();
    let settings = config.settings.unwrap();
    assert_eq!(settings.name, "From env");
    assert_eq!(settings.default_limits.unwrap().members, Some(10));
    assert_eq!(settings.local_accounts, pb::LocalAccounts::Open as i32);
    assert!(config.overridden.is_empty());
    assert!(config.startup.unwrap().admin_token);

    // Changes apply at once: a new name, sign-ups closed, no member cap.
    let closing = pb::InstanceSettings {
        name: "  Renamed ".into(),
        local_accounts: pb::LocalAccounts::Closed as i32,
        default_limits: Some(pb::ServerLimits { members: None, ..Default::default() }),
        ..Default::default()
    };
    let changed = c
        .admin
        .update_settings(authed(
            &admin,
            settings_update(closing, &["name", "local_accounts", "default_limits.members"], &[]),
        ))
        .await
        .unwrap()
        .into_inner()
        .config
        .unwrap();
    assert_eq!(changed.overridden, ["default_limits.members", "local_accounts", "name"]);
    assert_eq!(changed.settings.unwrap().name, "Renamed");
    assert_eq!(changed.defaults.unwrap().name, "From env");
    let node = c.node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap();
    assert_eq!(node.name, "Renamed");
    assert!(!node.auth.unwrap().local_sign_up);
    let closed = c
        .auth
        .sign_up(pb::SignUpRequest {
            username: "late".into(),
            password: "a fine password".into(),
            display_name: String::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(closed.code(), Code::FailedPrecondition);
    let usage = c.admin.get_node_usage(authed(&admin, pb::GetNodeUsageRequest {})).await.unwrap().into_inner();
    assert_eq!(usage.default_limits.unwrap().members, None);

    // Bad values, unknown settings and locking everyone out are refused.
    let refuse = async |c: &mut Clients, token: &str, request: pb::UpdateSettingsRequest| {
        c.admin.update_settings(authed(token, request)).await.unwrap_err().code()
    };
    let blank = pb::InstanceSettings { name: " ".into(), ..Default::default() };
    assert_eq!(refuse(&mut c, &admin, settings_update(blank, &["name"], &[])).await, Code::InvalidArgument);
    assert_eq!(
        refuse(&mut c, &admin, settings_update(Default::default(), &["port"], &[])).await,
        Code::InvalidArgument
    );
    let off = pb::InstanceSettings { local_accounts: pb::LocalAccounts::Off as i32, ..Default::default() };
    assert_eq!(refuse(&mut c, &admin, settings_update(off, &["local_accounts"], &[])).await, Code::FailedPrecondition);
    assert_eq!(
        refuse(&mut c, &member, settings_update(Default::default(), &[], &["name"])).await,
        Code::PermissionDenied
    );

    // Allowed origins apply to the very next request.
    let origins = pb::InstanceSettings { allowed_origins: vec!["https://app.example/".into()], ..Default::default() };
    c.admin.update_settings(authed(&admin, settings_update(origins, &["allowed_origins"], &[]))).await.unwrap();
    assert_eq!(preflight_origin(&instance, "https://app.example").await.as_deref(), Some("https://app.example"));
    assert_eq!(preflight_origin(&instance, "https://other.example").await, None);

    // Settings survive a restart, over the same environment.
    instance.stop().await;
    let instance = start(dir.path(), &env).await;
    let mut c = clients(&instance).await;
    let node = c.node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap();
    assert_eq!(node.name, "Renamed");

    // Resetting returns them to the environment's values.
    let reset =
        settings_update(Default::default(), &[], &["name", "local_accounts", "allowed_origins", "default_limits"]);
    let config = c.admin.update_settings(authed(&admin, reset)).await.unwrap().into_inner().config.unwrap();
    assert!(config.overridden.is_empty());
    assert_eq!(config.settings.unwrap().default_limits.unwrap().members, Some(10));
    let node = c.node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap();
    assert_eq!(node.name, "From env");
    assert!(node.auth.unwrap().local_sign_up);
    assert_eq!(preflight_origin(&instance, "https://other.example").await.as_deref(), Some("https://other.example"));
    instance.stop().await;
}

#[tokio::test]
async fn browsers_can_call_over_grpc_web() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[("FUWA_NODE_NAME", "Web")]).await;
    let http = reqwest::Client::new();
    let base = format!("http://{}", instance.addr);

    // CORS preflight from any origin, which is echoed back.
    let preflight = http
        .request(reqwest::Method::OPTIONS, format!("{base}/fuwa.v1.NodeService/GetNode"))
        .header("origin", "https://fuwa.waifu.dev")
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "authorization,content-type,x-grpc-web")
        .send()
        .await
        .unwrap();
    assert!(preflight.status().is_success());
    assert_eq!(preflight.headers()["access-control-allow-origin"], "https://fuwa.waifu.dev");

    // An empty GetNodeRequest, framed for gRPC-Web: flag byte 0, then a 4-byte length.
    let response = http
        .post(format!("{base}/fuwa.v1.NodeService/GetNode"))
        .header("content-type", "application/grpc-web+proto")
        .header("x-grpc-web", "1")
        .header("origin", "https://fuwa.waifu.dev")
        .body(vec![0u8, 0, 0, 0, 0])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "application/grpc-web+proto");
    let body = response.bytes().await.unwrap();
    let length = u32::from_be_bytes(body[1..5].try_into().unwrap()) as usize;
    let reply = <pb::GetNodeResponse as prost::Message>::decode(&body[5..5 + length]).unwrap();
    assert_eq!(reply.node.unwrap().name, "Web");
    let trailers = String::from_utf8_lossy(&body[5 + length + 5..]).to_lowercase();
    assert!(trailers.contains("grpc-status:0"), "trailers: {trailers}");

    assert_eq!(http.get(format!("{base}/healthz")).send().await.unwrap().text().await.unwrap(), "ok");
    if !fuwa_server::web::BUILT_IN {
        // With the web client built in, unknown paths open the app instead (see tests/web.rs).
        assert_eq!(http.get(format!("{base}/nope")).send().await.unwrap().status(), 404);
    }

    // Live events stream to browsers too, over plain HTTP/1.1.
    let mut c = clients(&instance).await;
    let (token, _, _) = sign_up(&mut c, "browser").await;
    let server = c
        .servers
        .create_server(authed(&token, pb::CreateServerRequest { name: "Web chat".into(), ..Default::default() }))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap();
    let request = pb::SubscribeRequest {
        servers: vec![pb::ServerCursor { server_id: server.id.clone(), after_sequence: Some(0) }],
    };
    let payload = prost::Message::encode_to_vec(&request);
    let mut framed = vec![0u8];
    framed.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    framed.extend_from_slice(&payload);
    let mut response = http
        .post(format!("{base}/fuwa.v1.EventService/Subscribe"))
        .version(reqwest::Version::HTTP_11)
        .header("content-type", "application/grpc-web+proto")
        .header("authorization", format!("Bearer {token}"))
        .body(framed)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut buffer = Vec::new();
    while buffer.len() < 5 || buffer.len() < 5 + u32::from_be_bytes(buffer[1..5].try_into().unwrap()) as usize {
        let chunk = tokio::time::timeout(Duration::from_secs(5), response.chunk()).await.unwrap().unwrap().unwrap();
        buffer.extend_from_slice(&chunk);
    }
    let length = u32::from_be_bytes(buffer[1..5].try_into().unwrap()) as usize;
    let first = <pb::SubscribeResponse as prost::Message>::decode(&buffer[5..5 + length]).unwrap();
    assert_eq!(first.event.unwrap().sequence, 1);
    drop(response);
    instance.stop().await;
}

/// Bytes 18 and 19 of a database file: 2 for plain SQLite (WAL), 255 for
/// Turso's concurrent-writer mode (MVCC).
fn file_mode(path: &Path) -> [u8; 2] {
    let file = std::fs::read(path).unwrap();
    [file[18], file[19]]
}

async fn create_server(c: &mut Clients, token: &str, name: &str, discoverable: bool) -> pb::Server {
    c.servers
        .create_server(authed(token, pb::CreateServerRequest { name: name.into(), discoverable, ..Default::default() }))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap()
}

// Several worker threads, so transactions really overlap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writes_stay_ordered_and_counted() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (owner, _, _) = sign_up(&mut c, "busy").await;
    let server = create_server(&mut c, &owner, "Busy", true).await;
    let mut tokens = vec![owner.clone()];
    for name in ["aoi", "hana", "kira"] {
        let (token, _, _) = sign_up(&mut c, name).await;
        c.servers.join_server(authed(&token, pb::JoinServerRequest { server_id: server.id.clone() })).await.unwrap();
        tokens.push(token);
    }
    let general = c
        .channels
        .list_channels(authed(&owner, pb::ListChannelsRequest { server_id: server.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels[0]
        .id
        .clone();
    let doomed = c
        .channels
        .create_channel(authed(
            &owner,
            pb::CreateChannelRequest { server_id: server.id.clone(), name: "doomed".into(), ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .channel
        .unwrap()
        .id;
    let mut stream = c
        .events
        .subscribe(authed(
            &owner,
            pb::SubscribeRequest {
                servers: vec![pb::ServerCursor { server_id: server.id.clone(), after_sequence: None }],
            },
        ))
        .await
        .unwrap()
        .into_inner();
    // The stream says where the server stands before anything live arrives:
    // the owner, #general, three joins and #doomed.
    let ready =
        tokio::time::timeout(Duration::from_secs(5), stream.next()).await.unwrap().unwrap().unwrap().ready.unwrap();
    assert_eq!(ready.servers, [pb::ServerHead { server_id: server.id.clone(), sequence: 6 }]);

    // 300 messages from four people at once, half of them into a channel that
    // gets deleted halfway through: more than enough to fold usage changes.
    let send = |n: usize, channel: &str| {
        let mut messages = c.messages.clone();
        let request = authed(
            &tokens[n % tokens.len()],
            pb::SendMessageRequest {
                server_id: server.id.clone(),
                channel_id: channel.to_string(),
                content: format!("burst {n}"),
                ..Default::default()
            },
        );
        tokio::spawn(async move { messages.send_message(request).await })
    };
    let first: Vec<_> = (0..150).map(|n| send(n, if n % 2 == 0 { &general } else { &doomed })).collect();
    let mut channels = c.channels.clone();
    let delete = authed(&owner, pb::DeleteChannelRequest { server_id: server.id.clone(), channel_id: doomed.clone() });
    let deleting = tokio::spawn(async move { channels.delete_channel(delete).await });
    let second: Vec<_> = (150..300).map(|n| send(n, if n % 2 == 0 { &general } else { &doomed })).collect();
    let (mut sent, mut refused) = (0, 0);
    for task in first.into_iter().chain(second) {
        match task.await.unwrap() {
            Ok(_) => sent += 1,
            Err(status) => {
                // Only messages for the deleted channel may be turned away.
                assert_eq!(status.code(), Code::NotFound, "{status:?}");
                refused += 1;
            }
        }
    }
    deleting.await.unwrap().unwrap();
    assert_eq!(sent + refused, 300);
    assert!(sent >= 150, "every message to #general lands");

    // Live events arrive once each, in sequence, with nothing missing.
    let expected = sent + 1; // the messages that landed, and the channel going
    let mut sequences = Vec::new();
    while sequences.len() < expected {
        let item = tokio::time::timeout(Duration::from_secs(5), stream.next()).await.unwrap().unwrap().unwrap();
        if let Some(event) = item.event {
            sequences.push(event.sequence);
        }
    }
    assert!(sequences.windows(2).all(|pair| pair[1] == pair[0] + 1), "live events arrive in sequence: {sequences:?}");
    assert_eq!((sequences[0], *sequences.last().unwrap()), (7, 6 + expected as i64));

    // Catching up from the start replays the same log, in the same order.
    let mut replay = c
        .events
        .subscribe(authed(
            &owner,
            pb::SubscribeRequest {
                servers: vec![pb::ServerCursor { server_id: server.id.clone(), after_sequence: Some(6) }],
            },
        ))
        .await
        .unwrap()
        .into_inner();
    let mut replayed = Vec::new();
    loop {
        let item = tokio::time::timeout(Duration::from_secs(5), replay.next()).await.unwrap().unwrap().unwrap();
        match item.event {
            Some(event) => replayed.push(event.sequence),
            None => break,
        }
    }
    assert_eq!(replayed, sequences);
    drop((stream, replay));

    // The totals count what's in #general, whichever order things landed in.
    let in_general = c
        .messages
        .list_messages(authed(
            &owner,
            pb::ListMessagesRequest {
                server_id: server.id.clone(),
                channel_id: general.clone(),
                limit: 100,
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(in_general.has_more);
    let u = usage(&mut c, &owner, &server.id).await;
    assert_eq!((u.members, u.channels), (4, 1));
    assert_eq!(u.messages, 150, "only #general's messages are left");
    assert_eq!(u.messages_sent, sent as i64);
    assert_eq!(u.events, 6 + expected as i64);
    drop(c);
    instance.stop().await;

    // Both kinds of file run in concurrent-writer mode, and the totals and the
    // log survive a restart (which folds the remaining usage changes).
    let server_file = dir.path().join("servers").join(format!("{}.db", server.id));
    assert_eq!(file_mode(&server_file), [255, 255]);
    assert_eq!(file_mode(&dir.path().join("node.db")), [255, 255]);
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let again = usage(&mut c, &owner, &server.id).await;
    assert_eq!((again.messages, again.messages_sent, again.events), (u.messages, u.messages_sent, u.events));
    drop(c);
    instance.stop().await;

    // No message outlived its channel.
    let db = turso::Builder::new_local(server_file.to_str().unwrap()).build().await.unwrap();
    let conn = db.connect().unwrap();
    let mut rows = conn.query("SELECT count(*), count(DISTINCT channel_id) FROM messages", ()).await.unwrap();
    let row = rows.next().await.unwrap().unwrap();
    assert_eq!((row.get::<i64>(0).unwrap(), row.get::<i64>(1).unwrap()), (150, 1));
}

// Several worker threads, so transactions really overlap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_writes_keep_caps_and_the_first_admin() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let c = clients(&instance).await;

    // Ten sign-ups at once on a fresh instance: exactly one becomes the admin.
    let signing_up: Vec<_> = (0..10)
        .map(|n| {
            let mut auth = c.auth.clone();
            tokio::spawn(async move {
                auth.sign_up(pb::SignUpRequest {
                    username: format!("racer{n}"),
                    password: "correct horse battery".into(),
                    display_name: String::new(),
                })
                .await
                .unwrap()
                .into_inner()
            })
        })
        .collect();
    let mut accounts = Vec::new();
    for task in signing_up {
        accounts.push(task.await.unwrap());
    }
    assert_eq!(accounts.iter().filter(|a| a.admin).count(), 1);

    // A server capped at 4 members, and nine people joining at the same moment.
    let owner = accounts[0].token.clone();
    let mut c = c;
    let server = create_server(&mut c, &owner, "Tiny", true).await;
    c.admin
        .set_server_limits(authed(
            ADMIN_TOKEN,
            pb::SetServerLimitsRequest {
                server_id: server.id.clone(),
                limits: Some(pb::ServerLimits { members: Some(4), ..Default::default() }),
            },
        ))
        .await
        .unwrap();
    let joining: Vec<_> = accounts[1..]
        .iter()
        .map(|account| {
            let mut servers = c.servers.clone();
            let request = authed(&account.token, pb::JoinServerRequest { server_id: server.id.clone() });
            tokio::spawn(async move { servers.join_server(request).await })
        })
        .collect();
    let mut joined = 0;
    for task in joining {
        match task.await.unwrap() {
            Ok(_) => joined += 1,
            Err(status) => assert_eq!(status.code(), Code::ResourceExhausted, "{status:?}"),
        }
    }
    assert_eq!(joined, 3, "the cap holds when joins race");
    assert_eq!(usage(&mut c, &owner, &server.id).await.members, 4);
    instance.stop().await;
}

#[tokio::test]
async fn files_switch_to_plain_sqlite_and_back() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (token, _, _) = sign_up(&mut c, "curious").await;
    let server = create_server(&mut c, &token, "Inspectable", false).await;
    drop(c);
    instance.stop().await;

    // For other SQLite tools: `fuwa to-sqlite` puts the file back in WAL mode.
    let file = dir.path().join("servers").join(format!("{}.db", server.id));
    fuwa_server::db::to_sqlite(&file, None).await.unwrap();
    assert_eq!(file_mode(&file), [2, 2]);

    // fuwa switches it back when it opens it, with everything still there.
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let servers = c.servers.list_servers(authed(&token, pb::ListServersRequest {})).await.unwrap().into_inner().servers;
    assert_eq!(servers[0].name, "Inspectable");
    assert_eq!(file_mode(&file), [255, 255]);
    instance.stop().await;
}

#[tokio::test]
async fn databases_can_be_encrypted_at_rest() {
    let dir = tempfile::tempdir().unwrap();
    let key = "b1bbfda4f589dc9daaf004fe21111e00dc00c98237102f5c7002a5669fc76327";
    let instance = start(dir.path(), &[("FUWA_ENCRYPTION_KEY", key)]).await;
    let mut c = clients(&instance).await;
    let (token, _, _) = sign_up(&mut c, "secret").await;
    let server = c
        .servers
        .create_server(authed(&token, pb::CreateServerRequest { name: "Vault".into(), ..Default::default() }))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap();
    drop(c);
    instance.stop().await;

    let file = std::fs::read(dir.path().join("servers").join(format!("{}.db", server.id))).unwrap();
    assert!(!file.starts_with(b"SQLite format 3"), "an encrypted database shouldn't look like plain SQLite");

    let instance = start(dir.path(), &[("FUWA_ENCRYPTION_KEY", key)]).await;
    let mut c = clients(&instance).await;
    let servers = c.servers.list_servers(authed(&token, pb::ListServersRequest {})).await.unwrap().into_inner().servers;
    assert_eq!(servers[0].name, "Vault");
    instance.stop().await;
}

async fn sign_in(c: &mut Clients, username: &str, password: &str) -> Result<pb::SignInResponse, tonic::Status> {
    c.auth
        .sign_in(pb::SignInRequest { username: username.into(), password: password.into() })
        .await
        .map(|r| r.into_inner())
}

async fn me(c: &mut Clients, token: &str) -> Result<pb::User, Code> {
    c.auth.get_me(authed(token, pb::GetMeRequest {})).await.map(|r| r.into_inner().user.unwrap()).map_err(|e| e.code())
}

#[tokio::test]
async fn devices_can_be_listed_and_signed_out() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;

    let (first, _, _) = sign_up(&mut c, "juan").await;
    let mut phone =
        Request::new(pb::SignInRequest { username: "juan".into(), password: "correct horse battery".into() });
    phone.metadata_mut().insert("user-agent", "Mozilla/5.0 (iPhone) fuwa-test".parse().unwrap());
    let second = c.auth.sign_in(phone).await.unwrap().into_inner().token;
    let third = sign_in(&mut c, "juan", "correct horse battery").await.unwrap().token;

    let sessions =
        c.account.list_sessions(authed(&first, pb::ListSessionsRequest {})).await.unwrap().into_inner().sessions;
    assert_eq!(sessions.len(), 3);
    assert!(sessions[0].current && sessions.iter().filter(|s| s.current).count() == 1);
    assert!(sessions.iter().all(|s| !s.id.is_empty() && s.last_active_at.is_some()));
    assert!(sessions.iter().any(|s| s.user_agent.contains("iPhone")));

    // Signing out one device ends it at once.
    let phone_id = sessions.iter().find(|s| s.user_agent.contains("iPhone")).unwrap().id.clone();
    c.account.revoke_session(authed(&first, pb::RevokeSessionRequest { session_id: phone_id.clone() })).await.unwrap();
    assert_eq!(me(&mut c, &second).await.unwrap_err(), Code::Unauthenticated);
    let again = c.account.revoke_session(authed(&first, pb::RevokeSessionRequest { session_id: phone_id })).await;
    assert_eq!(again.unwrap_err().code(), Code::NotFound);

    // Nobody else's sessions can be named.
    let (mika, _, _) = sign_up(&mut c, "mika").await;
    let first_id = sessions[0].id.clone();
    let theirs = c.account.revoke_session(authed(&mika, pb::RevokeSessionRequest { session_id: first_id })).await;
    assert_eq!(theirs.unwrap_err().code(), Code::NotFound);
    assert!(me(&mut c, &first).await.is_ok());

    let revoked = c
        .account
        .revoke_other_sessions(authed(&first, pb::RevokeOtherSessionsRequest {}))
        .await
        .unwrap()
        .into_inner()
        .revoked;
    assert_eq!(revoked, 1);
    assert_eq!(me(&mut c, &third).await.unwrap_err(), Code::Unauthenticated);
    assert!(me(&mut c, &first).await.is_ok());
    assert!(me(&mut c, &mika).await.is_ok());

    instance.stop().await;
}

#[tokio::test]
async fn two_step_sign_in() {
    use fuwa_server::twofactor;
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[("FUWA_NODE_NAME", "Waifu Devs")]).await;
    let mut c = clients(&instance).await;
    let (juan, _, _) = sign_up(&mut c, "juan").await;
    let password = "correct horse battery";

    let wrong =
        c.account.set_up_two_factor(authed(&juan, pb::SetUpTwoFactorRequest { password: "nope nope".into() })).await;
    assert_eq!(wrong.unwrap_err().code(), Code::PermissionDenied);
    let setup = c
        .account
        .set_up_two_factor(authed(&juan, pb::SetUpTwoFactorRequest { password: password.into() }))
        .await
        .unwrap()
        .into_inner();
    assert!(setup.uri.starts_with("otpauth://totp/Waifu%20Devs:juan?secret="));
    // Nothing changes until a code confirms it.
    assert!(sign_in(&mut c, "juan", password).await.unwrap().two_factor_ticket.is_empty());

    let bad = c.account.enable_two_factor(authed(&juan, pb::EnableTwoFactorRequest { code: "000000".into() })).await;
    // (A one-in-a-million chance the real code is 000000.)
    if twofactor::code_for(&setup.secret, fuwa_server::id::now_ms()).unwrap() != "000000" {
        assert_eq!(bad.unwrap_err().code(), Code::PermissionDenied);
    }
    let now = fuwa_server::id::now_ms();
    let code = twofactor::code_for(&setup.secret, now).unwrap();
    let backup = c
        .account
        .enable_two_factor(authed(&juan, pb::EnableTwoFactorRequest { code: code.clone() }))
        .await
        .unwrap()
        .into_inner()
        .backup_codes;
    assert_eq!(backup.len(), 10);
    let state = c.account.get_two_factor(authed(&juan, pb::GetTwoFactorRequest {})).await.unwrap().into_inner();
    assert!(state.enabled);
    assert_eq!(state.backup_codes_left, 10);

    // Signing in now takes two steps.
    let first = sign_in(&mut c, "juan", password).await.unwrap();
    assert!(first.token.is_empty() && first.user.is_none() && !first.two_factor_ticket.is_empty());
    let verify = |ticket: &str, code: &str| pb::VerifyTwoFactorRequest { ticket: ticket.into(), code: code.into() };
    // The code that turned it on is used up.
    let replay = c.auth.verify_two_factor(verify(&first.two_factor_ticket, &code)).await;
    assert_eq!(replay.unwrap_err().code(), Code::PermissionDenied);
    // The next step's code works once (clocks drift).
    let next = twofactor::code_for(&setup.secret, now + 30_000).unwrap();
    let signed_in = c.auth.verify_two_factor(verify(&first.two_factor_ticket, &next)).await.unwrap().into_inner();
    assert_eq!(signed_in.user.unwrap().username, "juan");
    assert!(me(&mut c, &signed_in.token).await.is_ok());
    let used = c.auth.verify_two_factor(verify(&first.two_factor_ticket, &next)).await;
    assert_eq!(used.unwrap_err().code(), Code::FailedPrecondition, "a ticket signs in once");

    // Backup codes work once each, typed however.
    let second = sign_in(&mut c, "juan", password).await.unwrap().two_factor_ticket;
    let typed = format!(" {} ", backup[0].to_uppercase().replace('-', " "));
    assert!(c.auth.verify_two_factor(verify(&second, &typed)).await.is_ok());
    let third = sign_in(&mut c, "juan", password).await.unwrap().two_factor_ticket;
    let reused = c.auth.verify_two_factor(verify(&third, &backup[0])).await;
    assert_eq!(reused.unwrap_err().code(), Code::PermissionDenied);
    let left = c.account.get_two_factor(authed(&juan, pb::GetTwoFactorRequest {})).await.unwrap().into_inner();
    assert_eq!(left.backup_codes_left, 9);

    // A sign-in gives up after a few wrong codes.
    for _ in 0..4 {
        let wrong = c.auth.verify_two_factor(verify(&third, "zzzz-zzzz")).await;
        assert_eq!(wrong.unwrap_err().code(), Code::PermissionDenied);
    }
    let gone = c.auth.verify_two_factor(verify(&third, &backup[1])).await;
    assert_eq!(gone.unwrap_err().code(), Code::FailedPrecondition);

    // New backup codes replace the old ones.
    let fresh = c
        .account
        .regenerate_backup_codes(authed(&juan, pb::RegenerateBackupCodesRequest { password: password.into() }))
        .await
        .unwrap()
        .into_inner()
        .backup_codes;
    let fourth = sign_in(&mut c, "juan", password).await.unwrap().two_factor_ticket;
    let old = c.auth.verify_two_factor(verify(&fourth, &backup[2])).await;
    assert_eq!(old.unwrap_err().code(), Code::PermissionDenied);

    // Turning it off takes the password and a code.
    let no_code = c
        .account
        .disable_two_factor(authed(
            &juan,
            pb::DisableTwoFactorRequest { password: password.into(), code: String::new() },
        ))
        .await;
    assert_eq!(no_code.unwrap_err().code(), Code::PermissionDenied);
    c.account
        .disable_two_factor(authed(
            &juan,
            pb::DisableTwoFactorRequest { password: password.into(), code: fresh[0].clone() },
        ))
        .await
        .unwrap();
    assert!(!sign_in(&mut c, "juan", password).await.unwrap().token.is_empty());
    let off = c
        .account
        .regenerate_backup_codes(authed(&juan, pb::RegenerateBackupCodesRequest { password: password.into() }))
        .await;
    assert_eq!(off.unwrap_err().code(), Code::FailedPrecondition);

    instance.stop().await;
}

#[tokio::test]
async fn profiles_nicknames_and_notification_settings() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (juan, juan_user, _) = sign_up(&mut c, "juan").await;
    let (mika, mika_user, _) = sign_up(&mut c, "mika").await;

    let expires = prost_types::Timestamp { seconds: 4_000_000_000, nanos: 0 };
    let updated = c
        .auth
        .update_profile(authed(
            &juan,
            pb::UpdateProfileRequest {
                pronouns: Some("he/him".into()),
                bio: Some("  Building **fuwa**.  ".into()),
                accent_color: Some(0xff66aa),
                status: Some("shipping wave 2".into()),
                status_expires_at: Some(expires),
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner();
    let profile = updated.profile.unwrap();
    assert_eq!((profile.pronouns.as_str(), profile.bio.as_str()), ("he/him", "Building **fuwa**."));
    assert_eq!(profile.accent_color, Some(0xff66aa));
    assert_eq!(updated.user.unwrap().status, "shipping wave 2");
    let bad_color = c
        .auth
        .update_profile(authed(&juan, pb::UpdateProfileRequest { accent_color: Some(0x1000000), ..Default::default() }))
        .await;
    assert_eq!(bad_color.unwrap_err().code(), Code::InvalidArgument);

    // Profiles show to people who share a server.
    let get = |id: &str| pb::GetProfileRequest { user_id: id.into() };
    let hidden = c.auth.get_profile(authed(&mika, get(&juan_user.id))).await;
    assert_eq!(hidden.unwrap_err().code(), Code::NotFound);
    let server = create_server(&mut c, &juan, "Waifu Devs", true).await;
    c.servers.join_server(authed(&mika, pb::JoinServerRequest { server_id: server.id.clone() })).await.unwrap();
    let seen = c.auth.get_profile(authed(&mika, get(&juan_user.id))).await.unwrap().into_inner().profile.unwrap();
    assert_eq!(seen.pronouns, "he/him");
    let members = c
        .servers
        .list_members(authed(&mika, pb::ListMembersRequest { server_id: server.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .members;
    let juan_member = members.iter().find(|m| m.user.as_ref().unwrap().id == juan_user.id).unwrap();
    assert_eq!(juan_member.user.as_ref().unwrap().status, "shipping wave 2");
    assert_eq!(juan_member.user.as_ref().unwrap().status_expires_at, Some(expires));
    // Clearing the status clears its expiry too.
    let cleared = c
        .auth
        .update_profile(authed(&juan, pb::UpdateProfileRequest { status: Some(String::new()), ..Default::default() }))
        .await
        .unwrap()
        .into_inner()
        .user
        .unwrap();
    assert!(cleared.status.is_empty() && cleared.status_expires_at.is_none());

    // Nicknames: your own, or those of people ranked below you.
    let mut stream = c
        .events
        .subscribe(authed(
            &juan,
            pb::SubscribeRequest {
                servers: vec![pb::ServerCursor { server_id: server.id.clone(), after_sequence: None }],
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(stream.next().await.unwrap().unwrap().ready.is_some());
    let nick = |user: &str, nickname: &str| pb::UpdateMemberRequest {
        server_id: server.id.clone(),
        user_id: user.into(),
        nickname: Some(nickname.into()),
    };
    let own = c.servers.update_member(authed(&mika, nick("", "  Mika ✨ "))).await.unwrap().into_inner();
    assert_eq!(own.member.unwrap().nickname, "Mika ✨");
    let event = stream.next().await.unwrap().unwrap().event.unwrap();
    let Some(Payload::MemberUpdated(update)) = event.payload else { panic!("expected a member update") };
    assert_eq!(update.member.unwrap().nickname, "Mika ✨");
    let upward = c.servers.update_member(authed(&mika, nick(&juan_user.id, "boss"))).await;
    assert_eq!(upward.unwrap_err().code(), Code::PermissionDenied);
    let downward = c.servers.update_member(authed(&juan, nick(&mika_user.id, ""))).await.unwrap().into_inner();
    assert_eq!(downward.member.unwrap().nickname, "");
    let long = c.servers.update_member(authed(&mika, nick("", &"a".repeat(33)))).await;
    assert_eq!(long.unwrap_err().code(), Code::InvalidArgument);

    // Notification settings, per server and channel.
    let channel = c
        .channels
        .list_channels(authed(&mika, pb::ListChannelsRequest { server_id: server.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels
        .remove(0);
    let change =
        |channel_id: &str, settings: pb::NotificationSettings, paths: &[&str]| pb::UpdateNotificationSettingsRequest {
            settings: Some(pb::NotificationSettings {
                server_id: server.id.clone(),
                channel_id: channel_id.into(),
                ..settings
            }),
            update_mask: Some(prost_types::FieldMask { paths: paths.iter().map(|p| p.to_string()).collect() }),
        };
    let level = pb::NotificationSettings { level: pb::NotificationLevel::Mentions as i32, ..Default::default() };
    let saved = c
        .account
        .update_notification_settings(authed(&mika, change("", level, &["level"])))
        .await
        .unwrap()
        .into_inner()
        .settings
        .unwrap();
    assert_eq!(saved.level, pb::NotificationLevel::Mentions as i32);
    let mute = pb::NotificationSettings { muted: true, ..Default::default() };
    c.account.update_notification_settings(authed(&mika, change(&channel.id, mute, &["muted"]))).await.unwrap();
    let everyone = pb::NotificationSettings { suppress_everyone: true, ..Default::default() };
    let per_channel = c
        .account
        .update_notification_settings(authed(&mika, change(&channel.id, everyone, &["suppress_everyone"])))
        .await;
    assert_eq!(per_channel.unwrap_err().code(), Code::InvalidArgument);
    let over = pb::NotificationSettings {
        muted: true,
        muted_until: Some(prost_types::Timestamp { seconds: 1, nanos: 0 }),
        ..Default::default()
    };
    let past = c
        .account
        .update_notification_settings(authed(&mika, change("", over, &["muted"])))
        .await
        .unwrap()
        .into_inner()
        .settings
        .unwrap();
    assert!(!past.muted, "a mute that already ran out is no mute");
    let all = c
        .account
        .get_notification_settings(authed(&mika, pb::GetNotificationSettingsRequest {}))
        .await
        .unwrap()
        .into_inner()
        .settings;
    assert_eq!(all.len(), 2);
    assert!(all.iter().any(|s| s.channel_id == channel.id && s.muted && s.muted_until.is_none()));
    // Back to following the server's default: forgotten.
    c.account
        .update_notification_settings(authed(&mika, change(&channel.id, Default::default(), &["muted"])))
        .await
        .unwrap();
    let outsider =
        c.account.update_notification_settings(authed(&juan, change("", Default::default(), &["bogus"]))).await;
    assert_eq!(outsider.unwrap_err().code(), Code::InvalidArgument);
    // Leaving forgets the rest.
    c.servers.leave_server(authed(&mika, pb::LeaveServerRequest { server_id: server.id.clone() })).await.unwrap();
    let left = c
        .account
        .get_notification_settings(authed(&mika, pb::GetNotificationSettingsRequest {}))
        .await
        .unwrap()
        .into_inner()
        .settings;
    assert!(left.is_empty());

    instance.stop().await;
}

#[tokio::test]
async fn data_export_and_account_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (juan, _, _) = sign_up(&mut c, "juan").await;
    let (mika, mika_user, _) = sign_up(&mut c, "mika").await;
    let server = create_server(&mut c, &juan, "Waifu Devs", true).await;
    c.servers.join_server(authed(&mika, pb::JoinServerRequest { server_id: server.id.clone() })).await.unwrap();
    let channel = c
        .channels
        .list_channels(authed(&mika, pb::ListChannelsRequest { server_id: server.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels
        .remove(0);
    for n in 0..3 {
        c.messages
            .send_message(authed(
                &mika,
                pb::SendMessageRequest {
                    server_id: server.id.clone(),
                    channel_id: channel.id.clone(),
                    content: format!("hello \"{n}\""),
                    ..Default::default()
                },
            ))
            .await
            .unwrap();
    }

    let mut chunks = c.account.export_data(authed(&mika, pb::ExportDataRequest {})).await.unwrap().into_inner();
    let mut bytes = Vec::new();
    while let Some(chunk) = chunks.next().await {
        bytes.extend(chunk.unwrap().chunk);
    }
    let export: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(export["format"], "fuwa.export.v1");
    assert_eq!(export["account"]["username"], "mika");
    assert_eq!(export["sessions"].as_array().unwrap().len(), 1);
    let servers = export["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0]["role"], "member");
    let messages = servers[0]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[2]["content"], "hello \"2\"");
    assert_eq!(messages[0]["channel"], "general");

    // Owners hand their servers over or delete them first.
    let owner = c
        .account
        .delete_account(authed(
            &juan,
            pb::DeleteAccountRequest { password: "correct horse battery".into(), ..Default::default() },
        ))
        .await;
    assert_eq!(owner.unwrap_err().code(), Code::FailedPrecondition);
    let wrong = c
        .account
        .delete_account(authed(
            &mika,
            pb::DeleteAccountRequest { password: "wrong password".into(), ..Default::default() },
        ))
        .await;
    assert_eq!(wrong.unwrap_err().code(), Code::PermissionDenied);

    let mut stream = c
        .events
        .subscribe(authed(
            &juan,
            pb::SubscribeRequest {
                servers: vec![pb::ServerCursor { server_id: server.id.clone(), after_sequence: None }],
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(stream.next().await.unwrap().unwrap().ready.is_some());
    c.account
        .delete_account(authed(
            &mika,
            pb::DeleteAccountRequest { password: "correct horse battery".into(), ..Default::default() },
        ))
        .await
        .unwrap();
    assert_eq!(me(&mut c, &mika).await.unwrap_err(), Code::Unauthenticated);
    let event = stream.next().await.unwrap().unwrap().event.unwrap();
    assert!(matches!(event.payload, Some(Payload::MemberLeft(ref left)) if left.user_id == mika_user.id));
    let event = stream.next().await.unwrap().unwrap().event.unwrap();
    let Some(Payload::UserUpdated(gone)) = event.payload else { panic!("expected the user to change") };
    assert_eq!(gone.user.unwrap().display_name, "Deleted account");
    // Their messages stay, from a deleted account.
    let listed = c
        .messages
        .list_messages(authed(
            &juan,
            pb::ListMessagesRequest {
                server_id: server.id.clone(),
                channel_id: channel.id.clone(),
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(listed.messages.len(), 3);
    assert_eq!(listed.authors[0].display_name, "Deleted account");
    let usage = usage(&mut c, &juan, &server.id).await;
    assert_eq!(usage.members, 1);
    // The username is free again.
    let (_, again, _) = sign_up(&mut c, "mika").await;
    assert_ne!(again.id, mika_user.id);

    // The only admin can't leave the instance without one.
    c.servers.delete_server(authed(&juan, pb::DeleteServerRequest { server_id: server.id.clone() })).await.unwrap();
    let only_admin = c
        .account
        .delete_account(authed(
            &juan,
            pb::DeleteAccountRequest { password: "correct horse battery".into(), ..Default::default() },
        ))
        .await;
    assert_eq!(only_admin.unwrap_err().code(), Code::FailedPrecondition);

    instance.stop().await;
}
