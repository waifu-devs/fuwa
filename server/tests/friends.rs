//! Friends end to end: requests, presence, blocks and privacy settings on a
//! real instance, and how they decide who may open or write in an encrypted
//! conversation (with real MLS devices, as the apps use them).

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fuwa_e2ee::Device;
use fuwa_server::app::App;
use fuwa_server::config::Config;
use fuwa_server::pb;
use fuwa_server::pb::FriendState;
use fuwa_server::pb::friend_event::Payload;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;
use tonic::transport::Channel;
use tonic::{Code, Request, Streaming};

type Friends = pb::friend_service_client::FriendServiceClient<Channel>;
type Dms = pb::direct_message_service_client::DirectMessageServiceClient<Channel>;

struct Instance {
    _app: Arc<App>,
    addr: SocketAddr,
    _serving: JoinHandle<()>,
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
    Instance { _app: app, addr, _serving: serving }
}

fn authed<T>(token: &str, message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

struct Person {
    token: String,
    id: String,
    device: Device,
}

async fn sign_up(channel: &Channel, username: &str) -> Person {
    let res = pb::auth_service_client::AuthServiceClient::new(channel.clone())
        .sign_up(pb::SignUpRequest {
            username: username.into(),
            password: "correct horse battery".into(),
            display_name: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    let id = res.user.unwrap().id;
    Person { token: res.token, device: Device::new(&id).unwrap(), id }
}

async fn register(dms: &mut Dms, person: &Person) {
    dms.register_device(authed(
        &person.token,
        pb::RegisterDeviceRequest {
            signature_key: person.device.signature_key().to_vec(),
            key_packages: person.device.key_packages(3).unwrap(),
            last_resort_key_package: person.device.last_resort_key_package().unwrap(),
        },
    ))
    .await
    .unwrap();
}

async fn watch(friends: &mut Friends, person: &Person) -> Streaming<pb::WatchFriendsResponse> {
    let mut stream =
        friends.watch_friends(authed(&person.token, pb::WatchFriendsRequest {})).await.unwrap().into_inner();
    assert!(stream.next().await.unwrap().unwrap().ready);
    stream
}

/// The next event, skipping heartbeats.
async fn next(stream: &mut Streaming<pb::WatchFriendsResponse>) -> Payload {
    loop {
        let response = tokio::time::timeout(Duration::from_secs(5), stream.next()).await.unwrap().unwrap().unwrap();
        if let Some(event) = response.event {
            return event.payload.unwrap();
        }
    }
}

/// Nothing arrives for a moment.
async fn quiet(stream: &mut Streaming<pb::WatchFriendsResponse>) {
    match tokio::time::timeout(Duration::from_millis(300), stream.next()).await {
        Err(_) => {}
        Ok(Some(Ok(pb::WatchFriendsResponse { event: None, .. }))) => {}
        Ok(other) => panic!("expected nothing, got {other:?}"),
    }
}

async fn list(friends: &mut Friends, person: &Person) -> Vec<(String, FriendState, bool)> {
    friends
        .list_friends(authed(&person.token, pb::ListFriendsRequest {}))
        .await
        .unwrap()
        .into_inner()
        .friends
        .into_iter()
        .map(|f| (f.state(), f.online, f.user.unwrap().username))
        .map(|(state, online, name)| (name, state, online))
        .collect()
}

async fn ask(friends: &mut Friends, from: &Person, to: &Person) -> Result<pb::Friend, tonic::Status> {
    friends
        .send_friend_request(authed(
            &from.token,
            pb::SendFriendRequestRequest { user_id: to.id.clone(), ..Default::default() },
        ))
        .await
        .map(|r| r.into_inner().friend.unwrap())
}

async fn settings(friends: &mut Friends, person: &Person, settings: pb::FriendSettings) {
    friends
        .update_friend_settings(authed(&person.token, pb::UpdateFriendSettingsRequest { settings: Some(settings) }))
        .await
        .unwrap();
}

async fn relationship(friends: &mut Friends, person: &Person, of: &Person) -> pb::GetRelationshipResponse {
    friends
        .get_relationship(authed(&person.token, pb::GetRelationshipRequest { user_id: of.id.clone() }))
        .await
        .unwrap()
        .into_inner()
}

#[tokio::test]
async fn friends_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let channel = Channel::from_shared(format!("http://{}", instance.addr)).unwrap().connect().await.unwrap();
    let mut friends = Friends::new(channel.clone());
    let mut dms = Dms::new(channel.clone());

    let juan = sign_up(&channel, "juan").await;
    let mika = sign_up(&channel, "mika").await;
    let rin = sign_up(&channel, "rin").await;
    let mut mika_live = watch(&mut friends, &mika).await;

    // Asking by username, as typed. Mika hears of it at once.
    let sent = friends
        .send_friend_request(authed(
            &juan.token,
            pb::SendFriendRequestRequest { username: "@Mika".into(), ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .friend
        .unwrap();
    assert_eq!(sent.state(), FriendState::Outgoing);
    assert!(sent.expires_at.is_some());
    let Payload::Changed(incoming) = next(&mut mika_live).await else { panic!() };
    assert_eq!((incoming.state(), incoming.user.unwrap().id), (FriendState::Incoming, juan.id.clone()));
    let yourself = ask(&mut friends, &juan, &juan).await.unwrap_err();
    assert_eq!(yourself.code(), Code::InvalidArgument);
    let nobody = friends
        .send_friend_request(authed(
            &juan.token,
            pb::SendFriendRequestRequest { username: "nobody".into(), ..Default::default() },
        ))
        .await
        .unwrap_err();
    assert_eq!(nobody.code(), Code::NotFound);

    // Strangers with no server in common can't open a conversation.
    let stranger = dms
        .open_conversation(authed(&juan.token, pb::OpenConversationRequest { user_id: mika.id.clone() }))
        .await
        .unwrap_err();
    assert_eq!(stranger.code(), Code::PermissionDenied);

    // Mika takes it: friends, and Mika (watching) shows online to Juan.
    let mut juan_live = watch(&mut friends, &juan).await;
    friends
        .accept_friend_request(authed(&mika.token, pb::AcceptFriendRequestRequest { user_id: juan.id.clone() }))
        .await
        .unwrap();
    let Payload::Changed(accepted) = next(&mut juan_live).await else { panic!() };
    assert_eq!(accepted.state(), FriendState::Friend);
    assert!(accepted.online);
    assert_eq!(list(&mut friends, &juan).await, vec![("mika".into(), FriendState::Friend, true)]);

    // Mika hides being online: Juan sees them go, and lists say offline.
    settings(&mut friends, &mika, pb::FriendSettings { hide_online: true, ..Default::default() }).await;
    loop {
        match next(&mut juan_live).await {
            Payload::Presence(p) => {
                assert_eq!((p.user_id, p.online), (mika.id.clone(), false));
                break;
            }
            Payload::Settings(_) => panic!("someone else's settings"),
            _ => {}
        }
    }
    assert_eq!(list(&mut friends, &juan).await, vec![("mika".into(), FriendState::Friend, false)]);
    settings(&mut friends, &mika, pb::FriendSettings::default()).await;
    // Rin, who isn't a friend, never hears about any of it.
    let mut rin_live = watch(&mut friends, &rin).await;
    quiet(&mut rin_live).await;

    // Friends open a conversation without sharing a server, and write in it.
    register(&mut dms, &juan).await;
    register(&mut dms, &mika).await;
    let cid = dms
        .open_conversation(authed(&juan.token, pb::OpenConversationRequest { user_id: mika.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .conversation
        .unwrap()
        .id;
    let allowed = vec![juan.id.clone(), mika.id.clone()];
    juan.device.create_group(&cid).unwrap();
    let mika_device = mika.device.device_id();
    let claimed = dms
        .claim_key_packages(authed(&juan.token, pb::ClaimKeyPackagesRequest { device_ids: vec![mika_device.clone()] }))
        .await
        .unwrap()
        .into_inner()
        .key_packages;
    let adds: Vec<(String, Vec<u8>)> = claimed.into_iter().map(|k| (k.device_id, k.key_package)).collect();
    let commit = juan.device.commit(&cid, &adds, &[], &allowed).unwrap();
    let record = dms
        .post_commit(authed(
            &juan.token,
            pb::PostCommitRequest {
                conversation_id: cid.clone(),
                commit: commit.commit,
                group_info: commit.group_info,
                welcome: commit.welcome.unwrap(),
                welcome_device_ids: vec![mika_device],
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .record
        .unwrap();
    juan.device.process(&cid, &record.data, true, &allowed).unwrap();
    let welcomes =
        dms.list_welcomes(authed(&mika.token, pb::ListWelcomesRequest {})).await.unwrap().into_inner().welcomes;
    mika.device.join_from_welcome(&cid, &welcomes[0].data, &allowed).unwrap();
    let post = |person: &Person, text: &[u8]| pb::PostMessageRequest {
        conversation_id: cid.clone(),
        message: person.device.encrypt(&cid, text).unwrap(),
    };
    dms.post_message(authed(&juan.token, post(&juan, b"hi"))).await.unwrap();

    // Mika blocks Juan. To Juan it reads as being unfriended; nothing says blocked.
    let blocked = friends
        .block_user(authed(&mika.token, pb::BlockUserRequest { user_id: juan.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .friend
        .unwrap();
    assert_eq!(blocked.state(), FriendState::Blocked);
    // (Mika showing online again came first.)
    let ended = loop {
        match next(&mut juan_live).await {
            Payload::Presence(_) => continue,
            other => break other,
        }
    };
    assert_eq!(ended, Payload::Removed(mika.id.clone()));
    assert!(list(&mut friends, &juan).await.is_empty());
    // Juan's messages go through as ever, and Mika never reads them: to Mika
    // they're gone before read, to Juan they're there.
    let hidden =
        dms.post_message(authed(&juan.token, post(&juan, b"hello?"))).await.unwrap().into_inner().record.unwrap();
    let records =
        || pb::ListRecordsRequest { conversation_id: cid.clone(), after_sequence: hidden.sequence - 1, limit: 1 };
    let to_mika = dms.list_records(authed(&mika.token, records())).await.unwrap().into_inner().records;
    assert!(to_mika[0].data.is_empty());
    let to_juan = dms.list_records(authed(&juan.token, records())).await.unwrap().into_inner().records;
    assert_eq!(to_juan[0].data, hidden.data);
    // The conversation still opens for Juan, and calling it never rings for Mika.
    dms.open_conversation(authed(&juan.token, pb::OpenConversationRequest { user_id: mika.id.clone() })).await.unwrap();
    // Mika still has the conversation from before the block.
    let kept = dms
        .list_conversations(authed(&mika.token, pb::ListConversationsRequest {}))
        .await
        .unwrap()
        .into_inner()
        .conversations;
    assert_eq!(kept.len(), 1);
    // Mika has to unblock to write.
    let own = dms.post_message(authed(&mika.token, post(&mika, b"bye"))).await.unwrap_err();
    assert_eq!(own.code(), Code::FailedPrecondition);
    // Juan's new request looks sent, but Mika never sees it.
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(200), mika_live.next()).await {}
    assert_eq!(ask(&mut friends, &juan, &mika).await.unwrap().state(), FriendState::Outgoing);
    quiet(&mut mika_live).await;
    assert_eq!(list(&mut friends, &mika).await, vec![("juan".into(), FriendState::Blocked, false)]);
    let seen = relationship(&mut friends, &juan, &mika).await;
    assert_eq!(seen.state(), FriendState::Outgoing);
    assert!(seen.may_request);
    assert!(seen.may_message);
    // Mika can't send Juan a request while blocking him.
    assert_eq!(ask(&mut friends, &mika, &juan).await.unwrap_err().code(), Code::FailedPrecondition);
    // Unblocking: Juan's waiting request is there to take.
    friends.unblock_user(authed(&mika.token, pb::UnblockUserRequest { user_id: juan.id.clone() })).await.unwrap();
    assert_eq!(ask(&mut friends, &mika, &juan).await.unwrap().state(), FriendState::Friend);

    // Privacy: Rin takes no requests, then only from people in a server with them.
    settings(
        &mut friends,
        &rin,
        pb::FriendSettings { requests_from: pb::FriendRequestsFrom::Nobody as i32, ..Default::default() },
    )
    .await;
    assert!(matches!(next(&mut rin_live).await, Payload::Settings(s) if s.requests_from == 2));
    assert_eq!(ask(&mut friends, &juan, &rin).await.unwrap_err().code(), Code::FailedPrecondition);
    // Strangers (no server, conversation or request between them) learn
    // nothing about each other.
    let unknown = friends
        .get_relationship(authed(&juan.token, pb::GetRelationshipRequest { user_id: rin.id.clone() }))
        .await
        .unwrap_err();
    assert_eq!(unknown.code(), Code::NotFound);
    settings(
        &mut friends,
        &rin,
        pb::FriendSettings { requests_from: pb::FriendRequestsFrom::SharedServers as i32, ..Default::default() },
    )
    .await;
    assert_eq!(ask(&mut friends, &juan, &rin).await.unwrap_err().code(), Code::FailedPrecondition);
    let mut servers = pb::server_service_client::ServerServiceClient::new(channel.clone());
    let sid = servers
        .create_server(authed(
            &juan.token,
            pb::CreateServerRequest { name: "Juan's".into(), discoverable: true, ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap()
        .id;
    for person in [&rin, &mika] {
        servers
            .join_server(authed(&person.token, pb::JoinServerRequest { server_id: sid.clone(), ..Default::default() }))
            .await
            .unwrap();
    }
    assert!(relationship(&mut friends, &juan, &rin).await.may_request);
    ask(&mut friends, &juan, &rin).await.unwrap();
    ask(&mut friends, &rin, &juan).await.unwrap();
    // Friends only: Mika, in the same server but no friend, can't start one with Rin.
    settings(
        &mut friends,
        &rin,
        pb::FriendSettings { direct_messages_from: pb::DirectMessagesFrom::Friends as i32, ..Default::default() },
    )
    .await;
    let not_friends = dms
        .open_conversation(authed(&mika.token, pb::OpenConversationRequest { user_id: rin.id.clone() }))
        .await
        .unwrap_err();
    assert_eq!(not_friends.message(), "they aren't taking direct messages from you");
    assert!(relationship(&mut friends, &juan, &rin).await.may_message);

    // Mutual friends: Juan and Rin both have Mika, until Mika hides it.
    ask(&mut friends, &rin, &mika).await.unwrap();
    ask(&mut friends, &mika, &rin).await.unwrap();
    let mutual: Vec<String> =
        relationship(&mut friends, &juan, &rin).await.mutual_friends.into_iter().map(|u| u.username).collect();
    assert_eq!(mutual, ["mika"]);
    settings(&mut friends, &mika, pb::FriendSettings { hide_mutual_friends: true, ..Default::default() }).await;
    assert!(relationship(&mut friends, &juan, &rin).await.mutual_friends.is_empty());

    // Declining is silent; removing a friend tells them.
    friends.remove_friend(authed(&juan.token, pb::RemoveFriendRequest { user_id: mika.id.clone() })).await.unwrap();
    assert_eq!(list(&mut friends, &mika).await, vec![("rin".into(), FriendState::Friend, true)]);

    // A deleted account leaves everyone's list.
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(200), juan_live.next()).await {}
    pb::account_service_client::AccountServiceClient::new(channel.clone())
        .delete_account(authed(
            &rin.token,
            pb::DeleteAccountRequest { password: "correct horse battery".into(), ..Default::default() },
        ))
        .await
        .unwrap();
    loop {
        if next(&mut juan_live).await == Payload::Removed(rin.id.clone()) {
            break;
        }
    }
    assert!(list(&mut friends, &juan).await.is_empty());
}
