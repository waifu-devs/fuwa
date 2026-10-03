//! Secure channels end to end: a real instance on a local port, and real MLS
//! devices (fuwa-e2ee, as the apps use it) keeping a channel's group in step
//! with who its permissions let see it.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use fuwa_e2ee::{Device, Processed};
use fuwa_server::app::App;
use fuwa_server::config::Config;
use fuwa_server::pb;
use fuwa_server::pb::event::Payload;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;
use tonic::transport::Channel;
use tonic::{Code, Request};

type Dms = pb::direct_message_service_client::DirectMessageServiceClient<Channel>;
type Secure = pb::secure_channel_service_client::SecureChannelServiceClient<Channel>;

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

/// Someone signed in on one device, registered for encrypted messages.
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
    let person = Person { token: res.token, device: Device::new(&id).unwrap(), id };
    Dms::new(channel.clone())
        .register_device(authed(
            &person.token,
            pb::RegisterDeviceRequest {
                signature_key: person.device.signature_key().to_vec(),
                key_packages: person.device.key_packages(3).unwrap(),
                last_resort_key_package: person.device.last_resort_key_package().unwrap(),
            },
        ))
        .await
        .unwrap();
    person
}

async fn records(secure: &mut Secure, person: &Person, sid: &str, cid: &str, after: i64) -> Vec<pb::SecureRecord> {
    secure
        .list_secure_records(authed(
            &person.token,
            pb::ListSecureRecordsRequest {
                server_id: sid.into(),
                channel_id: cid.into(),
                after_sequence: after,
                limit: 0,
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .records
}

async fn members(secure: &mut Secure, person: &Person, sid: &str, cid: &str) -> Vec<String> {
    let mut ids = secure
        .get_secure_channel(authed(
            &person.token,
            pb::GetSecureChannelRequest { server_id: sid.into(), channel_id: cid.into() },
        ))
        .await
        .unwrap()
        .into_inner()
        .member_ids;
    ids.sort();
    ids
}

#[tokio::test]
async fn a_secure_channel_follows_its_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let channel = Channel::from_shared(format!("http://{}", instance.addr)).unwrap().connect().await.unwrap();
    let mut dms = Dms::new(channel.clone());
    let mut secure = Secure::new(channel.clone());
    let mut servers = pb::server_service_client::ServerServiceClient::new(channel.clone());
    let mut channels = pb::channel_service_client::ChannelServiceClient::new(channel.clone());

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
    for person in [&mika, &rin] {
        servers
            .join_server(authed(&person.token, pb::JoinServerRequest { server_id: sid.clone(), ..Default::default() }))
            .await
            .unwrap();
    }

    // Making one takes Manage Channels, like any channel.
    let create = pb::CreateChannelRequest {
        server_id: sid.clone(),
        name: "Secret Plans".into(),
        r#type: pb::ChannelType::Secure as i32,
        ..Default::default()
    };
    let denied = channels.create_channel(authed(&mika.token, create.clone())).await.unwrap_err();
    assert_eq!(denied.code(), Code::PermissionDenied);
    let made = channels.create_channel(authed(&juan.token, create)).await.unwrap().into_inner().channel.unwrap();
    assert_eq!((made.name.as_str(), made.r#type), ("secret-plans", pb::ChannelType::Secure as i32));
    let cid = made.id.clone();

    // Plain messages never go in one.
    let plain = pb::message_service_client::MessageServiceClient::new(channel.clone())
        .send_message(authed(
            &juan.token,
            pb::SendMessageRequest {
                server_id: sid.clone(),
                channel_id: cid.clone(),
                content: "hello".into(),
                ..Default::default()
            },
        ))
        .await
        .unwrap_err();
    assert_eq!(plain.code(), Code::InvalidArgument);

    let mut everyone = vec![juan.id.clone(), mika.id.clone(), rin.id.clone()];
    everyone.sort();
    assert_eq!(members(&mut secure, &mika, &sid, &cid).await, everyone);

    // A message needs the group started first.
    juan.device.create_group(&cid).unwrap();
    let early = juan.device.encrypt(&cid, b"too soon").unwrap();
    let refused = secure
        .post_secure_message(authed(
            &juan.token,
            pb::PostSecureMessageRequest { server_id: sid.clone(), channel_id: cid.clone(), message: early },
        ))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Code::FailedPrecondition);

    // Juan adds everyone's devices: anyone you share a server with can be claimed.
    let devices = dms
        .list_devices(authed(&juan.token, pb::ListDevicesRequest { user_ids: everyone.clone() }))
        .await
        .unwrap()
        .into_inner()
        .devices;
    let others: Vec<String> =
        devices.iter().filter(|d| d.id != juan.device.device_id()).map(|d| d.id.clone()).collect();
    assert_eq!(others.len(), 2);
    let claimed = dms
        .claim_key_packages(authed(&juan.token, pb::ClaimKeyPackagesRequest { device_ids: others.clone() }))
        .await
        .unwrap()
        .into_inner()
        .key_packages;
    let adds: Vec<(String, Vec<u8>)> = claimed.into_iter().map(|k| (k.device_id, k.key_package)).collect();
    let commit = juan.device.commit(&cid, &adds, &[], &everyone).unwrap();
    let post = pb::PostSecureCommitRequest {
        server_id: sid.clone(),
        channel_id: cid.clone(),
        commit: commit.commit.clone(),
        group_info: commit.group_info.clone(),
        welcome: commit.welcome.clone().unwrap(),
        welcome_device_ids: others.clone(),
    };
    // A welcome only goes to devices of people who can see the channel.
    let mut stray = post.clone();
    stray.welcome_device_ids.push("00000000000000000000000000000000".into());
    let refused = secure.post_secure_commit(authed(&juan.token, stray)).await.unwrap_err();
    assert_eq!(refused.code(), Code::InvalidArgument);
    let record =
        secure.post_secure_commit(authed(&juan.token, post.clone())).await.unwrap().into_inner().record.unwrap();
    assert_eq!((record.sequence, record.epoch), (1, 0));
    juan.device.process(&cid, &record.data, true, &everyone).unwrap();
    let stale = secure.post_secure_commit(authed(&juan.token, post)).await.unwrap_err();
    assert_eq!(stale.code(), Code::FailedPrecondition);

    for person in [&mika, &rin] {
        let welcomes = secure
            .list_secure_welcomes(authed(&person.token, pb::ListSecureWelcomesRequest { server_id: sid.clone() }))
            .await
            .unwrap()
            .into_inner()
            .welcomes;
        assert_eq!(welcomes.len(), 1);
        assert_eq!(welcomes[0].channel_id, cid);
        person.device.join_from_welcome(&cid, &welcomes[0].data, &everyone).unwrap();
    }

    // Rin follows the server's events; secure records come through them.
    let mut rin_events = pb::event_service_client::EventServiceClient::new(channel.clone())
        .subscribe(authed(
            &rin.token,
            pb::SubscribeRequest { servers: vec![pb::ServerCursor { server_id: sid.clone(), after_sequence: None }] },
        ))
        .await
        .unwrap()
        .into_inner();
    while rin_events.next().await.unwrap().unwrap().ready.is_none() {}

    let sealed = juan.device.encrypt(&cid, b"meet at noon").unwrap();
    let sent = secure
        .post_secure_message(authed(
            &juan.token,
            pb::PostSecureMessageRequest { server_id: sid.clone(), channel_id: cid.clone(), message: sealed },
        ))
        .await
        .unwrap()
        .into_inner()
        .record
        .unwrap();
    let mut seen = None;
    while seen.is_none() {
        let event = rin_events.next().await.unwrap().unwrap();
        if let Some(Payload::SecureRecordAdded(added)) = event.event.and_then(|e| e.payload)
            && added.record.as_ref().is_some_and(|r| r.sequence == sent.sequence)
        {
            seen = added.record;
        }
    }
    for person in [&mika, &rin] {
        match person.device.process(&cid, &seen.as_ref().unwrap().data, false, &everyone).unwrap() {
            Processed::Message { sender, plaintext } => {
                assert_eq!(plaintext, b"meet at noon");
                assert_eq!(sender.user_id, juan.id);
            }
            other => panic!("{other:?}"),
        }
    }

    // Rin loses access: the server stops showing the channel at once, and
    // the next device to write takes Rin's devices out of the group.
    channels
        .set_channel_permissions(authed(
            &juan.token,
            pb::SetChannelPermissionsRequest {
                server_id: sid.clone(),
                channel_id: cid.clone(),
                overwrites: vec![pb::PermissionOverwrite {
                    target_id: rin.id.clone(),
                    target: pb::OverwriteTarget::Member as i32,
                    allow: vec![],
                    deny: vec![pb::Permission::ViewChannels as i32],
                }],
            },
        ))
        .await
        .unwrap();
    let mut permitted = vec![juan.id.clone(), mika.id.clone()];
    permitted.sort();
    assert_eq!(members(&mut secure, &mika, &sid, &cid).await, permitted);
    let gone = secure
        .list_secure_records(authed(
            &rin.token,
            pb::ListSecureRecordsRequest { server_id: sid.clone(), channel_id: cid.clone(), ..Default::default() },
        ))
        .await
        .unwrap_err();
    assert_eq!(gone.code(), Code::NotFound);

    let removal = mika.device.commit(&cid, &[], &[rin.device.device_id()], &permitted).unwrap();
    assert_eq!(removal.removed.len(), 1);
    let removed = secure
        .post_secure_commit(authed(
            &mika.token,
            pb::PostSecureCommitRequest {
                server_id: sid.clone(),
                channel_id: cid.clone(),
                commit: removal.commit,
                group_info: removal.group_info,
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .record
        .unwrap();
    mika.device.process(&cid, &removed.data, true, &permitted).unwrap();
    assert!(matches!(
        juan.device.process(&cid, &removed.data, false, &permitted).unwrap(),
        Processed::Commit { removed, .. } if removed.len() == 1
    ));
    // Even holding the commit, Rin's device is out: it can't read what comes next.
    assert!(matches!(
        rin.device.process(&cid, &removed.data, false, &everyone).unwrap(),
        Processed::Commit { removed_me: true, .. }
    ));
    let after = mika.device.encrypt(&cid, b"just us now").unwrap();
    let later = secure
        .post_secure_message(authed(
            &mika.token,
            pb::PostSecureMessageRequest { server_id: sid.clone(), channel_id: cid.clone(), message: after },
        ))
        .await
        .unwrap()
        .into_inner()
        .record
        .unwrap();
    assert!(rin.device.process(&cid, &later.data, false, &everyone).is_err());
    assert!(matches!(
        juan.device.process(&cid, &later.data, false, &permitted).unwrap(),
        Processed::Message { plaintext, .. } if plaintext == b"just us now"
    ));
    // And nothing more reaches Rin's event stream for that channel.
    let rest = tokio::time::timeout(std::time::Duration::from_millis(300), async {
        while let Some(Ok(event)) = rin_events.next().await {
            if let Some(Payload::SecureRecordAdded(_)) = event.event.and_then(|e| e.payload) {
                return true;
            }
        }
        false
    })
    .await;
    assert!(!matches!(rest, Ok(true)), "a removed member still got a secure record");

    // People delete their own messages; moderators anyone's. Only ciphertext goes.
    let theirs = secure
        .delete_secure_record(authed(
            &mika.token,
            pb::DeleteSecureRecordRequest { server_id: sid.clone(), channel_id: cid.clone(), sequence: sent.sequence },
        ))
        .await
        .unwrap_err();
    assert_eq!(theirs.code(), Code::PermissionDenied);
    secure
        .delete_secure_record(authed(
            &juan.token,
            pb::DeleteSecureRecordRequest { server_id: sid.clone(), channel_id: cid.clone(), sequence: later.sequence },
        ))
        .await
        .unwrap();
    let gap = records(&mut secure, &mika, &sid, &cid, later.sequence - 1).await.remove(0);
    assert!(gap.data.is_empty() && gap.deleted_at.is_some());
    assert_eq!(gap.deleted_by, juan.id);
    let commit_delete = secure
        .delete_secure_record(authed(
            &juan.token,
            pb::DeleteSecureRecordRequest { server_id: sid.clone(), channel_id: cid.clone(), sequence: 1 },
        ))
        .await
        .unwrap_err();
    assert_eq!(commit_delete.code(), Code::InvalidArgument);

    // Someone who may read but not write can't commit for the group: a
    // commit the server can't read could break it for everyone.
    channels
        .set_channel_permissions(authed(
            &juan.token,
            pb::SetChannelPermissionsRequest {
                server_id: sid.clone(),
                channel_id: cid.clone(),
                overwrites: vec![
                    pb::PermissionOverwrite {
                        target_id: rin.id.clone(),
                        target: pb::OverwriteTarget::Member as i32,
                        allow: vec![],
                        deny: vec![pb::Permission::ViewChannels as i32],
                    },
                    pb::PermissionOverwrite {
                        target_id: mika.id.clone(),
                        target: pb::OverwriteTarget::Member as i32,
                        allow: vec![],
                        deny: vec![pb::Permission::SendMessages as i32],
                    },
                ],
            },
        ))
        .await
        .unwrap();
    let reader = mika.device.commit(&cid, &[], &[juan.device.device_id()], &permitted).unwrap();
    let refused = secure
        .post_secure_commit(authed(
            &mika.token,
            pb::PostSecureCommitRequest {
                server_id: sid.clone(),
                channel_id: cid.clone(),
                commit: reader.commit,
                group_info: reader.group_info,
                ..Default::default()
            },
        ))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Code::PermissionDenied);
    mika.device.discard_pending(&cid).unwrap();

    // When the group can't be followed any more, someone with Manage Channels
    // starts it over: back to epoch 0, no group info, no welcomes waiting.
    let reset = pb::ResetSecureChannelRequest { server_id: sid.clone(), channel_id: cid.clone() };
    let refused = secure.reset_secure_channel(authed(&mika.token, reset.clone())).await.unwrap_err();
    assert_eq!(refused.code(), Code::PermissionDenied);
    let marker = secure.reset_secure_channel(authed(&juan.token, reset)).await.unwrap().into_inner().record.unwrap();
    assert_eq!(marker.kind, pb::SecureRecordKind::Reset as i32);
    assert!(marker.data.is_empty() && marker.sender_id == juan.id);
    let state = secure
        .get_secure_channel(authed(
            &mika.token,
            pb::GetSecureChannelRequest { server_id: sid.clone(), channel_id: cid.clone() },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!((state.epoch, state.last_sequence), (0, marker.sequence));
    let info = secure
        .get_secure_group_info(authed(
            &mika.token,
            pb::GetSecureGroupInfoRequest { server_id: sid.clone(), channel_id: cid.clone() },
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(info.group_info.is_empty());
    // Juan's device starts a new group and brings Mika's in.
    juan.device.forget(&cid).unwrap();
    mika.device.forget(&cid).unwrap();
    juan.device.create_group(&cid).unwrap();
    let mika_device = vec![mika.device.device_id()];
    let claimed = dms
        .claim_key_packages(authed(&juan.token, pb::ClaimKeyPackagesRequest { device_ids: mika_device.clone() }))
        .await
        .unwrap()
        .into_inner()
        .key_packages;
    let adds: Vec<(String, Vec<u8>)> = claimed.into_iter().map(|k| (k.device_id, k.key_package)).collect();
    let fresh = juan.device.commit(&cid, &adds, &[], &permitted).unwrap();
    let started = secure
        .post_secure_commit(authed(
            &juan.token,
            pb::PostSecureCommitRequest {
                server_id: sid.clone(),
                channel_id: cid.clone(),
                commit: fresh.commit,
                group_info: fresh.group_info,
                welcome: fresh.welcome.unwrap(),
                welcome_device_ids: mika_device,
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .record
        .unwrap();
    assert_eq!((started.sequence, started.epoch), (marker.sequence + 1, 0));

    // Deleting the channel takes everything it kept.
    channels
        .delete_channel(authed(
            &juan.token,
            pb::DeleteChannelRequest { server_id: sid.clone(), channel_id: cid.clone() },
        ))
        .await
        .unwrap();
    let missing = secure
        .get_secure_group_info(authed(
            &juan.token,
            pb::GetSecureGroupInfoRequest { server_id: sid.clone(), channel_id: cid.clone() },
        ))
        .await
        .unwrap_err();
    assert_eq!(missing.code(), Code::NotFound);
}

/// How big the group info a joining device downloads gets, by devices in the
/// group. Run by hand: `cargo test -p fuwa-server --release --test secure -- --ignored --nocapture`.
#[test]
#[ignore]
fn group_info_size_by_devices() {
    for count in [10usize, 100, 500, 1000] {
        let creator = Device::new("creator").unwrap();
        let id = "01JSECURESIZE";
        creator.create_group(id).unwrap();
        let mut allowed = vec!["creator".to_string()];
        let mut adds = Vec::with_capacity(count - 1);
        for n in 1..count {
            let user = format!("user{n}");
            let device = Device::new(&user).unwrap();
            adds.push((device.device_id(), device.key_packages(1).unwrap().remove(0)));
            allowed.push(user);
        }
        let started = std::time::Instant::now();
        let commit = creator.commit(id, &adds, &[], &allowed).unwrap();
        println!(
            "{count} devices: group info {} KiB, welcome {} KiB, commit {} KiB, made in {:?}",
            commit.group_info.len() / 1024,
            commit.welcome.as_ref().map_or(0, Vec::len) / 1024,
            commit.commit.len() / 1024,
            started.elapsed()
        );
    }
}
