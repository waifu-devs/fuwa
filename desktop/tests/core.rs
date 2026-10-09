//! The core against a real instance: two people on two copies of the app
//! sign up, meet in a server, talk there live, and write to each other in an
//! encrypted conversation.

use std::sync::Arc;
use std::time::{Duration, Instant};

use fuwa_desktop::core::config::Paths;
use fuwa_desktop::core::dms::{Content, DmStatus, now_ms};
use fuwa_desktop::core::moderation::{Action, timed_out_until};
use fuwa_desktop::core::polls;
use fuwa_desktop::core::reports;
use fuwa_desktop::core::search;
use fuwa_desktop::core::shared;
use fuwa_desktop::core::store::{Connection, Focus, Store};
use fuwa_desktop::core::threads;
use fuwa_desktop::core::updates;
use fuwa_desktop::core::vault::ItemKind;
use fuwa_desktop::core::voice_notes;
use fuwa_desktop::core::{Core, Notice};
use fuwa_desktop::pb;
use fuwa_server::app::App;
use fuwa_server::config::Config;

/// A 1×1 PNG, for an emoji's picture.
const TINY_PNG: [u8; 70] = [
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 31, 21, 196,
    137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 223, 224, 240, 31, 0, 7, 0, 2, 191, 43, 215, 199, 226, 0, 0,
    0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

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
            "FUWA_UPDATE_CHECK" => Some("off".into()),
            _ => None,
        })
        .unwrap();
        let app = App::open(config).await.unwrap();
        fuwa_server::api::spawn_search_indexer(app.clone());
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
    // The notification goes out just after the store shows the message.
    let deadline = Instant::now() + Duration::from_secs(5);
    let notice = loop {
        match notices.try_recv() {
            Ok(notice) => break notice,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => panic!("the mention didn't notify"),
        }
    };
    assert!(matches!(notice, Notice::Message { ref body, mention: true, .. } if *body == ping), "{notice:?}");

    // Alice edits it; Bob sees the edit.
    let id = bob.shared.read(|s| {
        s.instance(&key).unwrap().messages[&general].items.iter().find(|m| m.content == ping).unwrap().id.clone()
    });
    {
        let (core, key, sid, cid, id) = (alice.clone(), key.clone(), server.id.clone(), general.clone(), id.clone());
        wait(&alice, async move { core.edit_message(&key, &sid, &cid, &id, "hey, look (edited)").await }).unwrap();
    }
    until(&bob, "the edit", |s| {
        s.instance(&key).unwrap().messages[&general]
            .items
            .iter()
            .any(|m| m.content == "hey, look (edited)" && m.edited_at.is_some())
    });

    // Bob reacts to it and Alice sees it live; she reacts too, with a count of two for both.
    {
        let (core, key, sid, cid) = (alice.clone(), key.clone(), server.id.clone(), general.clone());
        wait(&alice, async move { core.load_messages(&key, &sid, &cid, false).await }).unwrap();
    }
    let reactions_on = |core: &Core, id: &str| {
        core.shared.read(|s| {
            s.instance(&key).unwrap().messages[&general]
                .items
                .iter()
                .find(|m| m.id == id)
                .map(|m| m.reactions.iter().map(|r| (r.emoji.clone(), r.count, r.me)).collect::<Vec<_>>())
                .unwrap_or_default()
        })
    };
    let react = |core: &Arc<Core>, emoji: &str, on: bool| {
        let (c, key, sid, cid, mid) = (core.clone(), key.clone(), server.id.clone(), general.clone(), id.clone());
        let r = pb::Reaction { emoji: emoji.into(), ..Default::default() };
        wait(core, async move { c.react(&key, &sid, &cid, &mid, r, on).await })
    };
    react(&bob, "👍", true).unwrap();
    assert_eq!(reactions_on(&bob, &id), vec![("👍".to_owned(), 1, true)], "Bob's own shows at once");
    until(&alice, "Bob's reaction", |s| {
        s.instance(&key).unwrap().messages[&general].items.iter().any(|m| m.id == id && m.reactions.len() == 1)
    });
    assert_eq!(reactions_on(&alice, &id), vec![("👍".to_owned(), 1, false)]);
    react(&alice, "👍", true).unwrap();
    until(&bob, "Alice's reaction", |s| {
        s.instance(&key).unwrap().messages[&general].items.iter().any(|m| m.id == id && m.reactions[0].count == 2)
    });
    assert_eq!(reactions_on(&bob, &id), vec![("👍".to_owned(), 2, true)]);
    // Who reacted, the earliest first.
    {
        let (core, key, sid, cid, mid) = (alice.clone(), key.clone(), server.id.clone(), general.clone(), id.clone());
        let emoji = fuwa_desktop::core::reactions::EmojiKey::standard("👍");
        let page = wait(&alice, async move { core.list_reactors(&key, &sid, &cid, &mid, &emoji, "").await }).unwrap();
        assert_eq!(page.users.iter().map(|u| u.id.clone()).collect::<Vec<_>>().len(), 2);
    }
    // An edit carries no reactions, and they stay.
    {
        let (core, key, sid, cid, mid) = (alice.clone(), key.clone(), server.id.clone(), general.clone(), id.clone());
        wait(&alice, async move { core.edit_message(&key, &sid, &cid, &mid, "hey, look (edited twice)").await })
            .unwrap();
    }
    until(&bob, "the second edit", |s| {
        s.instance(&key).unwrap().messages[&general].items.iter().any(|m| m.content == "hey, look (edited twice)")
    });
    assert_eq!(reactions_on(&bob, &id), vec![("👍".to_owned(), 2, true)]);
    // Bob takes his off; Alice, who owns the server, clears the rest.
    react(&bob, "👍", false).unwrap();
    until(&alice, "Bob's reaction gone", |s| {
        s.instance(&key).unwrap().messages[&general].items.iter().any(|m| m.id == id && m.reactions[0].count == 1)
    });
    assert_eq!(reactions_on(&alice, &id), vec![("👍".to_owned(), 1, true)]);
    {
        let (core, key, sid, cid, mid) = (alice.clone(), key.clone(), server.id.clone(), general.clone(), id.clone());
        wait(&alice, async move { core.clear_reactions(&key, &sid, &cid, &mid, None).await }).unwrap();
    }
    until(&bob, "the reactions cleared", |s| {
        s.instance(&key).unwrap().messages[&general].items.iter().any(|m| m.id == id && m.reactions.is_empty())
    });
    // Anything but one emoji is refused, and nothing stays behind.
    assert!(react(&bob, "not an emoji", true).is_err());
    assert!(reactions_on(&bob, &id).is_empty());

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

    // A status picked here is the account's: read back from the instance, it's still there.
    use fuwa_desktop::pb::PresenceStatus;
    assert_eq!(bob.shared.read(|s| s.instance(&key).unwrap().status()), PresenceStatus::Online);
    {
        let (core, key) = (bob.clone(), key.clone());
        wait(&bob, async move { core.set_status(&key, PresenceStatus::Invisible).await }).unwrap();
    }
    bob.shared.instance(&key, |i| i.presence = None);
    {
        let (core, key) = (bob.clone(), key.clone());
        wait(&bob, async move { core.refresh_presence(&key).await });
    }
    assert_eq!(bob.shared.read(|s| s.instance(&key).unwrap().status()), PresenceStatus::Invisible);
    // Another app of Bob's shares his activity, this one reads that, then the
    // other turns sharing off: picking a status here must keep it off.
    let other_app = |settings: pb::PresenceSettings| {
        let api = bob.api(&key).unwrap();
        wait(&bob, async move {
            let req = pb::UpdatePresenceSettingsRequest { settings: Some(settings) };
            fuwa_desktop::rpc!(api.presence(), update_presence_settings(req)).await.unwrap();
        });
    };
    let shared = pb::PresenceSettings {
        status: PresenceStatus::Invisible as i32,
        show_activity: true,
        hidden_server_ids: vec![server.id.clone()],
    };
    other_app(shared.clone());
    {
        let (core, key) = (bob.clone(), key.clone());
        wait(&bob, async move { core.refresh_presence(&key).await });
    }
    assert!(bob.shared.read(|s| s.instance(&key).unwrap().presence.as_ref().unwrap().show_activity));
    other_app(pb::PresenceSettings { show_activity: false, ..shared });
    {
        let (core, key) = (bob.clone(), key.clone());
        wait(&bob, async move { core.set_status(&key, PresenceStatus::Idle).await }).unwrap();
    }
    let saved = {
        let api = bob.api(&key).unwrap();
        wait(&bob, async move {
            fuwa_desktop::rpc!(api.presence(), get_presence_settings(pb::GetPresenceSettingsRequest {}))
                .await
                .unwrap()
                .settings
                .unwrap()
        })
    };
    assert_eq!(saved.status(), PresenceStatus::Idle);
    assert!(!saved.show_activity, "a status picked here turned sharing back on");
    assert_eq!(saved.hidden_server_ids, vec![server.id.clone()]);
    // Who may use the server through MCP: the owner picks only the chosen agents.
    {
        let (core, key, sid) = (alice.clone(), key.clone(), server.id.clone());
        let saved = wait(&alice, async move {
            let access = pb::McpAccess { mode: pb::McpAccessMode::Chosen as i32, agent_ids: vec![] };
            core.set_mcp_access(&key, &sid, access).await.unwrap();
            core.mcp_access(&key, &sid).await.unwrap()
        });
        assert_eq!(saved.mode(), pb::McpAccessMode::Chosen);
    }
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
    bob.set_focus(Some(Focus { instance: key.clone(), channel: general.clone(), thread: None }));
    assert_eq!(bob.shared.read(|s| s.instance(&key).unwrap().unread.get(&general).copied()), None);

    // Alice writes with an emoji from her other server, which Bob isn't in:
    // it goes along with the message, so Bob can draw it.
    let owls = {
        let (core, key) = (alice.clone(), key.clone());
        wait(&alice, async move { core.create_server(&key, "Owl post").await }).unwrap()
    };
    let owl = {
        let (core, key, sid) = (alice.clone(), key.clone(), owls.id.clone());
        wait(&alice, async move { core.add_emoji(&key, &sid, "owl", "image/png", TINY_PNG.to_vec()).await }).unwrap()
    };
    until(&alice, "the owl emoji", |s| s.instance(&key).unwrap().emojis.get(&owls.id).is_some_and(|l| l.len() == 1));
    let hoot = format!("hoot {}", fuwa_desktop::core::emoji::token(&owl));
    {
        let (core, key, sid, cid, text) =
            (alice.clone(), key.clone(), server.id.clone(), general.clone(), hoot.clone());
        wait(&alice, async move { core.send_message(&key, &sid, &cid, &text).await }).unwrap();
    }
    until(&bob, "the owl message", |s| {
        s.instance(&key).unwrap().messages[&general]
            .items
            .iter()
            .any(|m| m.content == hoot && m.emojis.iter().any(|e| e.id == owl.id && !e.url.is_empty()))
    });

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

    // Bob reacts to Alice's message inside the encryption: both devices tally it,
    // and it's no message of its own (nothing new to read, no unread count).
    let secret_seq = alice.shared.read(|s| {
        s.instance(&key).unwrap().dms.items[&conversation].iter().find(|i| i.content == "a secret 🍵").unwrap().seq
    });
    let texts = |core: &Core| {
        core.shared.read(|s| {
            s.instance(&key).unwrap().dms.items[&conversation].iter().filter(|i| i.kind == ItemKind::Text).count()
        })
    };
    let (texts_before, unread_before) =
        (texts(&alice), alice.shared.read(|s| s.instance(&key).unwrap().dms.unread.get(&conversation).copied()));
    {
        let (core, key, id) = (bob.clone(), key.clone(), conversation.clone());
        wait(&bob, async move { core.react_dm(&key, &id, secret_seq, "🍵", true).await }).unwrap();
    }
    let tallied = |core: &Core| {
        core.shared.read(|s| {
            let items = &s.instance(&key).unwrap().dms.items[&conversation];
            let me = s.instance(&key).unwrap().me.clone().unwrap().id;
            let item = items.iter().find(|i| i.seq == secret_seq).unwrap();
            fuwa_desktop::core::reactions::tally(&item.reactions, &me)
                .into_iter()
                .map(|r| (r.emoji, r.count, r.me))
                .collect::<Vec<_>>()
        })
    };
    let (k2, c2) = (key.clone(), conversation.clone());
    until(&alice, "Bob's encrypted reaction", move |s| {
        s.instance(&k2).unwrap().dms.items[&c2].iter().any(|i| i.seq == secret_seq && !i.reactions.is_empty())
    });
    assert_eq!(tallied(&alice), vec![("🍵".to_owned(), 1, false)]);
    assert_eq!(tallied(&bob), vec![("🍵".to_owned(), 1, true)]);
    assert_eq!(texts(&alice), texts_before);
    assert_eq!(alice.shared.read(|s| s.instance(&key).unwrap().dms.unread.get(&conversation).copied()), unread_before);
    // Taken off again: the latest says.
    {
        let (core, key, id) = (bob.clone(), key.clone(), conversation.clone());
        wait(&bob, async move { core.react_dm(&key, &id, secret_seq, "🍵", false).await }).unwrap();
    }
    let (k2, c2) = (key.clone(), conversation.clone());
    until(&alice, "Bob's reaction taken off", move |s| {
        s.instance(&k2).unwrap().dms.items[&c2]
            .iter()
            .any(|i| i.seq == secret_seq && i.reactions.iter().all(|r| r.removed))
    });
    assert!(tallied(&alice).is_empty());
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

    // Alice records a voice message; Bob fetches it, opens it and can play it.
    {
        let mut encoder = opus::Encoder::new(48_000, opus::Channels::Mono, opus::Application::Voip).unwrap();
        let mut packets = Vec::new();
        let mut buf = vec![0u8; 4000];
        for n in 0..60 {
            let frame: Vec<f32> = (0..960).map(|i| ((n * 960 + i) as f32 * 0.03).sin() * 0.4).collect();
            let len = encoder.encode_float(&frame, &mut buf).unwrap();
            packets.push(buf[..len].to_vec());
        }
        let clip = voice_notes::Clip {
            ogg: Arc::new(voice_notes::write_ogg(&packets, encoder.get_lookahead().unwrap() as u16)),
            duration_ms: 1200,
            waveform: vec![40, 200, 255, 90],
        };
        let (core, key, id) = (alice.clone(), key.clone(), conversation.clone());
        wait(&alice, async move { core.send_voice(&key, &id, &clip, 0).await }).unwrap();
    }
    let voiced = |s: &Store| {
        s.instance(&key)
            .unwrap()
            .dms
            .items
            .get(&conversation)
            .and_then(|items| items.iter().find_map(|i| i.voice.clone()))
    };
    until(&bob, "a voice message", |s| voiced(s).is_some());
    let file = bob.shared.read(voiced).unwrap();
    assert_eq!((file.duration_ms, file.waveform.clone()), (1200, vec![40, 200, 255, 90]));
    // Opened from what the instance holds, not from what Alice's copy of the app kept.
    voice_notes::forget_opened();
    let sound = {
        let (core, key) = (bob.clone(), key.clone());
        wait(&bob, async move { core.voice_sound(&key, &file).await }).unwrap()
    };
    assert!((60 * 960 - 1000..=60 * 960).contains(&sound.len()));
    // What the instance holds is sealed: none of the recording shows in it.
    for path in walk(data.path()) {
        let bytes = std::fs::read(path).unwrap_or_default();
        assert!(!bytes.windows(8).any(|w| w == b"OpusHead"));
    }

    // Alice sends a picture and a file; Bob sees them and saves the file as it was.
    {
        let (core, key, sid, cid) = (alice.clone(), key.clone(), server.id.clone(), general.clone());
        wait(&alice, async move { core.load_messages(&key, &sid, &cid, false).await }).unwrap();
    }
    let files = tempfile::tempdir().unwrap();
    let picture = files.path().join("tiny.png");
    let notes = files.path().join("notes.txt");
    std::fs::write(&picture, TINY_PNG).unwrap();
    std::fs::write(&notes, b"bring snacks").unwrap();
    {
        let (core, key, sid, cid) = (alice.clone(), key.clone(), server.id.clone(), general.clone());
        let (picture, notes) = (picture.clone(), notes.clone());
        wait(&alice, async move {
            let a = core.upload_attachment(&key, &sid, &picture).await?;
            let b = core.upload_attachment(&key, &sid, &notes).await?;
            assert_eq!((a.width, a.height), (1, 1));
            core.send_message_with(&key, &sid, &cid, "files!", vec![a, b]).await
        })
        .unwrap();
    }
    let sent = |s: &Store| {
        s.instance(&key).unwrap().messages.get(&general).and_then(|m| {
            m.items.iter().find(|m| m.content == "files!" && m.attachments.len() == 2).map(|m| m.attachments.clone())
        })
    };
    until(&bob, "the files", |s| sent(s).is_some());
    let got = bob.shared.read(sent).unwrap();
    assert_eq!(
        fuwa_desktop::core::attachments::look_of(&got[0].content_type),
        fuwa_desktop::core::attachments::Look::Picture
    );
    let saved = files.path().join("saved.txt");
    {
        let (core, key, url, saved) = (bob.clone(), key.clone(), got[1].url.clone(), saved.clone());
        wait(&bob, async move { core.save_attachment(&key, &url, 12, &saved).await }).unwrap();
    }
    assert_eq!(std::fs::read(&saved).unwrap(), b"bring snacks");
    // Nothing half-written is left beside it.
    assert!(!files.path().join("saved.txt.part").exists());
    // Only the instance's own files are fetched, whatever the link says.
    {
        let (core, key, url) = (bob.clone(), key.clone(), got[1].url.replace("/media/", "/api/"));
        let away = files.path().join("away.txt");
        assert!(wait(&bob, async move { core.save_attachment(&key, &url, 12, &away).await }).is_err());
        assert!(!files.path().join("away.txt").exists());
    }
    assert_eq!(got[1].filename, "notes.txt");

    // Bob finds Alice's files by a word in a file's name, sees where it
    // matched, and opens the message from the result.
    let alice_name = alice.shared.read(|s| s.instance(&key).unwrap().me.clone().unwrap().username);
    let query = format!("from:{alice_name} notes has:file");
    let request = bob
        .shared
        .read(|s| search::request_for(s.instance(&key).unwrap(), &server.id, &query, search::today()))
        .unwrap()
        .unwrap();
    assert_eq!(request.query, "notes");
    assert_eq!(request.has, vec![pb::SearchHas::File as i32]);
    let mut found = None;
    for _ in 0..100 {
        let (core, key, request) = (bob.clone(), key.clone(), request.clone());
        let page = wait(&bob, async move { core.search_page(&key, request, "").await }).unwrap();
        if let Some(hit) = page.results.into_iter().find(|r| r.message.as_ref().is_some_and(|m| m.content == "files!"))
        {
            found = Some(hit);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let hit = found.expect("the search finds the message");
    let message = hit.message.unwrap();
    {
        let (core, key, sid, cid, id) =
            (bob.clone(), key.clone(), server.id.clone(), general.clone(), message.id.clone());
        assert!(wait(&bob, async move { core.find_message(&key, &sid, &cid, &id).await }));
    }
    // A name that's no one here says so, and nothing is asked of the instance.
    let nobody = bob
        .shared
        .read(|s| search::request_for(s.instance(&key).unwrap(), &server.id, "from:nobody-here cake", search::today()));
    assert_eq!(nobody, Err("No member here is called nobody-here".into()));

    // Bob answers Alice's files in a thread: Alice's channel shows only the
    // count, the reply lives in the thread. Following it counts what's new,
    // a reply also sent to the channel shows in both, a lock keeps Bob out,
    // and deleting the message takes the thread with it.
    let parent = message.id.clone();
    let reply = |core: &Arc<Core>, text: &'static str, also: bool| {
        let (c, key, sid, cid) = (core.clone(), key.clone(), server.id.clone(), general.clone());
        let target = threads::ThreadTarget { thread_id: parent.clone(), also_to_channel: also };
        wait(core, async move { c.send_reply(&key, &sid, &cid, text, target).await })
    };
    let summary = |s: &Store| {
        s.instance(&key)
            .unwrap()
            .messages
            .get(&general)
            .and_then(|m| m.items.iter().find(|m| m.id == parent).and_then(|m| m.thread.clone()))
    };
    reply(&bob, "on it", false).unwrap();
    until(&alice, "the thread's count", |s| summary(s).is_some_and(|t| t.reply_count == 1));
    assert!(
        alice.shared.read(|s| !s.instance(&key).unwrap().messages[&general].items.iter().any(|m| m.content == "on it"))
    );
    {
        let (core, key, sid, cid, id) =
            (alice.clone(), key.clone(), server.id.clone(), general.clone(), parent.clone());
        wait(&alice, async move {
            core.load_thread(&key, &sid, &cid, &id, false).await?;
            core.follow_thread(&key, &sid, &cid, &id, true).await
        })
        .unwrap();
    }
    alice.shared.read(|s| {
        let i = s.instance(&key).unwrap();
        assert_eq!(i.thread_parents[&parent].content, "files!");
        assert_eq!(i.messages[&threads::thread_key(&parent)].items[0].content, "on it");
        assert_eq!(threads::follows(i, &server.id, &parent), Some(true));
    });
    reply(&bob, "everyone, look", true).unwrap();
    until(&alice, "the reply in both places", |s| {
        let i = s.instance(&key).unwrap();
        let shown = |at: &str| i.messages[at].items.iter().any(|m| m.content == "everyone, look");
        shown(&general) && shown(&threads::thread_key(&parent)) && i.thread_unread.get(&parent) == Some(&1)
    });
    // "Mark as read" on the channel clears the followed thread under it too.
    assert_eq!(alice.mark_read(&key, std::slice::from_ref(&general)), 1);
    alice.shared.read(|s| {
        let i = s.instance(&key).unwrap();
        assert!(!i.thread_unread.contains_key(&parent) && !i.unread.contains_key(&general));
    });
    // "Duplicate channel": a copy with its name, kind and category, then gone again.
    {
        let original = alice.shared.read(|s| s.instance(&key).unwrap().channel(&server.id, &general).cloned()).unwrap();
        let (core, k, sid) = (alice.clone(), key.clone(), server.id.clone());
        let of = original.clone();
        let copy = wait(&alice, async move { core.duplicate_channel(&k, &sid, &of).await }).unwrap();
        assert!(copy.id != original.id && copy.name == original.name && copy.r#type == original.r#type);
        assert!(alice.shared.read(|s| s.instance(&key).unwrap().channel(&server.id, &copy.id).is_some()));
        let (core, k, sid) = (alice.clone(), key.clone(), server.id.clone());
        wait(&alice, async move { core.delete_channel(&k, &sid, &copy.id).await }).unwrap();
    }
    {
        let (core, key, sid, cid) = (alice.clone(), key.clone(), server.id.clone(), general.clone());
        let listed = wait(&alice, async move { core.list_threads(&key, &sid, &cid, "", false, "").await }).unwrap();
        assert_eq!(
            listed.threads.iter().find(|t| t.id == parent).and_then(|t| t.thread.as_ref()).unwrap().reply_count,
            2
        );
    }
    {
        let (core, key, sid, cid, id) =
            (alice.clone(), key.clone(), server.id.clone(), general.clone(), parent.clone());
        wait(&alice, async move { core.lock_thread(&key, &sid, &cid, &id, true).await }).unwrap();
    }
    until(&bob, "the lock", |s| summary(s).is_some_and(|t| t.locked));
    assert!(reply(&bob, "let me in", false).is_err());
    {
        let (core, key, sid, cid, id) =
            (alice.clone(), key.clone(), server.id.clone(), general.clone(), parent.clone());
        wait(&alice, async move { core.delete_message(&key, &sid, &cid, &id).await }).unwrap();
    }
    alice.shared.read(|s| {
        let i = s.instance(&key).unwrap();
        assert!(!i.thread_parents.contains_key(&parent));
        assert!(!i.messages.contains_key(&threads::thread_key(&parent)));
        assert!(!i.thread_unread.contains_key(&parent));
    });

    // Alice asks a question; Bob votes, Alice sees the count live and who
    // voted, then ends it. Bob's own pick stays with him.
    {
        let (core, key, sid, cid) = (alice.clone(), key.clone(), server.id.clone(), general.clone());
        wait(&alice, async move { core.load_messages(&key, &sid, &cid, false).await }).unwrap();
    }
    {
        let draft = polls::Draft {
            question: "Snacks tonight?".into(),
            answers: vec![("Chips".into(), "🥔".into()), ("Fruit".into(), String::new())],
            ..Default::default()
        };
        let (core, key, sid, cid) = (alice.clone(), key.clone(), server.id.clone(), general.clone());
        wait(&alice, async move { core.send_poll(&key, &sid, &cid, &draft).await }).unwrap();
    }
    let poll_of = |s: &Store| {
        s.instance(&key).unwrap().messages[&general].items.iter().find_map(|m| Some((m.id.clone(), m.poll.clone()?)))
    };
    until(&bob, "the poll", |s| poll_of(s).is_some());
    let (poll_id, poll) = bob.shared.read(|s| poll_of(s).unwrap());
    assert_eq!(
        (poll.question.as_str(), poll.answers.len(), poll.answers[0].emoji.as_str()),
        ("Snacks tonight?", 2, "🥔")
    );
    let fruit = poll.answers[1].id;
    {
        let (core, key, sid, cid, mid) =
            (bob.clone(), key.clone(), server.id.clone(), general.clone(), poll_id.clone());
        wait(&bob, async move { core.vote_poll(&key, &sid, &cid, &mid, vec![fruit]).await }).unwrap();
    }
    assert_eq!(bob.shared.read(|s| poll_of(s).unwrap().1.my_answer_ids), vec![fruit]);
    until(&alice, "Bob's vote", |s| poll_of(s).is_some_and(|(_, p)| p.voters == 1 && p.answers[1].votes == 1));
    assert!(alice.shared.read(|s| poll_of(s).unwrap().1.my_answer_ids.is_empty()));
    let (voters, more) = {
        let (core, key, sid, mid) = (alice.clone(), key.clone(), server.id.clone(), poll_id.clone());
        wait(&alice, async move { core.poll_voters(&key, &sid, &mid, fruit, "").await }).unwrap()
    };
    assert_eq!((voters.iter().map(|u| u.id.clone()).collect::<Vec<_>>(), more), (vec![bob_id.clone()], false));
    {
        let (core, key, sid, cid, mid) =
            (alice.clone(), key.clone(), server.id.clone(), general.clone(), poll_id.clone());
        wait(&alice, async move { core.end_poll(&key, &sid, &cid, &mid).await }).unwrap();
    }
    until(&bob, "the poll ending", |s| poll_of(s).is_some_and(|(_, p)| p.ended_at.is_some()));
    assert_eq!(bob.shared.read(|s| poll_of(s).unwrap().1.my_answer_ids), vec![fruit]);

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
fn updates_are_found_through_the_instance_but_need_a_signature() {
    // SAFETY: set before anything reads it.
    unsafe { std::env::set_var("FUWA_DESKTOP_KEYCHAIN", "off") };
    let data = tempfile::tempdir().unwrap();
    let instance = start_instance(data.path());
    let home = tempfile::tempdir().unwrap();
    let app = Core::start(Paths::under(home.path())).unwrap();
    app.add_instance(&instance.url, None);

    // The instance knows of no release yet: nothing to show.
    {
        let core = app.clone();
        wait(&app, async move { core.check_for_update(true).await });
    }
    assert!(matches!(updates::status(), updates::Status::Failed { .. }), "{:?}", updates::status());

    // A newer release whose signature is from no key this app trusts is shown, never installed.
    instance.app.releases.set(fuwa_server::releases::Latest {
        version: "999.0.0".into(),
        published_at: "2026-10-04T01:00:00Z".into(),
        notes: "## New\n* shiny".into(),
        page: "https://github.com/waifu-devs/fuwa/releases/tag/v999.0.0".into(),
        sums: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  fuwa-desktop-999.0.0-x86_64-linux\n"
            .into(),
        signature: "JSwu6VQxqqCDKaTZYOnvyj4afvUkkLyz6FKWw8ykTUwEU39vLaUv9UT4eMqP41oL+zvvr1YWMls1pYGmlFDjDg==".into(),
        files: vec![fuwa_server::releases::File { name: "fuwa-desktop-999.0.0-x86_64-linux".into(), size: 3 }],
    });
    {
        let core = app.clone();
        wait(&app, async move { core.check_for_update(true).await });
    }
    match updates::status() {
        // No key is built in yet, or the release's signature isn't from one.
        updates::Status::Available { release, why: updates::Manual::Unsigned } => {
            assert_eq!(release.version, "999.0.0");
            assert_eq!(release.notes, "## New\n* shiny");
        }
        updates::Status::Failed { what } => assert!(what.contains("signature"), "{what}"),
        other => panic!("{other:?}"),
    }

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

#[test]
fn servers_share_a_channel() {
    // SAFETY: set before anything reads it.
    unsafe { std::env::set_var("FUWA_DESKTOP_KEYCHAIN", "off") };
    let data = tempfile::tempdir().unwrap();
    let instance = start_instance(data.path());
    let (home_a, home_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let alice = Core::start(Paths::under(home_a.path())).unwrap();
    let bob = Core::start(Paths::under(home_b.path())).unwrap();
    let key = {
        let (core, url) = (alice.clone(), instance.url.clone());
        wait(&alice, async move { core.sign_up(&url, "alice", "correct horse battery", "Alice").await }).unwrap()
    };
    {
        let (core, url) = (bob.clone(), instance.url.clone());
        wait(&bob, async move { core.sign_up(&url, "bob", "correct horse battery", "Bob").await }).unwrap();
    }
    for core in [&alice, &bob] {
        until(core, "signed in and live", |s| {
            s.instance(&key).is_some_and(|i| i.me.is_some() && i.connection == Connection::Live)
        });
    }
    // Each has a server of their own: Alice's is the channel's home, Bob's shows it.
    let make = |core: &Arc<Core>, name: &'static str| {
        let (c, k) = (core.clone(), key.clone());
        let server = wait(core, async move { c.create_server(&k, name).await }).unwrap();
        until(core, "the new server", |s| s.instance(&key).is_some_and(|i| i.synced.contains(&server.id)));
        server
    };
    let (tea, owls) = (make(&alice, "Tea house"), make(&bob, "Night owls"));
    let general = alice
        .shared
        .read(|s| s.instance(&key).unwrap().channels[&tea.id].iter().find(|c| c.r#type == 1).unwrap().id.clone());

    // Alice makes a code; pasted with words around it, Bob's app finds it and shows where it leads.
    let (core, k, sid, cid) = (alice.clone(), key.clone(), tea.id.clone(), general.clone());
    let code = wait(&alice, async move { core.create_share_code(&k, &sid, &cid, false).await }).unwrap().code;
    let pasted = format!("here you go: {code} (works for a week)");
    assert_eq!(shared::find_share_code(&pasted), code);
    assert_eq!(shared::share_code_instance(&code), "", "a code for this instance only");
    let (core, k, sid, c) = (bob.clone(), key.clone(), owls.id.clone(), code.clone());
    let preview = wait(&bob, async move { core.preview_share(&k, &sid, &c).await }).unwrap();
    assert_eq!(preview.home_server.unwrap().name, "Tea house");
    let (core, k, sid, c) = (bob.clone(), key.clone(), owls.id.clone(), code.clone());
    wait(&bob, async move { core.accept_share(&k, &sid, &c, "tea-talk", "").await }).unwrap();

    // Alice sees the request waiting, live, and approves it.
    let (core, k, sid) = (alice.clone(), key.clone(), tea.id.clone());
    wait(&alice, async move { core.list_connections(&k, &sid).await }).unwrap();
    until(&alice, "Bob's request", |s| {
        s.instance(&key).unwrap().shared[&tea.id].connections.iter().any(|c| c.home && shared::waiting(c))
    });
    let connection = alice.shared.read(|s| s.instance(&key).unwrap().shared[&tea.id].connections[0].id.clone());
    let (core, k, sid, id) = (alice.clone(), key.clone(), tea.id.clone(), connection.clone());
    wait(&alice, async move { core.review_share(&k, &sid, &id, true).await }).unwrap();
    until(&alice, "the connection, connected", |s| {
        s.instance(&key).unwrap().shared[&tea.id].connections.iter().any(|c| c.id == connection && !shared::waiting(c))
    });

    // The channel shows up in Bob's server under his name for it, marked as from Alice's.
    until(&bob, "the shown channel", |s| {
        s.instance(&key).unwrap().channels[&owls.id].iter().any(|c| c.name == "tea-talk" && c.shared.is_some())
    });
    let shown = bob
        .shared
        .read(|s| s.instance(&key).unwrap().channels[&owls.id].iter().find(|c| c.name == "tea-talk").unwrap().clone());
    assert_eq!(shared::shared_label(&shown).unwrap().text, "Shared from Tea house");

    // Bob writes there and edits it, naming the channel his server doesn't hold.
    let (core, k, sid, cid) = (bob.clone(), key.clone(), owls.id.clone(), shown.id.clone());
    wait(&bob, async move { core.load_messages(&k, &sid, &cid, false).await }).unwrap();
    let (core, k, sid, cid) = (bob.clone(), key.clone(), owls.id.clone(), shown.id.clone());
    wait(&bob, async move { core.send_message(&k, &sid, &cid, "hi from the owls").await }).unwrap();
    until(&bob, "Bob's message", |s| {
        s.instance(&key).unwrap().messages[&shown.id].items.iter().any(|m| m.content == "hi from the owls")
    });
    let id = bob.shared.read(|s| s.instance(&key).unwrap().messages[&shown.id].items.last().unwrap().id.clone());
    let (core, k, sid, cid, mid) = (bob.clone(), key.clone(), owls.id.clone(), shown.id.clone(), id.clone());
    wait(&bob, async move { core.edit_message(&k, &sid, &cid, &mid, "hi from the owls!").await }).unwrap();

    // At the home it's tagged with Bob's server, and his name is known though he isn't a member.
    let (core, k, sid, cid) = (alice.clone(), key.clone(), tea.id.clone(), general.clone());
    wait(&alice, async move { core.load_messages(&k, &sid, &cid, false).await }).unwrap();
    until(&alice, "Bob's edited message", |s| {
        s.instance(&key).unwrap().messages[&general].items.iter().any(|m| m.content == "hi from the owls!")
    });
    let (from, name) = alice.shared.read(|s| {
        let i = s.instance(&key).unwrap();
        let m = i.messages[&general].items.iter().find(|m| m.id == id).unwrap();
        (shared::foreign_server(m, &tea.id).map(|f| f.name.clone()), i.display_name(Some(&tea.id), &m.author_id))
    });
    assert_eq!(from.as_deref(), Some("Night owls"));
    assert_eq!(name, "Bob");

    // Alice lets Bob's server react there; Bob reacts from his side and Alice sees it live,
    // and her own reaction reaches him.
    let (core, k, sid, id2) = (alice.clone(), key.clone(), tea.id.clone(), connection.clone());
    let allowed = vec![pb::Permission::SendMessages, pb::Permission::AddReactions];
    wait(&alice, async move { core.update_connection(&k, &sid, &id2, allowed).await }).unwrap();
    let react = |core: &Arc<Core>, sid: &str, cid: &str, on: bool| {
        let (c, k, sid, cid, mid) = (core.clone(), key.clone(), sid.to_owned(), cid.to_owned(), id.clone());
        let r = pb::Reaction { emoji: "🦉".into(), ..Default::default() };
        wait(core, async move { c.react(&k, &sid, &cid, &mid, r, on).await })
    };
    react(&bob, &owls.id, &shown.id, true).unwrap();
    let owl_count = |core: &Core, at: &str| {
        core.shared.read(|s| {
            s.instance(&key).unwrap().messages[at]
                .items
                .iter()
                .find(|m| m.id == id)
                .and_then(|m| m.reactions.first().map(|r| (r.count, r.me)))
        })
    };
    let (k2, g2, i2) = (key.clone(), general.clone(), id.clone());
    until(&alice, "Bob's reaction from the other server", move |s| {
        s.instance(&k2).unwrap().messages[&g2].items.iter().any(|m| m.id == i2 && !m.reactions.is_empty())
    });
    assert_eq!(owl_count(&alice, &general), Some((1, false)));
    react(&alice, &tea.id, &general, true).unwrap();
    let (k2, s2, i2) = (key.clone(), shown.id.clone(), id.clone());
    until(&bob, "Alice's reaction at the guest", move |s| {
        s.instance(&k2).unwrap().messages[&s2]
            .items
            .iter()
            .any(|m| m.id == i2 && m.reactions.first().is_some_and(|r| r.count == 2))
    });
    assert_eq!(owl_count(&bob, &shown.id), Some((2, true)));
    // Who reacted, asked from the guest's side.
    let (core, k, sid, cid, mid) = (bob.clone(), key.clone(), owls.id.clone(), shown.id.clone(), id.clone());
    let emoji = fuwa_desktop::core::reactions::EmojiKey::standard("🦉");
    let page = wait(&bob, async move { core.list_reactors(&k, &sid, &cid, &mid, &emoji, "").await }).unwrap();
    assert_eq!(page.users.len(), 2);
    // Only the home clears.
    let (core, k, sid, cid, mid) = (bob.clone(), key.clone(), owls.id.clone(), shown.id.clone(), id.clone());
    assert!(wait(&bob, async move { core.clear_reactions(&k, &sid, &cid, &mid, None).await }).is_err());

    // Alice keeps Bob out of the channel; he's on her list of people kept out.
    let bob_id = bob.shared.read(|s| s.instance(&key).unwrap().me.clone().unwrap().id);
    let (core, k, sid, cid, uid) = (alice.clone(), key.clone(), tea.id.clone(), general.clone(), bob_id.clone());
    wait(&alice, async move { core.block_from_channel(&k, &sid, &cid, &uid, true).await }).unwrap();
    until(&alice, "Bob kept out", |s| {
        s.instance(&key).unwrap().shared[&tea.id].blocks.iter().any(|b| b.user.as_ref().is_some_and(|u| u.id == bob_id))
    });

    // Bob's server lets the channel go; it leaves his sidebar, and Alice's list.
    let (core, k, sid, id) = (bob.clone(), key.clone(), owls.id.clone(), connection.clone());
    wait(&bob, async move { core.disconnect_shared(&k, &sid, &id).await }).unwrap();
    until(&bob, "the channel gone", |s| s.instance(&key).unwrap().channels[&owls.id].iter().all(|c| c.id != shown.id));
    until(&alice, "the connection gone", |s| s.instance(&key).unwrap().shared[&tea.id].connections.is_empty());

    instance.app.shutdown.cancel();
    drop(instance.runtime);
}

#[test]
fn secure_channels_stay_between_devices() {
    // SAFETY: set before anything reads it.
    unsafe { std::env::set_var("FUWA_DESKTOP_KEYCHAIN", "off") };
    let data = tempfile::tempdir().unwrap();
    let instance = start_instance(data.path());
    let homes = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    let [alice, bob, carol] = [0, 1, 2].map(|n| Core::start(Paths::under(homes[n].path())).unwrap());
    let url = instance.url.clone();
    let mut key = String::new();
    for (core, name) in [(&alice, "alice"), (&bob, "bob"), (&carol, "carol")] {
        let (c, url) = (core.clone(), url.clone());
        key = wait(core, async move { c.sign_up(&url, name, "correct horse battery", name).await }).unwrap();
        until(core, "signed in", |s| s.instance(&key).is_some_and(|i| i.dms.status == DmStatus::Ready));
    }

    // Alice makes a server with a secure channel; Bob joins.
    let server = {
        let (core, key) = (alice.clone(), key.clone());
        wait(&alice, async move { core.create_server(&key, "Hideout").await }).unwrap()
    };
    let sid = server.id.clone();
    let channel = {
        let (core, key, sid) = (alice.clone(), key.clone(), sid.clone());
        wait(&alice, async move { core.create_channel(&key, &sid, "plans", pb::ChannelType::Secure, "").await })
            .unwrap()
    };
    let cid = channel.id.clone();
    let join = |core: &Arc<Core>| {
        let invite = {
            let (a, key, sid) = (alice.clone(), key.clone(), sid.clone());
            wait(&alice, async move { a.create_invite(&key, &sid).await }).unwrap()
        };
        let (c, key2) = (core.clone(), key.clone());
        wait(core, async move { c.join_by_invite(&key2, &invite).await }).unwrap();
        until(core, "the server's channels", |s| s.instance(&key).is_some_and(|i| i.synced.contains(&sid)));
    };
    join(&bob);
    until(&alice, "Bob in the server", |s| s.instance(&key).unwrap().members[&sid].len() == 2);

    // Alice opens it, which starts its group with Bob's device in it, and writes.
    let send = |core: &Arc<Core>, text: &str| {
        let (c, key, cid, text) = (core.clone(), key.clone(), cid.clone(), text.to_owned());
        wait(core, async move { c.send_dm(&key, &cid, Content::Text { text, reply_to: 0 }).await }).unwrap();
    };
    {
        let (c, key, sid, cid) = (alice.clone(), key.clone(), sid.clone(), cid.clone());
        wait(&alice, async move { c.prepare_secure_channel(&key, &sid, &cid, true).await }).unwrap();
    }
    send(&alice, "meet at the old mill");
    let said = |core: &Core, text: &'static str| {
        let (key, cid) = (key.clone(), cid.clone());
        until(core, text, move |s| {
            s.instance(&key)
                .unwrap()
                .dms
                .items
                .get(&cid)
                .is_some_and(|items| items.iter().any(|i| i.kind == ItemKind::Text && i.content == text))
        });
    };
    said(&bob, "meet at the old mill");
    // Signed by the device that sent it, so it can be passed on later; and counted unread with the server.
    assert!(bob.shared.read(|s| {
        let i = s.instance(&key).unwrap();
        i.dms.items[&cid].iter().any(|it| it.content == "meet at the old mill" && it.signed.is_some())
            && i.unread.get(&cid).copied() == Some(1)
    }));
    send(&bob, "bringing snacks");
    said(&alice, "bringing snacks");

    // With history sharing on, someone who joins later gets what was said, passed on by a member's device.
    {
        let (c, key, sid, cid) = (alice.clone(), key.clone(), sid.clone(), cid.clone());
        wait(&alice, async move { c.set_secure_history(&key, &sid, &cid, true).await }).unwrap();
    }
    until(&bob, "history sharing turned on", |s| {
        s.instance(&key).unwrap().dms.items[&cid].iter().any(|i| i.kind == ItemKind::Setting && i.content == "on")
    });
    send(&alice, "carol is coming too");
    said(&bob, "carol is coming too");
    // Bob reacts, signed like a text; Alice tallies it, shown on Bob's own device at once.
    let coming = bob.shared.read(|s| {
        s.instance(&key).unwrap().dms.items[&cid].iter().find(|i| i.content == "carol is coming too").unwrap().seq
    });
    {
        let (c, key, cid) = (bob.clone(), key.clone(), cid.clone());
        wait(&bob, async move { c.react_dm(&key, &cid, coming, "🥨", true).await }).unwrap();
    }
    let reacted = |core: &Core, what: &str| {
        let (key, cid) = (key.clone(), cid.clone());
        until(core, what, move |s| {
            s.instance(&key).unwrap().dms.items[&cid].iter().any(|i| {
                i.seq == coming && i.reactions.iter().any(|m| m.emoji == "🥨" && !m.removed && m.signed.is_some())
            })
        });
    };
    reacted(&bob, "Bob's own reaction, read back");
    reacted(&alice, "Bob's reaction");
    join(&carol);
    said(&carol, "carol is coming too");
    // Passed on with the history, still signed.
    reacted(&carol, "Bob's reaction in the shared history");
    // Only what was said since sharing was turned on is passed on.
    assert!(carol.shared.read(|s| {
        let texts: Vec<_> =
            s.instance(&key).unwrap().dms.items[&cid].iter().filter(|i| i.kind == ItemKind::Text).collect();
        texts.len() == 1 && texts.iter().all(|i| !i.shared_by.is_empty())
    }));
    send(&carol, "hi all");
    said(&alice, "hi all");
    said(&bob, "hi all");

    // Starting the encryption over: everyone notes it, and writing works again.
    {
        let (c, key, sid, cid) = (alice.clone(), key.clone(), sid.clone(), cid.clone());
        wait(&alice, async move { c.reset_secure_channel(&key, &sid, &cid, true).await }).unwrap();
    }
    until(&bob, "the reset", |s| s.instance(&key).unwrap().dms.items[&cid].iter().any(|i| i.kind == ItemKind::Reset));
    send(&alice, "fresh keys");
    said(&bob, "fresh keys");
    said(&carol, "fresh keys");

    // The instance only ever kept ciphertext.
    let mut found = false;
    for entry in walk(data.path()) {
        let bytes = std::fs::read(&entry).unwrap_or_default();
        found |= bytes.windows(b"old mill".len()).any(|w| w == b"old mill");
    }
    assert!(!found, "the instance kept a secure channel's words");

    instance.app.shutdown.cancel();
    drop(instance.runtime);
}

#[test]
fn friends_follow_live_and_blocks_hide_conversations() {
    use fuwa_desktop::core::friends::{self, BLOCKED, FRIEND, FriendsStatus, INCOMING, OUTGOING};
    // SAFETY: set before anything reads it.
    unsafe { std::env::set_var("FUWA_DESKTOP_KEYCHAIN", "off") };
    let data = tempfile::tempdir().unwrap();
    let instance = start_instance(data.path());
    let (home_a, home_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let alice = Core::start(Paths::under(home_a.path())).unwrap();
    let bob = Core::start(Paths::under(home_b.path())).unwrap();
    let key = {
        let (core, url) = (alice.clone(), instance.url.clone());
        wait(&alice, async move { core.sign_up(&url, "alice", "correct horse battery", "Alice").await }).unwrap()
    };
    {
        let (core, url) = (bob.clone(), instance.url.clone());
        wait(&bob, async move { core.sign_up(&url, "bob", "correct horse battery", "Bob").await }).unwrap();
    }
    for core in [&alice, &bob] {
        until(core, "friends followed", |s| {
            s.instance(&key)
                .is_some_and(|i| i.friends.status == FriendsStatus::Ready && i.dms.status == DmStatus::Ready)
        });
    }
    let me = |core: &Core| core.shared.read(|s| s.instance(&key).unwrap().me.clone().unwrap());
    let (a, b) = (me(&alice), me(&bob));
    let state = |core: &Core, id: &str| {
        core.shared.read(|s| friends::state_with(&s.instance(&key).unwrap().friends.list, id, now_ms()))
    };

    // Alice asks Bob by username, typed loosely; Bob hears of it live, with a notice.
    let mut notices = bob.take_notices().unwrap();
    let sent = {
        let (core, key, name) = (alice.clone(), key.clone(), format!("  @{}", b.username.to_uppercase()));
        wait(&alice, async move { core.send_friend_request(&key, "", &name).await }).unwrap()
    };
    assert_eq!(sent.state, OUTGOING);
    assert_eq!(state(&alice, &b.id), OUTGOING);
    until(&bob, "Alice's request", |s| {
        friends::waiting_for_you(&s.instance(&key).unwrap().friends.list, now_ms()) == 1
    });
    assert_eq!(state(&bob, &a.id), INCOMING);
    let deadline = Instant::now() + Duration::from_secs(5);
    let notice = loop {
        match notices.try_recv() {
            Ok(notice) => break notice,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => panic!("no notice for the request"),
        }
    };
    assert!(matches!(notice, Notice::Friend { ref title, .. } if title == "Alice wants to be friends"), "{notice:?}");

    // Bob takes it: friends on both sides, and each sees the other online.
    {
        let (core, key, id) = (bob.clone(), key.clone(), a.id.clone());
        wait(&bob, async move { core.accept_friend(&key, &id).await }).unwrap();
    }
    assert_eq!(state(&bob, &a.id), FRIEND);
    until(&alice, "Bob a friend, online", |s| {
        s.instance(&key).unwrap().friends.list.iter().any(|f| f.state == FRIEND && f.online)
    });
    let relation = {
        let (core, key, id) = (alice.clone(), key.clone(), b.id.clone());
        wait(&alice, async move { core.relationship(&key, &id).await }).unwrap()
    };
    assert_eq!(relation.state, FRIEND);
    assert!(relation.may_message, "friends may write without a server in common");

    // Settings follow the account: saved on one copy, read on the next list.
    {
        let (core, key) = (bob.clone(), key.clone());
        let settings =
            pb::FriendSettings { direct_messages_from: pb::DirectMessagesFrom::Friends as i32, ..Default::default() };
        wait(&bob, async move { core.save_friend_settings(&key, settings).await }).unwrap();
    }
    assert_eq!(
        bob.shared.read(|s| s.instance(&key).unwrap().friends.settings.unwrap().direct_messages_from),
        pb::DirectMessagesFrom::Friends as i32
    );

    // Friends can talk privately; blocking hides the conversation and unfriends.
    let conversation = {
        let (core, key, id) = (alice.clone(), key.clone(), b.id.clone());
        wait(&alice, async move { core.open_conversation(&key, &id).await }).unwrap()
    };
    let hidden = |core: &Core| {
        core.shared.read(|s| {
            let i = s.instance(&key).unwrap();
            i.dms.conversations.iter().find(|c| c.id == conversation).is_some_and(|c| friends::hidden(i, c))
        })
    };
    until(&alice, "the conversation", |s| {
        s.instance(&key).unwrap().dms.conversations.iter().any(|c| c.id == conversation)
    });
    assert!(!hidden(&alice));
    {
        let (core, key, id) = (alice.clone(), key.clone(), b.id.clone());
        wait(&alice, async move { core.block_user(&key, &id).await }).unwrap();
    }
    assert_eq!(state(&alice, &b.id), BLOCKED);
    assert!(hidden(&alice));
    until(&bob, "Alice gone from Bob's list", |s| {
        friends::state_with(&s.instance(&key).unwrap().friends.list, &a.id, now_ms()) == 0
    });
    {
        let (core, key, id) = (alice.clone(), key.clone(), b.id.clone());
        wait(&alice, async move { core.unblock_user(&key, &id).await }).unwrap();
    }
    assert_eq!(state(&alice, &b.id), 0);
    assert!(!hidden(&alice));
    instance.app.shutdown.cancel();
    drop(instance.runtime);
}

#[test]
fn banners_and_onboarding_greet_new_members() {
    use fuwa_desktop::core::onboarding::{self, PICK, SAY_HELLO};
    use fuwa_desktop::core::server_admin::ServerPatch;
    // SAFETY: set before anything reads it.
    unsafe { std::env::set_var("FUWA_DESKTOP_KEYCHAIN", "off") };
    let data = tempfile::tempdir().unwrap();
    let instance = start_instance(data.path());
    let (home_a, home_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let alice = Core::start(Paths::under(home_a.path())).unwrap();
    let bob = Core::start(Paths::under(home_b.path())).unwrap();
    let key = {
        let (core, url) = (alice.clone(), instance.url.clone());
        wait(&alice, async move { core.sign_up(&url, "alice", "correct horse battery", "Alice").await }).unwrap()
    };
    {
        let (core, url) = (bob.clone(), instance.url.clone());
        wait(&bob, async move { core.sign_up(&url, "bob", "correct horse battery", "Bob").await }).unwrap();
    }
    for core in [&alice, &bob] {
        until(core, "signed in and live", |s| {
            s.instance(&key).is_some_and(|i| i.me.is_some() && i.connection == Connection::Live)
        });
    }
    let server = {
        let (core, key) = (alice.clone(), key.clone());
        wait(&alice, async move { core.create_server(&key, "Tea house").await }).unwrap()
    };
    until(&alice, "the server's channels", |s| s.instance(&key).is_some_and(|i| i.synced.contains(&server.id)));
    let general = alice
        .shared
        .read(|s| s.instance(&key).unwrap().channels[&server.id].iter().find(|c| c.r#type == 1).unwrap().id.clone());

    // A banner is uploaded for the server, then set with its focus and accent.
    let saved = {
        let (core, key, sid) = (alice.clone(), key.clone(), server.id.clone());
        wait(&alice, async move {
            let url =
                core.upload_picture_for(&key, &sid, pb::MediaPurpose::Banner, "image/png", TINY_PNG.to_vec()).await?;
            let patch = ServerPatch {
                banner_url: Some(url),
                banner_focus: Some((20, 80)),
                accent_color: Some(0xff88aa),
                ..ServerPatch::default()
            };
            core.update_server(&key, &sid, patch).await
        })
        .unwrap()
    };
    assert!(saved.banner_url.contains("/media/"));
    assert_eq!((saved.banner_focus_x, saved.banner_focus_y, saved.accent_color), (20, 80, Some(0xff88aa)));

    // Alice makes a role and an onboarding that hands it out.
    let role = {
        let (core, key, sid) = (alice.clone(), key.clone(), server.id.clone());
        wait(&alice, async move { core.create_role(&key, &sid, "Artist").await }).unwrap()
    };
    let mut pick = onboarding::new_step(PICK, None);
    pick.options[0].label = "  Art ".into();
    pick.options[0].role_ids = vec![role.id.clone()];
    pick.options[0].channel_ids = vec![general.clone()];
    let hello = onboarding::new_step(SAY_HELLO, Some(&general));
    let set = {
        let (core, key, sid) = (alice.clone(), key.clone(), server.id.clone());
        let onb = pb::Onboarding { enabled: true, steps: vec![pick, hello], set_by: String::new() };
        wait(&alice, async move { core.set_onboarding(&key, &sid, onb).await }).unwrap()
    };
    assert_eq!(set.steps.len(), 2);
    assert_eq!(set.steps[0].options[0].label, "Art", "trimmed before it's sent");
    let option = set.steps[0].options[0].id.clone();
    assert!(!option.is_empty());
    assert!(alice.shared.read(|s| s.instance(&key).unwrap().server(&server.id).unwrap().has_onboarding));

    // Bob joins: the banner comes along, and the onboarding is due for him (not for Alice, who manages it).
    let invite = {
        let (core, key, id) = (alice.clone(), key.clone(), server.id.clone());
        wait(&alice, async move { core.create_invite(&key, &id).await }).unwrap()
    };
    {
        let (core, key) = (bob.clone(), key.clone());
        wait(&bob, async move { core.join_by_invite(&key, &invite).await }).unwrap();
    }
    until(&bob, "his member", |s| s.instance(&key).is_some_and(|i| i.my_member(&server.id).is_some()));
    let shown = bob.shared.read(|s| s.instance(&key).unwrap().server(&server.id).unwrap().clone());
    assert_eq!((shown.banner_url, shown.accent_color), (saved.banner_url.clone(), Some(0xff88aa)));
    let due = |core: &Core| core.shared.read(|s| onboarding::due(s.instance(&key).unwrap(), &server.id, now_ms()));
    assert_eq!(due(&bob), Some(true));
    until(&alice, "Bob in her list", |s| s.instance(&key).unwrap().members[&server.id].len() == 2);
    assert_eq!(due(&alice), Some(false));

    // He goes through it: the steps he sees, then his pick gives him the role.
    let seen = {
        let (core, key, sid) = (bob.clone(), key.clone(), server.id.clone());
        wait(&bob, async move { core.onboarding(&key, &sid).await }).unwrap()
    };
    let steps = onboarding::steps_for(&seen, false, |c| c == general);
    assert_eq!(steps.iter().map(|s| s.kind).collect::<Vec<_>>(), vec![PICK, SAY_HELLO]);
    let go = onboarding::go_here_first(&steps, std::slice::from_ref(&option), None, |c| c == general);
    assert_eq!(go[0].note, "Because you picked Art");
    {
        let (core, key, sid) = (bob.clone(), key.clone(), server.id.clone());
        wait(&bob, async move { core.finish_onboarding(&key, &sid, vec![option]).await }).unwrap();
    }
    let me = bob.shared.read(|s| s.instance(&key).unwrap().my_member(&server.id).cloned().unwrap());
    assert!(me.role_ids.contains(&role.id));
    assert!(me.onboarded_at.is_some());
    assert_eq!(due(&bob), Some(false));
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() { out.extend(walk(&path)) } else { out.push(path) }
    }
    out
}
