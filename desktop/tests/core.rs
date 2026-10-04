//! The core against a real instance: two people on two copies of the app
//! sign up, meet in a server, talk there live, and write to each other in an
//! encrypted conversation.

use std::sync::Arc;
use std::time::{Duration, Instant};

use fuwa_desktop::core::config::Paths;
use fuwa_desktop::core::dms::{Content, DmStatus, now_ms};
use fuwa_desktop::core::moderation::{Action, timed_out_until};
use fuwa_desktop::core::reports;
use fuwa_desktop::core::store::{Connection, Focus, Store};
use fuwa_desktop::core::vault::ItemKind;
use fuwa_desktop::core::{Core, Notice};
use fuwa_server::app::App;
use fuwa_server::config::Config;

struct Instance {
    url: String,
    runtime: tokio::runtime::Runtime,
    app: Arc<App>,
}

fn start_instance(dir: &std::path::Path) -> Instance {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let dir = dir.to_str().unwrap().to_string();
    let (app, url) = runtime.block_on(async {
        let config = Config::from_lookup(|key| match key {
            "FUWA_DATA_PATH" => Some(dir.clone()),
            "FUWA_TELEMETRY" => Some("off".into()),
            _ => None,
        })
        .unwrap();
        let app = App::open(config).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let router = app.router();
        let shutdown = app.shutdown.clone();
        tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move { shutdown.cancelled().await })
                .await
                .unwrap();
        });
        (app, url)
    });
    Instance { url, runtime, app }
}

fn wait<T: Send + 'static>(core: &Arc<Core>, future: impl Future<Output = T> + Send + 'static) -> T {
    futures::executor::block_on(core.spawn(future)).unwrap()
}

/// Waits until the store shows what `check` looks for.
fn until(core: &Core, what: &str, check: impl Fn(&Store) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !core.shared.read(&check) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn two_people_talk_in_a_server_and_in_private() {
    // Secrets stay in the test's own folders, out of this computer's keychain.
    // SAFETY: set before anything reads it.
    unsafe { std::env::set_var("FUWA_DESKTOP_KEYCHAIN", "off") };
    let data = tempfile::tempdir().unwrap();
    let instance = start_instance(data.path());
    let (home_a, home_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let alice = Core::start(Paths::under(home_a.path())).unwrap();
    let bob = Core::start(Paths::under(home_b.path())).unwrap();
    let url = instance.url.clone();

    let key = {
        let (core, url) = (alice.clone(), url.clone());
        wait(&alice, async move { core.sign_up(&url, "alice", "correct horse battery", "Alice").await }).unwrap()
    };
    {
        let (core, url) = (bob.clone(), url.clone());
        wait(&bob, async move { core.sign_up(&url, "bob", "correct horse battery", "Bob").await }).unwrap();
    }
    for core in [&alice, &bob] {
        until(core, "signed in and live", |s| {
            s.instance(&key)
                .is_some_and(|i| i.me.is_some() && i.connection == Connection::Live && i.dms.status == DmStatus::Ready)
        });
    }

    // Alice makes a server; Bob joins with an invite.
    let server = {
        let (core, key) = (alice.clone(), key.clone());
        wait(&alice, async move { core.create_server(&key, "Tea house").await }).unwrap()
    };
    let invite = {
        let (core, key, id) = (alice.clone(), key.clone(), server.id.clone());
        wait(&alice, async move { core.create_invite(&key, &id).await }).unwrap()
    };
    assert!(invite.starts_with(&format!("{url}/invite/")));
    {
        let (core, key) = (bob.clone(), key.clone());
        wait(&bob, async move { core.join_by_invite(&key, &invite).await }).unwrap();
    }
    let channel = |core: &Core| {
        until(core, "the server's channels", |s| s.instance(&key).is_some_and(|i| i.synced.contains(&server.id)));
        core.shared
            .read(|s| s.instance(&key).unwrap().channels[&server.id].iter().find(|c| c.r#type == 1).unwrap().id.clone())
    };
    let general = channel(&alice);
    assert_eq!(channel(&bob), general);
    until(&alice, "Bob in the member list", |s| s.instance(&key).unwrap().members[&server.id].len() == 2);

    // Bob has the channel open; Alice writes and it shows up live, with an unread count elsewhere.
    {
        let (core, key, sid, cid) = (bob.clone(), key.clone(), server.id.clone(), general.clone());
        wait(&bob, async move { core.load_messages(&key, &sid, &cid, false).await }).unwrap();
    }
    let mut notices = bob.take_notices().unwrap();
    {
        let (core, key, sid, cid) = (alice.clone(), key.clone(), server.id.clone(), general.clone());
        wait(&alice, async move { core.send_message(&key, &sid, &cid, "hello from the desktop").await }).unwrap();
    }
    until(&bob, "Alice's message", |s| {
        s.instance(&key).unwrap().messages[&general].items.iter().any(|m| m.content == "hello from the desktop")
    });
    assert_eq!(bob.shared.read(|s| s.instance(&key).unwrap().unread.get(&general).copied()), Some(1));
    // By default only mentions notify.
    assert!(notices.try_recv().is_err(), "a plain message notified");
    let bob_name = bob.shared.read(|s| s.instance(&key).unwrap().me.clone().unwrap().username);
    let ping = format!("hey @{bob_name}, look");
    {
        let (core, key, sid, cid, text) =
            (alice.clone(), key.clone(), server.id.clone(), general.clone(), ping.clone());
        wait(&alice, async move { core.send_message(&key, &sid, &cid, &text).await }).unwrap();
    }
    until(&bob, "Alice's mention", |s| {
        s.instance(&key).unwrap().messages[&general].items.iter().any(|m| m.content == ping)
    });
    let notice = notices.try_recv().expect("the mention notified");
    assert!(matches!(notice, Notice::Message { ref body, mention: true, .. } if *body == ping), "{notice:?}");

    // Alice edits it; Bob sees the edit.
    let id = bob.shared.read(|s| {
        s.instance(&key).unwrap().messages[&general].items.iter().find(|m| m.content == ping).unwrap().id.clone()
    });
    {
        let (core, key, sid) = (alice.clone(), key.clone(), server.id.clone());
        wait(&alice, async move { core.edit_message(&key, &sid, &id, "hey, look (edited)").await }).unwrap();
    }
    until(&bob, "the edit", |s| {
        s.instance(&key).unwrap().messages[&general]
            .items
            .iter()
            .any(|m| m.content == "hey, look (edited)" && m.edited_at.is_some())
    });

    // Bob mutes the channel on the instance: no unread count on the server, and no notification.
    {
        let (core, key, sid, cid) = (bob.clone(), key.clone(), server.id.clone(), general.clone());
        let patch = fuwa_desktop::core::account::NotificationPatch { mute_until: Some(None), ..Default::default() };
        wait(&bob, async move { core.update_notifications(&key, &sid, &cid, patch).await }).unwrap();
    }
    assert_eq!(bob.shared.read(|s| s.instance(&key).unwrap().server_unread(&server.id)), 0);
    {
        let (core, key, sid, cid, text) =
            (alice.clone(), key.clone(), server.id.clone(), general.clone(), ping.clone());
        wait(&alice, async move { core.send_message(&key, &sid, &cid, &text).await }).unwrap();
    }
    until(&bob, "the muted mention", |s| {
        s.instance(&key).unwrap().messages[&general].items.iter().filter(|m| m.content == ping).count() == 1
            && s.instance(&key).unwrap().unread.get(&general).copied() == Some(3)
    });
    assert!(notices.try_recv().is_err(), "a muted channel notified");
    // It's kept on the instance, so it follows Bob to his other devices.
    bob.shared.instance(&key, |i| i.notifications.clear());
    {
        let (core, key) = (bob.clone(), key.clone());
        wait(&bob, async move {
            core.refresh_notifications(&key).await;
            Ok::<_, ()>(())
        })
        .unwrap();
    }
    assert!(bob.shared.read(|s| s.instance(&key).unwrap().is_muted(
        &server.id,
        &general,
        fuwa_desktop::core::dms::now_ms()
    )));
    bob.set_focus(Some(Focus { instance: key.clone(), channel: general.clone() }));
    assert_eq!(bob.shared.read(|s| s.instance(&key).unwrap().unread.get(&general).copied()), None);

    // Alice opens a conversation with Bob and writes; only their devices can read it.
    let bob_id = bob.shared.read(|s| s.instance(&key).unwrap().me.clone().unwrap().id);
    let conversation = {
        let (core, key, bob_id) = (alice.clone(), key.clone(), bob_id.clone());
        wait(&alice, async move { core.open_conversation(&key, &bob_id).await }).unwrap()
    };
    {
        let (core, key, id) = (alice.clone(), key.clone(), conversation.clone());
        let content = Content::Text { text: "a secret 🍵".into(), reply_to: 0 };
        wait(&alice, async move { core.send_dm(&key, &id, content).await }).unwrap();
    }
    let said = |core: &Core, text: &'static str| {
        let (key, id) = (key.clone(), conversation.clone());
        until(core, text, move |s| {
            s.instance(&key)
                .unwrap()
                .dms
                .items
                .get(&id)
                .is_some_and(|items| items.iter().any(|i| i.kind == ItemKind::Text && i.content == text))
        });
    };
    said(&bob, "a secret 🍵");
    said(&alice, "a secret 🍵");
    // Bob answers; both see the same safety number.
    {
        let (core, key, id) = (bob.clone(), key.clone(), conversation.clone());
        let content = Content::Text { text: "got it".into(), reply_to: 0 };
        wait(&bob, async move { core.send_dm(&key, &id, content).await }).unwrap();
    }
    said(&alice, "got it");
    let safety = |core: &Core| {
        core.shared.read(|s| s.instance(&key).unwrap().dms.safety.get(&conversation).cloned().unwrap_or_default())
    };
    until(&bob, "a safety number", |s| {
        s.instance(&key).unwrap().dms.safety.get(&conversation).is_some_and(|n| n.len() == 60)
    });
    until(&alice, "a safety number", |s| {
        s.instance(&key).unwrap().dms.safety.get(&conversation).is_some_and(|n| n.len() == 60)
    });
    assert_eq!(safety(&alice), safety(&bob));

    // Alice owns the server, so she may time Bob out, kick or ban him; he
    // may do nothing to her. A time-out reaches Bob live, and so does a kick.
    let alice_id = alice.shared.read(|s| s.instance(&key).unwrap().me.clone().unwrap().id);
    let sid = server.id.clone();
    assert_eq!(alice.shared.read(|s| s.instance(&key).unwrap().can_moderate(&sid, &bob_id)).len(), 3);
    assert!(bob.shared.read(|s| s.instance(&key).unwrap().can_moderate(&sid, &alice_id)).is_empty());
    {
        let (core, key, sid, bob_id) = (alice.clone(), key.clone(), sid.clone(), bob_id.clone());
        wait(&alice, async move { core.moderate(&key, &sid, &bob_id, Action::TimeOut(600), "calm down").await })
            .unwrap();
    }
    until(&bob, "his time-out", |s| {
        s.instance(&key).unwrap().my_member(&sid).is_some_and(|m| timed_out_until(m, now_ms()).is_some())
    });
    {
        let (core, key, sid, bob_id) = (alice.clone(), key.clone(), sid.clone(), bob_id.clone());
        wait(&alice, async move { core.moderate(&key, &sid, &bob_id, Action::Kick, "").await }).unwrap();
    }
    until(&bob, "being kicked", |s| s.instance(&key).unwrap().server(&sid).is_none());
    assert!(alice.shared.read(|s| {
        s.instance(&key).unwrap().members[&sid].iter().all(|m| m.user.as_ref().is_none_or(|u| u.id != bob_id))
    }));

    // What the instance keeps is ciphertext only: the words appear nowhere in its files.
    let mut found = false;
    for entry in walk(data.path()) {
        let bytes = std::fs::read(&entry).unwrap_or_default();
        found |= bytes.windows(b"a secret".len()).any(|w| w == b"a secret");
    }
    assert!(!found, "the instance kept a direct message's words");

    // A restarted app picks up where it was: same session, same device, same history.
    drop(alice);
    let alice = Core::start(Paths::under(home_a.path())).unwrap();
    said(&alice, "got it");

    instance.app.shutdown.cancel();
    drop(instance.runtime);
}

#[test]
fn anonymous_reports_reach_the_instance() {
    // SAFETY: set before anything reads it.
    unsafe { std::env::set_var("FUWA_DESKTOP_KEYCHAIN", "off") };
    let data = tempfile::tempdir().unwrap();
    let instance = start_instance(data.path());
    let home = tempfile::tempdir().unwrap();
    let app = Core::start(Paths::under(home.path())).unwrap();
    assert!(app.prefs().share_reports, "on unless turned off");
    let key = {
        let (core, url) = (app.clone(), instance.url.clone());
        wait(&app, async move { core.sign_up(&url, "carol", "correct horse battery", "Carol").await }).unwrap()
    };
    until(&app, "signed in", |s| s.instance(&key).is_some_and(|i| i.me.is_some()));
    // The test instance's telemetry is off, so the app doesn't pick it by itself.
    assert_eq!(app.report_destination(), None);

    // Signing up was timed, among other calls.
    reports::used("message.send");
    assert!(reports::pending().timings > 0);
    let sent = {
        let (core, key) = (app.clone(), key.clone());
        wait(&app, async move { core.send_report_to(&key).await })
    };
    assert!(sent.unwrap(), "the report went out");

    // A second one within the minute is turned away, which shows the first
    // arrived, and it stays here for the next time.
    reports::used("message.send");
    let again = {
        let (core, key) = (app.clone(), key.clone());
        wait(&app, async move { core.send_report_to(&key).await })
    };
    assert_eq!(again.unwrap_err().code, tonic::Code::ResourceExhausted);
    assert!(reports::pending().usage >= 1, "kept for the next report");

    instance.app.shutdown.cancel();
    drop(instance.runtime);
}

#[test]
fn instance_admins_manage_every_server() {
    // SAFETY: set before anything reads it.
    unsafe { std::env::set_var("FUWA_DESKTOP_KEYCHAIN", "off") };
    let data = tempfile::tempdir().unwrap();
    let instance = start_instance(data.path());
    let home = tempfile::tempdir().unwrap();
    let app = Core::start(Paths::under(home.path())).unwrap();
    // The first account on an instance is its admin.
    let key = {
        let (core, url) = (app.clone(), instance.url.clone());
        wait(&app, async move { core.sign_up(&url, "dana", "correct horse battery", "Dana").await }).unwrap()
    };
    until(&app, "signed in", |s| s.instance(&key).is_some_and(|i| i.me.is_some()));
    let server = {
        let (core, key) = (app.clone(), key.clone());
        wait(&app, async move { core.create_server(&key, "Book Nook").await }).unwrap()
    };
    let listed = {
        let (core, key) = (app.clone(), key.clone());
        wait(&app, async move { core.list_instance_servers(&key).await }).unwrap()
    };
    assert_eq!(listed.len(), 1);
    assert!(listed[0].member, "Dana made it");

    // Its own caps; the ones left unset follow the instance's.
    let (core, k, id) = (app.clone(), key.clone(), server.id.clone());
    let own = wait(&app, async move {
        let mut own = core.server_own_limits(&k, &id).await.unwrap();
        own.members = Some(25);
        own.storage_bytes = Some(1 << 30);
        core.set_server_limits(&k, &id, own).await.unwrap();
        core.server_own_limits(&k, &id).await.unwrap()
    });
    assert_eq!((own.members, own.storage_bytes, own.channels), (Some(25), Some(1 << 30), None));

    // Its whole file arrives as a SQLite database, with progress on the way.
    let path = home.path().join("book-nook.db");
    let (core, k, id, to) = (app.clone(), key.clone(), server.id.clone(), path.clone());
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let progress = seen.clone();
    let size = wait(&app, async move {
        core.export_server(&k, &id, &to, move |f| progress.lock().unwrap().push(f)).await.unwrap()
    });
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.len() as u64, size);
    assert!(bytes.starts_with(b"SQLite format 3\0"));
    assert_eq!(seen.lock().unwrap().last().copied(), Some(1.0));

    // A save that fails leaves the file it would have replaced as it was, and nothing beside it.
    let (core, k, to) = (app.clone(), key.clone(), path.clone());
    assert!(wait(&app, async move { core.export_server(&k, "01NOPE", &to, |_| {}).await }).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(!home.path().join("book-nook.db.part").exists());

    // Other instances: a key to compare, and no reaching out while federation is off.
    let (core, k) = (app.clone(), key.clone());
    let info = wait(&app, async move { core.federation(&k).await }).unwrap();
    assert_eq!(info.fingerprint.split(' ').count(), 8);
    assert!(info.peers.is_empty());
    let (core, k) = (app.clone(), key.clone());
    assert!(wait(&app, async move { core.check_instance(&k, "chat.example.com").await }).is_err());

    // Deleting it takes it off the list.
    let (core, k, id) = (app.clone(), key.clone(), server.id.clone());
    wait(&app, async move { core.delete_any_server(&k, &id).await }).unwrap();
    let (core, k) = (app.clone(), key.clone());
    assert!(wait(&app, async move { core.list_instance_servers(&k).await }).unwrap().is_empty());

    instance.app.shutdown.cancel();
    drop(instance.runtime);
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() { out.extend(walk(&path)) } else { out.push(path) }
    }
    out
}
