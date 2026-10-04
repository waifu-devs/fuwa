//! Direct messages end to end: a real instance on a local port, and real MLS
//! devices (fuwa-e2ee, as the apps use it) talking through it.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use fuwa_e2ee::{Device, Processed};
use fuwa_server::app::App;
use fuwa_server::config::Config;
use fuwa_server::pb;
use fuwa_server::pb::direct_message_event::Payload;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;
use tonic::transport::Channel;
use tonic::{Code, Request};

type Dms = pb::direct_message_service_client::DirectMessageServiceClient<Channel>;

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

fn authed<T>(token: &str, message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

/// Someone signed in on one device.
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

async fn sign_in(channel: &Channel, username: &str, id: &str) -> Person {
    let res = pb::auth_service_client::AuthServiceClient::new(channel.clone())
        .sign_in(pb::SignInRequest { username: username.into(), password: "correct horse battery".into() })
        .await
        .unwrap()
        .into_inner();
    Person { token: res.token, device: Device::new(id).unwrap(), id: id.into() }
}

async fn register(dms: &mut Dms, person: &Person) -> pb::RegisterDeviceResponse {
    dms.register_device(authed(
        &person.token,
        pb::RegisterDeviceRequest {
            signature_key: person.device.signature_key().to_vec(),
            key_packages: person.device.key_packages(3).unwrap(),
            last_resort_key_package: person.device.last_resort_key_package().unwrap(),
        },
    ))
    .await
    .unwrap()
    .into_inner()
}

async fn records(dms: &mut Dms, person: &Person, conversation: &str, after: i64) -> Vec<pb::ConversationRecord> {
    dms.list_records(authed(
        &person.token,
        pb::ListRecordsRequest { conversation_id: conversation.into(), after_sequence: after, limit: 0 },
    ))
    .await
    .unwrap()
    .into_inner()
    .records
}

#[tokio::test]
async fn an_encrypted_conversation_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let channel = Channel::from_shared(format!("http://{}", instance.addr)).unwrap().connect().await.unwrap();
    let mut dms = Dms::new(channel.clone());
    let mut servers = pb::server_service_client::ServerServiceClient::new(channel.clone());

    let juan = sign_up(&channel, "juan").await;
    let mika = sign_up(&channel, "mika").await;
    let rin = sign_up(&channel, "rin").await;
    let sid = servers
        .create_server(authed(
            &juan.token,
            pb::CreateServerRequest { name: "Waifu Devs".into(), discoverable: true, ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap()
        .id;
    servers
        .join_server(authed(&mika.token, pb::JoinServerRequest { server_id: sid, ..Default::default() }))
        .await
        .unwrap();
    let allowed = vec![juan.id.clone(), mika.id.clone()];

    // Devices register their keys. A key package must be for the caller's own key.
    let wrong = dms
        .register_device(authed(
            &juan.token,
            pb::RegisterDeviceRequest {
                signature_key: juan.device.signature_key().to_vec(),
                key_packages: mika.device.key_packages(1).unwrap(),
                last_resort_key_package: vec![],
            },
        ))
        .await
        .unwrap_err();
    assert_eq!(wrong.code(), Code::InvalidArgument, "{wrong:?}");
    let registered = register(&mut dms, &juan).await;
    assert_eq!(registered.key_packages, 3);
    assert_eq!(registered.device.unwrap().id, juan.device.device_id());
    register(&mut dms, &mika).await;

    // Only people who share a server can start one.
    let stranger = dms
        .open_conversation(authed(&rin.token, pb::OpenConversationRequest { user_id: juan.id.clone() }))
        .await
        .unwrap_err();
    assert_eq!(stranger.code(), Code::PermissionDenied);
    let peek = dms
        .list_devices(authed(&rin.token, pb::ListDevicesRequest { user_ids: vec![juan.id.clone()] }))
        .await
        .unwrap_err();
    assert_eq!(peek.code(), Code::PermissionDenied);

    let mut watch = dms.watch(authed(&mika.token, pb::WatchRequest {})).await.unwrap().into_inner();
    assert!(watch.next().await.unwrap().unwrap().ready);

    let opened = dms
        .open_conversation(authed(&juan.token, pb::OpenConversationRequest { user_id: mika.id.clone() }))
        .await
        .unwrap()
        .into_inner();
    assert!(opened.created);
    let conversation = opened.conversation.unwrap();
    let cid = conversation.id.clone();
    let mut names: Vec<String> = conversation.users.iter().map(|u| u.username.clone()).collect();
    names.sort();
    assert_eq!(names, ["juan", "mika"]);
    let again = dms
        .open_conversation(authed(&mika.token, pb::OpenConversationRequest { user_id: juan.id.clone() }))
        .await
        .unwrap()
        .into_inner();
    assert!(!again.created);
    assert_eq!(again.conversation.unwrap().id, cid);
    let event = watch.next().await.unwrap().unwrap().event.unwrap();
    assert!(matches!(event.payload, Some(Payload::ConversationOpened(c)) if c.id == cid));

    // Juan starts the group and adds Mika's device with a key package.
    juan.device.create_group(&cid).unwrap();
    let devices = dms
        .list_devices(authed(&juan.token, pb::ListDevicesRequest { user_ids: allowed.clone() }))
        .await
        .unwrap()
        .into_inner()
        .devices;
    assert_eq!(devices.len(), 2);
    let others: Vec<String> =
        devices.iter().filter(|d| d.id != juan.device.device_id()).map(|d| d.id.clone()).collect();
    let claimed = dms
        .claim_key_packages(authed(&juan.token, pb::ClaimKeyPackagesRequest { device_ids: others.clone() }))
        .await
        .unwrap()
        .into_inner()
        .key_packages;
    assert_eq!(claimed.len(), 1);
    let adds: Vec<(String, Vec<u8>)> = claimed.into_iter().map(|k| (k.device_id, k.key_package)).collect();
    let commit = juan.device.commit(&cid, &adds, &[], &allowed).unwrap();
    let post = pb::PostCommitRequest {
        conversation_id: cid.clone(),
        commit: commit.commit.clone(),
        group_info: commit.group_info.clone(),
        welcome: commit.welcome.clone().unwrap(),
        welcome_device_ids: others,
    };
    let record = dms.post_commit(authed(&juan.token, post.clone())).await.unwrap().into_inner().record.unwrap();
    assert_eq!((record.sequence, record.epoch), (1, 0));
    assert!(matches!(juan.device.process(&cid, &record.data, true, &allowed).unwrap(), Processed::Commit { .. }));
    // The same commit again is for an epoch that's gone.
    let stale = dms.post_commit(authed(&juan.token, post)).await.unwrap_err();
    assert_eq!(stale.code(), Code::FailedPrecondition);
    let event = watch.next().await.unwrap().unwrap().event.unwrap();
    assert!(matches!(event.payload, Some(Payload::RecordAdded(r)) if r.sequence == 1));

    // Mika joins from the welcome, then reads what Juan sends.
    let welcomes =
        dms.list_welcomes(authed(&mika.token, pb::ListWelcomesRequest {})).await.unwrap().into_inner().welcomes;
    assert_eq!(welcomes.len(), 1);
    mika.device.join_from_welcome(&cid, &welcomes[0].data, &allowed).unwrap();

    let secret = b"a secret only they can read";
    let sealed = juan.device.encrypt(&cid, secret).unwrap();
    let sent = dms
        .post_message(authed(
            &juan.token,
            pb::PostMessageRequest { conversation_id: cid.clone(), message: sealed, ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .record
        .unwrap();
    assert_eq!(sent.sequence, 2);
    let event = watch.next().await.unwrap().unwrap().event.unwrap();
    let Some(Payload::RecordAdded(live)) = event.payload else { panic!("{event:?}") };
    match mika.device.process(&cid, &live.data, false, &allowed).unwrap() {
        Processed::Message { sender, plaintext } => {
            assert_eq!(plaintext, secret);
            assert_eq!(sender.user_id, juan.id);
        }
        other => panic!("{other:?}"),
    }
    // Plaintext isn't a message here.
    let plain = dms
        .post_message(authed(
            &juan.token,
            pb::PostMessageRequest { conversation_id: cid.clone(), message: secret.to_vec(), ..Default::default() },
        ))
        .await
        .unwrap_err();
    assert_eq!(plain.code(), Code::InvalidArgument);
    // Nor can someone outside the conversation read its records.
    let outside = dms
        .list_records(authed(&rin.token, pb::ListRecordsRequest { conversation_id: cid.clone(), ..Default::default() }))
        .await
        .unwrap_err();
    assert_eq!(outside.code(), Code::NotFound);

    // Mika signs in somewhere else; that device joins by itself.
    let mika_phone = sign_in(&channel, "mika", &mika.id).await;
    register(&mut dms, &mika_phone).await;
    let info = dms
        .get_group_info(authed(&mika_phone.token, pb::GetGroupInfoRequest { conversation_id: cid.clone() }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(info.epoch, 1);
    let join = mika_phone.device.join_by_itself(&cid, &info.group_info, &allowed).unwrap();
    let joined = dms
        .post_commit(authed(
            &mika_phone.token,
            pb::PostCommitRequest {
                conversation_id: cid.clone(),
                commit: join.commit,
                group_info: join.group_info,
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .record
        .unwrap();
    for person in [&juan, &mika] {
        match person.device.process(&cid, &joined.data, false, &allowed).unwrap() {
            Processed::Commit { added, .. } => assert_eq!(added[0].device_id, mika_phone.device.device_id()),
            other => panic!("{other:?}"),
        }
    }
    let hello = mika_phone.device.encrypt(&cid, b"from my phone").unwrap();
    let from_phone = dms
        .post_message(authed(
            &mika_phone.token,
            pb::PostMessageRequest { conversation_id: cid.clone(), message: hello, ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .record
        .unwrap();
    assert_eq!(from_phone.sender_device_id, mika_phone.device.device_id());
    let read = records(&mut dms, &juan, &cid, joined.sequence).await;
    assert!(matches!(
        juan.device.process(&cid, &read[0].data, false, &allowed).unwrap(),
        Processed::Message { plaintext, .. } if plaintext == b"from my phone"
    ));

    // Only the sender deletes a message, and only its ciphertext goes.
    let theirs = dms
        .delete_record(authed(
            &juan.token,
            pb::DeleteRecordRequest { conversation_id: cid.clone(), sequence: from_phone.sequence },
        ))
        .await
        .unwrap_err();
    assert_eq!(theirs.code(), Code::PermissionDenied);
    dms.delete_record(authed(
        &mika.token,
        pb::DeleteRecordRequest { conversation_id: cid.clone(), sequence: from_phone.sequence },
    ))
    .await
    .unwrap();
    let gap = &records(&mut dms, &juan, &cid, joined.sequence).await[0];
    assert!(gap.data.is_empty() && gap.deleted_at.is_some());

    // Signing out takes the device with it.
    pb::auth_service_client::AuthServiceClient::new(channel.clone())
        .sign_out(authed(&mika_phone.token, pb::SignOutRequest {}))
        .await
        .unwrap();
    let left = dms
        .list_devices(authed(&juan.token, pb::ListDevicesRequest { user_ids: vec![mika.id.clone()] }))
        .await
        .unwrap()
        .into_inner()
        .devices;
    assert_eq!(left.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), [mika.device.device_id()]);
    assert_eq!(instance.app.sweep_devices().await.unwrap(), 1);

    let listed = dms
        .list_conversations(authed(&mika.token, pb::ListConversationsRequest {}))
        .await
        .unwrap()
        .into_inner()
        .conversations;
    assert_eq!((listed.len(), listed[0].epoch, listed[0].last_sequence), (1, 2, 4));

    instance.app.shutdown.cancel();
    instance.serving.await.unwrap();
    drop(instance.app);

    // Nothing on disk holds what was said.
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let bytes = std::fs::read(&path).unwrap();
            assert!(!bytes.windows(secret.len()).any(|w| w == secret), "{} holds the plaintext", path.display());
        }
    }
}

/// The same path on the test instance (links are made with the public URL).
fn on(instance: &Instance, url: &str) -> String {
    format!("http://{}{}", instance.addr, reqwest::Url::parse(url).unwrap().path())
}

#[tokio::test]
async fn voice_messages_carry_sealed_files() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let channel = Channel::from_shared(format!("http://{}", instance.addr)).unwrap().connect().await.unwrap();
    let mut dms = Dms::new(channel.clone());
    let mut servers = pb::server_service_client::ServerServiceClient::new(channel.clone());
    let http = reqwest::Client::new();

    let juan = sign_up(&channel, "juan").await;
    let mika = sign_up(&channel, "mika").await;
    let sid = servers
        .create_server(authed(
            &juan.token,
            pb::CreateServerRequest { name: "Waifu Devs".into(), discoverable: true, ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap()
        .id;
    servers
        .join_server(authed(&mika.token, pb::JoinServerRequest { server_id: sid, ..Default::default() }))
        .await
        .unwrap();
    let allowed = vec![juan.id.clone(), mika.id.clone()];
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
                commit: commit.commit.clone(),
                group_info: commit.group_info.clone(),
                welcome: commit.welcome.clone().unwrap(),
                welcome_device_ids: vec![mika_device],
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .record
        .unwrap();
    juan.device.process(&cid, &record.data, true, &allowed).unwrap();

    // The caps say there are none until an admin sets them.
    let limits = dms.get_voice_limits(authed(&mika.token, pb::GetVoiceLimitsRequest {})).await.unwrap().into_inner();
    assert_eq!((limits.max_seconds, limits.max_bytes), (None, None));

    // A sealed file is any bytes: the instance can't tell what's in it.
    let sealed: Vec<u8> = (0..4096u32).map(|n| (n.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
    let reserve = |token: &str, size: usize| {
        authed(token, pb::CreateSealedUploadRequest { conversation_id: cid.clone(), size: size as i64 })
    };
    let reserved = dms.create_sealed_upload(reserve(&juan.token, sealed.len())).await.unwrap().into_inner();
    let put = http.put(on(&instance, &reserved.upload_url)).body(sealed.clone()).send().await.unwrap();
    assert_eq!(put.status(), reqwest::StatusCode::NO_CONTENT);
    let fetched = http.get(on(&instance, &reserved.url)).send().await.unwrap();
    assert_eq!(fetched.headers()["content-type"], "application/octet-stream");
    assert_eq!(fetched.bytes().await.unwrap().to_vec(), sealed);

    // Only for conversations you're in, and only yours to send.
    let rin = sign_up(&channel, "rin").await;
    register(&mut dms, &rin).await;
    let outside = dms.create_sealed_upload(reserve(&rin.token, sealed.len())).await.unwrap_err();
    assert_eq!(outside.code(), Code::NotFound);
    let message = |device: &Device| device.encrypt(&cid, b"a voice message").unwrap();
    let post = |token: &str, message: Vec<u8>, media_ids: Vec<String>| {
        authed(token, pb::PostMessageRequest { conversation_id: cid.clone(), message, media_ids })
    };
    let mika_welcomes =
        dms.list_welcomes(authed(&mika.token, pb::ListWelcomesRequest {})).await.unwrap().into_inner().welcomes;
    mika.device.join_from_welcome(&cid, &mika_welcomes[0].data, &allowed).unwrap();
    let theirs =
        dms.post_message(post(&mika.token, message(&mika.device), vec![reserved.media_id.clone()])).await.unwrap_err();
    assert_eq!(theirs.code(), Code::NotFound);

    let sent = dms
        .post_message(post(&juan.token, message(&juan.device), vec![reserved.media_id.clone()]))
        .await
        .unwrap()
        .into_inner()
        .record
        .unwrap();
    // One message carries a file.
    let twice =
        dms.post_message(post(&juan.token, message(&juan.device), vec![reserved.media_id.clone()])).await.unwrap_err();
    assert_eq!(twice.code(), Code::InvalidArgument);

    // Kept past the sweep for unused uploads.
    instance.app.sweep_media(i64::MAX / 2).await.unwrap();
    assert_eq!(http.get(on(&instance, &reserved.url)).send().await.unwrap().status(), reqwest::StatusCode::OK);

    // A cap admins set is checked on the sealed size.
    let mut settings = (*instance.app.settings()).clone();
    settings.limits.voice_message_bytes = Some(1000);
    instance.app.replace_settings(settings);
    let big = dms.create_sealed_upload(reserve(&juan.token, sealed.len())).await.unwrap_err();
    assert_eq!(big.code(), Code::ResourceExhausted);

    // Deleting the message deletes its file.
    dms.delete_record(authed(
        &juan.token,
        pb::DeleteRecordRequest { conversation_id: cid.clone(), sequence: sent.sequence },
    ))
    .await
    .unwrap();
    assert_eq!(http.get(on(&instance, &reserved.url)).send().await.unwrap().status(), reqwest::StatusCode::NOT_FOUND);

    instance.app.shutdown.cancel();
    instance.serving.await.unwrap();
}
