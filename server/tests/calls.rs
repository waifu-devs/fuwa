//! Calls end to end: two WebRTC peers (str0m, as an app would be) join a
//! voice channel on a real instance, and one's sound reaches the other
//! through the media part. Also who's in a call, as members see it, and
//! direct-message calls.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

mod common;
use common::{Peer, talk};

use fuwa_server::app::App;
use fuwa_server::config::Config;
use fuwa_server::pb;
use fuwa_server::pb::event::Payload;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;
use tonic::transport::Channel;
use tonic::{Code, Request};

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
        "FUWA_MEDIA_PORT" => Some("0".into()),
        "FUWA_MEDIA_ADDRESSES" => Some("127.0.0.1".into()),
        "FUWA_ICE_URLS" => Some("stun:stun.example.com:3478,turn:turn.example.com:3478".into()),
        "FUWA_TURN_SECRET" => Some("north wind".into()),
        _ => None,
    })
    .unwrap();
    let app = App::open(config).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router();
    let shutdown = app.shutdown.clone();
    fuwa_server::api::spawn_voice_sweeper(app.clone());
    let serving = tokio::spawn(async move {
        axum::serve(listener, router).with_graceful_shutdown(async move { shutdown.cancelled().await }).await.unwrap();
    });
    Instance { app, addr, serving }
}

fn authed<T>(token: &str, message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

#[derive(Clone)]
struct Clients {
    auth: pb::auth_service_client::AuthServiceClient<Channel>,
    servers: pb::server_service_client::ServerServiceClient<Channel>,
    channels: pb::channel_service_client::ChannelServiceClient<Channel>,
    events: pb::event_service_client::EventServiceClient<Channel>,
    calls: pb::call_service_client::CallServiceClient<Channel>,
    roles: pb::role_service_client::RoleServiceClient<Channel>,
    dms: pb::direct_message_service_client::DirectMessageServiceClient<Channel>,
}

async fn clients(instance: &Instance) -> Clients {
    let channel = Channel::from_shared(format!("http://{}", instance.addr)).unwrap().connect().await.unwrap();
    Clients {
        auth: pb::auth_service_client::AuthServiceClient::new(channel.clone()),
        servers: pb::server_service_client::ServerServiceClient::new(channel.clone()),
        channels: pb::channel_service_client::ChannelServiceClient::new(channel.clone()),
        events: pb::event_service_client::EventServiceClient::new(channel.clone()),
        calls: pb::call_service_client::CallServiceClient::new(channel.clone()),
        roles: pb::role_service_client::RoleServiceClient::new(channel.clone()),
        dms: pb::direct_message_service_client::DirectMessageServiceClient::new(channel),
    }
}

async fn sign_up(c: &mut Clients, username: &str) -> (String, String) {
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
    (res.token, res.user.unwrap().id)
}

async fn next_voice_event(stream: &mut tonic::Streaming<pb::SubscribeResponse>) -> Payload {
    loop {
        let response = tokio::time::timeout(Duration::from_secs(25), stream.next()).await.unwrap().unwrap().unwrap();
        if let Some(payload) = response.event.and_then(|e| e.payload)
            && matches!(payload, Payload::VoiceStateUpdated(_) | Payload::VoiceStateRemoved(_))
        {
            return payload;
        }
    }
}

#[tokio::test]
async fn sound_goes_from_one_to_the_other() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let mut c = clients(&instance).await;
    let (juan, juan_id) = sign_up(&mut c, "juan").await;
    let (mika, mika_id) = sign_up(&mut c, "mika").await;

    let settings = c.calls.get_call_settings(authed(&juan, pb::GetCallSettingsRequest {})).await.unwrap().into_inner();
    assert!(settings.enabled);
    assert_eq!(settings.ice_servers.len(), 2);
    assert!(!settings.ice_servers[1].username.contains(&juan_id), "TURN credentials name no account");

    let server = c
        .servers
        .create_server(authed(
            &juan,
            pb::CreateServerRequest { name: "Calls".into(), discoverable: true, ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap();
    let sid = server.id.clone();
    c.servers
        .join_server(authed(&mika, pb::JoinServerRequest { server_id: sid.clone(), ..Default::default() }))
        .await
        .unwrap();
    let voice = c
        .channels
        .create_channel(authed(
            &juan,
            pb::CreateChannelRequest {
                server_id: sid.clone(),
                name: "Lounge".into(),
                r#type: pb::ChannelType::Voice as i32,
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .channel
        .unwrap();
    let text = c
        .channels
        .list_channels(authed(&juan, pb::ListChannelsRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels
        .into_iter()
        .find(|ch| ch.r#type == pb::ChannelType::Text as i32)
        .unwrap();

    let mut mika_events = c
        .events
        .subscribe(authed(
            &mika,
            pb::SubscribeRequest { servers: vec![pb::ServerCursor { server_id: sid.clone(), after_sequence: None }] },
        ))
        .await
        .unwrap()
        .into_inner();

    // Only voice channels take calls.
    let (_, offer) = Peer::new().await;
    let refused = c
        .calls
        .join_voice(authed(
            &juan,
            pb::JoinVoiceRequest { server_id: sid.clone(), channel_id: text.id.clone(), offer, ..Default::default() },
        ))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Code::InvalidArgument);

    let (mut a, offer) = Peer::new().await;
    let joined = c
        .calls
        .join_voice(authed(
            &juan,
            pb::JoinVoiceRequest { server_id: sid.clone(), channel_id: voice.id.clone(), offer, ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner();
    a.answer(&joined.answer);
    let Payload::VoiceStateUpdated(update) = next_voice_event(&mut mika_events).await else { panic!("not a join") };
    assert_eq!(update.state.as_ref().unwrap().user_id, juan_id);

    let (mut b, offer) = Peer::new().await;
    let mika_joined = c
        .calls
        .join_voice(authed(
            &mika,
            pb::JoinVoiceRequest {
                server_id: sid.clone(),
                channel_id: voice.id.clone(),
                offer,
                self_mute: true,
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner();
    b.answer(&mika_joined.answer);

    talk(&mut a, &mut b, Duration::from_secs(4)).await;
    assert!(a.rtc.is_connected() && b.rtc.is_connected());
    assert!(b.signals.iter().any(|s| s == "offer"), "the media part offered mika juan's track");
    let heard: Vec<String> = b.heard.iter().map(|(_, d)| String::from_utf8_lossy(d).to_string()).collect();
    assert!(heard.len() > 20, "mika heard {} frames", heard.len());
    assert!(heard.iter().all(|f| f.starts_with("frame ")), "passed on untouched");
    assert!(!a.heard.is_empty(), "juan hears mika too");

    let states = c
        .calls
        .list_voice_states(authed(&mika, pb::ListVoiceStatesRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner()
        .states;
    assert_eq!(states.len(), 2);
    assert!(states.iter().any(|s| s.user_id == mika_id && s.self_mute));

    // Keeping a place with someone else's session is refused.
    let wrong = c
        .calls
        .keep_voice(authed(
            &mika,
            pb::KeepVoiceRequest {
                server_id: sid.clone(),
                session_id: joined.session_id.clone(),
                ..Default::default()
            },
        ))
        .await
        .unwrap_err();
    assert_eq!(wrong.code(), Code::FailedPrecondition);

    // The owner server-mutes mika: the media part stops passing her sound on.
    c.calls
        .moderate_voice(authed(
            &juan,
            pb::ModerateVoiceRequest {
                server_id: sid.clone(),
                user_id: mika_id.clone(),
                server_mute: Some(true),
                ..Default::default()
            },
        ))
        .await
        .unwrap();
    // Mika can't do it back.
    let denied = c
        .calls
        .moderate_voice(authed(
            &mika,
            pb::ModerateVoiceRequest {
                server_id: sid.clone(),
                user_id: juan_id.clone(),
                disconnect: true,
                ..Default::default()
            },
        ))
        .await
        .unwrap_err();
    assert_eq!(denied.code(), Code::PermissionDenied);
    talk(&mut a, &mut b, Duration::from_millis(300)).await;
    a.heard.clear();
    talk(&mut a, &mut b, Duration::from_secs(1)).await;
    assert!(a.heard.is_empty(), "a server-muted member isn't heard");

    // Mika's place lasts while she keeps it; Juan leaves.
    let kept = c
        .calls
        .keep_voice(authed(
            &mika,
            pb::KeepVoiceRequest {
                server_id: sid.clone(),
                session_id: mika_joined.session_id.clone(),
                channel_id: voice.id.clone(),
                self_mute: false,
                self_deaf: true,
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .state
        .unwrap();
    assert!(kept.self_deaf && kept.server_mute);
    c.calls
        .leave_voice(authed(
            &juan,
            pb::LeaveVoiceRequest { server_id: sid.clone(), session_id: joined.session_id.clone() },
        ))
        .await
        .unwrap();
    loop {
        if let Payload::VoiceStateRemoved(removed) = next_voice_event(&mut mika_events).await {
            assert_eq!(removed.user_id, juan_id);
            break;
        }
    }
    talk(&mut a, &mut b, Duration::from_millis(500)).await;
    assert!(a.signals.iter().any(|s| s == "closed"), "juan's app heard it hung up");

    // A place nobody keeps runs out: everyone hears she left.
    let lost = tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            if let Payload::VoiceStateRemoved(removed) = next_voice_event(&mut mika_events).await {
                return removed;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(lost.user_id, mika_id);

    instance.app.shutdown.cancel();
    instance.serving.await.unwrap();
}

#[tokio::test]
async fn places_come_back_after_a_restart_and_follow_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let mut c = clients(&instance).await;
    let (juan, _) = sign_up(&mut c, "juan").await;
    let (mika, mika_id) = sign_up(&mut c, "mika").await;
    let sid = c
        .servers
        .create_server(authed(
            &juan,
            pb::CreateServerRequest { name: "Calls".into(), discoverable: true, ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap()
        .id;
    c.servers
        .join_server(authed(&mika, pb::JoinServerRequest { server_id: sid.clone(), ..Default::default() }))
        .await
        .unwrap();
    let voice = c
        .channels
        .create_channel(authed(
            &juan,
            pb::CreateChannelRequest {
                server_id: sid.clone(),
                name: "Lounge".into(),
                r#type: pb::ChannelType::Voice as i32,
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .channel
        .unwrap();
    let (_peer, offer) = Peer::new().await;
    let joined = c
        .calls
        .join_voice(authed(
            &mika,
            pb::JoinVoiceRequest { server_id: sid.clone(), channel_id: voice.id.clone(), offer, ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner();

    // The part forgets (as a restart would); her next keep puts her back.
    instance.app.voice.remove(&sid, &mika_id, None);
    let keep = pb::KeepVoiceRequest {
        server_id: sid.clone(),
        session_id: joined.session_id.clone(),
        channel_id: voice.id.clone(),
        ..Default::default()
    };
    c.calls.keep_voice(authed(&mika, keep.clone())).await.unwrap();
    let states = c
        .calls
        .list_voice_states(authed(&juan, pb::ListVoiceStatesRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner()
        .states;
    assert_eq!(states.len(), 1);

    // @everyone loses CONNECT: her next keep hangs her up.
    let everyone = c
        .roles
        .list_roles(authed(&juan, pb::ListRolesRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner()
        .roles
        .into_iter()
        .find(|r| r.id == sid)
        .unwrap();
    assert!(everyone.permissions.contains(&(pb::Permission::Connect as i32)), "new servers let everyone talk");
    let permissions: Vec<i32> =
        everyone.permissions.iter().copied().filter(|p| *p != pb::Permission::Connect as i32).collect();
    c.roles
        .update_role(authed(
            &juan,
            pb::UpdateRoleRequest {
                server_id: sid.clone(),
                role_id: sid.clone(),
                permissions: Some(pb::PermissionSet { permissions }),
                ..Default::default()
            },
        ))
        .await
        .unwrap();
    let gone = c.calls.keep_voice(authed(&mika, keep)).await.unwrap_err();
    assert_eq!(gone.code(), Code::FailedPrecondition);
    let (_peer, offer) = Peer::new().await;
    let refused = c
        .calls
        .join_voice(authed(
            &mika,
            pb::JoinVoiceRequest { server_id: sid.clone(), channel_id: voice.id.clone(), offer, ..Default::default() },
        ))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Code::PermissionDenied);

    instance.app.shutdown.cancel();
    instance.serving.await.unwrap();
}

#[tokio::test]
async fn direct_message_calls_ring_the_other_person() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let mut c = clients(&instance).await;
    let (juan, juan_id) = sign_up(&mut c, "juan").await;
    let (mika, mika_id) = sign_up(&mut c, "mika").await;
    let (rin, _) = sign_up(&mut c, "rin").await;
    let sid = c
        .servers
        .create_server(authed(
            &juan,
            pb::CreateServerRequest { name: "Calls".into(), discoverable: true, ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap()
        .id;
    c.servers
        .join_server(authed(&mika, pb::JoinServerRequest { server_id: sid.clone(), ..Default::default() }))
        .await
        .unwrap();
    let conversation = c
        .dms
        .open_conversation(authed(&juan, pb::OpenConversationRequest { user_id: mika_id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .conversation
        .unwrap();

    let mut watch = c.dms.watch(authed(&mika, pb::WatchRequest {})).await.unwrap().into_inner();
    let ready = tokio::time::timeout(Duration::from_secs(5), watch.next()).await.unwrap().unwrap().unwrap();
    assert!(ready.ready);

    let (_peer, offer) = Peer::new().await;
    let join = pb::JoinDmCallRequest { conversation_id: conversation.id.clone(), offer, ..Default::default() };
    let outsider = c.calls.join_dm_call(authed(&rin, join.clone())).await.unwrap_err();
    assert_eq!(outsider.code(), Code::NotFound);
    let joined = c.calls.join_dm_call(authed(&juan, join)).await.unwrap().into_inner();

    let ringing = tokio::time::timeout(Duration::from_secs(5), watch.next()).await.unwrap().unwrap().unwrap();
    let Some(pb::direct_message_event::Payload::CallUpdated(call)) = ringing.event.and_then(|e| e.payload) else {
        panic!("mika's apps hear the call");
    };
    assert_eq!(call.started_by, juan_id);
    assert_eq!(call.participants.len(), 1);
    let listed = c.calls.list_dm_calls(authed(&mika, pb::ListDmCallsRequest {})).await.unwrap().into_inner().calls;
    assert_eq!(listed.len(), 1);
    assert!(
        c.calls.list_dm_calls(authed(&rin, pb::ListDmCallsRequest {})).await.unwrap().into_inner().calls.is_empty()
    );

    c.calls
        .leave_dm_call(authed(
            &juan,
            pb::LeaveDmCallRequest { conversation_id: conversation.id.clone(), session_id: joined.session_id },
        ))
        .await
        .unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(5), watch.next()).await.unwrap().unwrap().unwrap();
    let Some(pb::direct_message_event::Payload::CallUpdated(call)) = ended.event.and_then(|e| e.payload) else {
        panic!("the call ended");
    };
    assert!(call.participants.is_empty());

    instance.app.shutdown.cancel();
    instance.serving.await.unwrap();
}
