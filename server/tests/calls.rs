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
    fuwa_server::api::spawn_voice_guard(app.clone());
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
                ..Default::default()
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

    // A server mute stays with her: leaving and joining again doesn't lift it.
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
    c.calls
        .leave_voice(authed(
            &mika,
            pb::LeaveVoiceRequest { server_id: sid.clone(), session_id: joined.session_id.clone() },
        ))
        .await
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
    assert!(joined.state.unwrap().server_mute, "still server muted");
    let keep = pb::KeepVoiceRequest { session_id: joined.session_id.clone(), ..keep };

    // @everyone loses CONNECT: she's hung up right away, and her next keep says so.
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
    let hung_up = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let states = c
                .calls
                .list_voice_states(authed(&juan, pb::ListVoiceStatesRequest { server_id: sid.clone() }))
                .await
                .unwrap()
                .into_inner()
                .states;
            if states.is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(hung_up.is_ok(), "losing CONNECT hangs up without waiting for a keep");
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

/// What a ListenVoice stream gave, while it ran.
#[derive(Default)]
struct Listened {
    frames: Vec<pb::VoiceFrame>,
}

#[tokio::test]
async fn programs_hear_and_talk_without_webrtc() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let mut c = clients(&instance).await;
    let (juan, juan_id) = sign_up(&mut c, "juan").await;
    let (bot, bot_id) = sign_up(&mut c, "helper").await;
    let sid = c
        .servers
        .create_server(authed(
            &juan,
            pb::CreateServerRequest { name: "Bots".into(), discoverable: true, ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap()
        .id;
    c.servers
        .join_server(authed(&bot, pb::JoinServerRequest { server_id: sid.clone(), ..Default::default() }))
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
    let mut juan_events = c
        .events
        .subscribe(authed(
            &juan,
            pb::SubscribeRequest { servers: vec![pb::ServerCursor { server_id: sid.clone(), after_sequence: None }] },
        ))
        .await
        .unwrap()
        .into_inner();

    // Juan is in the channel with an app.
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
    a.run_for(Duration::from_millis(500)).await;
    next_voice_event(&mut juan_events).await;

    // The program listens: it's in the channel as itself.
    let mut stream = c
        .calls
        .listen_voice(authed(
            &bot,
            pb::ListenVoiceRequest { server_id: sid.clone(), channel_id: voice.id.clone(), ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner();
    let first = stream.next().await.unwrap().unwrap().event.unwrap();
    let pb::listen_voice_response::Event::Joined(bot_joined) = first else { panic!("joined comes first") };
    assert_eq!(bot_joined.state.as_ref().unwrap().user_id, bot_id);
    let Payload::VoiceStateUpdated(seen) = next_voice_event(&mut juan_events).await else { panic!("not a join") };
    assert_eq!(seen.state.unwrap().user_id, bot_id, "everyone sees the program join");
    let session = bot_joined.session_id.clone();

    // Juan talks while the program listens and talks back, a few frames at a time.
    let mut calls = c.calls.clone();
    let (bot_token, sid2, session2) = (bot.clone(), sid.clone(), session.clone());
    let program = async move {
        let mut listened = Listened::default();
        let until = tokio::time::Instant::now() + Duration::from_secs(4);
        let mut speak = tokio::time::interval(Duration::from_millis(100));
        let mut sent = 0;
        while tokio::time::Instant::now() < until {
            tokio::select! {
                message = stream.next() => {
                    if let Some(pb::listen_voice_response::Event::Frame(frame)) = message.unwrap().unwrap().event {
                        listened.frames.push(frame);
                    }
                }
                _ = speak.tick() => {
                    let frames = (0..5).map(|_| { sent += 1; format!("agent {sent}").into_bytes() }).collect();
                    let request = pb::SpeakVoiceRequest { server_id: sid2.clone(), session_id: session2.clone(), frames };
                    let queued = calls.speak_voice(authed(&bot_token, request)).await.unwrap().into_inner().queued;
                    assert!(queued <= 10, "frames go out as fast as they come: {queued} waiting");
                }
            }
        }
        (listened, stream)
    };
    let ((listened, stream), ()) = tokio::join!(program, a.run_for(Duration::from_secs(4)));

    assert!(listened.frames.len() > 50, "the program heard {} frames", listened.frames.len());
    assert!(listened.frames.iter().all(|f| f.user_id == juan_id), "labelled with whose they are");
    assert!(listened.frames.iter().all(|f| f.opus.starts_with(b"frame ")), "passed on untouched");
    let steps: Vec<u32> = listened.frames.windows(2).map(|w| w[1].timestamp.wrapping_sub(w[0].timestamp)).collect();
    assert!(steps.iter().filter(|s| **s == 960).count() > steps.len() / 2, "20 ms apart: {steps:?}");
    let said: Vec<String> = a.heard.iter().map(|(_, d)| String::from_utf8_lossy(d).to_string()).collect();
    let from_program = said.iter().filter(|f| f.starts_with("agent ")).count();
    assert!(from_program > 50, "juan heard {from_program} of the program's frames");
    assert!(a.signals.iter().any(|s| s == "offer"), "the program's sound came as a track of its own");

    // Only its own session speaks, and only a second at a time.
    let speak = |session: &str, frames: usize| {
        let frames = vec![b"x".to_vec(); frames];
        authed(&bot, pb::SpeakVoiceRequest { server_id: sid.clone(), session_id: session.into(), frames })
    };
    assert_eq!(c.calls.speak_voice(speak("nope", 1)).await.unwrap_err().code(), Code::FailedPrecondition);
    assert_eq!(c.calls.speak_voice(speak(&session, 51)).await.unwrap_err().code(), Code::InvalidArgument);

    // Server muted, it's told so.
    c.calls
        .moderate_voice(authed(
            &juan,
            pb::ModerateVoiceRequest {
                server_id: sid.clone(),
                user_id: bot_id.clone(),
                server_mute: Some(true),
                ..Default::default()
            },
        ))
        .await
        .unwrap();
    assert_eq!(c.calls.speak_voice(speak(&session, 1)).await.unwrap_err().code(), Code::PermissionDenied);

    // Closing the stream leaves the channel.
    drop(stream);
    loop {
        if let Payload::VoiceStateRemoved(removed) = next_voice_event(&mut juan_events).await {
            assert_eq!(removed.user_id, bot_id);
            break;
        }
    }

    // Taken out by a moderator, its stream ends saying so.
    let mut stream = c
        .calls
        .listen_voice(authed(
            &bot,
            pb::ListenVoiceRequest { server_id: sid.clone(), channel_id: voice.id.clone(), ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner();
    stream.next().await.unwrap().unwrap();
    c.calls
        .moderate_voice(authed(
            &juan,
            pb::ModerateVoiceRequest {
                server_id: sid.clone(),
                user_id: bot_id.clone(),
                disconnect: true,
                ..Default::default()
            },
        ))
        .await
        .unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match stream.next().await {
                Some(Ok(_)) => continue,
                Some(Err(status)) => return status.code(),
                None => return Code::Ok,
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(ended, Code::FailedPrecondition);

    instance.app.shutdown.cancel();
    instance.serving.await.unwrap();
}

#[tokio::test]
async fn the_voice_crate_hears_and_talks() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let mut c = clients(&instance).await;
    let (juan, juan_id) = sign_up(&mut c, "juan").await;
    let (bot, _) = sign_up(&mut c, "parrot").await;
    let sid = c
        .servers
        .create_server(authed(
            &juan,
            pb::CreateServerRequest { name: "Birds".into(), discoverable: true, ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap()
        .id;
    c.servers
        .join_server(authed(&bot, pb::JoinServerRequest { server_id: sid.clone(), ..Default::default() }))
        .await
        .unwrap();
    let request = pb::CreateChannelRequest {
        server_id: sid.clone(),
        name: "Perch".into(),
        r#type: pb::ChannelType::Voice as i32,
        ..Default::default()
    };
    let voice = c.channels.create_channel(authed(&juan, request)).await.unwrap().into_inner().channel.unwrap();
    let (mut a, offer) = Peer::new().await;
    let request =
        pb::JoinVoiceRequest { server_id: sid.clone(), channel_id: voice.id.clone(), offer, ..Default::default() };
    a.answer(&c.calls.join_voice(authed(&juan, request)).await.unwrap().into_inner().answer);

    let client = fuwa_voice::Client::connect(&format!("http://{}", instance.addr), &bot).await.unwrap();
    let (mut heard, speaker) = client.join(&sid, &voice.id).await.unwrap();
    assert!(heard.state().is_some_and(|s| s.channel_id == voice.id));
    let program = async move {
        let mut frames = Vec::new();
        while frames.len() < 25 {
            frames.push(heard.next().await.unwrap().unwrap());
        }
        let started = std::time::Instant::now();
        speaker.say(frames.iter().map(|f| [b"back ".as_slice(), &f.opus].concat())).await.unwrap();
        (frames, started.elapsed(), heard)
    };
    let ((frames, took, _heard), ()) = tokio::join!(program, a.run_for(Duration::from_secs(4)));
    assert!(frames.iter().all(|f| f.user_id == juan_id && f.opus.starts_with(b"frame ")));
    assert!(took >= Duration::from_millis(250), "it says them about as fast as they play: {took:?}");
    let back = a.heard.iter().filter(|(_, d)| d.starts_with(b"back frame ")).count();
    assert!(back >= 20, "juan heard {back} frames said back");

    instance.app.shutdown.cancel();
    instance.serving.await.unwrap();
}

#[tokio::test]
async fn programs_hear_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let mut c = clients(&instance).await;
    let (juan, _) = sign_up(&mut c, "juan").await;
    let (one, one_id) = sign_up(&mut c, "one").await;
    let (two, _) = sign_up(&mut c, "two").await;
    let request = pb::CreateServerRequest { name: "Bots".into(), discoverable: true, ..Default::default() };
    let sid = c.servers.create_server(authed(&juan, request)).await.unwrap().into_inner().server.unwrap().id;
    for token in [&one, &two] {
        let request = pb::JoinServerRequest { server_id: sid.clone(), ..Default::default() };
        c.servers.join_server(authed(token, request)).await.unwrap();
    }
    let request = pb::CreateChannelRequest {
        server_id: sid.clone(),
        name: "Lounge".into(),
        r#type: pb::ChannelType::Voice as i32,
        ..Default::default()
    };
    let voice = c.channels.create_channel(authed(&juan, request)).await.unwrap().into_inner().channel.unwrap();
    let url = format!("http://{}", instance.addr);
    let (_one_heard, one_says) =
        fuwa_voice::Client::connect(&url, &one).await.unwrap().join(&sid, &voice.id).await.unwrap();
    let (mut two_heard, _) =
        fuwa_voice::Client::connect(&url, &two).await.unwrap().join(&sid, &voice.id).await.unwrap();
    one_says.say((0..10).map(|i| format!("hello {i}").into_bytes())).await.unwrap();
    let mut heard = Vec::new();
    while heard.len() < 10 {
        let frame = tokio::time::timeout(Duration::from_secs(5), two_heard.next()).await.unwrap().unwrap().unwrap();
        heard.push(frame);
    }
    assert!(heard.iter().all(|f| f.user_id == one_id));
    assert_eq!(heard[0].opus, b"hello 0");
    assert_eq!(heard[1].timestamp.wrapping_sub(heard[0].timestamp), 960);

    instance.app.shutdown.cancel();
    instance.serving.await.unwrap();
}

#[tokio::test]
async fn cameras_come_in_the_size_each_viewer_wants() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let mut c = clients(&instance).await;
    let (juan, _) = sign_up(&mut c, "juan").await;
    let (mika, mika_id) = sign_up(&mut c, "mika").await;
    let request = pb::CreateServerRequest { name: "Cameras".into(), discoverable: true, ..Default::default() };
    let sid = c.servers.create_server(authed(&juan, request)).await.unwrap().into_inner().server.unwrap().id;
    let request = pb::JoinServerRequest { server_id: sid.clone(), ..Default::default() };
    c.servers.join_server(authed(&mika, request)).await.unwrap();
    let request = pb::CreateChannelRequest {
        server_id: sid.clone(),
        name: "Studio".into(),
        r#type: pb::ChannelType::Voice as i32,
        ..Default::default()
    };
    let voice = c.channels.create_channel(authed(&juan, request)).await.unwrap().into_inner().channel.unwrap();

    // Mika films (and can share a screen); Juan only watches.
    let (mut filming, offer) = Peer::with_camera_and_screen().await;
    let request = pb::JoinVoiceRequest {
        server_id: sid.clone(),
        channel_id: voice.id.clone(),
        offer,
        self_video: true,
        ..Default::default()
    };
    let joined = c.calls.join_voice(authed(&mika, request)).await.unwrap().into_inner();
    let state = joined.state.unwrap();
    assert!(state.self_video && !state.video_suppress, "everyone may film by default");
    filming.answer(&joined.answer);
    let (mut watching, offer) = Peer::new().await;
    let request =
        pb::JoinVoiceRequest { server_id: sid.clone(), channel_id: voice.id.clone(), offer, ..Default::default() };
    watching.answer(&c.calls.join_voice(authed(&juan, request)).await.unwrap().into_inner().answer);

    // Until asked, a viewer gets the smallest size, starting on a keyframe.
    // A bigger one may come first, when its first frames reach the media
    // part before the smallest's do (the viewer sees something rather than
    // nothing); it moves to the smallest on that size's next keyframe.
    talk(&mut filming, &mut watching, Duration::from_secs(3)).await;
    assert!(watching.seen.len() > 10, "the camera came through: {}", watching.seen.len());
    assert!(watching.seen[0].2, "it starts on a keyframe");
    let sizes = watching.sizes_seen(0);
    let small = sizes.iter().position(|s| s == "l").expect("the smallest size came");
    assert!(watching.seen[small].2, "the move to the smallest is on a keyframe");
    assert!(sizes[small..].iter().all(|s| s == "l"), "{sizes:?}");
    assert!(small < 30, "and it came soon: {sizes:?}");
    let mid = watching.seen[0].0.to_string();
    assert!(watching.heard.len() > 10, "and the sound");

    // Shown big: it switches to full size, on a keyframe the media part asks for.
    watching.say(serde_json::json!({ "type": "layers", "layers": { &mid: "h" } }));
    let from = watching.seen.len();
    talk(&mut filming, &mut watching, Duration::from_secs(2)).await;
    let sizes = watching.sizes_seen(from);
    let first_big = sizes.iter().position(|s| s == "h").expect("full size came");
    assert!(watching.seen[from + first_big].2, "the switch is on a keyframe");
    assert!(sizes[first_big..].iter().all(|s| s == "h"), "{sizes:?}");

    // Not showing it: nothing comes.
    watching.say(serde_json::json!({ "type": "layers", "layers": { &mid: "off" } }));
    talk(&mut filming, &mut watching, Duration::from_millis(500)).await;
    let from = watching.seen.len();
    talk(&mut filming, &mut watching, Duration::from_secs(1)).await;
    assert_eq!(watching.seen.len(), from, "a camera nobody shows isn't sent");

    // A camera turned off isn't passed on, even if its app keeps sending.
    watching.say(serde_json::json!({ "type": "layers", "layers": { &mid: "h" } }));
    let keep = |self_video, self_stream| pb::KeepVoiceRequest {
        server_id: sid.clone(),
        session_id: joined.session_id.clone(),
        channel_id: voice.id.clone(),
        self_video,
        self_stream,
        ..Default::default()
    };
    assert_eq!(watching.screens_seen(0), 0, "no screen until it's shared");
    c.calls.keep_voice(authed(&mika, keep(false, false))).await.unwrap();
    talk(&mut filming, &mut watching, Duration::from_millis(500)).await;
    let from = watching.seen.len();
    talk(&mut filming, &mut watching, Duration::from_secs(1)).await;
    assert_eq!(watching.seen.len(), from, "no frames while the camera says off");
    c.calls.keep_voice(authed(&mika, keep(true, false))).await.unwrap();
    talk(&mut filming, &mut watching, Duration::from_secs(1)).await;
    assert!(watching.seen.len() > from + 5, "back once it's on again");
    assert_eq!(watching.screens_seen(from), 0);

    // Sharing a screen: it comes as a track of its own, next to the camera.
    let state = c.calls.keep_voice(authed(&mika, keep(true, true))).await.unwrap().into_inner().state.unwrap();
    assert!(state.self_stream);
    let from = watching.seen.len();
    talk(&mut filming, &mut watching, Duration::from_secs(2)).await;
    assert!(watching.screens_seen(from) > 5, "the screen came through");
    let screen = watching.seen[from..].iter().find(|(_, f, _)| f.windows(6).any(|w| w == b"screen")).unwrap();
    assert_ne!(screen.0.to_string(), mid, "on its own track");
    assert!(screen.2, "starting on a keyframe");

    // A channel that takes VIDEO away stops the camera there, and says so.
    let everyone = pb::PermissionOverwrite {
        target_id: sid.clone(),
        target: pb::OverwriteTarget::Role as i32,
        deny: vec![pb::Permission::Video as i32],
        ..Default::default()
    };
    let request = pb::SetChannelPermissionsRequest {
        server_id: sid.clone(),
        channel_id: voice.id.clone(),
        overwrites: vec![everyone],
    };
    c.channels.set_channel_permissions(authed(&juan, request)).await.unwrap();
    let mut suppressed = false;
    for _ in 0..50 {
        let states = c
            .calls
            .list_voice_states(authed(&juan, pb::ListVoiceStatesRequest { server_id: sid.clone() }))
            .await
            .unwrap()
            .into_inner()
            .states;
        if states.iter().any(|s| s.user_id == mika_id && s.video_suppress) {
            suppressed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(suppressed, "Mika's camera shows as not allowed");
    watching.say(serde_json::json!({ "type": "layers", "layers": { &mid: "h" } }));
    // The media part hears of it a moment after the voice state changes,
    // and frames already on their way still land.
    talk(&mut filming, &mut watching, Duration::from_millis(500)).await;
    let (seen, heard) = (watching.seen.len(), watching.heard.len());
    talk(&mut filming, &mut watching, Duration::from_secs(1)).await;
    assert_eq!(watching.seen.len(), seen, "no camera without VIDEO");
    assert!(watching.heard.len() > heard + 10, "sound still goes");

    // Recording needs RECORD, which nobody has by default.
    let record = pb::KeepVoiceRequest { self_record: true, ..keep(false, false) };
    let state = c.calls.keep_voice(authed(&mika, record.clone())).await.unwrap().into_inner().state.unwrap();
    assert!(state.record_suppress && !state.self_record, "no RECORD: not recording");
    let everyone = pb::PermissionOverwrite {
        target_id: sid.clone(),
        target: pb::OverwriteTarget::Role as i32,
        allow: vec![pb::Permission::Record as i32],
        deny: vec![pb::Permission::Video as i32],
    };
    let request = pb::SetChannelPermissionsRequest {
        server_id: sid.clone(),
        channel_id: voice.id.clone(),
        overwrites: vec![everyone],
    };
    c.channels.set_channel_permissions(authed(&juan, request)).await.unwrap();
    let state = c.calls.keep_voice(authed(&mika, record)).await.unwrap().into_inner().state.unwrap();
    assert!(state.self_record && !state.record_suppress, "everyone sees Mika recording");

    instance.app.shutdown.cancel();
    instance.serving.await.unwrap();
}
