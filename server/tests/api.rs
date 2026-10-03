//! End-to-end tests: a real instance on a local port, driven through the
//! generated gRPC clients (and raw gRPC-Web, as a browser would).

use std::collections::HashSet;
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
    media: pb::media_service_client::MediaServiceClient<Channel>,
    roles: pb::role_service_client::RoleServiceClient<Channel>,
    invites: pb::invite_service_client::InviteServiceClient<Channel>,
    join: pb::join_service_client::JoinServiceClient<Channel>,
    automod: pb::auto_mod_service_client::AutoModServiceClient<Channel>,
    emojis: pb::emoji_service_client::EmojiServiceClient<Channel>,
    webhooks: pb::webhook_service_client::WebhookServiceClient<Channel>,
    agents: pb::agent_service_client::AgentServiceClient<Channel>,
    shared: pb::shared_channel_service_client::SharedChannelServiceClient<Channel>,
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
        node: pb::node_service_client::NodeServiceClient::new(channel.clone()),
        media: pb::media_service_client::MediaServiceClient::new(channel.clone()),
        roles: pb::role_service_client::RoleServiceClient::new(channel.clone()),
        invites: pb::invite_service_client::InviteServiceClient::new(channel.clone()),
        join: pb::join_service_client::JoinServiceClient::new(channel.clone()),
        automod: pb::auto_mod_service_client::AutoModServiceClient::new(channel.clone()),
        emojis: pb::emoji_service_client::EmojiServiceClient::new(channel.clone()),
        webhooks: pb::webhook_service_client::WebhookServiceClient::new(channel.clone()),
        agents: pb::agent_service_client::AgentServiceClient::new(channel.clone()),
        shared: pb::shared_channel_service_client::SharedChannelServiceClient::new(channel),
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
    let build = node.build.unwrap();
    assert_eq!((build.version.as_str(), build.commit.as_str()), (env!("CARGO_PKG_VERSION"), env!("FUWA_COMMIT")));
    assert_eq!(build.source, "https://github.com/waifu-devs/fuwa");
    let auth = node.auth.unwrap();
    assert!(auth.local_sign_in && auth.local_sign_up);
    assert!(auth.linked_sign_in && auth.linked_sign_up, "on by default, and localhost can be sent back to");
    assert_eq!(auth.linked_issuer, "https://api.waifu.dev");

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
    let not_yet = c
        .servers
        .join_server(authed(&mika, pb::JoinServerRequest { server_id: sid.clone(), ..Default::default() }))
        .await
        .unwrap_err();
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
        .join_server(authed(&mika, pb::JoinServerRequest { server_id: sid.clone(), ..Default::default() }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(joined.server.unwrap().member_count, 2);
    let again = c
        .servers
        .join_server(authed(&mika, pb::JoinServerRequest { server_id: sid.clone(), ..Default::default() }))
        .await
        .unwrap_err();
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
                channel_id: String::new(),
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
                channel_id: String::new(),
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
            pb::DeleteMessageRequest {
                channel_id: String::new(),
                server_id: sid.clone(),
                message_id: sent[1].id.clone(),
            },
        ))
        .await
        .unwrap();

    // Usage follows along, counting Mika's join message.
    let u = usage(&mut c, &juan, &sid).await;
    assert_eq!((u.members, u.channels, u.messages, u.messages_sent), (2, 2, 7, 8));
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
    // A new server starts with its roles, then its owner.
    assert!(matches!(&seen[0].payload, Some(Payload::RoleCreated(r)) if r.role.as_ref().unwrap().name == "Admin"));
    assert!(matches!(&seen[1].payload, Some(Payload::RoleCreated(r)) if r.role.as_ref().unwrap().id == sid));
    assert!(matches!(seen[2].payload, Some(Payload::MemberJoined(_))));
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
    assert_eq!((u.channels, u.messages, u.messages_sent, u.message_bytes), (1, 7, 9, expected_bytes));

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
    assert!(members[0].role_ids.is_empty() && members[1].role_ids.is_empty());
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
    c.servers
        .join_server(authed(&guest, pb::JoinServerRequest { server_id: server.id.clone(), ..Default::default() }))
        .await
        .unwrap();
    let full = c
        .servers
        .join_server(authed(&late, pb::JoinServerRequest { server_id: server.id.clone(), ..Default::default() }))
        .await
        .unwrap_err();
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
    let off = pb::InstanceSettings {
        local_accounts: pb::LocalAccounts::Off as i32,
        linked_accounts: pb::LinkedAccounts::Off as i32,
        ..Default::default()
    };
    assert_eq!(
        refuse(&mut c, &admin, settings_update(off, &["local_accounts", "linked_accounts"], &[])).await,
        Code::FailedPrecondition
    );
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
// Stopping (say, for a deploy) tells open streams to follow again, rather than
// ending them as if they were done, which clients would take as final.
#[tokio::test]
async fn streams_are_told_to_reconnect_when_the_instance_stops() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (token, _, _) = sign_up(&mut c, "juan").await;
    let server = create_server(&mut c, &token, "Den", false).await;
    let mut stream = c
        .events
        .subscribe(authed(
            &token,
            pb::SubscribeRequest { servers: vec![pb::ServerCursor { server_id: server.id, after_sequence: None }] },
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(stream.next().await.unwrap().unwrap().ready.is_some());

    instance.app.shutdown.cancel();
    let ended = stream.next().await.unwrap().unwrap_err();
    assert_eq!(ended.code(), Code::Unavailable);
    instance.serving.await.unwrap();
}

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
        c.servers
            .join_server(authed(&token, pb::JoinServerRequest { server_id: server.id.clone(), ..Default::default() }))
            .await
            .unwrap();
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
    // two roles, the owner, #general, three joins with their join messages, and #doomed.
    let ready =
        tokio::time::timeout(Duration::from_secs(5), stream.next()).await.unwrap().unwrap().unwrap().ready.unwrap();
    assert_eq!(ready.servers, [pb::ServerHead { server_id: server.id.clone(), sequence: 11 }]);

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
    let mut in_doomed = HashSet::new();
    while sequences.len() < expected {
        let item = tokio::time::timeout(Duration::from_secs(5), stream.next()).await.unwrap().unwrap().unwrap();
        if let Some(event) = item.event {
            if let Some(Payload::MessageCreated(created)) = &event.payload
                && created.message.as_ref().unwrap().channel_id == doomed
            {
                in_doomed.insert(event.sequence);
            }
            sequences.push(event.sequence);
        }
    }
    assert!(sequences.windows(2).all(|pair| pair[1] == pair[0] + 1), "live events arrive in sequence: {sequences:?}");
    assert_eq!((sequences[0], *sequences.last().unwrap()), (12, 11 + expected as i64));

    // Catching up replays the same log, in the same order, less the messages
    // in #doomed: a replay shows what can be seen now.
    let mut replay = c
        .events
        .subscribe(authed(
            &owner,
            pb::SubscribeRequest {
                servers: vec![pb::ServerCursor { server_id: server.id.clone(), after_sequence: Some(11) }],
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
    let visible: Vec<i64> = sequences.iter().copied().filter(|s| !in_doomed.contains(s)).collect();
    assert_eq!(replayed, visible);
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
    assert_eq!(u.messages, 153, "only #general's messages are left, join messages included");
    assert_eq!(u.messages_sent, sent as i64 + 3);
    assert_eq!(u.events, 11 + expected as i64);
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
    assert_eq!((row.get::<i64>(0).unwrap(), row.get::<i64>(1).unwrap()), (153, 1));
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
            let request =
                authed(&account.token, pb::JoinServerRequest { server_id: server.id.clone(), ..Default::default() });
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
    drop(c);
    instance.stop().await;

    // Starting with another key, or none, says the key is what to check.
    let other = "0".repeat(64);
    for vars in [&[("FUWA_ENCRYPTION_KEY", other.as_str())][..], &[]] {
        let err = open_error(dir.path(), vars).await;
        assert!(err.contains("FUWA_ENCRYPTION_KEY"), "{err}");
    }
    // And so does a key on data that was made without one.
    let plain = tempfile::tempdir().unwrap();
    start(plain.path(), &[]).await.stop().await;
    let err = open_error(plain.path(), &[("FUWA_ENCRYPTION_KEY", key)]).await;
    assert!(err.contains("isn't the key this data was made with"), "{err}");
}

/// Why App::open refused the data directory.
async fn open_error(dir: &Path, vars: &[(&str, &str)]) -> String {
    let dir = dir.to_str().unwrap().to_string();
    let config = Config::from_lookup(|key| match key {
        "FUWA_DATA_PATH" => Some(dir.clone()),
        "FUWA_TELEMETRY" => Some("off".into()),
        _ => vars.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string()),
    })
    .unwrap();
    match App::open(config).await {
        Ok(_) => panic!("the data directory opened"),
        Err(err) => err.to_string(),
    }
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn guesses_sent_at_once_are_all_counted() {
    use fuwa_server::twofactor;
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (juan, _, _) = sign_up(&mut c, "juan").await;
    let password = "correct horse battery";
    let setup = c
        .account
        .set_up_two_factor(authed(&juan, pb::SetUpTwoFactorRequest { password: password.into() }))
        .await
        .unwrap()
        .into_inner();
    let now = fuwa_server::id::now_ms();
    let code = twofactor::code_for(&setup.secret, now).unwrap();
    c.account.enable_two_factor(authed(&juan, pb::EnableTwoFactorRequest { code })).await.unwrap();

    // A ticket takes five codes, however many arrive at once.
    let ticket = sign_in(&mut c, "juan", password).await.unwrap().two_factor_ticket;
    let guesses = (0..20).map(|_| {
        let mut auth = c.auth.clone();
        let ticket = ticket.clone();
        tokio::spawn(async move {
            let request = pb::VerifyTwoFactorRequest { ticket, code: "zzzz-zzzz".into() };
            auth.verify_two_factor(request).await.unwrap_err().code()
        })
    });
    let codes: Vec<Code> = futures::future::join_all(guesses).await.into_iter().map(|r| r.unwrap()).collect();
    assert_eq!(codes.iter().filter(|c| **c == Code::PermissionDenied).count(), 5, "{codes:?}");
    assert!(codes.iter().all(|c| matches!(c, Code::PermissionDenied | Code::FailedPrecondition)));

    // Passwords too: ten tries, then a wait, even when sent all at once.
    let guesses = (0..15).map(|_| {
        let mut auth = c.auth.clone();
        tokio::spawn(async move {
            let request = pb::SignInRequest { username: "juan".into(), password: "wrong password".into() };
            auth.sign_in(request).await.unwrap_err().code()
        })
    });
    let codes: Vec<Code> = futures::future::join_all(guesses).await.into_iter().map(|r| r.unwrap()).collect();
    assert_eq!(codes.iter().filter(|c| **c == Code::Unauthenticated).count(), 10, "{codes:?}");
    assert_eq!(codes.iter().filter(|c| **c == Code::ResourceExhausted).count(), 5);
    let locked = sign_in(&mut c, "juan", password).await.unwrap_err();
    assert_eq!(locked.code(), Code::ResourceExhausted, "even the right password waits");

    // Changing the password counts guesses at the current one the same way.
    for _ in 0..10 {
        let wrong = c
            .auth
            .change_password(authed(
                &juan,
                pb::ChangePasswordRequest { current_password: "nope nope".into(), new_password: "another one".into() },
            ))
            .await;
        assert_eq!(wrong.unwrap_err().code(), Code::PermissionDenied);
    }
    let blocked = c
        .auth
        .change_password(authed(
            &juan,
            pb::ChangePasswordRequest { current_password: password.into(), new_password: "another one".into() },
        ))
        .await;
    assert_eq!(blocked.unwrap_err().code(), Code::ResourceExhausted);

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
    c.servers
        .join_server(authed(&mika, pb::JoinServerRequest { server_id: server.id.clone(), ..Default::default() }))
        .await
        .unwrap();
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
    c.servers
        .join_server(authed(&mika, pb::JoinServerRequest { server_id: server.id.clone(), ..Default::default() }))
        .await
        .unwrap();
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
    assert_eq!((&servers[0]["owner"], &servers[0]["roles"]), (&serde_json::json!(false), &serde_json::json!([])));
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
    assert_eq!(listed.messages.len(), 4, "three messages and the join message");
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

async fn send(
    c: &mut Clients,
    token: &str,
    server_id: &str,
    channel_id: &str,
    content: &str,
) -> Result<pb::Message, tonic::Status> {
    c.messages
        .send_message(authed(
            token,
            pb::SendMessageRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                content: content.into(),
                ..Default::default()
            },
        ))
        .await
        .map(|r| r.into_inner().message.unwrap())
}

async fn messages(c: &mut Clients, token: &str, server_id: &str, channel_id: &str) -> Vec<pb::Message> {
    c.messages
        .list_messages(authed(
            token,
            pb::ListMessagesRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .messages
}

async fn new_channel(c: &mut Clients, token: &str, server_id: &str, name: &str, kind: pb::ChannelType) -> pb::Channel {
    c.channels
        .create_channel(authed(
            token,
            pb::CreateChannelRequest {
                server_id: server_id.into(),
                name: name.into(),
                r#type: kind as i32,
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .channel
        .unwrap()
}

async fn audit_log(c: &mut Clients, token: &str, request: pb::ListAuditLogRequest) -> pb::ListAuditLogResponse {
    c.servers.list_audit_log(authed(token, request)).await.unwrap().into_inner()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_settings_and_moderation() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (juan, _, _) = sign_up(&mut c, "juan").await;
    let (mika, mika_user, _) = sign_up(&mut c, "mika").await;
    let (aoi, aoi_user, _) = sign_up(&mut c, "aoi").await;
    let server = create_server(&mut c, &juan, "Mods", true).await;
    let sid = server.id.clone();
    let general = c
        .channels
        .list_channels(authed(&juan, pb::ListChannelsRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels
        .remove(0);
    // New servers welcome people in #general, and leave notifications to each person.
    assert_eq!(server.system_channel_id, general.id);
    assert_eq!(server.default_notifications, pb::NotificationLevel::Unspecified as i32);

    // Joining posts a join message, which nobody can edit.
    for token in [&mika, &aoi] {
        c.servers
            .join_server(authed(token, pb::JoinServerRequest { server_id: sid.clone(), ..Default::default() }))
            .await
            .unwrap();
    }
    let welcomed = messages(&mut c, &juan, &sid, &general.id).await;
    assert_eq!(
        welcomed.iter().map(|m| (m.kind, m.author_id.as_str(), m.content.as_str())).collect::<Vec<_>>(),
        [
            (pb::MessageKind::MemberJoined as i32, mika_user.id.as_str(), ""),
            (pb::MessageKind::MemberJoined as i32, aoi_user.id.as_str(), "")
        ]
    );
    let edit = c
        .messages
        .update_message(authed(
            &mika,
            pb::UpdateMessageRequest {
                channel_id: String::new(),
                server_id: sid.clone(),
                message_id: welcomed[0].id.clone(),
                content: "hi".into(),
            },
        ))
        .await
        .unwrap_err();
    assert_eq!(edit.code(), Code::InvalidArgument);

    // Server settings: default notifications and where join messages go.
    let welcome = new_channel(&mut c, &juan, &sid, "welcome", pb::ChannelType::Text).await;
    let lounge = new_channel(&mut c, &juan, &sid, "Lounge", pb::ChannelType::Category).await;
    let update = |token: &str, level: Option<pb::NotificationLevel>, system: Option<&str>| {
        authed(
            token,
            pb::UpdateServerRequest {
                server_id: sid.clone(),
                default_notifications: level.map(|l| l as i32),
                system_channel_id: system.map(Into::into),
                ..Default::default()
            },
        )
    };
    let not_admin = c.servers.update_server(update(&mika, Some(pb::NotificationLevel::Mentions), None)).await;
    assert_eq!(not_admin.unwrap_err().code(), Code::PermissionDenied);
    let nothing = c.servers.update_server(update(&juan, Some(pb::NotificationLevel::Nothing), None)).await;
    assert_eq!(nothing.unwrap_err().code(), Code::InvalidArgument);
    let category = c.servers.update_server(update(&juan, None, Some(&lounge.id))).await;
    assert_eq!(category.unwrap_err().code(), Code::InvalidArgument);
    let updated = c
        .servers
        .update_server(update(&juan, Some(pb::NotificationLevel::Mentions), Some(&welcome.id)))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap();
    assert_eq!(
        (updated.default_notifications, updated.system_channel_id.as_str()),
        (pb::NotificationLevel::Mentions as i32, welcome.id.as_str())
    );
    let listed = c.servers.list_servers(authed(&mika, pb::ListServersRequest {})).await.unwrap().into_inner().servers;
    assert_eq!(listed[0].system_channel_id, welcome.id, "the index keeps the new settings");

    // New servers start with an Admin role, which only those above it hand out.
    let roles = c.roles.list_roles(authed(&mika, pb::ListRolesRequest { server_id: sid.clone() })).await.unwrap();
    let admin_role = roles.into_inner().roles.into_iter().find(|r| r.name == "Admin").unwrap();
    let give = |token: &str, user: &str, role: &str| {
        authed(token, pb::AddMemberRoleRequest { server_id: sid.clone(), user_id: user.into(), role_id: role.into() })
    };
    let promoted = c.roles.add_member_role(give(&juan, &mika_user.id, &admin_role.id)).await.unwrap().into_inner();
    assert_eq!(promoted.member.unwrap().role_ids, std::slice::from_ref(&admin_role.id));
    let by_admin = c.roles.add_member_role(give(&mika, &aoi_user.id, &admin_role.id)).await;
    assert_eq!(by_admin.unwrap_err().code(), Code::PermissionDenied);
    let everyone = c.roles.add_member_role(give(&juan, &aoi_user.id, &sid)).await;
    assert_eq!(everyone.unwrap_err().code(), Code::InvalidArgument);

    // Time-outs stop someone sending, until they end.
    let time_out = |token: &str, user: &str, seconds: i64| {
        authed(
            token,
            pb::TimeOutMemberRequest {
                server_id: sid.clone(),
                user_id: user.into(),
                seconds,
                reason: "cool off".into(),
            },
        )
    };
    let owner_id = server.owner_id.clone();
    let upward = c.servers.time_out_member(time_out(&mika, &owner_id, 60)).await;
    assert_eq!(upward.unwrap_err().code(), Code::PermissionDenied);
    let too_long = c.servers.time_out_member(time_out(&mika, &aoi_user.id, 29 * 24 * 60 * 60)).await;
    assert_eq!(too_long.unwrap_err().code(), Code::InvalidArgument);
    let timed_out =
        c.servers.time_out_member(time_out(&mika, &aoi_user.id, 60)).await.unwrap().into_inner().member.unwrap();
    assert!(timed_out.timed_out_until.is_some());
    let quiet = send(&mut c, &aoi, &sid, &general.id, "let me talk").await.unwrap_err();
    assert_eq!(quiet.code(), Code::PermissionDenied);
    assert!(quiet.message().contains("timed out"), "{}", quiet.message());
    let ended = c.servers.time_out_member(time_out(&mika, &aoi_user.id, 0)).await.unwrap().into_inner().member.unwrap();
    assert!(ended.timed_out_until.is_none());
    send(&mut c, &aoi, &sid, &general.id, "thanks").await.unwrap();

    // Slow mode holds members, not admins.
    let slow = |token: &str, channel: &str, seconds: i32| {
        authed(
            token,
            pb::UpdateChannelRequest {
                server_id: sid.clone(),
                channel_id: channel.into(),
                slowmode_seconds: Some(seconds),
                ..Default::default()
            },
        )
    };
    let too_slow = c.channels.update_channel(slow(&juan, &general.id, 21601)).await;
    assert_eq!(too_slow.unwrap_err().code(), Code::InvalidArgument);
    let slowed = c.channels.update_channel(slow(&juan, &general.id, 30)).await.unwrap().into_inner().channel.unwrap();
    assert_eq!(slowed.slowmode_seconds, 30);
    send(&mut c, &aoi, &sid, &general.id, "one").await.unwrap();
    let wait = send(&mut c, &aoi, &sid, &general.id, "two").await.unwrap_err();
    assert_eq!(wait.code(), Code::ResourceExhausted);
    assert!(wait.message().contains("30 seconds"), "{}", wait.message());
    for text in ["admins", "don't wait"] {
        send(&mut c, &mika, &sid, &general.id, text).await.unwrap();
    }
    // Two sent at once by the same person: one gets in.
    c.channels.update_channel(slow(&juan, &welcome.id, 60)).await.unwrap();
    let racing: Vec<_> = (0..6)
        .map(|n| {
            let mut messages = c.messages.clone();
            let request = authed(
                &aoi,
                pb::SendMessageRequest {
                    server_id: sid.clone(),
                    channel_id: welcome.id.clone(),
                    content: format!("race {n}"),
                    ..Default::default()
                },
            );
            tokio::spawn(async move { messages.send_message(request).await })
        })
        .collect();
    let mut landed = 0;
    for task in racing {
        match task.await.unwrap() {
            Ok(_) => landed += 1,
            Err(status) => assert_eq!(status.code(), Code::ResourceExhausted, "{status:?}"),
        }
    }
    assert_eq!(landed, 1);
    // Turning it off lets them talk again.
    c.channels.update_channel(slow(&juan, &general.id, 0)).await.unwrap();
    send(&mut c, &aoi, &sid, &general.id, "free").await.unwrap();

    // A moderator deleting someone's message is logged; deleting your own isn't.
    let spam = send(&mut c, &aoi, &sid, &general.id, "spam").await.unwrap();
    c.messages
        .delete_message(authed(
            &mika,
            pb::DeleteMessageRequest { channel_id: String::new(), server_id: sid.clone(), message_id: spam.id },
        ))
        .await
        .unwrap();
    let mine = send(&mut c, &mika, &sid, &general.id, "oops").await.unwrap();
    c.messages
        .delete_message(authed(
            &mika,
            pb::DeleteMessageRequest { channel_id: String::new(), server_id: sid.clone(), message_id: mine.id },
        ))
        .await
        .unwrap();

    // Kicks end the person's stream and let them come back.
    let mut aoi_stream = c
        .events
        .subscribe(authed(
            &aoi,
            pb::SubscribeRequest { servers: vec![pb::ServerCursor { server_id: sid.clone(), after_sequence: None }] },
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(aoi_stream.next().await.unwrap().unwrap().ready.is_some());
    let kick = |token: &str, user: &str| {
        authed(token, pb::KickMemberRequest { server_id: sid.clone(), user_id: user.into(), reason: "rude".into() })
    };
    assert_eq!(c.servers.kick_member(kick(&aoi, &mika_user.id)).await.unwrap_err().code(), Code::PermissionDenied);
    assert_eq!(c.servers.kick_member(kick(&mika, &mika_user.id)).await.unwrap_err().code(), Code::InvalidArgument);
    c.servers.kick_member(kick(&mika, &aoi_user.id)).await.unwrap();
    let mut left = None;
    while let Some(item) = tokio::time::timeout(Duration::from_secs(5), aoi_stream.next()).await.unwrap() {
        if let Some(Payload::MemberLeft(l)) = item.unwrap().event.and_then(|e| e.payload) {
            left = Some(l);
        }
    }
    let left = left.expect("aoi hears they were kicked, then the stream ends");
    assert_eq!((left.user_id.as_str(), left.reason), (aoi_user.id.as_str(), pb::LeaveReason::Kicked as i32));
    assert!(
        !c.servers
            .list_servers(authed(&aoi, pb::ListServersRequest {}))
            .await
            .unwrap()
            .into_inner()
            .servers
            .iter()
            .any(|s| s.id == sid)
    );
    // Coming back online afterwards, they hear they're out, rather than the stream failing.
    let mut later = c
        .events
        .subscribe(authed(
            &aoi,
            pb::SubscribeRequest {
                servers: vec![pb::ServerCursor { server_id: sid.clone(), after_sequence: Some(0) }],
            },
        ))
        .await
        .unwrap()
        .into_inner();
    let out = later.next().await.unwrap().unwrap().event.unwrap();
    assert!(matches!(out.payload, Some(Payload::MemberLeft(ref l)) if l.user_id == aoi_user.id));
    assert!(later.next().await.unwrap().unwrap().ready.unwrap().servers.is_empty());
    assert!(tokio::time::timeout(Duration::from_secs(5), later.next()).await.unwrap().is_none());
    c.servers
        .join_server(authed(&aoi, pb::JoinServerRequest { server_id: sid.clone(), ..Default::default() }))
        .await
        .unwrap();

    // Bans keep them out, and can take their recent messages with them.
    let before = usage(&mut c, &juan, &sid).await;
    let recent = messages(&mut c, &juan, &sid, &general.id).await.iter().filter(|m| m.author_id == aoi_user.id).count();
    let banned = c
        .servers
        .ban_member(authed(
            &juan,
            pb::BanMemberRequest {
                server_id: sid.clone(),
                user_id: aoi_user.id.clone(),
                reason: "raiding".into(),
                delete_message_seconds: 60 * 60,
            },
        ))
        .await
        .unwrap()
        .into_inner();
    // Everything they sent in #general and #welcome, their join messages included.
    assert_eq!(banned.deleted_messages as usize, recent + 2);
    assert_eq!(banned.ban.unwrap().user.unwrap().id, aoi_user.id);
    let after = usage(&mut c, &juan, &sid).await;
    assert_eq!((after.members, after.messages), (before.members - 1, before.messages - banned.deleted_messages));
    assert!(messages(&mut c, &juan, &sid, &general.id).await.iter().all(|m| m.author_id != aoi_user.id));
    let rejoin = c
        .servers
        .join_server(authed(&aoi, pb::JoinServerRequest { server_id: sid.clone(), ..Default::default() }))
        .await;
    assert_eq!(rejoin.unwrap_err().code(), Code::PermissionDenied);
    let twice = c
        .servers
        .ban_member(authed(
            &juan,
            pb::BanMemberRequest { server_id: sid.clone(), user_id: aoi_user.id.clone(), ..Default::default() },
        ))
        .await;
    assert_eq!(twice.unwrap_err().code(), Code::AlreadyExists);
    let bans =
        c.servers.list_bans(authed(&mika, pb::ListBansRequest { server_id: sid.clone() })).await.unwrap().into_inner();
    assert_eq!(bans.bans.len(), 1);
    assert_eq!((bans.bans[0].reason.as_str(), bans.bans[0].banned_by_id.as_str()), ("raiding", owner_id.as_str()));
    assert_eq!(bans.moderators.iter().map(|u| u.username.as_str()).collect::<Vec<_>>(), ["juan"]);
    c.servers
        .unban_member(authed(&mika, pb::UnbanMemberRequest { server_id: sid.clone(), user_id: aoi_user.id.clone() }))
        .await
        .unwrap();
    c.servers
        .join_server(authed(&aoi, pb::JoinServerRequest { server_id: sid.clone(), ..Default::default() }))
        .await
        .unwrap();

    // Reordering: every channel once, categories at the top level.
    let place = |id: &str, parent: &str| pb::ChannelPlacement { channel_id: id.into(), parent_id: parent.into() };
    let reorder = |channels: Vec<pb::ChannelPlacement>| {
        authed(&juan, pb::ReorderChannelsRequest { server_id: sid.clone(), channels })
    };
    let missing = c.channels.reorder_channels(reorder(vec![place(&general.id, "")])).await;
    assert_eq!(missing.unwrap_err().code(), Code::FailedPrecondition);
    let nested = c
        .channels
        .reorder_channels(reorder(vec![place(&lounge.id, &lounge.id), place(&general.id, ""), place(&welcome.id, "")]))
        .await;
    assert_eq!(nested.unwrap_err().code(), Code::InvalidArgument);
    let ordered = c
        .channels
        .reorder_channels(reorder(vec![place(&welcome.id, ""), place(&lounge.id, ""), place(&general.id, &lounge.id)]))
        .await
        .unwrap()
        .into_inner()
        .channels;
    assert_eq!(
        ordered.iter().map(|ch| (ch.name.as_str(), ch.position, ch.parent_id.as_str())).collect::<Vec<_>>(),
        [("welcome", 0, ""), ("Lounge", 1, ""), ("general", 2, lounge.id.as_str())]
    );
    // Only people who can manage channels move them around.
    let everyone_out = vec![place(&general.id, ""), place(&welcome.id, ""), place(&lounge.id, "")];
    let viewer = c
        .channels
        .reorder_channels(authed(
            &aoi,
            pb::ReorderChannelsRequest { server_id: sid.clone(), channels: everyone_out.clone() },
        ))
        .await;
    assert_eq!(viewer.unwrap_err().code(), Code::PermissionDenied);
    // A channel leaves its category and goes to the top in the same move.
    let out = c.channels.reorder_channels(reorder(everyone_out)).await.unwrap().into_inner().channels;
    assert_eq!(
        out.iter().map(|ch| (ch.name.as_str(), ch.position, ch.parent_id.as_str())).collect::<Vec<_>>(),
        [("general", 0, ""), ("welcome", 1, ""), ("Lounge", 2, "")]
    );

    // Deleting the system channel stops join messages.
    c.channels
        .delete_channel(authed(
            &juan,
            pb::DeleteChannelRequest { server_id: sid.clone(), channel_id: welcome.id.clone() },
        ))
        .await
        .unwrap();
    let server_now = c
        .servers
        .get_server(authed(&juan, pb::GetServerRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap();
    assert_eq!(server_now.system_channel_id, "");

    // The audit log: newest first, filtered by who and what, only for managers.
    let log = audit_log(&mut c, &juan, pb::ListAuditLogRequest { server_id: sid.clone(), ..Default::default() }).await;
    let actions: Vec<pb::AuditAction> = log.entries.iter().map(|e| e.action()).collect();
    use pb::AuditAction as A;
    assert_eq!(
        actions,
        [
            A::ChannelDelete,
            A::ChannelsReorder,
            A::ChannelsReorder,
            A::MemberUnban,
            A::MemberBan,
            A::MemberKick,
            A::MessageDelete,
            A::ChannelUpdate,
            A::ChannelUpdate,
            A::ChannelUpdate,
            A::MemberTimeOut,
            A::MemberTimeOut,
            A::MemberRolesUpdate,
            A::ServerUpdate,
            A::ChannelCreate,
            A::ChannelCreate,
        ]
    );
    let ban = &log.entries[4];
    assert_eq!((ban.reason.as_str(), ban.target_id.as_str()), ("raiding", aoi_user.id.as_str()));
    let deleted = &log.entries[6];
    assert_eq!((deleted.target_id.as_str(), deleted.channel_name.as_str()), (aoi_user.id.as_str(), "general"));
    let server_change = &log.entries[13];
    assert_eq!(
        server_change.changes.iter().map(|ch| ch.field.as_str()).collect::<Vec<_>>(),
        ["default_notifications", "system_channel_id"]
    );
    assert_eq!(
        (server_change.changes[1].before.as_str(), server_change.changes[1].after.as_str()),
        (general.id.as_str(), welcome.id.as_str())
    );
    let mut names: Vec<_> = log.users.iter().map(|u| u.username.as_str()).collect();
    names.sort();
    assert_eq!(names, ["aoi", "juan", "mika"]);
    let by_mika = audit_log(
        &mut c,
        &juan,
        pb::ListAuditLogRequest { server_id: sid.clone(), actor_id: mika_user.id.clone(), ..Default::default() },
    )
    .await;
    assert_eq!(
        by_mika.entries.iter().map(|e| e.action()).collect::<Vec<_>>(),
        [A::MemberUnban, A::MemberKick, A::MessageDelete, A::MemberTimeOut, A::MemberTimeOut]
    );
    let bans_only = audit_log(
        &mut c,
        &juan,
        pb::ListAuditLogRequest { server_id: sid.clone(), action: A::MemberBan as i32, ..Default::default() },
    )
    .await;
    assert_eq!(bans_only.entries.len(), 1);
    let first_page =
        audit_log(&mut c, &juan, pb::ListAuditLogRequest { server_id: sid.clone(), limit: 10, ..Default::default() })
            .await;
    assert!(first_page.has_more);
    let second_page = audit_log(
        &mut c,
        &juan,
        pb::ListAuditLogRequest {
            server_id: sid.clone(),
            limit: 10,
            before_id: first_page.entries.last().unwrap().id.clone(),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(second_page.entries.len(), 6);
    assert!(!second_page.has_more);
    let hidden = c
        .servers
        .list_audit_log(authed(&aoi, pb::ListAuditLogRequest { server_id: sid.clone(), ..Default::default() }))
        .await;
    assert_eq!(hidden.unwrap_err().code(), Code::PermissionDenied);

    // Handing the server on: the new owner owns it, the old one stays as an admin.
    let transfer = |token: &str, user: &str| {
        authed(token, pb::TransferOwnershipRequest { server_id: sid.clone(), user_id: user.into() })
    };
    assert_eq!(
        c.servers.transfer_ownership(transfer(&mika, &aoi_user.id)).await.unwrap_err().code(),
        Code::PermissionDenied
    );
    let handed =
        c.servers.transfer_ownership(transfer(&juan, &mika_user.id)).await.unwrap().into_inner().server.unwrap();
    assert_eq!(handed.owner_id, mika_user.id);
    let members = c
        .servers
        .list_members(authed(&juan, pb::ListMembersRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner()
        .members;
    assert_eq!(members[0].user.as_ref().unwrap().id, mika_user.id, "the owner ranks first");
    let roles_of = |id: &str| members.iter().find(|m| m.user.as_ref().unwrap().id == id).unwrap().role_ids.clone();
    assert!(roles_of(&owner_id).is_empty(), "the old owner keeps their roles, which were none");
    // The old owner no longer hands out roles, and can leave like anyone else.
    let roles = c.roles.list_roles(authed(&juan, pb::ListRolesRequest { server_id: sid.clone() })).await.unwrap();
    let admin_role = roles.into_inner().roles.into_iter().find(|r| r.name == "Admin").unwrap();
    let old_owner = c
        .roles
        .add_member_role(authed(
            &juan,
            pb::AddMemberRoleRequest {
                server_id: sid.clone(),
                user_id: aoi_user.id.clone(),
                role_id: admin_role.id.clone(),
            },
        ))
        .await;
    assert_eq!(old_owner.unwrap_err().code(), Code::PermissionDenied);
    c.servers.leave_server(authed(&juan, pb::LeaveServerRequest { server_id: sid.clone() })).await.unwrap();

    instance.stop().await;
}

async fn accounts(c: &mut Clients, token: &str, request: pb::ListAccountsRequest) -> pb::ListAccountsResponse {
    c.admin.list_accounts(authed(token, request)).await.unwrap().into_inner()
}

async fn update_account(
    c: &mut Clients,
    token: &str,
    request: pb::UpdateAccountRequest,
) -> Result<pb::AccountSummary, tonic::Status> {
    c.admin.update_account(authed(token, request)).await.map(|r| r.into_inner().account.unwrap())
}

async fn announce(
    c: &mut Clients,
    token: &str,
    announcement: pb::Announcement,
) -> Result<Option<pb::Announcement>, Code> {
    c.admin
        .set_announcement(authed(token, pb::SetAnnouncementRequest { announcement: Some(announcement) }))
        .await
        .map(|r| r.into_inner().announcement)
        .map_err(|e| e.code())
}

async fn node_announcement(c: &mut Clients) -> Option<pb::Announcement> {
    c.node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap().announcement
}

#[tokio::test]
async fn instance_admins_manage_accounts_servers_and_announcements() {
    let dir = tempfile::tempdir().unwrap();
    // Encrypted, to show exports come out as plain SQLite anyway.
    let key = [("FUWA_ENCRYPTION_KEY", "b1bbfda4f589dc9daaf004fe21111e00dc00c98237102f5c7002a5669fc76327")];
    let instance = start(dir.path(), &key).await;
    let mut c = clients(&instance).await;
    let (juan, juan_user, _) = sign_up(&mut c, "juan").await;
    let (mika, mika_user, _) = sign_up(&mut c, "mika").await;
    let (kai, kai_user, _) = sign_up(&mut c, "kai").await;

    // ── The accounts list: admins only, newest first, searchable, in pages.
    let denied = c.admin.list_accounts(authed(&mika, pb::ListAccountsRequest::default())).await;
    assert_eq!(denied.unwrap_err().code(), Code::PermissionDenied);
    let all = accounts(&mut c, &juan, pb::ListAccountsRequest::default()).await;
    let names: Vec<_> = all.accounts.iter().map(|a| a.user.as_ref().unwrap().username.as_str()).collect();
    assert_eq!(names, ["kai", "mika", "juan"]);
    let totals = all.totals.unwrap();
    assert_eq!((totals.all, totals.admins, totals.disabled), (3, 1, 0));
    assert!(all.accounts[2].admin && all.accounts[2].sessions == 1 && all.accounts[2].created_at.is_some());
    let found = accounts(&mut c, &juan, pb::ListAccountsRequest { query: "MIK".into(), ..Default::default() }).await;
    assert_eq!(found.accounts.len(), 1);
    let page = accounts(&mut c, &juan, pb::ListAccountsRequest { limit: 2, ..Default::default() }).await;
    assert!(page.has_more && page.accounts.len() == 2);
    let rest = accounts(
        &mut c,
        &juan,
        pb::ListAccountsRequest {
            limit: 2,
            before_id: page.accounts[1].user.as_ref().unwrap().id.clone(),
            ..Default::default()
        },
    )
    .await;
    assert!(!rest.has_more && rest.accounts[0].user.as_ref().unwrap().username == "juan");

    // ── Admin rights: never your own, and never the last admin.
    let own = update_account(
        &mut c,
        &juan,
        pb::UpdateAccountRequest { account_id: juan_user.id.clone(), admin: Some(false), ..Default::default() },
    )
    .await;
    assert_eq!(own.unwrap_err().code(), Code::FailedPrecondition);
    let promoted = update_account(
        &mut c,
        &juan,
        pb::UpdateAccountRequest { account_id: mika_user.id.clone(), admin: Some(true), ..Default::default() },
    )
    .await
    .unwrap();
    assert!(promoted.admin);
    assert!(c.admin.list_accounts(authed(&mika, pb::ListAccountsRequest::default())).await.is_ok());

    // ── Turning an account off: admins lose that first, devices sign out, sign-in stops.
    let still_admin = update_account(
        &mut c,
        &juan,
        pb::UpdateAccountRequest { account_id: mika_user.id.clone(), disabled: Some(true), ..Default::default() },
    )
    .await;
    assert_eq!(still_admin.unwrap_err().code(), Code::FailedPrecondition);
    let off = update_account(
        &mut c,
        &juan,
        pb::UpdateAccountRequest {
            account_id: mika_user.id.clone(),
            admin: Some(false),
            disabled: Some(true),
            reason: "Spam from this account".into(),
        },
    )
    .await
    .unwrap();
    assert!(!off.admin && off.disabled && off.disabled_at.is_some() && off.sessions == 0);
    assert_eq!(off.disabled_reason, "Spam from this account");
    assert_eq!(me(&mut c, &mika).await.unwrap_err(), Code::Unauthenticated);
    let refused = sign_in(&mut c, "mika", "correct horse battery").await.unwrap_err();
    assert_eq!(refused.code(), Code::PermissionDenied);
    assert!(refused.message().contains("turned off"));
    let disabled = accounts(
        &mut c,
        &juan,
        pb::ListAccountsRequest { filter: pb::AccountFilter::Disabled as i32, ..Default::default() },
    )
    .await;
    assert_eq!(disabled.accounts.len(), 1);
    assert_eq!(disabled.totals.unwrap().disabled, 1);
    let back = update_account(
        &mut c,
        &juan,
        pb::UpdateAccountRequest { account_id: mika_user.id.clone(), disabled: Some(false), ..Default::default() },
    )
    .await
    .unwrap();
    assert!(!back.disabled && back.disabled_reason.is_empty());
    assert!(sign_in(&mut c, "mika", "correct horse battery").await.is_ok());

    // The operator's token can do what an admin can, but not remove the last admin.
    let last = update_account(
        &mut c,
        ADMIN_TOKEN,
        pb::UpdateAccountRequest { account_id: juan_user.id.clone(), admin: Some(false), ..Default::default() },
    )
    .await;
    assert_eq!(last.unwrap_err().code(), Code::FailedPrecondition);

    // ── Password resets: a new password shown once, every device signed out.
    let reset = c
        .admin
        .reset_account_password(authed(
            &juan,
            pb::ResetAccountPasswordRequest { account_id: kai_user.id.clone(), turn_off_two_factor: true },
        ))
        .await
        .unwrap()
        .into_inner()
        .password;
    assert_eq!(reset.len(), 19);
    assert_eq!(me(&mut c, &kai).await.unwrap_err(), Code::Unauthenticated);
    assert!(sign_in(&mut c, "kai", "correct horse battery").await.is_err());
    let kai = sign_in(&mut c, "kai", &reset).await.unwrap().token;
    let own_reset = c
        .admin
        .reset_account_password(authed(
            &juan,
            pb::ResetAccountPasswordRequest { account_id: juan_user.id.clone(), ..Default::default() },
        ))
        .await;
    assert_eq!(own_reset.unwrap_err().code(), Code::FailedPrecondition);

    // ── Every server, members or not, and a plain SQLite copy of one.
    let corner = create_server(&mut c, &kai, "Kai's Corner!", false).await;
    create_server(&mut c, &juan, "Admin HQ", false).await;
    let general = c
        .channels
        .list_channels(authed(&kai, pb::ListChannelsRequest { server_id: corner.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels
        .into_iter()
        .find(|ch| ch.name == "general")
        .unwrap();
    send(&mut c, &kai, &corner.id, &general.id, "only in the export").await.unwrap();
    let servers = c
        .admin
        .list_instance_servers(authed(&juan, pb::ListInstanceServersRequest {}))
        .await
        .unwrap()
        .into_inner()
        .servers;
    assert_eq!(servers.len(), 2);
    let listed = servers.iter().find(|s| s.server.as_ref().unwrap().id == corner.id).unwrap();
    assert_eq!(listed.owner.as_ref().unwrap().id, kai_user.id);
    assert!(!listed.member && listed.usage.as_ref().unwrap().messages >= 1 && listed.limits.is_some());
    assert!(servers.iter().any(|s| s.member));

    let denied = c.admin.export_server(authed(&kai, pb::ExportServerRequest { server_id: corner.id.clone() })).await;
    assert_eq!(denied.unwrap_err().code(), Code::PermissionDenied);
    let mut stream = c
        .admin
        .export_server(authed(&juan, pb::ExportServerRequest { server_id: corner.id.clone() }))
        .await
        .unwrap()
        .into_inner();
    let mut bytes = Vec::new();
    let mut first = None;
    while let Some(piece) = stream.next().await {
        let piece = piece.unwrap();
        first.get_or_insert((piece.size, piece.filename.clone()));
        bytes.extend_from_slice(&piece.chunk);
    }
    let (size, filename) = first.unwrap();
    assert_eq!(filename, "kai-s-corner.db");
    assert_eq!(size as usize, bytes.len());
    assert!(bytes.starts_with(b"SQLite format 3\0"));
    assert_eq!([bytes[18], bytes[19]], [2, 2], "exports come out in WAL mode");
    let copy = dir.path().join("copy.db");
    std::fs::write(&copy, &bytes).unwrap();
    let exported = turso::Builder::new_local(copy.to_str().unwrap()).build().await.unwrap();
    let conn = exported.connect().unwrap();
    let mut rows = conn.query("SELECT content FROM messages WHERE kind = 0", ()).await.unwrap();
    let row = rows.next().await.unwrap().unwrap();
    assert_eq!(row.get::<String>(0).unwrap(), "only in the export");
    let leftovers = std::fs::read_dir(dir.path().join("exports")).unwrap().count();
    assert_eq!(leftovers, 0, "the export's file is removed once it's sent");

    // ── The announcement banner: on every client, even signed out.
    assert!(node_announcement(&mut c).await.is_none());
    let denied =
        announce(&mut c, &kai, pb::Announcement { text: "hi".into(), ..Default::default() }).await.unwrap_err();
    assert_eq!(denied, Code::PermissionDenied);
    let up = announce(
        &mut c,
        &juan,
        pb::Announcement {
            text: "Maintenance tonight at **22:00 UTC**".into(),
            tone: pb::AnnouncementTone::Warning as i32,
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!up.id.is_empty() && up.created_at.is_some());
    assert_eq!(node_announcement(&mut c).await.unwrap().id, up.id);
    // Same text, new tone: same banner, so anyone who closed it isn't shown it again.
    let critical = announce(
        &mut c,
        &juan,
        pb::Announcement { text: up.text.clone(), tone: pb::AnnouncementTone::Critical as i32, ..Default::default() },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(critical.id, up.id);
    assert_eq!(critical.tone, pb::AnnouncementTone::Critical as i32);
    let past = fuwa_server::id::timestamp(fuwa_server::id::now_ms() - 1000);
    let late =
        announce(&mut c, &juan, pb::Announcement { text: "late".into(), ends_at: Some(past), ..Default::default() })
            .await;
    assert_eq!(late.unwrap_err(), Code::InvalidArgument);
    let soon = fuwa_server::id::timestamp(fuwa_server::id::now_ms() + 800);
    let brief = announce(
        &mut c,
        &juan,
        pb::Announcement { text: "Back soon".into(), ends_at: Some(soon), ..Default::default() },
    )
    .await
    .unwrap()
    .unwrap();
    assert_ne!(brief.id, up.id);
    assert_eq!(node_announcement(&mut c).await.unwrap().text, "Back soon");
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(node_announcement(&mut c).await.is_none(), "it comes down by itself once it runs out");
    announce(&mut c, &juan, pb::Announcement { text: "Stays up".into(), ..Default::default() }).await.unwrap();
    drop(c);
    instance.stop().await;

    // It survives a restart; empty text takes it down.
    let instance = start(dir.path(), &key).await;
    let mut c = clients(&instance).await;
    assert_eq!(node_announcement(&mut c).await.unwrap().text, "Stays up");
    let down = announce(&mut c, &juan, pb::Announcement::default()).await.unwrap();
    assert!(down.is_none() && node_announcement(&mut c).await.is_none());
    instance.stop().await;
}

/// A PNG's chunks, `size` bytes long (at least [`PNG_MIN`]): a header, a
/// private chunk of `fill` bytes, and the end, so it's kept as it is.
fn png(size: usize, fill: u8) -> Vec<u8> {
    assert!(size >= PNG_MIN, "a test PNG is at least {PNG_MIN} bytes");
    let chunk =
        |kind: &[u8; 4], data: &[u8]| [&(data.len() as u32).to_be_bytes()[..], kind, data, &[0, 0, 0, 0]].concat();
    [
        &b"\x89PNG\r\n\x1a\n"[..],
        &chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]),
        &chunk(b"fuWa", &vec![fill; size - PNG_MIN]),
        &chunk(b"IEND", &[]),
    ]
    .concat()
}

const PNG_MIN: usize = 8 + 25 + 12 + 12;

/// The same address on the test instance: links are made with the public
/// URL, which the test doesn't know before the port is picked.
fn on(instance: &Instance, url: &str) -> String {
    let url = reqwest::Url::parse(url).unwrap();
    format!("http://{}{}", instance.addr, url.path())
}

async fn create_upload(
    c: &mut Clients,
    token: &str,
    purpose: pb::MediaPurpose,
    content_type: &str,
    size: usize,
) -> Result<pb::CreateUploadResponse, tonic::Status> {
    c.media
        .create_upload(authed(
            token,
            pb::CreateUploadRequest { purpose: purpose as i32, content_type: content_type.into(), size: size as i64 },
        ))
        .await
        .map(|r| r.into_inner())
}

async fn put(instance: &Instance, upload_url: &str, bytes: Vec<u8>) -> reqwest::StatusCode {
    reqwest::Client::new().put(on(instance, upload_url)).body(bytes).send().await.unwrap().status()
}

async fn upload(
    c: &mut Clients,
    instance: &Instance,
    token: &str,
    purpose: pb::MediaPurpose,
    bytes: Vec<u8>,
) -> String {
    let reserved = create_upload(c, token, purpose, "image/png", bytes.len()).await.unwrap();
    assert_eq!(put(instance, &reserved.upload_url, bytes).await, reqwest::StatusCode::NO_CONTENT);
    reserved.media.unwrap().url
}

async fn fetch(instance: &Instance, url: &str) -> (reqwest::StatusCode, reqwest::header::HeaderMap, Vec<u8>) {
    let response = reqwest::get(on(instance, url)).await.unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    (status, headers, response.bytes().await.unwrap().to_vec())
}

#[tokio::test]
async fn backgrounds_are_kept_listed_and_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[("FUWA_PUBLIC_URL", "https://chat.example.com")]).await;
    let mut c = clients(&instance).await;
    let (juan, _, _) = sign_up(&mut c, "juan").await;
    let (mika, _, _) = sign_up(&mut c, "mika").await;
    let background = pb::MediaPurpose::Background;
    let keep = |token: &str, url: &str| authed(token, pb::KeepBackgroundRequest { url: url.into() });
    let list = async |c: &mut Clients, token: &str| {
        let listed = c.media.list_backgrounds(authed(token, pb::ListBackgroundsRequest {})).await.unwrap();
        listed.into_inner().backgrounds.into_iter().map(|m| m.url).collect::<Vec<_>>()
    };

    // A background is uploaded like any picture, then kept.
    let first = upload(&mut c, &instance, &juan, background, png(400, 1)).await;
    let kept = c.media.keep_background(keep(&juan, &first)).await.unwrap().into_inner().media.unwrap();
    assert_eq!((kept.url.as_str(), kept.content_type.as_str(), kept.size), (first.as_str(), "image/png", 400));
    let second = upload(&mut c, &instance, &juan, background, png(300, 2)).await;
    c.media.keep_background(keep(&juan, &second)).await.unwrap();
    c.media.keep_background(keep(&juan, &second)).await.unwrap();
    assert_eq!(list(&mut c, &juan).await, [second.clone(), first.clone()], "newest first, once each");
    assert!(list(&mut c, &mika).await.is_empty());

    // Only your own background uploads can be kept: not someone else's, not
    // an avatar, not a link elsewhere.
    let stolen = c.media.keep_background(keep(&mika, &first)).await;
    assert_eq!(stolen.unwrap_err().code(), Code::PermissionDenied);
    let avatar = upload(&mut c, &instance, &juan, pb::MediaPurpose::Avatar, png(100, 3)).await;
    assert_eq!(c.media.keep_background(keep(&juan, &avatar)).await.unwrap_err().code(), Code::PermissionDenied);
    let elsewhere = c.media.keep_background(keep(&juan, "https://example.com/bg.png")).await;
    assert_eq!(elsewhere.unwrap_err().code(), Code::InvalidArgument);

    // Kept backgrounds aren't swept; ones never kept are.
    let loose = upload(&mut c, &instance, &juan, background, png(200, 4)).await;
    instance.app.sweep_media(fuwa_server::id::now_ms() + fuwa_server::media::UNUSED_TTL_MS + 1).await.unwrap();
    assert_eq!(fetch(&instance, &loose).await.0, reqwest::StatusCode::NOT_FOUND);
    assert_eq!(fetch(&instance, &first).await.2, png(400, 1));

    // Deleting one removes it and its file; only its owner can.
    let theirs = c.media.delete_background(authed(&mika, pb::DeleteBackgroundRequest { url: first.clone() })).await;
    assert_eq!(theirs.unwrap_err().code(), Code::PermissionDenied);
    c.media.delete_background(authed(&juan, pb::DeleteBackgroundRequest { url: first.clone() })).await.unwrap();
    c.media.delete_background(authed(&juan, pb::DeleteBackgroundRequest { url: first.clone() })).await.unwrap();
    assert_eq!(fetch(&instance, &first).await.0, reqwest::StatusCode::NOT_FOUND);
    assert_eq!(list(&mut c, &juan).await, std::slice::from_ref(&second));

    // There's a cap on how many one account keeps.
    for n in 1..fuwa_server::media::MAX_BACKGROUNDS {
        let url = upload(&mut c, &instance, &mika, background, png(60, n as u8)).await;
        c.media.keep_background(keep(&mika, &url)).await.unwrap();
    }
    let last = upload(&mut c, &instance, &mika, background, png(60, 0)).await;
    c.media.keep_background(keep(&mika, &last)).await.unwrap();
    let over = upload(&mut c, &instance, &mika, background, png(61, 0)).await;
    assert_eq!(c.media.keep_background(keep(&mika, &over)).await.unwrap_err().code(), Code::ResourceExhausted);
}

#[tokio::test]
async fn pictures_are_kept_without_where_they_were_taken() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[("FUWA_PUBLIC_URL", "https://chat.example.com")]).await;
    let mut c = clients(&instance).await;
    let (juan, _, _) = sign_up(&mut c, "juan").await;
    let avatar = pb::MediaPurpose::Avatar;
    let send = async |c: &mut Clients, kind: &str, bytes: Vec<u8>| {
        let reserved = create_upload(c, &juan, avatar, kind, bytes.len()).await.unwrap();
        let status = put(&instance, &reserved.upload_url, bytes).await;
        (status, reserved.media.unwrap().url)
    };

    // A phone photo: its EXIF names where it was taken, and another picture
    // with its own EXIF follows the end.
    let segment =
        |marker: u8, data: &[u8]| [&[0xff, marker][..], &((data.len() + 2) as u16).to_be_bytes(), data].concat();
    let picture =
        [segment(0xdb, &[0; 65]), segment(0xda, &[1, 1, 0, 0, 0x3f, 0]), vec![0x12, 0x34, 0xff, 0xd9]].concat();
    let photo = [
        &[0xff, 0xd8][..],
        &segment(0xe1, b"Exif\0\0GPS 35.6812N 139.7671E"),
        &picture,
        &[0xff, 0xd8],
        &segment(0xe1, b"Exif\0\0GPS again"),
        &[0xff, 0xd9],
    ]
    .concat();
    let (status, url) = send(&mut c, "image/jpeg", photo).await;
    assert_eq!(status, reqwest::StatusCode::NO_CONTENT);
    let (status, headers, body) = fetch(&instance, &url).await;
    assert_eq!(status, reqwest::StatusCode::OK);
    assert_eq!(body, [&[0xff, 0xd8][..], &picture].concat());
    assert_eq!(headers[reqwest::header::CONTENT_LENGTH], body.len().to_string().as_str());

    // A PNG's text chunks go too.
    let with_text = {
        let plain = png(100, 1);
        let text = [&5u32.to_be_bytes()[..], b"tEXt", b"GPS\0x", &[0; 4]].concat();
        [&plain[..33], &text, &plain[33..]].concat()
    };
    let (status, url) = send(&mut c, "image/png", with_text).await;
    assert_eq!(status, reqwest::StatusCode::NO_CONTENT);
    assert_eq!(fetch(&instance, &url).await.2, png(100, 1));

    // One whose chunks can't be followed isn't kept, metadata and all.
    let mut broken = b"\x89PNG\r\n\x1a\n".to_vec();
    broken.resize(100, 0xff);
    let (status, url) = send(&mut c, "image/png", broken).await;
    assert_eq!(status, reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(fetch(&instance, &url).await.0, reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pictures_upload_serve_and_clean_up() {
    let dir = tempfile::tempdir().unwrap();
    let instance =
        start(dir.path(), &[("FUWA_LIMIT_PICTURE_UPLOAD", "4KB"), ("FUWA_PUBLIC_URL", "https://chat.example.com")])
            .await;
    let mut c = clients(&instance).await;
    let (juan, juan_user, _) = sign_up(&mut c, "juan").await;
    let (mika, _, _) = sign_up(&mut c, "mika").await;
    let avatar = pb::MediaPurpose::Avatar;

    // What an upload may be is checked up front.
    let not_an_upload = create_upload(&mut c, &juan, pb::MediaPurpose::Unspecified, "image/png", 10).await;
    assert_eq!(not_an_upload.unwrap_err().code(), Code::InvalidArgument);
    let svg = create_upload(&mut c, &juan, avatar, "image/svg+xml", 10).await;
    assert_eq!(svg.unwrap_err().code(), Code::InvalidArgument);
    let too_big = create_upload(&mut c, &juan, avatar, "image/png", 4001).await.unwrap_err();
    assert_eq!(too_big.code(), Code::ResourceExhausted);
    assert!(too_big.message().contains("4 KB"), "{}", too_big.message());
    let anonymous = c
        .media
        .create_upload(pb::CreateUploadRequest { purpose: avatar as i32, content_type: "image/png".into(), size: 10 })
        .await;
    assert_eq!(anonymous.unwrap_err().code(), Code::Unauthenticated);

    // An upload: reserve, PUT the bytes, then it's served at its link.
    let reserved = create_upload(&mut c, &juan, avatar, "image/png", 300).await.unwrap();
    assert!(reserved.upload_url.starts_with("https://chat.example.com/media/upload/"));
    let media = reserved.media.clone().unwrap();
    assert_eq!(media.url, format!("https://chat.example.com/media/{}", media.id));
    let (status, _, _) = fetch(&instance, &media.url).await;
    assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "not served before its bytes arrive");
    assert_eq!(put(&instance, &reserved.upload_url, png(300, 1)).await, reqwest::StatusCode::NO_CONTENT);
    let (status, headers, body) = fetch(&instance, &media.url).await;
    assert_eq!(status, reqwest::StatusCode::OK);
    assert_eq!(body, png(300, 1));
    assert_eq!(headers["content-type"], "image/png");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["cross-origin-resource-policy"], "cross-origin");
    assert!(headers["cache-control"].to_str().unwrap().contains("immutable"));
    let cached = reqwest::Client::new()
        .get(on(&instance, &media.url))
        .header("if-none-match", headers["etag"].clone())
        .send()
        .await
        .unwrap();
    assert_eq!(cached.status(), reqwest::StatusCode::NOT_MODIFIED);
    let first_avatar = media.url;

    // Browsers on other sites (any fuwa client) may send the bytes.
    let preflight = reqwest::Client::new()
        .request(reqwest::Method::OPTIONS, on(&instance, &reserved.upload_url))
        .header("origin", "https://app.example")
        .header("access-control-request-method", "PUT")
        .header("access-control-request-headers", "content-type")
        .send()
        .await
        .unwrap();
    assert!(preflight.headers()["access-control-allow-methods"].to_str().unwrap().contains("PUT"));

    // A link works once, and the bytes must be the picture they said they were.
    assert_eq!(put(&instance, &reserved.upload_url, png(300, 1)).await, reqwest::StatusCode::NOT_FOUND);
    let lie = create_upload(&mut c, &juan, avatar, "image/png", 64).await.unwrap();
    let mut script = b"<svg xmlns='http://www.w3.org/2000/svg'><script>".to_vec();
    script.resize(64, b' ');
    assert_eq!(put(&instance, &lie.upload_url, script).await, reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(fetch(&instance, &lie.media.unwrap().url).await.0, reqwest::StatusCode::NOT_FOUND);
    let long = create_upload(&mut c, &juan, avatar, "image/png", 64).await.unwrap();
    assert_eq!(put(&instance, &long.upload_url, png(65, 0)).await, reqwest::StatusCode::PAYLOAD_TOO_LARGE);
    let short = create_upload(&mut c, &juan, avatar, "image/png", 64).await.unwrap();
    assert_eq!(put(&instance, &short.upload_url, png(63, 0)).await, reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(put(&instance, &short.upload_url, png(64, 0)).await, reqwest::StatusCode::NOT_FOUND);

    // Setting it as your avatar keeps it. Only its uploader can use it, and
    // only for what it was uploaded for.
    let set_avatar = |url: &str| pb::UpdateProfileRequest { avatar_url: Some(url.into()), ..Default::default() };
    let set_banner = |url: &str| pb::UpdateProfileRequest { banner_url: Some(url.into()), ..Default::default() };
    c.auth.update_profile(authed(&juan, set_avatar(&first_avatar))).await.unwrap();
    assert_eq!(me(&mut c, &juan).await.unwrap().avatar_url, first_avatar);
    let stolen = c.auth.update_profile(authed(&mika, set_avatar(&first_avatar))).await;
    assert_eq!(stolen.unwrap_err().code(), Code::PermissionDenied);
    let wrong_use = c.auth.update_profile(authed(&juan, set_banner(&first_avatar))).await;
    assert_eq!(wrong_use.unwrap_err().code(), Code::PermissionDenied);
    let unfinished = create_upload(&mut c, &juan, avatar, "image/png", 10).await.unwrap().media.unwrap();
    let early = c.auth.update_profile(authed(&juan, set_avatar(&unfinished.url))).await;
    assert_eq!(early.unwrap_err().code(), Code::FailedPrecondition);
    // Links elsewhere still work as before.
    c.auth.update_profile(authed(&mika, set_avatar("https://example.com/mika.png"))).await.unwrap();

    // A new avatar replaces the old one, which is deleted.
    let second_avatar = upload(&mut c, &instance, &juan, avatar, png(200, 2)).await;
    c.auth.update_profile(authed(&juan, set_avatar(&second_avatar))).await.unwrap();
    assert_eq!(fetch(&instance, &first_avatar).await.0, reqwest::StatusCode::NOT_FOUND);
    let first_id = first_avatar.rsplit('/').next().unwrap();
    assert!(!dir.path().join("media").join(first_id).exists());
    assert_eq!(fetch(&instance, &second_avatar).await.2, png(200, 2));
    let banner = upload(&mut c, &instance, &juan, pb::MediaPurpose::Banner, png(500, 3)).await;
    c.auth.update_profile(authed(&juan, set_banner(&banner))).await.unwrap();
    let profile = c.auth.get_profile(authed(&juan, pb::GetProfileRequest { user_id: juan_user.id.clone() })).await;
    assert_eq!(profile.unwrap().into_inner().profile.unwrap().banner_url, banner);

    // Server icons: set when the server is made, replaced later.
    let icon = upload(&mut c, &instance, &juan, pb::MediaPurpose::ServerIcon, png(100, 4)).await;
    let server = c
        .servers
        .create_server(authed(
            &juan,
            pb::CreateServerRequest { name: "Pictures".into(), icon_url: icon.clone(), ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap();
    assert_eq!(server.icon_url, icon);
    let avatar_as_icon = c
        .servers
        .update_server(authed(
            &juan,
            pb::UpdateServerRequest {
                server_id: server.id.clone(),
                icon_url: Some(second_avatar.clone()),
                ..Default::default()
            },
        ))
        .await;
    assert_eq!(avatar_as_icon.unwrap_err().code(), Code::PermissionDenied);
    let new_icon = upload(&mut c, &instance, &juan, pb::MediaPurpose::ServerIcon, png(120, 5)).await;
    let updated = c
        .servers
        .update_server(authed(
            &juan,
            pb::UpdateServerRequest {
                server_id: server.id.clone(),
                icon_url: Some(new_icon.clone()),
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap();
    assert_eq!(updated.icon_url, new_icon);
    assert_eq!(fetch(&instance, &icon).await.0, reqwest::StatusCode::NOT_FOUND);

    // Admins see how much pictures take.
    let usage = c.admin.get_node_usage(authed(ADMIN_TOKEN, pb::GetNodeUsageRequest {})).await.unwrap().into_inner();
    assert_eq!((usage.pictures, usage.picture_bytes), (3, 200 + 500 + 120));

    // An account can't hold many unfinished uploads at once.
    for _ in 0..fuwa_server::media::MAX_PENDING_UPLOADS {
        create_upload(&mut c, &mika, avatar, "image/png", 10).await.unwrap();
    }
    let crowded = create_upload(&mut c, &mika, avatar, "image/png", 10).await;
    assert_eq!(crowded.unwrap_err().code(), Code::ResourceExhausted);

    // Uploads nothing uses are swept: unsent ones when their link runs out,
    // stored ones after a day.
    let unused = upload(&mut c, &instance, &juan, pb::MediaPurpose::Banner, png(60, 6)).await;
    let now = fuwa_server::id::now_ms();
    instance.app.sweep_media(now).await.unwrap();
    assert_eq!(fetch(&instance, &unused).await.0, reqwest::StatusCode::OK, "a fresh upload waits to be used");
    let swept = instance.app.sweep_media(now + fuwa_server::media::UPLOAD_TTL_MS + 1).await.unwrap();
    assert_eq!(swept, 11, "mika's unsent uploads and juan's unfinished one");
    assert!(create_upload(&mut c, &mika, avatar, "image/png", 10).await.is_ok());
    instance.app.sweep_media(now + fuwa_server::media::UNUSED_TTL_MS + 1).await.unwrap();
    assert_eq!(fetch(&instance, &unused).await.0, reqwest::StatusCode::NOT_FOUND);
    for used in [&second_avatar, &banner, &new_icon] {
        assert_eq!(fetch(&instance, used).await.0, reqwest::StatusCode::OK, "{used} is in use");
    }

    // Deleting an account deletes its pictures, but not the icons of servers.
    let (rin, _, _) = sign_up(&mut c, "rin").await;
    let rin_avatar = upload(&mut c, &instance, &rin, avatar, png(90, 7)).await;
    c.auth.update_profile(authed(&rin, set_avatar(&rin_avatar))).await.unwrap();
    c.account
        .delete_account(authed(
            &rin,
            pb::DeleteAccountRequest { password: "correct horse battery".into(), ..Default::default() },
        ))
        .await
        .unwrap();
    assert_eq!(fetch(&instance, &rin_avatar).await.0, reqwest::StatusCode::NOT_FOUND);

    // A file no row points to (a crash between the two) is cleared on start.
    let stray = fuwa_server::media::new_id();
    std::fs::write(dir.path().join("media").join(&stray), b"left behind").unwrap();
    instance.stop().await;
    let instance = start(dir.path(), &[("FUWA_PUBLIC_URL", "https://chat.example.com")]).await;
    assert!(!dir.path().join("media").join(&stray).exists());
    assert_eq!(fetch(&instance, &second_avatar).await.2, png(200, 2));
    instance.stop().await;
}

async fn join(c: &mut Clients, token: &str, server_id: &str) {
    c.servers
        .join_server(authed(token, pb::JoinServerRequest { server_id: server_id.into(), ..Default::default() }))
        .await
        .unwrap();
}

async fn roles(c: &mut Clients, token: &str, server_id: &str) -> Vec<pb::Role> {
    c.roles
        .list_roles(authed(token, pb::ListRolesRequest { server_id: server_id.into() }))
        .await
        .unwrap()
        .into_inner()
        .roles
}

async fn create_role(
    c: &mut Clients,
    token: &str,
    server_id: &str,
    name: &str,
    permissions: &[pb::Permission],
) -> Result<pb::Role, tonic::Status> {
    c.roles
        .create_role(authed(
            token,
            pb::CreateRoleRequest {
                server_id: server_id.into(),
                name: name.into(),
                permissions: permissions.iter().map(|&p| p as i32).collect(),
                ..Default::default()
            },
        ))
        .await
        .map(|r| r.into_inner().role.unwrap())
}

async fn give_role(
    c: &mut Clients,
    token: &str,
    server_id: &str,
    user_id: &str,
    role_id: &str,
) -> Result<pb::Member, Code> {
    c.roles
        .add_member_role(authed(
            token,
            pb::AddMemberRoleRequest { server_id: server_id.into(), user_id: user_id.into(), role_id: role_id.into() },
        ))
        .await
        .map(|r| r.into_inner().member.unwrap())
        .map_err(|s| s.code())
}

async fn channel_names(c: &mut Clients, token: &str, server_id: &str) -> Vec<String> {
    c.channels
        .list_channels(authed(token, pb::ListChannelsRequest { server_id: server_id.into() }))
        .await
        .unwrap()
        .into_inner()
        .channels
        .into_iter()
        .map(|ch| ch.name)
        .collect()
}

fn overwrite(
    target_id: &str,
    target: pb::OverwriteTarget,
    allow: &[pb::Permission],
    deny: &[pb::Permission],
) -> pb::PermissionOverwrite {
    pb::PermissionOverwrite {
        target_id: target_id.into(),
        target: target as i32,
        allow: allow.iter().map(|&p| p as i32).collect(),
        deny: deny.iter().map(|&p| p as i32).collect(),
    }
}

async fn set_permissions(
    c: &mut Clients,
    token: &str,
    server_id: &str,
    channel_id: &str,
    overwrites: Vec<pb::PermissionOverwrite>,
) -> Result<pb::Channel, Code> {
    c.channels
        .set_channel_permissions(authed(
            token,
            pb::SetChannelPermissionsRequest { server_id: server_id.into(), channel_id: channel_id.into(), overwrites },
        ))
        .await
        .map(|r| r.into_inner().channel.unwrap())
        .map_err(|s| s.code())
}

/// The next event on a stream, skipping heartbeats and `ready`.
async fn next_event(stream: &mut tonic::Streaming<pb::SubscribeResponse>) -> pb::Event {
    loop {
        let item = tokio::time::timeout(Duration::from_secs(5), stream.next()).await.unwrap().unwrap().unwrap();
        if let Some(event) = item.event {
            return event;
        }
    }
}

#[tokio::test]
async fn reordering_skips_hidden_categories() {
    use pb::OverwriteTarget as T;
    use pb::Permission as P;
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (juan, _, _) = sign_up(&mut c, "juan").await;
    let (mika, mika_user, _) = sign_up(&mut c, "mika").await;
    let sid = create_server(&mut c, &juan, "Arranging", true).await.id;
    join(&mut c, &mika, &sid).await;
    let arrangers = create_role(&mut c, &juan, &sid, "Arrangers", &[P::ManageChannels]).await.unwrap();
    give_role(&mut c, &juan, &sid, &mika_user.id, &arrangers.id).await.unwrap();
    let staff = new_channel(&mut c, &juan, &sid, "Staff", pb::ChannelType::Category).await;
    set_permissions(&mut c, &juan, &sid, &staff.id, vec![overwrite(&sid, T::Role, &[], &[P::ViewChannels])])
        .await
        .unwrap();
    let seen = c
        .channels
        .list_channels(authed(&mika, pb::ListChannelsRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels;
    assert!(seen.iter().all(|ch| ch.id != staff.id));

    // Mika can arrange what she sees, but not into a category hidden from her, even knowing its id.
    let place = |parent: &str| {
        seen.iter().map(|ch| pb::ChannelPlacement { channel_id: ch.id.clone(), parent_id: parent.into() }).collect()
    };
    let reorder = |channels: Vec<pb::ChannelPlacement>| {
        authed(&mika, pb::ReorderChannelsRequest { server_id: sid.clone(), channels })
    };
    let hidden = c.channels.reorder_channels(reorder(place(&staff.id))).await.unwrap_err();
    assert_eq!(hidden.code(), Code::InvalidArgument);
    c.channels.reorder_channels(reorder(place(""))).await.unwrap();
}

#[tokio::test]
async fn roles_and_channel_permissions() {
    use pb::Permission as P;
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (juan, _, _) = sign_up(&mut c, "juan").await;
    let (mika, mika_user, _) = sign_up(&mut c, "mika").await;
    let (aoi, aoi_user, _) = sign_up(&mut c, "aoi").await;
    let (rin, _, _) = sign_up(&mut c, "rin").await;
    let server = create_server(&mut c, &juan, "Roles", true).await;
    let sid = server.id.clone();
    for token in [&mika, &aoi, &rin] {
        join(&mut c, token, &sid).await;
    }
    let general = c
        .channels
        .list_channels(authed(&juan, pb::ListChannelsRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels[0]
        .id
        .clone();

    // Every server starts with Admin above @everyone, whose id is the server's.
    let start = roles(&mut c, &rin, &sid).await;
    let names: Vec<(&str, i32)> = start.iter().map(|r| (r.name.as_str(), r.position)).collect();
    assert_eq!(names, [("Admin", 1), ("@everyone", 0)]);
    assert_eq!(start[1].id, sid);
    assert!(start[1].permissions.contains(&(P::SendMessages as i32)));
    assert!(!start[1].permissions.contains(&(P::MentionEveryone as i32)));
    let admin = start[0].clone();

    // Only people with Manage Roles make roles; new ones land right above @everyone.
    let denied = create_role(&mut c, &rin, &sid, "Nope", &[]).await.unwrap_err();
    assert_eq!(denied.code(), Code::PermissionDenied);
    let mods =
        create_role(&mut c, &juan, &sid, "Mods", &[P::ManageRoles, P::ManageMessages, P::KickMembers]).await.unwrap();
    assert_eq!(mods.position, 1);
    let unknown = create_role(&mut c, &juan, &sid, "Odd", &[P::Unspecified]).await.unwrap_err();
    assert_eq!(unknown.code(), Code::InvalidArgument);
    let order: Vec<String> = roles(&mut c, &juan, &sid).await.into_iter().map(|r| r.name).collect();
    assert_eq!(order, ["Admin", "Mods", "@everyone"]);
    let mika_member = give_role(&mut c, &juan, &sid, &mika_user.id, &mods.id).await.unwrap();
    assert_eq!(mika_member.role_ids, std::slice::from_ref(&mods.id));
    assert_eq!(give_role(&mut c, &juan, &sid, &mika_user.id, &sid).await.unwrap_err(), Code::InvalidArgument);

    // A moderator hands out only what they have, and only below their own role.
    let ban = create_role(&mut c, &mika, &sid, "Banners", &[P::BanMembers]).await.unwrap_err();
    assert_eq!(ban.code(), Code::PermissionDenied);
    let helpers = create_role(&mut c, &mika, &sid, "Helpers", &[P::ManageMessages]).await.unwrap();
    for role in [&admin, &mods] {
        let touch = c
            .roles
            .update_role(authed(
                &mika,
                pb::UpdateRoleRequest {
                    server_id: sid.clone(),
                    role_id: role.id.clone(),
                    name: Some("Mine".into()),
                    ..Default::default()
                },
            ))
            .await
            .unwrap_err();
        assert_eq!(touch.code(), Code::PermissionDenied, "{}", role.name);
    }
    let renamed = c
        .roles
        .update_role(authed(
            &mika,
            pb::UpdateRoleRequest {
                server_id: sid.clone(),
                role_id: helpers.id.clone(),
                name: Some("Helpers ✿".into()),
                color: Some(0x60A5FA),
                hoist: Some(true),
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .role
        .unwrap();
    assert_eq!((renamed.name.as_str(), renamed.color, renamed.hoist), ("Helpers ✿", Some(0x60A5FA), true));
    let everyone_name = c
        .roles
        .update_role(authed(
            &juan,
            pb::UpdateRoleRequest {
                server_id: sid.clone(),
                role_id: sid.clone(),
                name: Some("all".into()),
                ..Default::default()
            },
        ))
        .await
        .unwrap_err();
    assert_eq!(everyone_name.code(), Code::InvalidArgument);
    assert_eq!(
        give_role(&mut c, &mika, &sid, &aoi_user.id, &helpers.id).await.unwrap().role_ids,
        std::slice::from_ref(&helpers.id)
    );
    assert_eq!(give_role(&mut c, &mika, &sid, &aoi_user.id, &mods.id).await.unwrap_err(), Code::PermissionDenied);

    // Reordering: a moderator can't lift a role over their own; the owner can.
    let reorder = |token: &str, ids: [&String; 3]| {
        authed(
            token,
            pb::ReorderRolesRequest { server_id: sid.clone(), role_ids: ids.iter().map(|s| s.to_string()).collect() },
        )
    };
    let lifted = c.roles.reorder_roles(reorder(&mika, [&admin.id, &helpers.id, &mods.id])).await.unwrap_err();
    assert_eq!(lifted.code(), Code::PermissionDenied);
    let stale = c.roles.reorder_roles(authed(
        &juan,
        pb::ReorderRolesRequest { server_id: sid.clone(), role_ids: vec![admin.id.clone()] },
    ));
    assert_eq!(stale.await.unwrap_err().code(), Code::FailedPrecondition);
    let reordered =
        c.roles.reorder_roles(reorder(&juan, [&mods.id, &admin.id, &helpers.id])).await.unwrap().into_inner().roles;
    let order: Vec<(&str, i32)> = reordered.iter().map(|r| (r.name.as_str(), r.position)).collect();
    assert_eq!(order, [("Mods", 3), ("Admin", 2), ("Helpers ✿", 1), ("@everyone", 0)]);

    // Members are listed by rank: the owner, then by their highest role.
    let members = c
        .servers
        .list_members(authed(&rin, pb::ListMembersRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner()
        .members;
    let usernames: Vec<&str> = members.iter().map(|m| m.user.as_ref().unwrap().username.as_str()).collect();
    assert_eq!(usernames, ["juan", "mika", "aoi", "rin"]);

    // A private channel: Aoi watches it appear and then go, live.
    let mut aoi_stream = c
        .events
        .subscribe(authed(
            &aoi,
            pb::SubscribeRequest { servers: vec![pb::ServerCursor { server_id: sid.clone(), after_sequence: None }] },
        ))
        .await
        .unwrap()
        .into_inner();
    let staff = new_channel(&mut c, &juan, &sid, "staff", pb::ChannelType::Text).await;
    let created = next_event(&mut aoi_stream).await;
    assert!(created.sequence > 0);
    assert!(matches!(&created.payload, Some(Payload::ChannelCreated(e)) if e.channel.as_ref().unwrap().id == staff.id));

    use pb::OverwriteTarget as T;
    let private =
        vec![overwrite(&sid, T::Role, &[], &[P::ViewChannels]), overwrite(&mods.id, T::Role, &[P::ViewChannels], &[])];
    let shut = set_permissions(&mut c, &juan, &sid, &staff.id, private.clone()).await.unwrap();
    assert_eq!(shut.permission_overwrites.len(), 2);
    let went = next_event(&mut aoi_stream).await;
    assert_eq!(went.sequence, 0);
    assert!(matches!(&went.payload, Some(Payload::ChannelDeleted(e)) if e.channel_id == staff.id));
    assert_eq!(channel_names(&mut c, &aoi, &sid).await, ["general"]);
    assert_eq!(channel_names(&mut c, &mika, &sid).await, ["general", "staff"]);
    let read = c
        .messages
        .list_messages(authed(
            &aoi,
            pb::ListMessagesRequest { server_id: sid.clone(), channel_id: staff.id.clone(), ..Default::default() },
        ))
        .await
        .unwrap_err();
    assert_eq!(read.code(), Code::NotFound);
    assert_eq!(send(&mut c, &aoi, &sid, &staff.id, "hi?").await.unwrap_err().code(), Code::NotFound);
    let secret = send(&mut c, &mika, &sid, &staff.id, "staff only").await.unwrap();
    // Aoi's stream skips the staff message and goes straight to the next thing she can see.
    let public = send(&mut c, &rin, &sid, &general, "hello").await.unwrap();
    let next = next_event(&mut aoi_stream).await;
    assert!(matches!(&next.payload, Some(Payload::MessageCreated(e)) if e.message.as_ref().unwrap().id == public.id));

    // Getting the role brings the channel back, live.
    give_role(&mut c, &juan, &sid, &aoi_user.id, &mods.id).await.unwrap();
    let updated = next_event(&mut aoi_stream).await;
    assert!(matches!(&updated.payload, Some(Payload::MemberUpdated(_))));
    let back = next_event(&mut aoi_stream).await;
    assert_eq!(back.sequence, 0);
    assert!(matches!(&back.payload, Some(Payload::ChannelCreated(e)) if e.channel.as_ref().unwrap().id == staff.id));
    assert_eq!(messages(&mut c, &aoi, &sid, &staff.id).await[0].id, secret.id);
    drop(aoi_stream);

    // Overwrites hold channel permissions only, never both ways, and only ones the setter has.
    let server_wide = vec![overwrite(&sid, T::Role, &[P::KickMembers], &[])];
    assert_eq!(set_permissions(&mut c, &juan, &sid, &staff.id, server_wide).await.unwrap_err(), Code::InvalidArgument);
    let both = vec![overwrite(&sid, T::Role, &[P::SendMessages], &[P::SendMessages])];
    assert_eq!(set_permissions(&mut c, &juan, &sid, &staff.id, both).await.unwrap_err(), Code::InvalidArgument);
    let nobody = vec![overwrite(&new_id_like(&sid), T::Role, &[P::SendMessages], &[])];
    assert_eq!(set_permissions(&mut c, &juan, &sid, &staff.id, nobody).await.unwrap_err(), Code::NotFound);
    let mut grant = private.clone();
    grant.push(overwrite(&aoi_user.id, T::Member, &[P::MentionEveryone], &[]));
    assert_eq!(set_permissions(&mut c, &mika, &sid, &staff.id, grant).await.unwrap_err(), Code::PermissionDenied);
    assert_eq!(set_permissions(&mut c, &rin, &sid, &general, vec![]).await.unwrap_err(), Code::PermissionDenied);

    // @everyone and role pings count only from people allowed to make them.
    let ping = send(&mut c, &rin, &sid, &general, "@everyone look").await.unwrap();
    assert!(!ping.mentions_everyone);
    let ping = send(&mut c, &juan, &sid, &general, "@here look").await.unwrap();
    assert!(ping.mentions_everyone);
    let role_ping = format!("<@&{}> help please", helpers.id);
    assert!(send(&mut c, &rin, &sid, &general, &role_ping).await.unwrap().mention_role_ids.is_empty());
    assert_eq!(
        send(&mut c, &juan, &sid, &general, &role_ping).await.unwrap().mention_role_ids,
        std::slice::from_ref(&helpers.id)
    );
    c.roles
        .update_role(authed(
            &juan,
            pb::UpdateRoleRequest {
                server_id: sid.clone(),
                role_id: helpers.id.clone(),
                mentionable: Some(true),
                ..Default::default()
            },
        ))
        .await
        .unwrap();
    let pinged = send(&mut c, &rin, &sid, &general, &role_ping).await.unwrap();
    assert_eq!(pinged.mention_role_ids, std::slice::from_ref(&helpers.id));
    assert_eq!(
        messages(&mut c, &rin, &sid, &general).await.iter().find(|m| m.id == pinged.id).unwrap().mention_role_ids,
        std::slice::from_ref(&helpers.id)
    );

    // Slow mode holds back plain members, not people who manage messages.
    c.channels
        .update_channel(authed(
            &juan,
            pb::UpdateChannelRequest {
                server_id: sid.clone(),
                channel_id: general.clone(),
                slowmode_seconds: Some(60),
                ..Default::default()
            },
        ))
        .await
        .unwrap();
    send(&mut c, &rin, &sid, &general, "one").await.unwrap();
    assert_eq!(send(&mut c, &rin, &sid, &general, "two").await.unwrap_err().code(), Code::ResourceExhausted);
    for text in ["one", "two"] {
        send(&mut c, &aoi, &sid, &general, text).await.unwrap();
    }

    // Taking @everyone's Send Messages away silences plain members everywhere.
    let mut everyone = roles(&mut c, &juan, &sid).await.into_iter().find(|r| r.id == sid).unwrap();
    everyone.permissions.retain(|&p| p != P::SendMessages as i32);
    c.roles
        .update_role(authed(
            &juan,
            pb::UpdateRoleRequest {
                server_id: sid.clone(),
                role_id: sid.clone(),
                permissions: Some(pb::PermissionSet { permissions: everyone.permissions.clone() }),
                ..Default::default()
            },
        ))
        .await
        .unwrap();
    assert_eq!(send(&mut c, &rin, &sid, &general, "three").await.unwrap_err().code(), Code::PermissionDenied);
    send(&mut c, &juan, &sid, &general, "three").await.unwrap();

    // Deleting a role takes it from its members and from every channel.
    c.roles
        .delete_role(authed(&juan, pb::DeleteRoleRequest { server_id: sid.clone(), role_id: mods.id.clone() }))
        .await
        .unwrap();
    assert_eq!(channel_names(&mut c, &mika, &sid).await, ["general"]);
    let staff_now = c
        .channels
        .get_channel(authed(&juan, pb::GetChannelRequest { server_id: sid.clone(), channel_id: staff.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channel
        .unwrap();
    assert_eq!(staff_now.permission_overwrites.len(), 1);
    let order: Vec<(String, i32)> =
        roles(&mut c, &juan, &sid).await.into_iter().map(|r| (r.name, r.position)).collect();
    assert_eq!(order, [("Admin".into(), 2), ("Helpers ✿".into(), 1), ("@everyone".into(), 0)]);
    let audit =
        audit_log(&mut c, &juan, pb::ListAuditLogRequest { server_id: sid.clone(), ..Default::default() }).await;
    let deleted = &audit.entries[0];
    assert_eq!((deleted.action(), deleted.role_name.as_str()), (pb::AuditAction::RoleDelete, "Mods"));
    assert!(audit.entries.iter().any(|e| e.action() == pb::AuditAction::ChannelPermissionsUpdate));
    assert!(audit.entries.iter().any(|e| e.action() == pb::AuditAction::RolesReorder));

    drop(c);
    instance.stop().await;
}

/// An id shaped like a real one that names nothing.
fn new_id_like(id: &str) -> String {
    let mut chars: Vec<char> = id.chars().collect();
    let last = chars.len() - 1;
    chars[last] = if chars[last] == '0' { '1' } else { '0' };
    chars.into_iter().collect()
}

#[tokio::test]
async fn servers_from_before_roles_keep_their_admins() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (juan, _, _) = sign_up(&mut c, "juan").await;
    let (mika, mika_user, _) = sign_up(&mut c, "mika").await;
    let server = create_server(&mut c, &juan, "Older", true).await;
    join(&mut c, &mika, &server.id).await;
    drop(c);
    instance.stop().await;

    // Make the file look like one from before roles, with Mika an admin by rank.
    let file = dir.path().join("servers").join(format!("{}.db", server.id));
    let db = fuwa_server::db::open(&file, None, &[]).await.unwrap();
    let conn = fuwa_server::db::connect(&db).unwrap();
    conn.execute("DELETE FROM roles", ()).await.unwrap();
    conn.execute("DELETE FROM member_roles", ()).await.unwrap();
    conn.execute("UPDATE members SET role = 2 WHERE user_id = ?1", [mika_user.id.as_str()]).await.unwrap();
    drop((conn, db));

    // Opening it again gives it @everyone and an Admin role that Mika holds.
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let seeded = roles(&mut c, &mika, &server.id).await;
    let names: Vec<&str> = seeded.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["Admin", "@everyone"]);
    let members = c
        .servers
        .list_members(authed(&mika, pb::ListMembersRequest { server_id: server.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .members;
    let mika_member = members.iter().find(|m| m.user.as_ref().unwrap().id == mika_user.id).unwrap();
    assert_eq!(mika_member.role_ids, std::slice::from_ref(&seeded[0].id));
    let renamed = c
        .servers
        .update_server(authed(
            &mika,
            pb::UpdateServerRequest {
                server_id: server.id.clone(),
                name: Some("Still here".into()),
                ..Default::default()
            },
        ))
        .await
        .unwrap();
    assert_eq!(renamed.into_inner().server.unwrap().name, "Still here");
    drop(c);
    instance.stop().await;
}

async fn invite(
    c: &mut Clients,
    token: &str,
    server_id: &str,
    channel_id: &str,
    max_uses: i32,
    max_age_seconds: i32,
) -> Result<pb::Invite, Code> {
    c.invites
        .create_invite(authed(
            token,
            pb::CreateInviteRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                max_uses,
                max_age_seconds,
            },
        ))
        .await
        .map(|r| r.into_inner().invite.unwrap())
        .map_err(|s| s.code())
}

/// What a code leads to, asked without signing in.
async fn look_up(c: &mut Clients, code: &str) -> Result<pb::GetInviteResponse, Code> {
    c.invites.get_invite(pb::GetInviteRequest { code: code.into() }).await.map(|r| r.into_inner()).map_err(|s| s.code())
}

async fn join_with(c: &mut Clients, token: &str, server_id: &str, code: &str) -> Result<pb::Member, Code> {
    c.servers
        .join_server(authed(token, pb::JoinServerRequest { server_id: server_id.into(), invite_code: code.into() }))
        .await
        .map(|r| r.into_inner().member.unwrap())
        .map_err(|s| s.code())
}

async fn list_invites(c: &mut Clients, token: &str, server_id: &str) -> pb::ListInvitesResponse {
    c.invites
        .list_invites(authed(token, pb::ListInvitesRequest { server_id: server_id.into() }))
        .await
        .unwrap()
        .into_inner()
}

async fn delete_invite(c: &mut Clients, token: &str, server_id: &str, code: &str) -> Result<(), Code> {
    c.invites
        .delete_invite(authed(token, pb::DeleteInviteRequest { server_id: server_id.into(), code: code.into() }))
        .await
        .map(|_| ())
        .map_err(|s| s.code())
}

#[tokio::test]
async fn invites_let_people_into_servers() {
    use pb::Permission as P;
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (juan, juan_user, _) = sign_up(&mut c, "juan").await;
    let (mika, mika_user, _) = sign_up(&mut c, "mika").await;
    let (rin, _, _) = sign_up(&mut c, "rin").await;
    let (kai, kai_user, _) = sign_up(&mut c, "kai").await;
    let server = create_server(&mut c, &juan, "Hideout", false).await;
    let sid = server.id.clone();
    let general = c
        .channels
        .list_channels(authed(&juan, pb::ListChannelsRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels
        .remove(0);

    // A server nobody can find takes an invite.
    assert_eq!(join_with(&mut c, &mika, &sid, "").await.unwrap_err(), Code::NotFound);
    assert_eq!(join_with(&mut c, &mika, &sid, "nope").await.unwrap_err(), Code::NotFound);
    assert_eq!(look_up(&mut c, "nope").await.unwrap_err(), Code::NotFound);
    assert_eq!(look_up(&mut c, "../../etc").await.unwrap_err(), Code::NotFound);
    assert_eq!(invite(&mut c, &juan, &sid, "", 101, 0).await.unwrap_err(), Code::InvalidArgument);
    assert_eq!(invite(&mut c, &juan, &sid, "", 0, -1).await.unwrap_err(), Code::InvalidArgument);
    assert_eq!(invite(&mut c, &mika, &sid, "", 0, 0).await.unwrap_err(), Code::PermissionDenied);

    let twice = invite(&mut c, &juan, &sid, "", 2, 3600).await.unwrap();
    assert_eq!(twice.code.len(), 10);
    assert!(twice.expires_at.is_some());
    let shown = look_up(&mut c, &twice.code).await.unwrap();
    assert_eq!(shown.server.unwrap().name, "Hideout");
    assert_eq!(shown.inviter.unwrap().id, juan_user.id);
    assert_eq!(shown.channel_name, "");
    join_with(&mut c, &mika, &sid, &twice.code).await.unwrap();
    assert_eq!(join_with(&mut c, &mika, &sid, &twice.code).await.unwrap_err(), Code::AlreadyExists);
    assert_eq!(look_up(&mut c, &twice.code).await.unwrap().invite.unwrap().uses, 1);

    // Members can invite people too, into a channel.
    let mikas = invite(&mut c, &mika, &sid, &general.id, 0, 0).await.unwrap();
    assert_eq!(mikas.expires_at, None);
    assert_eq!(look_up(&mut c, &mikas.code).await.unwrap().channel_name, "general");
    let theirs = list_invites(&mut c, &mika, &sid).await;
    assert_eq!(theirs.invites.iter().map(|i| i.code.as_str()).collect::<Vec<_>>(), [mikas.code.as_str()]);
    let all = list_invites(&mut c, &juan, &sid).await;
    assert_eq!(all.invites.len(), 2);
    assert_eq!(all.inviters.len(), 2);
    assert_eq!(delete_invite(&mut c, &mika, &sid, &twice.code).await.unwrap_err(), Code::PermissionDenied);
    delete_invite(&mut c, &juan, &sid, &mikas.code).await.unwrap();
    assert_eq!(look_up(&mut c, &mikas.code).await.unwrap_err(), Code::NotFound);

    // Its last use deletes it.
    join_with(&mut c, &rin, &sid, &twice.code).await.unwrap();
    assert_eq!(look_up(&mut c, &twice.code).await.unwrap_err(), Code::NotFound);
    assert!(list_invites(&mut c, &juan, &sid).await.invites.is_empty());

    // Expired ones stop working.
    let brief = invite(&mut c, &juan, &sid, "", 0, 1).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(look_up(&mut c, &brief.code).await.unwrap_err(), Code::NotFound);
    assert_eq!(join_with(&mut c, &kai, &sid, &brief.code).await.unwrap_err(), Code::NotFound);

    // A private channel's name stays private.
    let secret = new_channel(&mut c, &juan, &sid, "secret", pb::ChannelType::Text).await;
    set_permissions(
        &mut c,
        &juan,
        &sid,
        &secret.id,
        vec![overwrite(&sid, pb::OverwriteTarget::Role, &[], &[P::ViewChannels])],
    )
    .await
    .unwrap();
    let into_secret = invite(&mut c, &juan, &sid, &secret.id, 0, 0).await.unwrap();
    assert_eq!(look_up(&mut c, &into_secret.code).await.unwrap().channel_name, "");
    assert_eq!(invite(&mut c, &mika, &sid, &secret.id, 0, 0).await.unwrap_err(), Code::NotFound);

    // Servers can turn away accounts that are too new.
    let update = |age: i32| pb::UpdateServerRequest {
        server_id: sid.clone(),
        min_account_age_seconds: Some(age),
        ..Default::default()
    };
    let updated = c.servers.update_server(authed(&juan, update(3600))).await.unwrap().into_inner().server.unwrap();
    assert_eq!(updated.min_account_age_seconds, 3600);
    let open = invite(&mut c, &juan, &sid, "", 0, 0).await.unwrap();
    let too_new = c
        .servers
        .join_server(authed(&kai, pb::JoinServerRequest { server_id: sid.clone(), invite_code: open.code.clone() }))
        .await
        .unwrap_err();
    assert_eq!(too_new.code(), Code::FailedPrecondition);
    assert!(too_new.message().contains("1 hour"), "{}", too_new.message());
    assert_eq!(look_up(&mut c, &open.code).await.unwrap().invite.unwrap().uses, 0);
    assert_eq!(c.servers.update_server(authed(&juan, update(-1))).await.unwrap_err().code(), Code::InvalidArgument);
    c.servers.update_server(authed(&juan, update(0))).await.unwrap();
    join_with(&mut c, &kai, &sid, &open.code).await.unwrap();

    // Bans hold whatever the invite.
    c.servers
        .ban_member(authed(
            &juan,
            pb::BanMemberRequest { server_id: sid.clone(), user_id: kai_user.id.clone(), ..Default::default() },
        ))
        .await
        .unwrap();
    assert_eq!(join_with(&mut c, &kai, &sid, &open.code).await.unwrap_err(), Code::PermissionDenied);

    // Without Create invite, members can't.
    let everyone = roles(&mut c, &juan, &sid).await.into_iter().find(|r| r.id == sid).unwrap();
    assert!(everyone.permissions.contains(&(P::CreateInvite as i32)));
    let without: Vec<i32> = everyone.permissions.iter().copied().filter(|&p| p != P::CreateInvite as i32).collect();
    c.roles
        .update_role(authed(
            &juan,
            pb::UpdateRoleRequest {
                server_id: sid.clone(),
                role_id: sid.clone(),
                permissions: Some(pb::PermissionSet { permissions: without }),
                ..Default::default()
            },
        ))
        .await
        .unwrap();
    assert_eq!(invite(&mut c, &mika, &sid, "", 0, 0).await.unwrap_err(), Code::PermissionDenied);

    let log = audit_log(&mut c, &juan, pb::ListAuditLogRequest { server_id: sid.clone(), ..Default::default() }).await;
    let actions: Vec<i32> = log.entries.iter().map(|e| e.action).collect();
    assert!(actions.contains(&(pb::AuditAction::InviteCreate as i32)));
    assert!(actions.contains(&(pb::AuditAction::InviteDelete as i32)));
    let _ = mika_user;

    // Invites still lead somewhere after a restart.
    drop(c);
    instance.stop().await;
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    assert_eq!(look_up(&mut c, &open.code).await.unwrap().server.unwrap().id, sid);
    assert_eq!(look_up(&mut c, &brief.code).await.unwrap_err(), Code::NotFound);
}

/// A stand-in for waifu.dev's OpenAuth issuer: codes the test hands out, and
/// the token, userinfo and sign-out endpoints fuwa calls.
#[derive(Default)]
struct FakeIssuer {
    /// code → (PKCE challenge, client_id, redirect_uri, who signed in)
    codes: std::sync::Mutex<std::collections::HashMap<String, (String, String, String, serde_json::Value)>>,
    tokens: std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,
    revoked: std::sync::Mutex<Vec<String>>,
}

async fn fake_issuer() -> (String, Arc<FakeIssuer>) {
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::response::IntoResponse;
    use axum::routing::{get, post};

    fn json(status: StatusCode, value: serde_json::Value) -> axum::response::Response {
        (status, [(header::CONTENT_TYPE, "application/json")], value.to_string()).into_response()
    }

    async fn token(State(issuer): State<Arc<FakeIssuer>>, body: String) -> axum::response::Response {
        let form: std::collections::HashMap<String, String> =
            reqwest::Url::parse(&format!("http://form/?{body}")).unwrap().query_pairs().into_owned().collect();
        let field = |name: &str| form.get(name).cloned().unwrap_or_default();
        let Some((challenge, client_id, redirect_uri, user)) = issuer.codes.lock().unwrap().remove(&field("code"))
        else {
            return json(StatusCode::BAD_REQUEST, serde_json::json!({ "error": "invalid_grant" }));
        };
        use base64::Engine;
        use sha2::Digest;
        let verified = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(field("code_verifier").as_bytes()))
            == challenge;
        if field("grant_type") != "authorization_code"
            || field("client_id") != client_id
            || field("redirect_uri") != redirect_uri
            || !verified
        {
            return json(StatusCode::BAD_REQUEST, serde_json::json!({ "error": "invalid_grant" }));
        }
        let access = format!("access-{}", issuer.tokens.lock().unwrap().len());
        issuer.tokens.lock().unwrap().insert(access.clone(), user);
        json(
            StatusCode::OK,
            serde_json::json!({ "access_token": access, "refresh_token": "refresh-me", "expires_in": 3600 }),
        )
    }

    async fn userinfo(State(issuer): State<Arc<FakeIssuer>>, headers: HeaderMap) -> axum::response::Response {
        let bearer = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or_default();
        match issuer.tokens.lock().unwrap().get(bearer.trim_start_matches("Bearer ")) {
            Some(user) => json(StatusCode::OK, user.clone()),
            None => json(StatusCode::UNAUTHORIZED, serde_json::json!({})),
        }
    }

    async fn revoke(State(issuer): State<Arc<FakeIssuer>>, body: String) -> StatusCode {
        issuer.revoked.lock().unwrap().push(body);
        StatusCode::NO_CONTENT
    }

    let issuer = Arc::new(FakeIssuer::default());
    let router = axum::Router::new()
        .route("/token", post(token))
        .route("/userinfo", get(userinfo))
        .route("/session/revoke", post(revoke))
        .with_state(issuer.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, issuer)
}

/// What the browser does between StartLinkedSignIn and FinishLinkedSignIn:
/// visits the issuer, which signs `user` in and sends back a code.
fn approve(issuer: &FakeIssuer, authorize_url: &str, user: serde_json::Value) -> String {
    let url = reqwest::Url::parse(authorize_url).unwrap();
    let query: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();
    assert_eq!(url.path(), "/authorize");
    assert_eq!(query["response_type"], "code");
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(query["redirect_uri"], format!("{}/auth/waifu/callback", query["client_id"]));
    let code = format!("code-{}", issuer.codes.lock().unwrap().len() + issuer.tokens.lock().unwrap().len());
    issuer.codes.lock().unwrap().insert(
        code.clone(),
        (query["code_challenge"].clone(), query["client_id"].clone(), query["redirect_uri"].clone(), user),
    );
    code
}

async fn start_linked(c: &mut Clients, secret: &str) -> Result<pb::StartLinkedSignInResponse, Code> {
    let request = pb::StartLinkedSignInRequest {
        return_origin: "http://localhost:5173".into(),
        secret_hash: fuwa_server::linked::secret_hash(secret),
    };
    c.auth.start_linked_sign_in(request).await.map(|r| r.into_inner()).map_err(|e| e.code())
}

async fn finish_linked(
    c: &mut Clients,
    state: &str,
    code: &str,
    secret: &str,
) -> Result<pb::FinishLinkedSignInResponse, Code> {
    let request = pb::FinishLinkedSignInRequest { state: state.into(), code: code.into(), secret: secret.into() };
    c.auth.finish_linked_sign_in(request).await.map(|r| r.into_inner()).map_err(|e| e.code())
}

/// One whole sign-in with waifu.dev, as `user`.
async fn linked_sign_in(
    c: &mut Clients,
    issuer: &FakeIssuer,
    user: serde_json::Value,
) -> Result<pb::FinishLinkedSignInResponse, Code> {
    let started = start_linked(c, "the app's secret").await?;
    let code = approve(issuer, &started.authorize_url, user);
    finish_linked(c, &started.state, &code, "the app's secret").await
}

#[tokio::test]
async fn linked_accounts_sign_in_with_waifu_dev() {
    let dir = tempfile::tempdir().unwrap();
    let (issuer_url, issuer) = fake_issuer().await;
    let instance = start(dir.path(), &[("FUWA_LINKED_ISSUER", &issuer_url)]).await;
    let mut c = clients(&instance).await;
    let juan = serde_json::json!({
        "sub": "user-juan", "preferred_username": "Juan-Dev", "name": "Juan",
        "picture": "https://avatars.example/juan.png", "profile": "https://www.waifu.dev/u/Juan-Dev",
    });

    let auth = c.node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap().auth.unwrap();
    assert!(auth.linked_sign_in && auth.linked_sign_up);
    assert_eq!(auth.linked_issuer, issuer_url);

    // The app's origin and secret are checked.
    let bad_origin = pb::StartLinkedSignInRequest {
        return_origin: "http://evil.example".into(),
        secret_hash: fuwa_server::linked::secret_hash("s"),
    };
    assert_eq!(c.auth.start_linked_sign_in(bad_origin).await.unwrap_err().code(), Code::InvalidArgument);
    let bad_hash =
        pb::StartLinkedSignInRequest { return_origin: "http://localhost:5173".into(), secret_hash: "x".into() };
    assert_eq!(c.auth.start_linked_sign_in(bad_hash).await.unwrap_err().code(), Code::InvalidArgument);

    // Sign-ins only go back to apps the instance trusts: its own, ones on this
    // device, and ones its admins list by name (any origin, "*", isn't enough).
    let elsewhere = || pb::StartLinkedSignInRequest {
        return_origin: "https://app.example".into(),
        secret_hash: fuwa_server::linked::secret_hash("s"),
    };
    assert_eq!(c.auth.start_linked_sign_in(elsewhere()).await.unwrap_err().code(), Code::PermissionDenied);
    let settings = c.admin.get_settings(authed(ADMIN_TOKEN, pb::GetSettingsRequest {})).await.unwrap().into_inner();
    let mut listed = settings.config.unwrap().settings.unwrap();
    listed.allowed_origins = vec!["https://app.example".into(), "http://localhost:5173".into()];
    c.admin.update_settings(authed(ADMIN_TOKEN, settings_update(listed, &["allowed_origins"], &[]))).await.unwrap();
    assert!(c.auth.start_linked_sign_in(elsewhere()).await.is_ok());
    let reset = pb::InstanceSettings::default();
    c.admin.update_settings(authed(ADMIN_TOKEN, settings_update(reset, &[], &["allowed_origins"]))).await.unwrap();

    // A sign-in goes to the issuer, as this instance, and comes back here.
    let started = start_linked(&mut c, "the app's secret").await.unwrap();
    let query: std::collections::HashMap<String, String> =
        reqwest::Url::parse(&started.authorize_url).unwrap().query_pairs().into_owned().collect();
    assert!(started.authorize_url.starts_with(&format!("{issuer_url}/authorize?")));
    assert_eq!(query["client_id"], "http://localhost:8080");
    assert_eq!(query["state"], started.state);
    let get = |state: &str| pb::GetLinkedSignInRequest { state: state.into() };
    let shown = c.auth.get_linked_sign_in(get(&started.state)).await.unwrap().into_inner();
    assert_eq!(shown.return_origin, "http://localhost:5173");
    assert_eq!(c.auth.get_linked_sign_in(get("nope")).await.unwrap_err().code(), Code::NotFound);

    // Only the app holding the secret can finish it, once.
    let code = approve(&issuer, &started.authorize_url, juan.clone());
    assert_eq!(finish_linked(&mut c, &started.state, &code, "a guess").await.unwrap_err(), Code::PermissionDenied);
    let first = finish_linked(&mut c, &started.state, &code, "the app's secret").await.unwrap();
    assert!(first.created && first.admin, "the instance's first account is its admin");
    let user = first.user.unwrap();
    assert_eq!(user.kind, pb::AccountKind::Linked as i32);
    assert_eq!((user.username.as_str(), user.display_name.as_str()), ("juan_dev", "Juan"));
    // The picture from waifu.dev comes through the instance, so people who
    // see it aren't seen by the site it's on.
    assert!(user.avatar_url.starts_with("http://localhost:8080/media/outside/"), "{}", user.avatar_url);
    assert!(user.avatar_url.ends_with("?url=https%3A%2F%2Favatars%2Eexample%2Fjuan%2Epng"), "{}", user.avatar_url);
    assert_eq!(
        finish_linked(&mut c, &started.state, &code, "the app's secret").await.unwrap_err(),
        Code::FailedPrecondition
    );
    let me = c.auth.get_me(authed(&first.token, pb::GetMeRequest {})).await.unwrap().into_inner();
    assert_eq!(me.user.unwrap().id, user.id);
    // The refresh token fuwa doesn't need goes back.
    for _ in 0..50 {
        if !issuer.revoked.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(issuer.revoked.lock().unwrap()[0].contains("refresh-me"));

    // Signing in again is the same account; a code the issuer doesn't know isn't.
    let again = linked_sign_in(&mut c, &issuer, juan.clone()).await.unwrap();
    assert!(!again.created);
    assert_eq!(again.user.unwrap().id, user.id);
    let started = start_linked(&mut c, "the app's secret").await.unwrap();
    assert_eq!(
        finish_linked(&mut c, &started.state, "made-up", "the app's secret").await.unwrap_err(),
        Code::FailedPrecondition
    );

    // Usernames already taken get a number.
    let (_, local_mika, _) = sign_up(&mut c, "mika").await;
    let mika = serde_json::json!({ "sub": "user-mika", "preferred_username": "mika", "name": "", "picture": null });
    let linked_mika = linked_sign_in(&mut c, &issuer, mika).await.unwrap();
    let linked_user = linked_mika.user.unwrap();
    assert!(linked_mika.created && !linked_mika.admin);
    assert_eq!((linked_user.username.as_str(), linked_user.display_name.as_str()), ("mika_2", "mika"));
    assert_ne!(linked_user.id, local_mika.id);
    assert_eq!(linked_user.avatar_url, "");

    // Closed: people who have an account still sign in, nobody new gets one.
    let admin = first.token;
    let closed = pb::InstanceSettings { linked_accounts: pb::LinkedAccounts::Closed as i32, ..Default::default() };
    c.admin.update_settings(authed(&admin, settings_update(closed, &["linked_accounts"], &[]))).await.unwrap();
    let auth = c.node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap().auth.unwrap();
    assert!(auth.linked_sign_in && !auth.linked_sign_up);
    assert!(linked_sign_in(&mut c, &issuer, juan.clone()).await.is_ok());
    let kai = serde_json::json!({ "sub": "user-kai", "preferred_username": "kai" });
    assert_eq!(linked_sign_in(&mut c, &issuer, kai).await.unwrap_err(), Code::FailedPrecondition);

    // Turned-off accounts stay out.
    update_account(
        &mut c,
        &admin,
        pb::UpdateAccountRequest { account_id: linked_user.id.clone(), disabled: Some(true), ..Default::default() },
    )
    .await
    .unwrap();
    let mika = serde_json::json!({ "sub": "user-mika", "preferred_username": "mika" });
    assert_eq!(linked_sign_in(&mut c, &issuer, mika).await.unwrap_err(), Code::PermissionDenied);

    // Off, or without an https address to come back to, there's no signing in with waifu.dev.
    let local = pb::InstanceSettings { public_url: "http://192.168.1.5:8080".into(), ..Default::default() };
    c.admin.update_settings(authed(&admin, settings_update(local, &["public_url"], &[]))).await.unwrap();
    let auth = c.node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap().auth.unwrap();
    assert!(!auth.linked_sign_in && auth.linked_issuer.is_empty());
    assert_eq!(start_linked(&mut c, "s").await.unwrap_err(), Code::FailedPrecondition);
    // With waifu.dev out of reach too, standalone accounts can't be turned off.
    let off = pb::InstanceSettings { local_accounts: pb::LocalAccounts::Off as i32, ..Default::default() };
    assert_eq!(
        c.admin
            .update_settings(authed(&admin, settings_update(off, &["local_accounts"], &[])))
            .await
            .unwrap_err()
            .code(),
        Code::FailedPrecondition
    );
    let https = pb::InstanceSettings {
        public_url: "https://chat.example.com".into(),
        linked_accounts: pb::LinkedAccounts::Off as i32,
        ..Default::default()
    };
    c.admin
        .update_settings(authed(&admin, settings_update(https, &["public_url", "linked_accounts"], &[])))
        .await
        .unwrap();
    assert_eq!(start_linked(&mut c, "s").await.unwrap_err(), Code::FailedPrecondition);
    let on = pb::InstanceSettings { linked_accounts: pb::LinkedAccounts::Open as i32, ..Default::default() };
    c.admin.update_settings(authed(&admin, settings_update(on, &["linked_accounts"], &[]))).await.unwrap();
    let started = start_linked(&mut c, "s").await.unwrap();
    assert!(started.authorize_url.contains("client_id=https%3A%2F%2Fchat.example.com"));
    // Standalone accounts can go now, since waifu.dev sign-in works.
    let off = pb::InstanceSettings { local_accounts: pb::LocalAccounts::Off as i32, ..Default::default() };
    c.admin.update_settings(authed(&admin, settings_update(off, &["local_accounts"], &[]))).await.unwrap();
    // ...and then the public URL has to stay one waifu.dev can send people back to.
    let lan = pb::InstanceSettings { public_url: "http://192.168.1.5:8080".into(), ..Default::default() };
    let refused =
        c.admin.update_settings(authed(&admin, settings_update(lan, &["public_url"], &[]))).await.unwrap_err();
    assert_eq!(refused.code(), Code::FailedPrecondition);
    assert!(refused.message().contains("https"), "{}", refused.message());
}

async fn set_form(
    c: &mut Clients,
    token: &str,
    server_id: &str,
    rules: &[&str],
    questions: &[(&str, bool)],
) -> Result<(), Code> {
    let form = pb::JoinForm {
        rules: rules.iter().map(|r| r.to_string()).collect(),
        questions: questions
            .iter()
            .map(|(prompt, required)| pb::JoinQuestion {
                prompt: prompt.to_string(),
                paragraph: false,
                required: *required,
            })
            .collect(),
    };
    c.join
        .set_join_form(authed(token, pb::SetJoinFormRequest { server_id: server_id.into(), form: Some(form) }))
        .await
        .map(|_| ())
        .map_err(|s| s.code())
}

async fn get_form(c: &mut Clients, token: &str, server_id: &str, code: &str) -> Result<pb::JoinForm, Code> {
    c.join
        .get_join_form(authed(token, pb::GetJoinFormRequest { server_id: server_id.into(), invite_code: code.into() }))
        .await
        .map(|r| r.into_inner().form.unwrap())
        .map_err(|s| s.code())
}

async fn update_server(c: &mut Clients, token: &str, request: pb::UpdateServerRequest) -> pb::Server {
    c.servers.update_server(authed(token, request)).await.unwrap().into_inner().server.unwrap()
}

async fn apply(
    c: &mut Clients,
    token: &str,
    server_id: &str,
    answers: &[(&str, &str)],
) -> Result<pb::Application, Code> {
    let answers =
        answers.iter().map(|(q, a)| pb::ApplicationAnswer { question: q.to_string(), answer: a.to_string() }).collect();
    c.join
        .apply_to_join(authed(
            token,
            pb::ApplyToJoinRequest { server_id: server_id.into(), invite_code: String::new(), answers },
        ))
        .await
        .map(|r| r.into_inner().application.unwrap())
        .map_err(|s| s.code())
}

async fn my_application(c: &mut Clients, token: &str, server_id: &str) -> pb::GetApplicationResponse {
    c.join
        .get_application(authed(token, pb::GetApplicationRequest { server_id: server_id.into() }))
        .await
        .unwrap()
        .into_inner()
}

async fn waiting(c: &mut Clients, token: &str, server_id: &str) -> Result<Vec<String>, Code> {
    c.join
        .list_applications(authed(token, pb::ListApplicationsRequest { server_id: server_id.into() }))
        .await
        .map(|r| r.into_inner().applications.into_iter().map(|a| a.user.unwrap().username).collect())
        .map_err(|s| s.code())
}

async fn review(
    c: &mut Clients,
    token: &str,
    server_id: &str,
    user_id: &str,
    approve: bool,
    reason: &str,
) -> Result<Option<pb::Member>, Code> {
    c.join
        .review_application(authed(
            token,
            pb::ReviewApplicationRequest {
                server_id: server_id.into(),
                user_id: user_id.into(),
                approve,
                reason: reason.into(),
            },
        ))
        .await
        .map(|r| r.into_inner().member)
        .map_err(|s| s.code())
}

/// The application events in a server's log, as one member is shown them.
async fn application_events(
    c: &mut Clients,
    token: &str,
    server_id: &str,
) -> Vec<(String, pb::ApplicationStatus, usize)> {
    let request = pb::ListEventsRequest { server_id: server_id.into(), after_sequence: 0, limit: 500 };
    c.events
        .list_events(authed(token, request))
        .await
        .unwrap()
        .into_inner()
        .events
        .into_iter()
        .filter_map(|e| match e.payload {
            Some(Payload::ApplicationUpdated(u)) => {
                let a = u.application.unwrap();
                let status = a.status();
                Some((a.user.unwrap().username, status, a.answers.len()))
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn rules_and_applications() {
    use pb::ApplicationStatus as S;
    let dir = tempfile::tempdir().unwrap();
    let (issuer_url, issuer) = fake_issuer().await;
    let instance = start(dir.path(), &[("FUWA_LINKED_ISSUER", &issuer_url)]).await;
    let mut c = clients(&instance).await;
    let (juan, _, _) = sign_up(&mut c, "juan").await;
    let (mika, _, _) = sign_up(&mut c, "mika").await;
    let (rin, rin_user, _) = sign_up(&mut c, "rin").await;
    let (kai, kai_user, _) = sign_up(&mut c, "kai").await;
    let server = create_server(&mut c, &juan, "Garden", true).await;
    let sid = server.id.clone();
    assert!(!server.has_rules && !server.applications && !server.linked_only);
    let general = c
        .channels
        .list_channels(authed(&juan, pb::ListChannelsRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels
        .remove(0);

    // Only people who manage the server write the rules, within limits.
    assert_eq!(set_form(&mut c, &mika, &sid, &["Be kind"], &[]).await.unwrap_err(), Code::PermissionDenied);
    assert_eq!(set_form(&mut c, &juan, &sid, &["  "], &[]).await.unwrap_err(), Code::InvalidArgument);
    assert_eq!(set_form(&mut c, &juan, &sid, &["x"; 17], &[]).await.unwrap_err(), Code::InvalidArgument);
    let six = [("q", false); 6];
    assert_eq!(set_form(&mut c, &juan, &sid, &[], &six).await.unwrap_err(), Code::InvalidArgument);
    let questions = [("Why do you want to join?", true), ("Favorite tea?", false)];
    set_form(&mut c, &juan, &sid, &[" Be kind ", "No spoilers outside **#spoilers**"], &questions).await.unwrap();
    let form = get_form(&mut c, &mika, &sid, "").await.unwrap();
    assert_eq!(form.rules, ["Be kind", "No spoilers outside **#spoilers**"]);
    assert_eq!(form.questions.len(), 2);
    assert!(form.questions[0].required);
    let server = c.servers.get_server(authed(&juan, pb::GetServerRequest { server_id: sid.clone() })).await;
    assert!(server.unwrap().into_inner().server.unwrap().has_rules);

    // Joining straight away, new members read first and talk once they agree.
    let member = join_with(&mut c, &mika, &sid, "").await.unwrap();
    assert!(member.pending);
    let refused = send(&mut c, &mika, &sid, &general.id, "hi!").await.unwrap_err();
    assert_eq!(refused.code(), Code::FailedPrecondition);
    assert!(refused.message().contains("rules"), "{}", refused.message());
    assert_eq!(invite(&mut c, &mika, &sid, "", 0, 0).await.unwrap_err(), Code::FailedPrecondition);
    assert!(!messages(&mut c, &mika, &sid, &general.id).await.is_empty(), "pending members can read");
    let agreed = c.join.agree_to_rules(authed(&mika, pb::AgreeToRulesRequest { server_id: sid.clone() })).await;
    assert!(!agreed.unwrap().into_inner().member.unwrap().pending);
    send(&mut c, &mika, &sid, &general.id, "hi!").await.unwrap();

    // With applications on, people apply instead, and answer the questions as asked.
    let on = pb::UpdateServerRequest { server_id: sid.clone(), applications: Some(true), ..Default::default() };
    assert!(update_server(&mut c, &juan, on).await.applications);
    assert_eq!(join_with(&mut c, &rin, &sid, "").await.unwrap_err(), Code::FailedPrecondition);
    let answers = [("Why do you want to join?", "I like gardens"), ("Favorite tea?", "")];
    assert_eq!(apply(&mut c, &rin, &sid, &answers[..1]).await.unwrap_err(), Code::FailedPrecondition);
    let reworded = [("Why join?", "I like gardens"), ("Favorite tea?", "")];
    assert_eq!(apply(&mut c, &rin, &sid, &reworded).await.unwrap_err(), Code::FailedPrecondition);
    let blank = [("Why do you want to join?", "  "), ("Favorite tea?", "")];
    assert_eq!(apply(&mut c, &rin, &sid, &blank).await.unwrap_err(), Code::InvalidArgument);
    let sent = apply(&mut c, &rin, &sid, &answers).await.unwrap();
    assert_eq!(sent.status(), S::Pending);
    assert_eq!(sent.answers[0].answer, "I like gardens");
    assert_eq!(apply(&mut c, &rin, &sid, &answers).await.unwrap_err(), Code::AlreadyExists);
    assert_eq!(my_application(&mut c, &rin, &sid).await.application.unwrap().status(), S::Pending);
    assert_eq!(apply(&mut c, &mika, &sid, &answers).await.unwrap_err(), Code::AlreadyExists);

    // People who can kick members review them; nobody else sees them.
    assert_eq!(waiting(&mut c, &juan, &sid).await.unwrap(), ["rin"]);
    assert_eq!(waiting(&mut c, &mika, &sid).await.unwrap_err(), Code::PermissionDenied);
    assert_eq!(review(&mut c, &mika, &sid, &rin_user.id, true, "").await.unwrap_err(), Code::PermissionDenied);
    assert_eq!(review(&mut c, &juan, &sid, &kai_user.id, true, "").await.unwrap_err(), Code::NotFound);
    assert_eq!(review(&mut c, &juan, &sid, &rin_user.id, false, "Tell us a bit more").await.unwrap(), None);
    let turned_down = my_application(&mut c, &rin, &sid).await.application.unwrap();
    assert_eq!((turned_down.status(), turned_down.reason.as_str()), (S::Rejected, "Tell us a bit more"));
    assert_eq!(waiting(&mut c, &juan, &sid).await.unwrap(), Vec::<String>::new());
    assert_eq!(review(&mut c, &juan, &sid, &rin_user.id, true, "").await.unwrap_err(), Code::NotFound);

    // A turned-down applicant can try again; this time they're let in, rules agreed.
    let again = [("Why do you want to join?", "I grow roses and want to share"), ("Favorite tea?", "Sencha")];
    apply(&mut c, &rin, &sid, &again).await.unwrap();
    let member = review(&mut c, &juan, &sid, &rin_user.id, true, "").await.unwrap().unwrap();
    assert!(!member.pending);
    let status = my_application(&mut c, &rin, &sid).await;
    assert!(status.member && status.application.is_none());
    assert_eq!(status.server.unwrap().member_count, 3);
    send(&mut c, &rin, &sid, &general.id, "hello garden").await.unwrap();

    // Applicants can take it back.
    apply(&mut c, &kai, &sid, &answers).await.unwrap();
    c.join.withdraw_application(authed(&kai, pb::WithdrawApplicationRequest { server_id: sid.clone() })).await.unwrap();
    assert!(my_application(&mut c, &kai, &sid).await.application.is_none());
    let gone =
        c.join.withdraw_application(authed(&kai, pb::WithdrawApplicationRequest { server_id: sid.clone() })).await;
    assert_eq!(gone.unwrap_err().code(), Code::NotFound);

    // Banning an applicant takes their application away.
    apply(&mut c, &kai, &sid, &answers).await.unwrap();
    let ban = pb::BanMemberRequest { server_id: sid.clone(), user_id: kai_user.id.clone(), ..Default::default() };
    c.servers.ban_member(authed(&juan, ban)).await.unwrap();
    assert!(my_application(&mut c, &kai, &sid).await.application.is_none());
    assert_eq!(waiting(&mut c, &juan, &sid).await.unwrap(), Vec::<String>::new());
    assert_eq!(apply(&mut c, &kai, &sid, &answers).await.unwrap_err(), Code::PermissionDenied);

    // Reviewers hear about applications; other members don't, and closed ones carry no answers.
    assert_eq!(application_events(&mut c, &mika, &sid).await, []);
    assert_eq!(
        application_events(&mut c, &juan, &sid).await,
        [
            ("rin".to_string(), S::Pending, 2),
            ("rin".to_string(), S::Rejected, 0),
            ("rin".to_string(), S::Pending, 2),
            ("rin".to_string(), S::Approved, 0),
            ("kai".to_string(), S::Pending, 2),
            ("kai".to_string(), S::Withdrawn, 0),
            ("kai".to_string(), S::Pending, 2),
            ("kai".to_string(), S::Withdrawn, 0),
        ]
    );
    let log = audit_log(&mut c, &juan, pb::ListAuditLogRequest { server_id: sid.clone(), ..Default::default() }).await;
    let actions: Vec<_> = log.entries.iter().map(|e| e.action()).collect();
    use pb::AuditAction as A;
    for action in [A::JoinFormUpdate, A::ApplicationReject, A::ApplicationApprove] {
        assert!(actions.contains(&action), "{action:?} in {actions:?}");
    }
    let rejected = log.entries.iter().find(|e| e.action() == A::ApplicationReject).unwrap();
    assert_eq!((rejected.target_id.as_str(), rejected.reason.as_str()), (rin_user.id.as_str(), "Tell us a bit more"));

    // waifu.dev accounts only.
    let linked = pb::UpdateServerRequest { server_id: sid.clone(), linked_only: Some(true), ..Default::default() };
    assert!(update_server(&mut c, &juan, linked).await.linked_only);
    let (momo, _, _) = sign_up(&mut c, "momo").await;
    assert_eq!(apply(&mut c, &momo, &sid, &answers).await.unwrap_err(), Code::FailedPrecondition);
    let user = serde_json::json!({ "sub": "user-sora", "preferred_username": "sora", "name": "Sora" });
    let sora = linked_sign_in(&mut c, &issuer, user).await.unwrap();
    apply(&mut c, &sora.token, &sid, &answers).await.unwrap();
    assert_eq!(waiting(&mut c, &juan, &sid).await.unwrap(), ["sora"]);

    // Turning applications off: people join straight away, and leftovers go.
    let off = pb::UpdateServerRequest {
        server_id: sid.clone(),
        applications: Some(false),
        linked_only: Some(false),
        ..Default::default()
    };
    update_server(&mut c, &juan, off).await;
    assert!(join_with(&mut c, &momo, &sid, "").await.unwrap().pending);
    join_with(&mut c, &sora.token, &sid, "").await.unwrap();
    assert_eq!(waiting(&mut c, &juan, &sid).await.unwrap(), Vec::<String>::new());

    // Without rules there's nothing to agree to: people waiting can talk, newcomers too.
    set_form(&mut c, &juan, &sid, &[], &questions).await.unwrap();
    send(&mut c, &momo, &sid, &general.id, "finally").await.unwrap();
    let (nao, _, _) = sign_up(&mut c, "nao").await;
    assert!(!join_with(&mut c, &nao, &sid, "").await.unwrap().pending);

    // A server out of Browse shows its form only to members and people with an invite.
    let hidden = create_server(&mut c, &juan, "Hidden", false).await;
    assert_eq!(get_form(&mut c, &mika, &hidden.id, "").await.unwrap_err(), Code::NotFound);
    let code = invite(&mut c, &juan, &hidden.id, "", 0, 0).await.unwrap().code;
    assert!(get_form(&mut c, &mika, &hidden.id, &code).await.is_ok());
    assert_eq!(
        c.join
            .get_application(authed(&mika, pb::GetApplicationRequest { server_id: hidden.id.clone() }))
            .await
            .unwrap_err()
            .code(),
        Code::NotFound
    );

    // It all survives a restart.
    drop(c);
    instance.stop().await;
    let instance = start(dir.path(), &[("FUWA_LINKED_ISSUER", &issuer_url)]).await;
    let mut c = clients(&instance).await;
    let form = get_form(&mut c, &juan, &sid, "").await.unwrap();
    assert!(form.rules.is_empty());
    assert_eq!(form.questions.len(), 2);
    drop(c);
    instance.stop().await;
}

async fn save_rule(
    c: &mut Clients,
    token: &str,
    server_id: &str,
    rule: pb::AutoModRule,
) -> Result<pb::AutoModRule, tonic::Status> {
    c.automod
        .save_auto_mod_rule(authed(token, pb::SaveAutoModRuleRequest { server_id: server_id.into(), rule: Some(rule) }))
        .await
        .map(|r| r.into_inner().rule.unwrap())
}

fn act(kind: pb::AutoModActionKind) -> pb::AutoModAction {
    pb::AutoModAction { kind: kind as i32, ..Default::default() }
}

#[tokio::test]
async fn automod_catches_messages() {
    use pb::{AutoModActionKind as Kind, AutoModTrigger as Trigger};
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (owner, _, _) = sign_up(&mut c, "owner").await;
    let (member, member_user, _) = sign_up(&mut c, "member").await;
    let server = create_server(&mut c, &owner, "Guarded", true).await;
    join(&mut c, &member, &server.id).await;
    let general = c
        .channels
        .list_channels(authed(&owner, pb::ListChannelsRequest { server_id: server.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels
        .into_iter()
        .find(|ch| ch.r#type == pb::ChannelType::Text as i32)
        .unwrap();
    let mods = new_channel(&mut c, &owner, &server.id, "mod-log", pb::ChannelType::Text).await;

    // Only managers see or change the rules.
    let denied = save_rule(
        &mut c,
        &member,
        &server.id,
        pb::AutoModRule {
            trigger: Trigger::Keywords as i32,
            keywords: vec!["x".into()],
            actions: vec![act(Kind::Block)],
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(denied.code(), Code::PermissionDenied);

    // A rule needs something to look for and something to do.
    let empty = save_rule(
        &mut c,
        &owner,
        &server.id,
        pb::AutoModRule { trigger: Trigger::Keywords as i32, ..Default::default() },
    )
    .await
    .unwrap_err();
    assert_eq!(empty.code(), Code::InvalidArgument);

    let words = save_rule(
        &mut c,
        &owner,
        &server.id,
        pb::AutoModRule {
            enabled: true,
            trigger: Trigger::Keywords as i32,
            keywords: vec!["Badword".into(), "*scam*".into(), "badword".into()],
            allowed: vec!["scampi".into()],
            actions: vec![
                pb::AutoModAction { kind: Kind::Block as i32, message: "Keep it kind.".into(), ..Default::default() },
                pb::AutoModAction { kind: Kind::Alert as i32, channel_id: mods.id.clone(), ..Default::default() },
            ],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(words.name, "Blocked words");
    assert_eq!(words.keywords, ["badword", "*scam*"]);

    // Caught: not sent, the author is told why, and the moderators hear about it.
    let blocked = send(&mut c, &member, &server.id, &general.id, "what a BADWORD").await.unwrap_err();
    assert_eq!(blocked.code(), Code::PermissionDenied);
    assert_eq!(blocked.message(), "AutoMod: Keep it kind.");
    assert!(messages(&mut c, &owner, &server.id, &general.id).await.iter().all(|m| !m.content.contains("BADWORD")));
    let alerts = messages(&mut c, &owner, &server.id, &mods.id).await;
    let alert = alerts.last().unwrap();
    assert_eq!(alert.kind, pb::MessageKind::AutoModAlert as i32);
    assert_eq!(alert.author_id, member_user.id);
    assert!(alert.content.is_empty());
    let details = alert.auto_mod.as_ref().unwrap();
    assert_eq!(
        (details.content.as_str(), details.matched.as_slice(), details.blocked),
        ("what a BADWORD", &["badword".to_string()][..], true)
    );
    assert_eq!(details.channel_id, general.id);

    // Allowed words and other words pass; the owner is never stopped.
    send(&mut c, &member, &server.id, &general.id, "garlic scampi tonight").await.unwrap();
    send(&mut c, &owner, &server.id, &general.id, "badword, as the owner").await.unwrap();

    // Edits are checked too.
    let sent = send(&mut c, &member, &server.id, &general.id, "hello").await.unwrap();
    let edit = c
        .messages
        .update_message(authed(
            &member,
            pb::UpdateMessageRequest {
                channel_id: String::new(),
                server_id: server.id.clone(),
                message_id: sent.id.clone(),
                content: "a scammer!".into(),
            },
        ))
        .await
        .unwrap_err();
    assert_eq!(edit.code(), Code::PermissionDenied);

    // Exempt channels are left alone.
    let lounge = new_channel(&mut c, &owner, &server.id, "lounge", pb::ChannelType::Text).await;
    save_rule(
        &mut c,
        &owner,
        &server.id,
        pb::AutoModRule { exempt_channel_ids: vec![lounge.id.clone()], ..words.clone() },
    )
    .await
    .unwrap();
    send(&mut c, &member, &server.id, &lounge.id, "badword in the lounge").await.unwrap();

    // Mention spam times people out; there's one such rule per server.
    let spam = pb::AutoModRule {
        enabled: true,
        trigger: Trigger::MentionSpam as i32,
        mention_limit: 2,
        actions: vec![pb::AutoModAction { kind: Kind::TimeOut as i32, duration_seconds: 600, ..Default::default() }],
        ..Default::default()
    };
    save_rule(&mut c, &owner, &server.id, spam.clone()).await.unwrap();
    assert_eq!(save_rule(&mut c, &owner, &server.id, spam).await.unwrap_err().code(), Code::FailedPrecondition);
    send(&mut c, &member, &server.id, &general.id, "@a @b @c").await.unwrap();
    let timed_out = send(&mut c, &member, &server.id, &general.id, "hi").await.unwrap_err();
    assert!(timed_out.message().contains("timed out"), "{}", timed_out.message());
    let log =
        audit_log(&mut c, &owner, pb::ListAuditLogRequest { server_id: server.id.clone(), ..Default::default() }).await;
    let entry = log.entries.iter().find(|e| e.action == pb::AuditAction::AutoModTimeOut as i32).unwrap();
    assert_eq!((entry.target_id.as_str(), entry.reason.as_str()), (member_user.id.as_str(), "Mention spam"));
    assert!(log.entries.iter().any(|e| e.action == pb::AuditAction::AutoModRuleCreate as i32));

    // Trying a rule out sends nothing.
    let tried = c
        .automod
        .test_auto_mod_rule(authed(
            &owner,
            pb::TestAutoModRuleRequest {
                server_id: server.id.clone(),
                rule: Some(pb::AutoModRule {
                    trigger: Trigger::Links as i32,
                    allowed: vec!["waifu.dev".into()],
                    ..Default::default()
                }),
                content: "see https://waifu.dev and https://evil.example".into(),
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(tried.matched);
    assert_eq!(tried.matches, ["evil.example"]);

    let rules = c
        .automod
        .list_auto_mod_rules(authed(&owner, pb::ListAutoModRulesRequest { server_id: server.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .rules;
    assert_eq!(rules.len(), 2);
    c.automod
        .delete_auto_mod_rule(authed(
            &owner,
            pb::DeleteAutoModRuleRequest { server_id: server.id.clone(), rule_id: words.id.clone() },
        ))
        .await
        .unwrap();
    instance.stop().await;
}

#[tokio::test]
async fn automod_providers_are_set_up_once_and_picked_per_server() {
    use pb::{AutoModActionKind as Kind, AutoModLevel as Level, AutoModTrigger as Trigger};
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (admin, _, _) = sign_up(&mut c, "admin").await;
    let (owner, _, _) = sign_up(&mut c, "owner").await;
    let (member, _, _) = sign_up(&mut c, "member").await;
    let server = create_server(&mut c, &owner, "Smart", true).await;
    join(&mut c, &member, &server.id).await;
    let general = new_channel(&mut c, &owner, &server.id, "general", pb::ChannelType::Text).await;
    let mods = new_channel(&mut c, &owner, &server.id, "mod-log", pb::ChannelType::Text).await;
    let list = async |c: &mut Clients| {
        c.automod
            .list_auto_mod_rules(authed(&owner, pb::ListAutoModRulesRequest { server_id: server.id.clone() }))
            .await
            .unwrap()
            .into_inner()
    };
    let smart = pb::AutoModRule {
        enabled: true,
        trigger: Trigger::Provider as i32,
        provider: "cloudflare-clef".into(),
        actions: vec![pb::AutoModAction {
            kind: Kind::Alert as i32,
            channel_id: mods.id.clone(),
            ..Default::default()
        }],
        ..Default::default()
    };

    // Every provider fuwa knows is listed, none on, and servers can't pick one yet.
    let settings = c.admin.get_settings(authed(&admin, pb::GetSettingsRequest {})).await.unwrap().into_inner();
    let providers = settings.config.unwrap().settings.unwrap().automod_providers;
    assert_eq!(providers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["typesafe-jev", "cloudflare-clef"]);
    assert!(providers.iter().all(|p| !p.enabled && !p.api_key_set));
    assert!(list(&mut c).await.providers.is_empty());
    let early = save_rule(&mut c, &owner, &server.id, smart.clone()).await.unwrap_err();
    assert_eq!(early.code(), Code::FailedPrecondition);

    // Turning one on needs its key (and Cloudflare's account id); the key never comes back.
    let clef = |key: &str| pb::AutoModProviderSettings {
        id: "cloudflare-clef".into(),
        enabled: true,
        api_key: key.into(),
        account_id: "0".repeat(32),
        ..Default::default()
    };
    let update = |providers: Vec<pb::AutoModProviderSettings>| {
        settings_update(
            pb::InstanceSettings { automod_providers: providers, ..Default::default() },
            &["automod_providers"],
            &[],
        )
    };
    let keyless = c.admin.update_settings(authed(&admin, update(vec![clef("")]))).await.unwrap_err();
    assert_eq!(keyless.code(), Code::InvalidArgument);
    let denied = c.admin.update_settings(authed(&owner, update(vec![clef("not-a-real-token-1234")]))).await;
    assert_eq!(denied.unwrap_err().code(), Code::PermissionDenied);
    let saved = c
        .admin
        .update_settings(authed(&admin, update(vec![clef("not-a-real-token-1234")])))
        .await
        .unwrap()
        .into_inner()
        .config
        .unwrap()
        .settings
        .unwrap()
        .automod_providers;
    let shown = saved.iter().find(|p| p.id == "cloudflare-clef").unwrap();
    assert!(shown.enabled && shown.api_key_set && shown.api_key.is_empty());
    assert_eq!(shown.api_key_hint, "1234");
    // Saving what came back (no key) keeps the key.
    let again = c.admin.update_settings(authed(&admin, update(saved.clone()))).await.unwrap().into_inner();
    let kept = again.config.unwrap().settings.unwrap().automod_providers;
    assert!(kept.iter().find(|p| p.id == "cloudflare-clef").unwrap().api_key_set);

    // Servers now see it, with where checked messages go and a default level per label.
    let offered = list(&mut c).await.providers;
    assert_eq!(offered.len(), 1);
    assert_eq!((offered[0].name.as_str(), offered[0].host.as_str()), ("Cloudflare Clef", "api.cloudflare.com"));
    assert!(offered[0].labels.iter().any(|l| l.id == "hate" && l.default_level == Level::Block as i32));
    // Clef reads pictures too.
    assert!(offered[0].pictures);

    // One switch: a rule with no labels gets the defaults. Flags need a channel.
    let rule = save_rule(&mut c, &owner, &server.id, smart.clone()).await.unwrap();
    assert_eq!(rule.name, "Smart filter");
    assert_eq!(rule.labels.len(), offered[0].labels.len());
    assert!(rule.labels.iter().all(|l| l.threshold == 80));
    let no_channel = save_rule(&mut c, &owner, &server.id, pb::AutoModRule { actions: vec![], ..rule.clone() }).await;
    assert_eq!(no_channel.unwrap_err().code(), Code::InvalidArgument);
    let other = save_rule(&mut c, &owner, &server.id, pb::AutoModRule { id: String::new(), ..rule.clone() }).await;
    assert_eq!(other.unwrap_err().code(), Code::FailedPrecondition);

    // The key is made up, so the provider turns it down (or can't be reached):
    // messages still go through, and the test says why.
    send(&mut c, &member, &server.id, &general.id, "hello there").await.unwrap();

    // Showing it pictures too: a message that's only a picture is read and
    // asked about, and still goes through when the provider doesn't answer.
    let rule = save_rule(&mut c, &owner, &server.id, pb::AutoModRule { pictures: true, ..rule.clone() }).await.unwrap();
    assert!(rule.pictures);
    // A 16 by 16 PNG: its header is all the server looks at before sending it.
    let mut cat = png(300, 0);
    cat[16..24].copy_from_slice(&[0, 0, 0, 16, 0, 0, 0, 16]);
    let picture = upload(&mut c, &instance, &owner, pb::MediaPurpose::Emoji, cat).await;
    let sent = c
        .messages
        .send_message(authed(
            &member,
            pb::SendMessageRequest {
                server_id: server.id.clone(),
                channel_id: general.id.clone(),
                attachments: vec![pb::Attachment {
                    filename: "cat.png".into(),
                    content_type: "image/png".into(),
                    url: picture.clone(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(sent.message.unwrap().attachments.len(), 1);
    let tried = c
        .automod
        .test_auto_mod_rule(authed(
            &owner,
            pb::TestAutoModRuleRequest {
                server_id: server.id.clone(),
                rule: Some(rule.clone()),
                content: "hello there".into(),
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(!tried.matched && !tried.error.is_empty());
    let tested = c
        .admin
        .test_auto_mod_provider(authed(
            &admin,
            pb::TestAutoModProviderRequest { provider: Some(clef("")), content: "hello".into() },
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(!tested.ok && !tested.error.is_empty());

    // The admins' own provider: any https address, named by them, its key in
    // the header they pick. It gets an id; the key never comes back.
    let ours = pb::AutoModProviderSettings {
        id: "custom".into(),
        enabled: true,
        name: "Our classifier".into(),
        url: "https://fuwa-test.invalid/v1/check".into(),
        api_key: "our-secret-key-5678".into(),
        header: "X-Api-Key".into(),
        ..Default::default()
    };
    let plain = pb::AutoModProviderSettings { url: "http://mod.example.com/check".into(), ..ours.clone() };
    let refused = c.admin.update_settings(authed(&admin, update(vec![clef(""), plain]))).await.unwrap_err();
    assert_eq!(refused.code(), Code::InvalidArgument);
    // Nor anything on the instance's own network.
    let inside = pb::AutoModProviderSettings { url: "https://169.254.169.254/latest".into(), ..ours.clone() };
    let refused = c.admin.update_settings(authed(&admin, update(vec![clef(""), inside]))).await.unwrap_err();
    assert_eq!(refused.code(), Code::InvalidArgument);
    let saved = c
        .admin
        .update_settings(authed(&admin, update(vec![clef(""), ours.clone()])))
        .await
        .unwrap()
        .into_inner()
        .config
        .unwrap()
        .settings
        .unwrap()
        .automod_providers;
    assert_eq!(saved.len(), 3);
    let mine = saved[2].clone();
    assert!(mine.id.starts_with("custom-") && mine.api_key.is_empty() && mine.api_key_set, "{mine:?}");
    assert_eq!(
        (mine.name.as_str(), mine.header.as_str(), mine.api_key_hint.as_str()),
        ("Our classifier", "x-api-key", "5678")
    );
    let offered = list(&mut c).await.providers;
    let theirs = offered.iter().find(|p| p.id == mine.id).unwrap();
    assert_eq!((theirs.name.as_str(), theirs.host.as_str()), ("Our classifier", "fuwa-test.invalid"));

    // A server's smart filter picks it. Nothing answers there, so messages go
    // through, and the test says why.
    let switched = save_rule(&mut c, &owner, &server.id, pb::AutoModRule { provider: mine.id.clone(), ..rule.clone() })
        .await
        .unwrap();
    assert_eq!(switched.provider, mine.id);
    // The admins' own providers read text only, so the rule stops asking for pictures.
    assert!(!switched.pictures && !theirs.pictures);
    send(&mut c, &member, &server.id, &general.id, "hello again").await.unwrap();
    let tried = c
        .automod
        .test_auto_mod_rule(authed(
            &owner,
            pb::TestAutoModRuleRequest {
                server_id: server.id.clone(),
                rule: Some(switched.clone()),
                content: "hello again".into(),
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(!tried.matched && tried.error.contains("couldn't"), "{tried:?}");

    // Turned off on the instance: servers no longer see it, and their rule lets messages through.
    let off = pb::AutoModProviderSettings { enabled: false, ..clef("") };
    c.admin.update_settings(authed(&admin, update(vec![off]))).await.unwrap();
    assert!(list(&mut c).await.providers.is_empty());
    send(&mut c, &member, &server.id, &general.id, "still here").await.unwrap();
    instance.stop().await;
}

#[tokio::test]
async fn custom_emoji_and_the_welcome_screen() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[("FUWA_LIMIT_EMOJIS", "2")]).await;
    let mut c = clients(&instance).await;
    let (owner, _, _) = sign_up(&mut c, "owner").await;
    let (member, _, _) = sign_up(&mut c, "member").await;
    let server = create_server(&mut c, &owner, "Emotes", true).await;
    join(&mut c, &member, &server.id).await;
    let mut stream = c
        .events
        .subscribe(authed(
            &member,
            pb::SubscribeRequest {
                servers: vec![pb::ServerCursor { server_id: server.id.clone(), after_sequence: None }],
            },
        ))
        .await
        .unwrap()
        .into_inner();

    let create = |name: &str, url: &str| pb::CreateEmojiRequest {
        server_id: server.id.clone(),
        name: name.into(),
        url: url.into(),
    };
    let picture = upload(&mut c, &instance, &owner, pb::MediaPurpose::Emoji, png(300, 1)).await;
    // Members without Manage emoji can't add any, and only uploads made for emoji work.
    let theirs = upload(&mut c, &instance, &member, pb::MediaPurpose::Emoji, png(300, 2)).await;
    assert_eq!(
        c.emojis.create_emoji(authed(&member, create("nope", &theirs))).await.unwrap_err().code(),
        Code::PermissionDenied
    );
    assert_eq!(
        c.emojis.create_emoji(authed(&owner, create("outside", "https://example.com/x.png"))).await.unwrap_err().code(),
        Code::InvalidArgument
    );
    let icon = upload(&mut c, &instance, &owner, pb::MediaPurpose::ServerIcon, png(300, 3)).await;
    assert_eq!(
        c.emojis.create_emoji(authed(&owner, create("icon", &icon))).await.unwrap_err().code(),
        Code::PermissionDenied
    );
    assert_eq!(
        c.emojis.create_emoji(authed(&owner, create("bad name!", &picture))).await.unwrap_err().code(),
        Code::InvalidArgument
    );

    let wave =
        c.emojis.create_emoji(authed(&owner, create(":wave:", &picture))).await.unwrap().into_inner().emoji.unwrap();
    assert_eq!((wave.name.as_str(), wave.size, wave.animated), ("wave", 300, false));
    let event = next_event(&mut stream).await;
    let Some(pb::event::Payload::EmojisUpdated(updated)) = event.payload else { panic!("{event:?}") };
    assert_eq!(updated.emojis.len(), 1);

    let second = upload(&mut c, &instance, &owner, pb::MediaPurpose::Emoji, png(200, 4)).await;
    assert_eq!(
        c.emojis.create_emoji(authed(&owner, create("WAVE", &second))).await.unwrap_err().code(),
        Code::AlreadyExists
    );
    let blob =
        c.emojis.create_emoji(authed(&owner, create("blob", &second))).await.unwrap().into_inner().emoji.unwrap();
    let third = upload(&mut c, &instance, &owner, pb::MediaPurpose::Emoji, png(100, 5)).await;
    assert_eq!(
        c.emojis.create_emoji(authed(&owner, create("third", &third))).await.unwrap_err().code(),
        Code::ResourceExhausted
    );

    let used = usage(&mut c, &owner, &server.id).await;
    assert_eq!((used.emojis, used.attachments, used.attachment_bytes), (2, 2, 500));

    let renamed = c
        .emojis
        .update_emoji(authed(
            &owner,
            pb::UpdateEmojiRequest { server_id: server.id.clone(), emoji_id: blob.id.clone(), name: "blobcat".into() },
        ))
        .await
        .unwrap()
        .into_inner()
        .emoji
        .unwrap();
    assert_eq!(renamed.name, "blobcat");
    c.emojis
        .delete_emoji(authed(
            &owner,
            pb::DeleteEmojiRequest { server_id: server.id.clone(), emoji_id: blob.id.clone() },
        ))
        .await
        .unwrap();
    assert_eq!(fetch(&instance, &second).await.0, reqwest::StatusCode::NOT_FOUND);
    let listed = c
        .emojis
        .list_emojis(authed(&member, pb::ListEmojisRequest { server_id: server.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .emojis;
    assert_eq!(listed.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["wave"]);
    let used = usage(&mut c, &owner, &server.id).await;
    assert_eq!((used.emojis, used.attachment_bytes), (1, 300));

    // The welcome screen: managers set it, members see the channels they can.
    let channels = c
        .channels
        .list_channels(authed(&owner, pb::ListChannelsRequest { server_id: server.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels;
    let general = channels.iter().find(|ch| ch.r#type == pb::ChannelType::Text as i32).unwrap().clone();
    let secret = new_channel(&mut c, &owner, &server.id, "secret", pb::ChannelType::Text).await;
    let everyone = server.id.clone();
    set_permissions(
        &mut c,
        &owner,
        &server.id,
        &secret.id,
        vec![overwrite(&everyone, pb::OverwriteTarget::Role, &[], &[pb::Permission::ViewChannels])],
    )
    .await
    .unwrap();
    let welcome = pb::WelcomeScreen {
        enabled: true,
        description: "  Hi **there**  ".into(),
        channels: vec![
            pb::WelcomeChannel {
                channel_id: general.id.clone(),
                description: "Say hi".into(),
                emoji: format!("<:wave:{}>", wave.id),
            },
            pb::WelcomeChannel { channel_id: secret.id.clone(), description: "Shh".into(), emoji: "🤫".into() },
        ],
    };
    let set =
        |w: pb::WelcomeScreen| pb::SetWelcomeScreenRequest { server_id: server.id.clone(), welcome_screen: Some(w) };
    assert_eq!(
        c.join.set_welcome_screen(authed(&member, set(welcome.clone()))).await.unwrap_err().code(),
        Code::PermissionDenied
    );
    let mut wrong = welcome.clone();
    wrong.channels[1].emoji = "<:gone:01ARZ3NDEKTSV4RRFFQ69G5FAV>".into();
    assert_eq!(c.join.set_welcome_screen(authed(&owner, set(wrong))).await.unwrap_err().code(), Code::InvalidArgument);
    let saved =
        c.join.set_welcome_screen(authed(&owner, set(welcome))).await.unwrap().into_inner().welcome_screen.unwrap();
    assert_eq!(saved.description, "Hi **there**");
    let got = |token: &str| {
        let mut join = c.join.clone();
        let request = authed(token, pb::GetWelcomeScreenRequest { server_id: server.id.clone() });
        async move { join.get_welcome_screen(request).await.unwrap().into_inner().welcome_screen.unwrap() }
    };
    assert_eq!(got(&member).await.channels.len(), 1);
    assert_eq!(got(&owner).await.channels.len(), 2);
    let server_now = c
        .servers
        .get_server(authed(&member, pb::GetServerRequest { server_id: server.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap();
    assert!(server_now.has_welcome_screen);
    instance.stop().await;
}

/// Posts to a webhook's address the way other apps do.
async fn post_webhook(instance: &Instance, webhook: &pb::Webhook, body: &str, wait: bool) -> reqwest::Response {
    let url = format!(
        "http://{}/webhooks/{}/{}/{}{}",
        instance.addr,
        webhook.server_id,
        webhook.id,
        webhook.token,
        if wait { "?wait=true" } else { "" }
    );
    reqwest::Client::new()
        .post(url)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn webhooks_post_into_channels() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (owner, _, _) = sign_up(&mut c, "owner").await;
    let (member, _, _) = sign_up(&mut c, "member").await;
    let server = create_server(&mut c, &owner, "Hooks", true).await;
    join(&mut c, &member, &server.id).await;
    let general = c
        .channels
        .list_channels(authed(&owner, pb::ListChannelsRequest { server_id: server.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels
        .into_iter()
        .find(|ch| ch.name == "general")
        .unwrap();
    let news = new_channel(&mut c, &owner, &server.id, "news", pb::ChannelType::Text).await;

    let create = |channel_id: &str, name: &str, avatar_url: &str| pb::CreateWebhookRequest {
        server_id: server.id.clone(),
        channel_id: channel_id.into(),
        name: name.into(),
        avatar_url: avatar_url.into(),
    };
    // Members without Manage webhooks can't make or see any.
    assert_eq!(
        c.webhooks.create_webhook(authed(&member, create(&general.id, "Nope", ""))).await.unwrap_err().code(),
        Code::PermissionDenied
    );
    assert_eq!(
        c.webhooks
            .list_webhooks(authed(&member, pb::ListWebhooksRequest { server_id: server.id.clone() }))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );
    assert_eq!(
        c.webhooks.create_webhook(authed(&owner, create(&general.id, "", ""))).await.unwrap_err().code(),
        Code::InvalidArgument
    );
    assert_eq!(
        c.webhooks
            .create_webhook(authed(&owner, create(&general.id, "Outside", "https://example.com/a.png")))
            .await
            .unwrap_err()
            .code(),
        Code::InvalidArgument
    );

    let picture = upload(&mut c, &instance, &owner, pb::MediaPurpose::Avatar, png(300, 7)).await;
    let hook = c
        .webhooks
        .create_webhook(authed(&owner, create(&general.id, "Build bot", &picture)))
        .await
        .unwrap()
        .into_inner()
        .webhook
        .unwrap();
    assert_eq!((hook.name.as_str(), hook.avatar_url.as_str()), ("Build bot", picture.as_str()));
    assert_eq!(hook.token.len(), 64);

    // A Discord-shaped post lands in its channel under the webhook's name.
    let response = post_webhook(
        &instance,
        &hook,
        r#"{"content":"build **passed** @everyone","embeds":[{"title":"main","color":65280,"fields":[{"name":"took","value":"3m"}]}],"tts":false}"#,
        true,
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let posted: serde_json::Value = response.json().await.unwrap();
    let listed = messages(&mut c, &member, &server.id, &general.id).await;
    let message = listed.iter().find(|m| m.id == posted["id"].as_str().unwrap()).unwrap();
    assert_eq!(message.author_id, hook.id);
    assert!(!message.mentions_everyone, "webhooks never ping everyone");
    let author = message.webhook.as_ref().unwrap();
    assert_eq!((author.name.as_str(), author.avatar_url.as_str()), ("Build bot", picture.as_str()));
    assert_eq!(message.embeds[0].color, 0x00FF00);

    // A post can go out under another name; no ?wait answers 204.
    let response =
        post_webhook(&instance, &hook, r#"{"content":"deploying","username":"Deploys","avatar_url":""}"#, false).await;
    assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
    let last = messages(&mut c, &member, &server.id, &general.id).await.pop().unwrap();
    assert_eq!(last.webhook.unwrap().name, "Deploys");

    // Pictures from other sites, in embeds or as a post's avatar, come
    // through the instance; links stay as they are.
    let response = post_webhook(
        &instance,
        &hook,
        r#"{"avatar_url":"https://tracker.example/a.png","embeds":[{"url":"https://example.com/","image":{"url":"https://tracker.example/i.png"},"thumbnail":{"url":"https://tracker.example/t.png"}}]}"#,
        false,
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
    let last = messages(&mut c, &member, &server.id, &general.id).await.pop().unwrap();
    let embed = &last.embeds[0];
    assert_eq!(embed.url, "https://example.com/");
    for picture in [&last.webhook.as_ref().unwrap().avatar_url, &embed.image_url, &embed.thumbnail_url] {
        assert!(picture.starts_with("http://localhost:8080/media/outside/"), "{picture}");
    }
    let fetched =
        reqwest::get(format!("http://{}/media/outside/{}?url=x", instance.addr, "0".repeat(32))).await.unwrap();
    assert_eq!(fetched.status(), reqwest::StatusCode::NOT_FOUND, "unsigned links are never fetched");

    // Bad posts and wrong tokens.
    assert_eq!(post_webhook(&instance, &hook, "not json", false).await.status(), reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(
        post_webhook(&instance, &hook, r#"{"content":"  "}"#, false).await.status(),
        reqwest::StatusCode::BAD_REQUEST
    );
    let wrong = pb::Webhook { token: "x".repeat(64), ..hook.clone() };
    assert_eq!(
        post_webhook(&instance, &wrong, r#"{"content":"hi"}"#, false).await.status(),
        reqwest::StatusCode::NOT_FOUND
    );

    // Members can't edit webhook messages; managers can delete them.
    assert_eq!(
        c.messages
            .update_message(authed(
                &owner,
                pb::UpdateMessageRequest {
                    channel_id: String::new(),
                    server_id: server.id.clone(),
                    message_id: message.id.clone(),
                    content: "mine now".into()
                }
            ))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );

    // Moving it and resetting its address.
    let moved = c
        .webhooks
        .update_webhook(authed(
            &owner,
            pb::UpdateWebhookRequest {
                server_id: server.id.clone(),
                webhook_id: hook.id.clone(),
                name: "CI".into(),
                avatar_url: picture.clone(),
                channel_id: news.id.clone(),
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .webhook
        .unwrap();
    assert_eq!((moved.name.as_str(), moved.channel_id.as_str()), ("CI", news.id.as_str()));
    assert_eq!(
        post_webhook(&instance, &moved, r#"{"content":"over here"}"#, false).await.status(),
        reqwest::StatusCode::NO_CONTENT
    );
    assert_eq!(messages(&mut c, &member, &server.id, &news.id).await.len(), 1);
    let reset = c
        .webhooks
        .reset_webhook_token(authed(
            &owner,
            pb::ResetWebhookTokenRequest { server_id: server.id.clone(), webhook_id: hook.id.clone() },
        ))
        .await
        .unwrap()
        .into_inner()
        .webhook
        .unwrap();
    assert_ne!(reset.token, hook.token);
    assert_eq!(
        post_webhook(&instance, &hook, r#"{"content":"old"}"#, false).await.status(),
        reqwest::StatusCode::NOT_FOUND
    );

    let listed = c
        .webhooks
        .list_webhooks(authed(&owner, pb::ListWebhooksRequest { server_id: server.id.clone() }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(listed.webhooks.len(), 1);
    assert_eq!(listed.webhooks[0].messages, 4);
    assert!(listed.webhooks[0].last_used_at.is_some());
    assert_eq!(listed.creators[0].username, "owner");

    // Thirty posts a minute, then 429 with Retry-After.
    let mut limited = None;
    for _ in 0..30 {
        let response = post_webhook(&instance, &reset, r#"{"content":"spam"}"#, false).await;
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            limited = Some(response);
            break;
        }
    }
    let limited = limited.expect("the 31st post a minute is turned away");
    assert!(limited.headers().contains_key("retry-after"));

    // The audit log has all of it, never the token.
    let log =
        audit_log(&mut c, &owner, pb::ListAuditLogRequest { server_id: server.id.clone(), ..Default::default() }).await;
    let actions: Vec<_> = log.entries.iter().map(|e| e.action()).collect();
    assert!(actions.contains(&pb::AuditAction::WebhookCreate));
    assert!(actions.contains(&pb::AuditAction::WebhookUpdate));
    assert!(log.entries.iter().all(|e| e.changes.iter().all(|ch| !ch.after.contains(&reset.token))));

    // Deleting its channel takes the webhook and its picture with it.
    c.channels
        .delete_channel(authed(
            &owner,
            pb::DeleteChannelRequest { server_id: server.id.clone(), channel_id: news.id.clone() },
        ))
        .await
        .unwrap();
    assert_eq!(
        post_webhook(&instance, &reset, r#"{"content":"gone"}"#, false).await.status(),
        reqwest::StatusCode::NOT_FOUND
    );
    assert_eq!(fetch(&instance, &picture).await.0, reqwest::StatusCode::NOT_FOUND);
    instance.stop().await;
}

#[tokio::test]
async fn agents_are_made_by_people_and_added_by_managers() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (owner, owner_user, _) = sign_up(&mut c, "owner").await;
    let (other, _, _) = sign_up(&mut c, "other").await;
    let server = create_server(&mut c, &owner, "Bots", true).await;
    let elsewhere = create_server(&mut c, &other, "Elsewhere", true).await;
    set_form(&mut c, &owner, &server.id, &["Be kind"], &[]).await.unwrap();
    let general = c
        .channels
        .list_channels(authed(&owner, pb::ListChannelsRequest { server_id: server.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels
        .into_iter()
        .find(|ch| ch.name == "general")
        .unwrap();

    let create = |username: &str| pb::CreateAgentRequest { username: username.into(), display_name: "Helper".into() };
    assert_eq!(
        c.agents.create_agent(authed(&owner, create("owner"))).await.unwrap_err().code(),
        Code::AlreadyExists,
        "agents share usernames with people"
    );
    let made = c.agents.create_agent(authed(&owner, create("helper"))).await.unwrap().into_inner();
    let agent = made.agent.unwrap();
    let token = made.token;
    let agent_id = agent.user.as_ref().unwrap().id.clone();
    assert_eq!(agent.user.as_ref().unwrap().kind, pb::AccountKind::Agent as i32);
    assert_eq!(agent.owner_id, owner_user.id);
    assert!(!agent.public);
    assert!(agent.last_active_at.is_none());

    // The token signs the agent in; it has no password and no devices.
    let me_agent = me(&mut c, &token).await.unwrap();
    assert_eq!(me_agent.username, "helper");
    assert_eq!(sign_in(&mut c, "helper", "whatever123").await.unwrap_err().code(), Code::Unauthenticated);
    let listed = c.agents.list_agents(authed(&owner, pb::ListAgentsRequest {})).await.unwrap().into_inner().agents;
    assert_eq!(listed.len(), 1);
    assert!(listed[0].last_active_at.is_some(), "using the token shows");
    assert!(
        c.agents.list_agents(authed(&other, pb::ListAgentsRequest {})).await.unwrap().into_inner().agents.is_empty()
    );
    assert_eq!(
        c.agents.list_agents(authed(&token, pb::ListAgentsRequest {})).await.unwrap_err().code(),
        Code::PermissionDenied
    );

    // Agents don't make servers or come in by themselves.
    assert_eq!(
        c.servers
            .create_server(authed(&token, pb::CreateServerRequest { name: "Mine".into(), ..Default::default() }))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );
    assert_eq!(
        c.servers
            .join_server(authed(&token, pb::JoinServerRequest { server_id: server.id.clone(), ..Default::default() }))
            .await
            .unwrap_err()
            .code(),
        Code::FailedPrecondition
    );

    // Someone else can't touch it.
    let update = |public: Option<bool>| pb::UpdateAgentRequest {
        agent_id: agent_id.clone(),
        display_name: Some("Helper Bot".into()),
        bio: Some("I **help**.".into()),
        public,
        ..Default::default()
    };
    assert_eq!(c.agents.update_agent(authed(&other, update(None))).await.unwrap_err().code(), Code::NotFound);
    let picture = upload(&mut c, &instance, &owner, pb::MediaPurpose::Avatar, png(300, 3)).await;
    let updated = c
        .agents
        .update_agent(authed(&owner, pb::UpdateAgentRequest { avatar_url: Some(picture.clone()), ..update(None) }))
        .await
        .unwrap()
        .into_inner()
        .agent
        .unwrap();
    assert_eq!(updated.user.as_ref().unwrap().display_name, "Helper Bot");
    assert_eq!(updated.user.as_ref().unwrap().avatar_url, picture);
    assert_eq!(updated.bio, "I **help**.");

    // A private agent: only its owner adds it, and only where they manage.
    let add = |server_id: &str| pb::AddAgentRequest { server_id: server_id.into(), username: "Helper".into() };
    assert_eq!(
        c.agents.add_agent(authed(&other, add(&elsewhere.id))).await.unwrap_err().code(),
        Code::PermissionDenied
    );
    assert_eq!(
        c.agents.add_agent(authed(&owner, add(&elsewhere.id))).await.unwrap_err().code(),
        Code::PermissionDenied
    );
    let member = c.agents.add_agent(authed(&owner, add(&server.id))).await.unwrap().into_inner().member.unwrap();
    assert!(!member.pending, "agents skip the rules");
    assert_eq!(member.user.as_ref().unwrap().kind, pb::AccountKind::Agent as i32);
    assert_eq!(c.agents.add_agent(authed(&owner, add(&server.id))).await.unwrap_err().code(), Code::AlreadyExists);
    let log =
        audit_log(&mut c, &owner, pb::ListAuditLogRequest { server_id: server.id.clone(), ..Default::default() }).await;
    assert!(log.entries.iter().any(|e| e.action == pb::AuditAction::AgentAdd as i32 && e.target_id == agent_id));

    // Public: anyone managing a server may add it.
    c.agents.update_agent(authed(&owner, update(Some(true)))).await.unwrap();
    c.agents.add_agent(authed(&other, add(&elsewhere.id))).await.unwrap();
    let listed = c.agents.list_agents(authed(&owner, pb::ListAgentsRequest {})).await.unwrap().into_inner().agents;
    assert_eq!(listed[0].servers, 2);

    // In a server, it talks like anyone.
    let sent = send(&mut c, &token, &server.id, &general.id, "beep boop").await.unwrap();
    assert_eq!(sent.author_id, agent_id);

    // It can't use direct messages.
    let mut dms = pb::direct_message_service_client::DirectMessageServiceClient::new(instance.channel().await);
    assert_eq!(
        dms.open_conversation(authed(&owner, pb::OpenConversationRequest { user_id: agent_id.clone() }))
            .await
            .unwrap_err()
            .code(),
        Code::FailedPrecondition
    );

    // A new token: the old one stops working at once.
    let fresh = c
        .agents
        .reset_agent_token(authed(&owner, pb::ResetAgentTokenRequest { agent_id: agent_id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .token;
    assert_eq!(me(&mut c, &token).await.unwrap_err(), Code::Unauthenticated);
    assert_eq!(me(&mut c, &fresh).await.unwrap().id, agent_id);

    // Admins can't make it an admin.
    assert_eq!(
        update_account(
            &mut c,
            &owner,
            pb::UpdateAccountRequest { account_id: agent_id.clone(), admin: Some(true), ..Default::default() }
        )
        .await
        .unwrap_err()
        .code(),
        Code::FailedPrecondition
    );

    // Deleting it takes it out of every server; what it said stays.
    c.agents.delete_agent(authed(&owner, pb::DeleteAgentRequest { agent_id: agent_id.clone() })).await.unwrap();
    assert_eq!(me(&mut c, &fresh).await.unwrap_err(), Code::Unauthenticated);
    let members = c
        .servers
        .list_members(authed(&owner, pb::ListMembersRequest { server_id: server.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .members;
    assert!(members.iter().all(|m| m.user.as_ref().unwrap().id != agent_id));
    assert!(messages(&mut c, &owner, &server.id, &general.id).await.iter().any(|m| m.id == sent.id));
    assert_eq!(fetch(&instance, &picture).await.0, reqwest::StatusCode::NOT_FOUND, "its picture goes too");
    assert!(
        c.agents.list_agents(authed(&owner, pb::ListAgentsRequest {})).await.unwrap().into_inner().agents.is_empty()
    );

    // Admins can stop new ones; and a person's agents go when they do.
    let settings = c.admin.get_settings(authed(&owner, pb::GetSettingsRequest {})).await.unwrap().into_inner();
    let mut changed = settings.config.unwrap().settings.unwrap();
    changed.agent_creation = pb::AgentCreation::Admins as i32;
    c.admin.update_settings(authed(&owner, settings_update(changed, &["agent_creation"], &[]))).await.unwrap();
    assert_eq!(c.agents.create_agent(authed(&other, create("nope"))).await.unwrap_err().code(), Code::PermissionDenied);
    let second = c.agents.create_agent(authed(&owner, create("second"))).await.unwrap().into_inner();
    let node = c.node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap();
    assert_eq!(node.agent_creation, pb::AgentCreation::Admins as i32);
    // Someone else stays admin, so the owner can leave.
    let other_id = me(&mut c, &other).await.unwrap().id;
    let make_admin = pb::UpdateAccountRequest { account_id: other_id, admin: Some(true), ..Default::default() };
    update_account(&mut c, &owner, make_admin).await.unwrap();
    // They own a server; it goes first.
    c.servers.delete_server(authed(&owner, pb::DeleteServerRequest { server_id: server.id.clone() })).await.unwrap();
    c.account
        .delete_account(authed(
            &owner,
            pb::DeleteAccountRequest { password: "correct horse battery".into(), ..Default::default() },
        ))
        .await
        .unwrap_or_else(|err| panic!("{err:?}"));
    assert_eq!(me(&mut c, &second.token).await.unwrap_err(), Code::Unauthenticated);
    instance.stop().await;
}

#[tokio::test]
async fn agents_stop_when_their_owner_is_turned_off() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (admin, _, _) = sign_up(&mut c, "admin").await;
    let (owner, owner_user, _) = sign_up(&mut c, "owner").await;
    let made = c
        .agents
        .create_agent(authed(
            &owner,
            pb::CreateAgentRequest { username: "helper".into(), display_name: "Helper".into() },
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(me(&mut c, &made.token).await.is_ok());

    let off =
        pb::UpdateAccountRequest { account_id: owner_user.id.clone(), disabled: Some(true), ..Default::default() };
    update_account(&mut c, &admin, off).await.unwrap();
    assert_eq!(me(&mut c, &owner).await.unwrap_err(), Code::Unauthenticated);
    assert_eq!(me(&mut c, &made.token).await.unwrap_err(), Code::Unauthenticated, "its agents stop with it");

    // Turned back on, the owner hands out a new token; the old one stays dead.
    let on =
        pb::UpdateAccountRequest { account_id: owner_user.id.clone(), disabled: Some(false), ..Default::default() };
    update_account(&mut c, &admin, on).await.unwrap();
    assert_eq!(me(&mut c, &made.token).await.unwrap_err(), Code::Unauthenticated);
    let owner = sign_in(&mut c, "owner", "correct horse battery").await.unwrap().token;
    let agent_id = made.agent.unwrap().user.unwrap().id;
    let fresh = c
        .agents
        .reset_agent_token(authed(&owner, pb::ResetAgentTokenRequest { agent_id }))
        .await
        .unwrap()
        .into_inner()
        .token;
    assert!(me(&mut c, &fresh).await.is_ok());

    instance.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn picture_uploads_take_the_caps_admins_set() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (juan, _, _) = sign_up(&mut c, "juan").await;
    let (mika, _, _) = sign_up(&mut c, "mika").await;
    let avatar = pb::MediaPurpose::Avatar;

    // Unlimited until an admin sets them.
    let settings = c.admin.get_settings(authed(&juan, pb::GetSettingsRequest {})).await.unwrap().into_inner();
    let defaults = settings.config.unwrap().settings.unwrap();
    assert_eq!(defaults.picture_upload_bytes, None);
    assert_eq!(defaults.picture_upload_bytes_per_day, None);
    create_upload(&mut c, &juan, avatar, "image/png", 8 * 1024 * 1024 + 1).await.unwrap();

    // A size for each picture, and so many bytes a day for each account,
    // however many ask at once.
    let mut changed = defaults.clone();
    changed.picture_upload_bytes = Some(8 * 1024 * 1024);
    changed.picture_upload_bytes_per_day = Some(8 * 1024 * 1024 + 1 + 2000);
    c.admin
        .update_settings(authed(
            &juan,
            settings_update(changed, &["picture_upload_bytes", "picture_upload_bytes_per_day"], &[]),
        ))
        .await
        .unwrap();
    let too_big = create_upload(&mut c, &juan, avatar, "image/png", 8 * 1024 * 1024 + 1).await.unwrap_err();
    assert_eq!(too_big.code(), Code::ResourceExhausted);
    let reservations = (0..8).map(|_| {
        let mut media = c.media.clone();
        let juan = juan.clone();
        tokio::spawn(async move {
            let request =
                pb::CreateUploadRequest { purpose: avatar as i32, content_type: "image/png".into(), size: 500 };
            media.create_upload(authed(&juan, request)).await.map_err(|err| err.code())
        })
    });
    let results: Vec<_> = futures::future::join_all(reservations).await.into_iter().map(|r| r.unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 4, "{results:?}");
    assert!(results.iter().filter_map(|r| r.as_ref().err()).all(|code| *code == Code::ResourceExhausted));
    assert!(create_upload(&mut c, &mika, avatar, "image/png", 2000).await.is_ok(), "each account has its own");

    // Unset, there's no cap.
    let mut unlimited = defaults.clone();
    unlimited.picture_upload_bytes_per_day = None;
    c.admin
        .update_settings(authed(&juan, settings_update(unlimited, &["picture_upload_bytes_per_day"], &[])))
        .await
        .unwrap();
    assert!(create_upload(&mut c, &juan, avatar, "image/png", 500).await.is_ok());

    instance.stop().await;
}

#[tokio::test]
async fn webhooks_stay_out_of_channels_their_managers_cannot_see() {
    use pb::Permission as P;
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (owner, _, _) = sign_up(&mut c, "owner").await;
    let (mika, mika_user, _) = sign_up(&mut c, "mika").await;
    let server = create_server(&mut c, &owner, "Hooks", true).await;
    let sid = server.id.clone();
    join(&mut c, &mika, &sid).await;
    let hooks = create_role(&mut c, &owner, &sid, "Hooks", &[P::ManageWebhooks]).await.unwrap();
    give_role(&mut c, &owner, &sid, &mika_user.id, &hooks.id).await.unwrap();
    let open = new_channel(&mut c, &owner, &sid, "open", pb::ChannelType::Text).await;
    let secret = new_channel(&mut c, &owner, &sid, "secret", pb::ChannelType::Text).await;
    set_permissions(
        &mut c,
        &owner,
        &sid,
        &secret.id,
        vec![overwrite(&sid, pb::OverwriteTarget::Role, &[], &[P::ViewChannels])],
    )
    .await
    .unwrap();
    let create = |token: &str, channel_id: &str| {
        authed(
            token,
            pb::CreateWebhookRequest {
                server_id: sid.clone(),
                channel_id: channel_id.into(),
                name: "Hook".into(),
                ..Default::default()
            },
        )
    };
    let hidden = c.webhooks.create_webhook(create(&owner, &secret.id)).await.unwrap().into_inner().webhook.unwrap();

    // Someone managing webhooks who can't see the channel can't find it there…
    let listed = c
        .webhooks
        .list_webhooks(authed(&mika, pb::ListWebhooksRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner()
        .webhooks;
    assert!(listed.is_empty(), "{listed:?}");
    let refused = c.webhooks.create_webhook(create(&mika, &secret.id)).await.unwrap_err();
    assert_eq!(refused.code(), Code::NotFound);
    let token = c
        .webhooks
        .reset_webhook_token(authed(
            &mika,
            pb::ResetWebhookTokenRequest { server_id: sid.clone(), webhook_id: hidden.id.clone() },
        ))
        .await;
    assert_eq!(token.unwrap_err().code(), Code::NotFound);
    let deleted = c
        .webhooks
        .delete_webhook(authed(
            &mika,
            pb::DeleteWebhookRequest { server_id: sid.clone(), webhook_id: hidden.id.clone() },
        ))
        .await;
    assert_eq!(deleted.unwrap_err().code(), Code::NotFound);

    // …nor move one of theirs into it.
    let theirs = c.webhooks.create_webhook(create(&mika, &open.id)).await.unwrap().into_inner().webhook.unwrap();
    let moved = c
        .webhooks
        .update_webhook(authed(
            &mika,
            pb::UpdateWebhookRequest {
                server_id: sid.clone(),
                webhook_id: theirs.id.clone(),
                channel_id: secret.id.clone(),
                name: "Hook".into(),
                ..Default::default()
            },
        ))
        .await;
    assert_eq!(moved.unwrap_err().code(), Code::NotFound);
    let all = c
        .webhooks
        .list_webhooks(authed(&owner, pb::ListWebhooksRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner()
        .webhooks;
    assert_eq!(all.len(), 2, "the owner sees both");

    instance.stop().await;
}

#[tokio::test]
async fn timed_out_members_only_read() {
    use pb::Permission as P;
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (owner, _, _) = sign_up(&mut c, "owner").await;
    let (moderator, mod_user, _) = sign_up(&mut c, "moderator").await;
    let (aoi, aoi_user, _) = sign_up(&mut c, "aoi").await;
    let server = create_server(&mut c, &owner, "Quiet", true).await;
    let sid = server.id.clone();
    join(&mut c, &moderator, &sid).await;
    join(&mut c, &aoi, &sid).await;
    let mods = create_role(&mut c, &owner, &sid, "Mods", &[P::KickMembers, P::ManageMessages, P::ManageChannels])
        .await
        .unwrap();
    give_role(&mut c, &owner, &sid, &mod_user.id, &mods.id).await.unwrap();
    let general = c
        .channels
        .list_channels(authed(&owner, pb::ListChannelsRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels
        .remove(0);
    let theirs = send(&mut c, &aoi, &sid, &general.id, "hi").await.unwrap();
    let own = send(&mut c, &moderator, &sid, &general.id, "hello").await.unwrap();
    let time_out = |seconds: i64| {
        authed(
            &owner,
            pb::TimeOutMemberRequest {
                server_id: sid.clone(),
                user_id: mod_user.id.clone(),
                seconds,
                reason: "".into(),
            },
        )
    };
    c.servers.time_out_member(time_out(600)).await.unwrap();

    // A time-out takes away moderating as well as talking.
    let kick = c
        .servers
        .kick_member(authed(
            &moderator,
            pb::KickMemberRequest { server_id: sid.clone(), user_id: aoi_user.id.clone(), reason: "".into() },
        ))
        .await;
    assert_eq!(kick.unwrap_err().code(), Code::PermissionDenied);
    let delete = |token: &str, message_id: &str| {
        authed(
            token,
            pb::DeleteMessageRequest {
                channel_id: String::new(),
                server_id: sid.clone(),
                message_id: message_id.into(),
            },
        )
    };
    assert_eq!(
        c.messages.delete_message(delete(&moderator, &theirs.id)).await.unwrap_err().code(),
        Code::PermissionDenied
    );
    assert_eq!(
        c.messages.delete_message(delete(&moderator, &own.id)).await.unwrap_err().code(),
        Code::PermissionDenied
    );
    let channel = c
        .channels
        .create_channel(authed(
            &moderator,
            pb::CreateChannelRequest { server_id: sid.clone(), name: "mine".into(), ..Default::default() },
        ))
        .await;
    assert_eq!(channel.unwrap_err().code(), Code::PermissionDenied);
    let nickname = c
        .servers
        .update_member(authed(
            &moderator,
            pb::UpdateMemberRequest {
                server_id: sid.clone(),
                user_id: mod_user.id.clone(),
                nickname: Some("loud".into()),
            },
        ))
        .await;
    assert_eq!(nickname.unwrap_err().code(), Code::PermissionDenied);
    // Reading still works.
    assert!(messages(&mut c, &moderator, &sid, &general.id).await.iter().any(|m| m.id == theirs.id));

    // Once it ends, everything comes back.
    c.servers.time_out_member(time_out(0)).await.unwrap();
    c.messages.delete_message(delete(&moderator, &theirs.id)).await.unwrap();
    c.messages.delete_message(delete(&moderator, &own.id)).await.unwrap();

    // Timed out again, they can still leave.
    c.servers.time_out_member(time_out(600)).await.unwrap();
    c.servers.leave_server(authed(&moderator, pb::LeaveServerRequest { server_id: sid.clone() })).await.unwrap();

    instance.stop().await;
}

#[tokio::test]
async fn servers_are_never_handed_to_agents() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (owner, _, _) = sign_up(&mut c, "owner").await;
    let server = create_server(&mut c, &owner, "Mine", false).await;
    let made = c
        .agents
        .create_agent(authed(
            &owner,
            pb::CreateAgentRequest { username: "helper".into(), display_name: "Helper".into() },
        ))
        .await
        .unwrap()
        .into_inner();
    let agent_id = made.agent.unwrap().user.unwrap().id;
    c.agents
        .add_agent(authed(&owner, pb::AddAgentRequest { server_id: server.id.clone(), username: "helper".into() }))
        .await
        .unwrap();
    let handed = c
        .servers
        .transfer_ownership(authed(
            &owner,
            pb::TransferOwnershipRequest { server_id: server.id.clone(), user_id: agent_id },
        ))
        .await;
    assert_eq!(handed.unwrap_err().code(), Code::FailedPrecondition);
    instance.stop().await;
}

#[tokio::test]
async fn channel_overwrites_and_moves_respect_rank_and_reach() {
    use pb::Permission as P;
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (owner, _, _) = sign_up(&mut c, "owner").await;
    let (moderator, mod_user, _) = sign_up(&mut c, "moderator").await;
    let (boss, boss_user, _) = sign_up(&mut c, "boss").await;
    let (helper, helper_user, _) = sign_up(&mut c, "helper").await;
    let server = create_server(&mut c, &owner, "Ranks", true).await;
    let sid = server.id.clone();
    for token in [&moderator, &boss, &helper] {
        join(&mut c, token, &sid).await;
    }
    let high = create_role(&mut c, &owner, &sid, "High", &[]).await.unwrap();
    // New roles land right above @everyone, so Mods ends up below High, and Low below Mods.
    let mods = create_role(&mut c, &owner, &sid, "Mods", &[P::ManageRoles]).await.unwrap();
    let low = create_role(&mut c, &owner, &sid, "Low", &[]).await.unwrap();
    give_role(&mut c, &owner, &sid, &mod_user.id, &mods.id).await.unwrap();
    give_role(&mut c, &owner, &sid, &boss_user.id, &high.id).await.unwrap();
    let channel = new_channel(&mut c, &owner, &sid, "ranked", pb::ChannelType::Text).await;
    let mute =
        |target_id: &str, target: pb::OverwriteTarget| vec![overwrite(target_id, target, &[], &[P::SendMessages])];

    // Overwrites change only for what ranks below the caller.
    let above = set_permissions(&mut c, &moderator, &sid, &channel.id, mute(&high.id, pb::OverwriteTarget::Role)).await;
    assert_eq!(above.unwrap_err(), Code::PermissionDenied);
    let over_boss =
        set_permissions(&mut c, &moderator, &sid, &channel.id, mute(&boss_user.id, pb::OverwriteTarget::Member)).await;
    assert_eq!(over_boss.unwrap_err(), Code::PermissionDenied);
    set_permissions(&mut c, &moderator, &sid, &channel.id, mute(&low.id, pb::OverwriteTarget::Role)).await.unwrap();
    set_permissions(&mut c, &moderator, &sid, &channel.id, mute(&sid, pb::OverwriteTarget::Role)).await.unwrap();
    // One the owner set above them stays out of their reach, even to remove.
    set_permissions(&mut c, &owner, &sid, &channel.id, mute(&high.id, pb::OverwriteTarget::Role)).await.unwrap();
    assert_eq!(
        set_permissions(&mut c, &moderator, &sid, &channel.id, vec![]).await.unwrap_err(),
        Code::PermissionDenied
    );

    // Managing one channel doesn't reach moving it out of its category.
    let category = c
        .channels
        .create_channel(authed(
            &owner,
            pb::CreateChannelRequest {
                server_id: sid.clone(),
                name: "Things".into(),
                r#type: pb::ChannelType::Category as i32,
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .channel
        .unwrap();
    let inside = c
        .channels
        .create_channel(authed(
            &owner,
            pb::CreateChannelRequest {
                server_id: sid.clone(),
                name: "inside".into(),
                r#type: pb::ChannelType::Text as i32,
                parent_id: category.id.clone(),
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .channel
        .unwrap();
    let manage_here = vec![overwrite(&helper_user.id, pb::OverwriteTarget::Member, &[P::ManageChannels], &[])];
    set_permissions(&mut c, &owner, &sid, &inside.id, manage_here).await.unwrap();
    let update = |parent_id: Option<&str>, name: &str| {
        authed(
            &helper,
            pb::UpdateChannelRequest {
                server_id: sid.clone(),
                channel_id: inside.id.clone(),
                name: Some(name.into()),
                parent_id: parent_id.map(str::to_string),
                ..Default::default()
            },
        )
    };
    let out = c.channels.update_channel(update(Some(""), "inside")).await;
    assert_eq!(out.unwrap_err().code(), Code::PermissionDenied);
    let renamed = c.channels.update_channel(update(None, "renamed")).await.unwrap().into_inner();
    assert_eq!(renamed.channel.unwrap().parent_id, category.id, "the rest is theirs to change");

    instance.stop().await;
}

#[tokio::test]
async fn https_instances_ask_browsers_to_keep_to_https() {
    let dir = tempfile::tempdir().unwrap();
    let plain = start(dir.path(), &[]).await;
    let home = reqwest::get(format!("http://{}/", plain.addr)).await.unwrap();
    assert!(home.headers().get("strict-transport-security").is_none());
    plain.stop().await;

    let dir = tempfile::tempdir().unwrap();
    let https = start(dir.path(), &[("FUWA_PUBLIC_URL", "https://chat.example.com")]).await;
    let home = reqwest::get(format!("http://{}/", https.addr)).await.unwrap();
    assert_eq!(home.headers()["strict-transport-security"], "max-age=63072000");
    https.stop().await;
}

#[tokio::test]
async fn apps_send_reports_through_their_instance() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let report = || pb::SendReportRequest {
        report: Some(pb::AppReport {
            app: "web".into(),
            version: "0.1.0".into(),
            platform: "firefox".into(),
            os: "linux".into(),
            errors: vec![pb::ReportError { kind: "TypeError".into(), place: "chat".into(), count: 1 }],
            timings: vec![pb::ReportTiming { metric: "startup".into(), buckets: vec![0; 13], sum_ms: 0 }],
            usage: vec![pb::ReportUsage { feature: "message.send".into(), count: 4 }],
        }),
    };

    // Only signed-in apps, so each account can be held to one a minute.
    let refused = c.node.send_report(report()).await.unwrap_err();
    assert_eq!(refused.code(), tonic::Code::Unauthenticated);

    let (token, _, _) = sign_up(&mut c, "reporter").await;
    let mut bad = report();
    bad.report.as_mut().unwrap().errors[0].place = "a message body with spaces".into();
    assert_eq!(c.node.send_report(authed(&token, bad)).await.unwrap_err().code(), tonic::Code::InvalidArgument);

    // Telemetry is off here: taken, then dropped.
    c.node.send_report(authed(&token, report())).await.unwrap();
    let again = c.node.send_report(authed(&token, report())).await.unwrap_err();
    assert_eq!(again.code(), tonic::Code::ResourceExhausted);
    instance.stop().await;
}

async fn list_channels(c: &mut Clients, token: &str, server_id: &str) -> Vec<pb::Channel> {
    c.channels
        .list_channels(authed(token, pb::ListChannelsRequest { server_id: server_id.into() }))
        .await
        .unwrap()
        .into_inner()
        .channels
}

async fn connections(c: &mut Clients, token: &str, server_id: &str) -> pb::ListConnectionsResponse {
    c.shared
        .list_connections(authed(token, pb::ListConnectionsRequest { server_id: server_id.into() }))
        .await
        .unwrap()
        .into_inner()
}

/// Shares `channel_id` from `home` into `guest`: a code, the guest's ask, the
/// home's approval. Returns the guest's channel.
async fn share(
    c: &mut Clients,
    home_token: &str,
    home: &str,
    channel_id: &str,
    guest_token: &str,
    guest: &str,
) -> pb::Channel {
    let code = c
        .shared
        .create_share_code(authed(
            home_token,
            pb::CreateShareCodeRequest { server_id: home.into(), channel_id: channel_id.into() },
        ))
        .await
        .unwrap()
        .into_inner()
        .code
        .unwrap();
    let asked = c
        .shared
        .accept_share(authed(
            guest_token,
            pb::AcceptShareRequest { server_id: guest.into(), code: code.code, ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .connection
        .unwrap();
    c.shared
        .review_share(authed(
            home_token,
            pb::ReviewShareRequest { server_id: home.into(), connection_id: asked.id, approve: true },
        ))
        .await
        .unwrap();
    list_channels(c, guest_token, guest)
        .await
        .into_iter()
        .find(|ch| ch.shared.as_ref().is_some_and(|s| !s.home))
        .unwrap()
}

/// The next message created on a stream, skipping other events.
async fn next_message(stream: &mut tonic::Streaming<pb::SubscribeResponse>) -> pb::Message {
    loop {
        if let Some(pb::event::Payload::MessageCreated(created)) = next_event(stream).await.payload {
            return created.message.unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn channels_shared_between_servers() {
    use pb::Permission as P;
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (juan, juan_user, _) = sign_up(&mut c, "juan").await;
    let (mika, _, _) = sign_up(&mut c, "mika").await;
    let (rin, rin_user, _) = sign_up(&mut c, "rin").await;
    let (sora, sora_user, _) = sign_up(&mut c, "sora").await;
    let home = create_server(&mut c, &juan, "Home", true).await.id;
    let guest = create_server(&mut c, &mika, "Guest", true).await.id;
    join(&mut c, &rin, &guest).await;
    join(&mut c, &sora, &home).await;
    let dev = new_channel(&mut c, &juan, &home, "dev", pb::ChannelType::Text).await;

    // Only people who manage the server make codes, and only for text channels.
    let denied = c
        .shared
        .create_share_code(authed(
            &sora,
            pb::CreateShareCodeRequest { server_id: home.clone(), channel_id: dev.id.clone() },
        ))
        .await
        .unwrap_err();
    assert_eq!(denied.code(), Code::PermissionDenied);
    let voice = new_channel(&mut c, &juan, &home, "Lounge", pb::ChannelType::Voice).await;
    let refused = c
        .shared
        .create_share_code(authed(&juan, pb::CreateShareCodeRequest { server_id: home.clone(), channel_id: voice.id }))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Code::InvalidArgument);

    // The guest's admin sees where the messages live before asking.
    let code = c
        .shared
        .create_share_code(authed(
            &juan,
            pb::CreateShareCodeRequest { server_id: home.clone(), channel_id: dev.id.clone() },
        ))
        .await
        .unwrap()
        .into_inner()
        .code
        .unwrap();
    assert!(code.code.starts_with(&home));
    let own = c
        .shared
        .preview_share(authed(&juan, pb::PreviewShareRequest { server_id: home.clone(), code: code.code.clone() }))
        .await
        .unwrap_err();
    assert_eq!(own.code(), Code::InvalidArgument);
    let preview = c
        .shared
        .preview_share(authed(&mika, pb::PreviewShareRequest { server_id: guest.clone(), code: code.code.clone() }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(preview.home_server.unwrap().name, "Home");
    assert_eq!(preview.channel_name, "dev");
    assert_eq!(preview.guest_count, 0);
    let asked = c
        .shared
        .accept_share(authed(
            &mika,
            pb::AcceptShareRequest {
                server_id: guest.clone(),
                code: code.code.clone(),
                name: "Partner Dev".into(),
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .connection
        .unwrap();
    assert_eq!(asked.state, pb::SharedConnectionState::Waiting as i32);
    // Each code lets one server ask.
    let again = c
        .shared
        .preview_share(authed(&mika, pb::PreviewShareRequest { server_id: guest.clone(), code: code.code }))
        .await
        .unwrap_err();
    assert_eq!(again.code(), Code::NotFound);
    // Nothing shows until the home approves.
    assert!(list_channels(&mut c, &rin, &guest).await.iter().all(|ch| ch.shared.is_none()));
    let waiting = connections(&mut c, &juan, &home).await.connections;
    assert_eq!(waiting.len(), 1);
    assert!(waiting[0].home);
    assert_eq!(waiting[0].server.as_ref().unwrap().name, "Guest");
    c.shared
        .review_share(authed(
            &juan,
            pb::ReviewShareRequest { server_id: home.clone(), connection_id: waiting[0].id.clone(), approve: true },
        ))
        .await
        .unwrap();

    // Both sides see it's shared, and where it lives.
    let shown = list_channels(&mut c, &rin, &guest).await.into_iter().find(|ch| ch.shared.is_some()).unwrap();
    assert_eq!(shown.name, "partner-dev");
    let link = shown.shared.clone().unwrap();
    assert!(!link.home);
    assert_eq!(link.home_server.unwrap().name, "Home");
    assert_eq!(link.home_channel_name, "dev");
    let at_home = list_channels(&mut c, &sora, &home).await.into_iter().find(|ch| ch.id == dev.id).unwrap();
    assert_eq!(at_home.shared.unwrap().guests[0].name, "Guest");

    // What's said at home reaches the guest's people live.
    let mut stream = c
        .events
        .subscribe(authed(
            &rin,
            pb::SubscribeRequest { servers: vec![pb::ServerCursor { server_id: guest.clone(), after_sequence: None }] },
        ))
        .await
        .unwrap()
        .into_inner();
    send(&mut c, &juan, &home, &dev.id, "@everyone hello from home").await.unwrap();
    let live = next_message(&mut stream).await;
    assert_eq!(live.content, "@everyone hello from home");
    assert!(!live.mentions_everyone, "the home's @everyone doesn't ping the guest's people");
    assert_eq!(live.channel_id, shown.id);
    assert_eq!(live.server_id, guest);
    let author = live.shared.unwrap();
    assert_eq!(author.user.unwrap().id, juan_user.id);
    assert_eq!(author.server.unwrap().name, "Home");

    // A guest writes; the message lives only at home.
    let kept_before = usage(&mut c, &mika, &guest).await.messages;
    let hi = send(&mut c, &rin, &guest, &shown.id, "hi from guest @everyone").await.unwrap();
    assert_eq!(hi.channel_id, shown.id);
    let home_side = messages(&mut c, &juan, &home, &dev.id).await;
    let stored = home_side.iter().find(|m| m.id == hi.id).unwrap();
    assert!(!stored.mentions_everyone, "pings never cross servers");
    assert_eq!(stored.shared.as_ref().unwrap().server.as_ref().unwrap().name, "Guest");
    let from_home = home_side.iter().find(|m| m.content == "@everyone hello from home").unwrap();
    assert!(from_home.shared.is_none() && from_home.mentions_everyone, "it pings the home's own people");
    let guest_side = messages(&mut c, &rin, &guest, &shown.id).await;
    assert_eq!(
        guest_side.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(),
        vec!["@everyone hello from home", "hi from guest @everyone"]
    );
    assert!(guest_side.iter().all(|m| m.channel_id == shown.id && m.shared.is_some() && !m.mentions_everyone));
    assert_eq!(usage(&mut c, &mika, &guest).await.messages, kept_before, "the guest keeps nothing");

    // Editing works with or without the channel named; others' messages aren't theirs to touch.
    let edited = c
        .messages
        .update_message(authed(
            &rin,
            pb::UpdateMessageRequest {
                server_id: guest.clone(),
                message_id: hi.id.clone(),
                content: "hi from guest".into(),
                channel_id: String::new(),
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .message
        .unwrap();
    assert_eq!(edited.content, "hi from guest");
    let theirs = guest_side.iter().find(|m| m.content == "@everyone hello from home").unwrap().id.clone();
    let not_mine = c
        .messages
        .delete_message(authed(
            &rin,
            pb::DeleteMessageRequest { server_id: guest.clone(), message_id: theirs, channel_id: shown.id.clone() },
        ))
        .await
        .unwrap_err();
    assert_eq!(not_mine.code(), Code::PermissionDenied);

    // The home keeps a guest out; its own members are kicked or banned instead.
    let member = c
        .shared
        .block_from_channel(authed(
            &juan,
            pb::BlockFromChannelRequest {
                server_id: home.clone(),
                channel_id: dev.id.clone(),
                user_id: sora_user.id.clone(),
                blocked: true,
            },
        ))
        .await
        .unwrap_err();
    assert_eq!(member.code(), Code::InvalidArgument);
    c.shared
        .block_from_channel(authed(
            &juan,
            pb::BlockFromChannelRequest {
                server_id: home.clone(),
                channel_id: dev.id.clone(),
                user_id: rin_user.id.clone(),
                blocked: true,
            },
        ))
        .await
        .unwrap();
    let kept_out = send(&mut c, &rin, &guest, &shown.id, "let me in").await.unwrap_err();
    assert_eq!(kept_out.code(), Code::PermissionDenied);
    assert_eq!(connections(&mut c, &juan, &home).await.blocks.len(), 1);
    c.shared
        .block_from_channel(authed(
            &juan,
            pb::BlockFromChannelRequest {
                server_id: home.clone(),
                channel_id: dev.id.clone(),
                user_id: rin_user.id.clone(),
                blocked: false,
            },
        ))
        .await
        .unwrap();

    // Each side's AutoMod reads what the guest's people write. A home rule
    // that would time someone out keeps a guest out of the channel instead.
    let rule = |word: &str, actions: Vec<pb::AutoModAction>| pb::AutoModRule {
        enabled: true,
        trigger: pb::AutoModTrigger::Keywords as i32,
        keywords: vec![word.into()],
        actions,
        ..Default::default()
    };
    let block = pb::AutoModAction { kind: pb::AutoModActionKind::Block as i32, ..Default::default() };
    let time_out =
        pb::AutoModAction { kind: pb::AutoModActionKind::TimeOut as i32, duration_seconds: 60, ..Default::default() };
    save_rule(&mut c, &mika, &guest, rule("nope", vec![block.clone()])).await.unwrap();
    let caught_here = send(&mut c, &rin, &guest, &shown.id, "nope").await.unwrap_err();
    assert_eq!(caught_here.code(), Code::PermissionDenied);
    save_rule(&mut c, &juan, &home, rule("forbidden", vec![block, time_out])).await.unwrap();
    let caught_there = send(&mut c, &rin, &guest, &shown.id, "forbidden").await.unwrap_err();
    assert!(caught_there.message().starts_with("AutoMod"), "{caught_there:?}");
    assert!(
        messages(&mut c, &juan, &home, &dev.id).await.iter().all(|m| m.content != "nope" && m.content != "forbidden")
    );
    let blocks = connections(&mut c, &juan, &home).await.blocks;
    assert_eq!(blocks[0].user.as_ref().unwrap().id, rin_user.id);
    assert_eq!(send(&mut c, &rin, &guest, &shown.id, "hello?").await.unwrap_err().code(), Code::PermissionDenied);
    c.shared
        .block_from_channel(authed(
            &juan,
            pb::BlockFromChannelRequest {
                server_id: home.clone(),
                channel_id: dev.id.clone(),
                user_id: rin_user.id.clone(),
                blocked: false,
            },
        ))
        .await
        .unwrap();
    send(&mut c, &rin, &guest, &shown.id, "back again").await.unwrap();

    // The home decides what guests may do, never more than sending.
    let connection_id = connections(&mut c, &juan, &home).await.connections[0].id.clone();
    let too_much = c
        .shared
        .update_connection(authed(
            &juan,
            pb::UpdateConnectionRequest {
                server_id: home.clone(),
                connection_id: connection_id.clone(),
                allowed: vec![P::ManageMessages as i32],
            },
        ))
        .await
        .unwrap_err();
    assert_eq!(too_much.code(), Code::InvalidArgument);
    c.shared
        .update_connection(authed(
            &juan,
            pb::UpdateConnectionRequest { server_id: home.clone(), connection_id, allowed: vec![] },
        ))
        .await
        .unwrap();
    let read_only = send(&mut c, &rin, &guest, &shown.id, "can I?").await.unwrap_err();
    assert_eq!(read_only.code(), Code::PermissionDenied);

    // Either side can end it; the messages stay at home.
    let guest_connection = connections(&mut c, &mika, &guest).await.connections[0].id.clone();
    c.shared
        .disconnect(authed(&mika, pb::DisconnectRequest { server_id: guest.clone(), connection_id: guest_connection }))
        .await
        .unwrap();
    assert!(list_channels(&mut c, &rin, &guest).await.iter().all(|ch| ch.id != shown.id));
    assert!(list_channels(&mut c, &juan, &home).await.iter().all(|ch| ch.shared.is_none()));
    assert!(connections(&mut c, &juan, &home).await.connections.is_empty());
    assert!(messages(&mut c, &juan, &home, &dev.id).await.iter().any(|m| m.content == "hi from guest"));

    // Deleting the channel at home takes it away from the guest too.
    let shown = share(&mut c, &juan, &home, &dev.id, &mika, &guest).await;
    c.channels
        .delete_channel(authed(&juan, pb::DeleteChannelRequest { server_id: home.clone(), channel_id: dev.id.clone() }))
        .await
        .unwrap();
    assert!(list_channels(&mut c, &rin, &guest).await.iter().all(|ch| ch.id != shown.id));
    assert!(connections(&mut c, &mika, &guest).await.connections.is_empty());

    // So does deleting the guest server, at home.
    let ops = new_channel(&mut c, &juan, &home, "ops", pb::ChannelType::Text).await;
    share(&mut c, &juan, &home, &ops.id, &mika, &guest).await;
    c.servers.delete_server(authed(&mika, pb::DeleteServerRequest { server_id: guest.clone() })).await.unwrap();
    assert!(connections(&mut c, &juan, &home).await.connections.is_empty());
    instance.stop().await;
}

#[tokio::test]
async fn shared_channels_can_be_turned_off() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[("FUWA_SHARED_CHANNELS", "off")]).await;
    let mut c = clients(&instance).await;
    let (juan, _, _) = sign_up(&mut c, "juan").await;
    let home = create_server(&mut c, &juan, "Home", false).await.id;
    let general = list_channels(&mut c, &juan, &home).await[0].id.clone();
    let off = c
        .shared
        .create_share_code(authed(&juan, pb::CreateShareCodeRequest { server_id: home, channel_id: general }))
        .await
        .unwrap_err();
    assert_eq!(off.code(), Code::FailedPrecondition);
    instance.stop().await;
}

#[tokio::test]
async fn shared_preview_names_outside_providers() {
    use pb::{AutoModActionKind as Kind, AutoModTrigger as Trigger};
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path(), &[]).await;
    let mut c = clients(&instance).await;
    let (admin, _, _) = sign_up(&mut c, "admin").await;
    let (juan, _, _) = sign_up(&mut c, "juan").await;
    let (mika, _, _) = sign_up(&mut c, "mika").await;
    let home = create_server(&mut c, &juan, "Home", true).await.id;
    let guest = create_server(&mut c, &mika, "Guest", true).await.id;
    let dev = new_channel(&mut c, &juan, &home, "dev", pb::ChannelType::Text).await;
    let mods = new_channel(&mut c, &juan, &home, "mod-log", pb::ChannelType::Text).await;
    let code = c
        .shared
        .create_share_code(authed(
            &juan,
            pb::CreateShareCodeRequest { server_id: home.clone(), channel_id: dev.id.clone() },
        ))
        .await
        .unwrap()
        .into_inner()
        .code
        .unwrap()
        .code;
    let preview = async |c: &mut Clients| {
        c.shared
            .preview_share(authed(&mika, pb::PreviewShareRequest { server_id: guest.clone(), code: code.clone() }))
            .await
            .unwrap()
            .into_inner()
            .checked_by
    };
    // No provider rule: nothing outside reads the channel.
    assert!(preview(&mut c).await.is_empty());

    let jev = pb::AutoModProviderSettings {
        id: "typesafe-jev".into(),
        enabled: true,
        api_key: "not-a-real-token-1234".into(),
        ..Default::default()
    };
    c.admin
        .update_settings(authed(
            &admin,
            settings_update(
                pb::InstanceSettings { automod_providers: vec![jev], ..Default::default() },
                &["automod_providers"],
                &[],
            ),
        ))
        .await
        .unwrap();
    let rule = save_rule(
        &mut c,
        &juan,
        &home,
        pb::AutoModRule {
            enabled: true,
            trigger: Trigger::Provider as i32,
            provider: "typesafe-jev".into(),
            actions: vec![pb::AutoModAction {
                kind: Kind::Alert as i32,
                channel_id: mods.id.clone(),
                ..Default::default()
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // The guest's admin sees who reads their people's messages before asking.
    assert_eq!(preview(&mut c).await, ["TypeSafe Jev (api.typesafe.ai)"]);

    // A channel the rule leaves alone isn't sent anywhere.
    let exempt = pb::AutoModRule { exempt_channel_ids: vec![dev.id.clone()], ..rule.clone() };
    save_rule(&mut c, &juan, &home, exempt).await.unwrap();
    assert!(preview(&mut c).await.is_empty());
    save_rule(&mut c, &juan, &home, rule.clone()).await.unwrap();

    // Once connected, both sides' Shared channels pages say so too, asked of
    // the home each time, so a rule changed later shows up.
    let asked = c
        .shared
        .accept_share(authed(
            &mika,
            pb::AcceptShareRequest { server_id: guest.clone(), code: code.clone(), ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .connection
        .unwrap();
    let listed = async |c: &mut Clients, token: &str, server_id: &str| {
        c.shared
            .list_connections(authed(token, pb::ListConnectionsRequest { server_id: server_id.into() }))
            .await
            .unwrap()
            .into_inner()
            .connections
            .into_iter()
            .map(|c| c.checked_by)
            .collect::<Vec<_>>()
    };
    let jev = vec!["TypeSafe Jev (api.typesafe.ai)".to_string()];
    assert_eq!(listed(&mut c, &mika, &guest).await, vec![jev.clone()], "a waiting request");
    c.shared
        .review_share(authed(
            &juan,
            pb::ReviewShareRequest { server_id: home.clone(), connection_id: asked.id, approve: true },
        ))
        .await
        .unwrap();
    assert_eq!(listed(&mut c, &mika, &guest).await, vec![jev.clone()]);
    assert_eq!(listed(&mut c, &juan, &home).await, vec![jev]);
    save_rule(&mut c, &juan, &home, pb::AutoModRule { enabled: false, ..rule }).await.unwrap();
    assert_eq!(listed(&mut c, &mika, &guest).await, [Vec::<String>::new()]);
}
