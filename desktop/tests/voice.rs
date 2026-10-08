//! Voice channels against a real instance: two people on two copies of the
//! app join the same voice channel and hear each other through the media
//! part, Opus and all, with sound fed in and taken out where the devices
//! would be.

use std::sync::Arc;
use std::time::{Duration, Instant};

use fuwa_desktop::core::Core;
use fuwa_desktop::core::config::Paths;
use fuwa_desktop::core::dms::DmStatus;
use fuwa_desktop::core::store::{Connection, Store};
use fuwa_desktop::core::voice::Status;
use fuwa_desktop::core::voice::sound::{FRAME, Pipe, RATE, level};
use fuwa_desktop::pb;
use fuwa_server::app::App;
use fuwa_server::config::Config;

fn start_instance(dir: &std::path::Path, addresses: &str) -> (String, tokio::runtime::Runtime, Arc<App>) {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let dir = dir.to_str().unwrap().to_string();
    let addresses = addresses.to_string();
    let (app, url) = runtime.block_on(async {
        let config = Config::from_lookup(|key| match key {
            "FUWA_DATA_PATH" => Some(dir.clone()),
            "FUWA_TELEMETRY" => Some("off".into()),
            "FUWA_MEDIA_PORT" => Some("0".into()),
            "FUWA_MEDIA_ADDRESSES" => Some(addresses.clone()),
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
    (url, runtime, app)
}

fn wait<T: Send + 'static>(core: &Arc<Core>, future: impl Future<Output = T> + Send + 'static) -> T {
    futures::executor::block_on(core.spawn(future)).unwrap()
}

fn until(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn until_store(core: &Core, what: &str, check: impl Fn(&Store) -> bool) {
    until(what, || core.shared.read(&check));
}

/// Someone's voice state as the instance tells `core` it is now.
fn shown(core: &Arc<Core>, key: &str, server_id: &str, user_id: &str) -> Option<pb::VoiceState> {
    let api = core.api(key)?;
    let request = pb::ListVoiceStatesRequest { server_id: server_id.into() };
    let states = wait(core, async move { api.calls().list_voice_states(request).await }).ok()?.into_inner().states;
    states.into_iter().find(|v| v.user_id == user_id)
}

/// Plays a tone into a microphone pipe in real time, 20 ms at a time, while `going`.
fn speak(pipe: Arc<Pipe>, hz: f32, going: Arc<std::sync::atomic::AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut n = 0usize;
        let start = Instant::now();
        while going.load(std::sync::atomic::Ordering::Relaxed) {
            let frame: Vec<f32> = (0..FRAME)
                .map(|i| ((n * FRAME + i) as f32 / RATE as f32 * hz * std::f32::consts::TAU).sin() * 0.3)
                .collect();
            pipe.push(&frame);
            n += 1;
            let due = start + Duration::from_millis(20 * n as u64);
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
        }
    })
}

/// How loud the next 100 ms out of a speaker pipe are, read as a device would.
fn listen(pipe: &Pipe) -> f32 {
    let mut loudest = 0.0f32;
    for _ in 0..5 {
        std::thread::sleep(Duration::from_millis(20));
        let mut out = [0.0; FRAME];
        pipe.pull(&mut out);
        loudest = loudest.max(level(&out));
    }
    loudest
}

struct Setup {
    url: String,
    key: String,
    server: pb::Server,
    voice: pb::Channel,
    alice: Arc<Core>,
    bob: Arc<Core>,
    _homes: Vec<tempfile::TempDir>,
    _instance: (tokio::runtime::Runtime, Arc<App>, tempfile::TempDir),
}

/// Alice and Bob on two copies of the app, in a server with a voice channel.
fn setup(addresses: &str) -> Setup {
    // SAFETY: set before anything reads it.
    unsafe { std::env::set_var("FUWA_DESKTOP_KEYCHAIN", "off") };
    let data = tempfile::tempdir().unwrap();
    let (url, runtime, app) = start_instance(data.path(), addresses);
    let (home_a, home_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let alice = Core::start(Paths::under(home_a.path())).unwrap();
    let bob = Core::start(Paths::under(home_b.path())).unwrap();

    let key = {
        let (core, url) = (alice.clone(), url.clone());
        wait(&alice, async move { core.sign_up(&url, "alice", "correct horse battery", "Alice").await }).unwrap()
    };
    {
        let (core, url) = (bob.clone(), url.clone());
        wait(&bob, async move { core.sign_up(&url, "bob", "correct horse battery", "Bob").await }).unwrap();
    }
    for core in [&alice, &bob] {
        until_store(core, "signed in and live", |s| {
            s.instance(&key)
                .is_some_and(|i| i.me.is_some() && i.connection == Connection::Live && i.dms.status == DmStatus::Ready)
        });
    }
    let server = {
        let (core, key) = (alice.clone(), key.clone());
        wait(&alice, async move { core.create_server(&key, "Tea house").await }).unwrap()
    };
    let voice = {
        let (core, key, sid) = (alice.clone(), key.clone(), server.id.clone());
        wait(&alice, async move { core.create_channel(&key, &sid, "Lounge", pb::ChannelType::Voice, "").await })
            .unwrap()
    };
    let invite = {
        let (core, key, id) = (alice.clone(), key.clone(), server.id.clone());
        wait(&alice, async move { core.create_invite(&key, &id).await }).unwrap()
    };
    {
        let (core, key) = (bob.clone(), key.clone());
        wait(&bob, async move { core.join_by_invite(&key, &invite).await }).unwrap();
    }
    Setup { url, key, server, voice, alice, bob, _homes: vec![home_a, home_b], _instance: (runtime, app, data) }
}

#[test]
fn two_people_hear_each_other_in_a_voice_channel() {
    let Setup { url, key, server, voice, alice, bob, _homes, _instance } = setup("127.0.0.1");
    let id_of = |core: &Core| core.shared.read(|s| s.instance(&key).unwrap().me.clone().unwrap().id);
    let (alice_id, bob_id) = (id_of(&alice), id_of(&bob));
    let pipes = || (Arc::new(Pipe::microphone()), Arc::new(Pipe::speakers()));
    let ((a_mic, a_out), (b_mic, b_out)) = (pipes(), pipes());
    alice.join_voice_with(&key, &server.id, &voice.id, Some((a_mic.clone(), a_out.clone())));
    bob.join_voice_with(&key, &server.id, &voice.id, Some((b_mic.clone(), b_out.clone())));
    for core in [&alice, &bob] {
        until("connected", || core.call().is_some_and(|c| c.status == Status::Connected));
    }
    // Both show up in the channel, for each other.
    until_store(&bob, "both in the voice channel", |s| {
        let i = s.instance(&key).unwrap();
        fuwa_desktop::core::calls::in_channel(&i.voice, &server.id, &voice.id).len() == 2
    });

    // Alice speaks: Bob hears her, and sees her speaking; she sees herself speaking.
    let going = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let speaker = speak(a_mic.clone(), 440.0, going.clone());
    until("Bob hearing Alice", || listen(&b_out) > 0.1);
    until("Bob seeing Alice speak", || bob.call().is_some_and(|c| c.speaking.contains(&alice_id)));
    until("Alice seeing herself speak", || alice.call().is_some_and(|c| c.speaking.contains(&alice_id)));
    assert!(listen(&a_out) < 0.01, "nobody hears themselves");

    // Muted, she goes quiet for Bob, and the instance shows her muted.
    alice.set_self_mute(true);
    until("Alice muted", || listen(&b_out) < 0.01);
    until("Alice shown muted", || shown(&bob, &key, &server.id, &alice_id).is_some_and(|v| v.self_mute));
    alice.set_self_mute(false);
    until("Alice back", || listen(&b_out) > 0.1);

    // Bob deafened hears nothing, and is shown deafened.
    bob.set_self_deaf(true);
    until("Bob deafened", || listen(&b_out) < 0.01);
    until("Bob shown deafened", || shown(&alice, &key, &server.id, &bob_id).is_some_and(|v| v.self_deaf));
    bob.set_self_deaf(false);
    until("Bob hearing again", || listen(&b_out) > 0.1);
    going.store(false, std::sync::atomic::Ordering::Relaxed);
    speaker.join().unwrap();

    // Leaving takes Bob out of the channel for Alice, right away.
    bob.leave_voice();
    assert!(bob.call().is_none());
    until("Bob gone from the channel", || shown(&alice, &key, &server.id, &bob_id).is_none());

    // Joining from somewhere else takes the place over: the first app hears it.
    let other_home = tempfile::tempdir().unwrap();
    let alice_again = Core::start(Paths::under(other_home.path())).unwrap();
    {
        let (core, url) = (alice_again.clone(), url.clone());
        wait(&alice_again, async move { core.sign_in(&url, "alice", "correct horse battery").await }).unwrap();
    }
    until_store(&alice_again, "signed in", |s| s.instance(&key).is_some_and(|i| i.me.is_some()));
    alice_again.join_voice_with(&key, &server.id, &voice.id, Some(pipes()));
    until("the first app hanging up", || alice.call().is_none());
    assert_eq!(
        alice.take_call_ended().map(|e| e.message()),
        Some("You joined this voice channel somewhere else.".into())
    );
}

/// Where the media part is only reachable over TCP, as behind Railway's TCP
/// proxy: the app gets through with ICE-TCP.
#[test]
fn calls_get_through_over_tcp_alone() {
    let Setup { key, server, voice, alice, bob, _homes, _instance, .. } = setup("tcp/127.0.0.1");
    let pipes = || (Arc::new(Pipe::microphone()), Arc::new(Pipe::speakers()));
    let ((a_mic, a_out), (b_mic, b_out)) = (pipes(), pipes());
    alice.join_voice_with(&key, &server.id, &voice.id, Some((a_mic.clone(), a_out)));
    bob.join_voice_with(&key, &server.id, &voice.id, Some((b_mic, b_out.clone())));
    for core in [&alice, &bob] {
        until("connected over TCP", || core.call().is_some_and(|c| c.status == Status::Connected));
    }
    let going = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let speaker = speak(a_mic, 440.0, going.clone());
    until("Bob hearing Alice over TCP", || listen(&b_out) > 0.1);
    going.store(false, std::sync::atomic::Ordering::Relaxed);
    speaker.join().unwrap();
}

/// Muting while the call is trying to get back in (the instance is down)
/// closes the microphone then and there, not once the call is back.
#[test]
fn muting_while_rejoining_closes_the_microphone() {
    let Setup { key, server, voice, alice, _homes, _instance, .. } = setup("127.0.0.1");
    alice.join_voice_with(
        &key,
        &server.id,
        &voice.id,
        Some((Arc::new(Pipe::microphone()), Arc::new(Pipe::speakers()))),
    );
    until("connected", || alice.call().is_some_and(|c| c.status == Status::Connected));
    assert_eq!(alice.microphone_open(), Some(true));
    // The instance and its media part go away.
    _instance.1.shutdown.cancel();
    until("trying to get back in", || alice.call().is_some_and(|c| c.status == Status::Reconnecting));
    alice.set_self_mute(true);
    assert_eq!(alice.microphone_open(), Some(false), "closed at once");
    alice.set_self_mute(false);
    assert_eq!(alice.microphone_open(), Some(true));
    alice.set_self_deaf(true);
    assert_eq!(alice.microphone_open(), Some(false), "deafened closes it too");
}

/// Waits for a picture of `feed` `width` wide (the media part switches
/// sizes on the next keyframe) and gives its height and a sample of it.
fn picture(core: &Core, feed: &str, width: u32) -> (u32, u32, [u8; 3]) {
    let mut got = None;
    until(&format!("a picture of {feed} {width} wide"), || {
        got = core.videos().take(feed).filter(|p| p.width == width);
        got.is_some()
    });
    let p = got.unwrap();
    assert_eq!(p.bgra.len(), (p.width * p.height * 4) as usize);
    // The test pattern's background, a third of the way down, at the left.
    let at = ((p.height / 3 * p.width + 4) * 4) as usize;
    (p.width, p.height, [p.bgra[at], p.bgra[at + 1], p.bgra[at + 2]])
}

/// Says that a window shows `feed` this tall, as the window would.
fn show(core: &Core, feed: &str, height: u32) {
    core.videos().want(1, std::collections::HashMap::from([(feed.to_owned(), height)]));
}

/// Cameras and shared screens between two copies of the app, through the
/// media part: Alice's goes out in three sizes, Bob gets the one that fits
/// how big he shows it, and sees her picture as she sent it.
#[test]
fn two_people_see_each_other_in_a_voice_channel() {
    let Setup { key, server, voice, alice, bob, _homes, _instance, .. } = setup("127.0.0.1");
    let id_of = |core: &Core| core.shared.read(|s| s.instance(&key).unwrap().me.clone().unwrap().id);
    let (alice_id, bob_id) = (id_of(&alice), id_of(&bob));
    for core in [&alice, &bob] {
        core.use_test_pattern();
        let pipes = (Arc::new(Pipe::microphone()), Arc::new(Pipe::speakers()));
        core.join_voice_with(&key, &server.id, &voice.id, Some(pipes));
    }
    for core in [&alice, &bob] {
        until("connected", || core.call().is_some_and(|c| c.status == Status::Connected));
    }

    // Alice turns her camera on: the instance shows it, and Bob, showing it
    // big, gets the full size (the pattern is 640×360).
    show(&bob, &alice_id, 720);
    alice.set_camera(true);
    assert!(alice.call().unwrap().self_video);
    until("Alice's camera shown on", || shown(&bob, &key, &server.id, &alice_id).is_some_and(|v| v.self_video));
    let (w, h, color) = picture(&bob, &alice_id, 640);
    assert_eq!((w, h), (640, 360), "the full size");
    let (w2, h2, again) = picture(&bob, &alice_id, 640);
    assert_eq!((w2, h2), (640, 360));
    for (a, b) in color.iter().zip(again) {
        assert!(a.abs_diff(b) < 24, "a steady picture: {color:?} {again:?}");
    }

    // Showing it small, he gets a quarter.
    show(&bob, &alice_id, 200);
    until("the small size", || bob.videos().take(&alice_id).is_some_and(|p| (p.width, p.height) == (160, 90)));
    // Half.
    show(&bob, &alice_id, 400);
    until("half", || bob.videos().take(&alice_id).is_some_and(|p| (p.width, p.height) == (320, 180)));
    // Not showing it at all, nothing comes.
    show(&bob, &alice_id, 0);
    std::thread::sleep(Duration::from_millis(600));
    let _ = bob.videos().take(&alice_id);
    let seq = bob.videos().seq(&alice_id);
    std::thread::sleep(Duration::from_millis(600));
    assert!(bob.videos().seq(&alice_id) <= seq + 1, "stopped");

    // Her own preview, mirrored or not, comes from her camera at the size she shows it.
    show(&alice, &alice_id, 200);
    assert_eq!(picture(&alice, &alice_id, 160).1, 90);

    // A shared screen is a feed of its own, at up to 1080p.
    show(&bob, &format!("{alice_id}-screen"), 1080);
    alice.set_screen(true, None);
    until("Alice shown sharing", || shown(&bob, &key, &server.id, &alice_id).is_some_and(|v| v.self_stream));
    assert_eq!(picture(&bob, &format!("{alice_id}-screen"), 1280).1, 720);

    // Bob's camera goes the other way.
    show(&alice, &bob_id, 720);
    bob.set_camera(true);
    assert_eq!(picture(&alice, &bob_id, 640).1, 360);

    // Turning it off shows it off.
    alice.set_camera(false);
    alice.set_screen(false, None);
    until("Alice's camera shown off", || {
        shown(&bob, &key, &server.id, &alice_id).is_some_and(|v| !v.self_video && !v.self_stream)
    });
}

/// In a direct message's call, cameras are sealed end to end, the web's
/// way (core/calls.rs checks the format): Bob opens Alice's frames with the
/// conversation's secret, and the media part only ever had ciphertext.
#[test]
fn cameras_in_direct_messages_are_sealed_and_open_for_the_other_person() {
    use fuwa_desktop::core::dms::Content;
    let Setup { key, alice, bob, _homes, _instance, .. } = setup("127.0.0.1");
    let id_of = |core: &Core| core.shared.read(|s| s.instance(&key).unwrap().me.clone().unwrap().id);
    let (alice_id, bob_id) = (id_of(&alice), id_of(&bob));
    let conversation = {
        let (core, key, bob_id) = (alice.clone(), key.clone(), bob_id.clone());
        wait(&alice, async move { core.open_conversation(&key, &bob_id).await }).unwrap()
    };
    {
        let (core, key, id) = (alice.clone(), key.clone(), conversation.clone());
        let content = Content::Text { text: "call?".into(), reply_to: 0 };
        wait(&alice, async move { core.send_dm(&key, &id, content).await }).unwrap();
    }
    until_store(&bob, "the message", |s| {
        s.instance(&key).unwrap().dms.items.get(&conversation).is_some_and(|items| !items.is_empty())
    });
    for core in [&alice, &bob] {
        core.use_test_pattern();
        core.join_dm_call(&key, &conversation);
    }
    for core in [&alice, &bob] {
        until("connected", || core.call().is_some_and(|c| c.status == Status::Connected));
    }
    show(&bob, &alice_id, 720);
    alice.set_camera(true);
    let (w, h, _) = picture(&bob, &alice_id, 640);
    assert_eq!((w, h), (640, 360), "opened and decoded");
    show(&alice, &format!("{bob_id}-screen"), 720);
    bob.set_screen(true, None);
    assert_eq!(picture(&alice, &format!("{bob_id}-screen"), 1280).1, 720);
}
