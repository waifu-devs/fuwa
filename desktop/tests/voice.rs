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

fn until(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn until_store(core: &Core, what: &str, check: impl Fn(&Store) -> bool) {
    until(what, || core.shared.read(&check));
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
    until_store(&bob, "Alice shown muted", |s| {
        s.instance(&key).unwrap().voice[&server.id].iter().any(|v| v.user_id == alice_id && v.self_mute)
    });
    alice.set_self_mute(false);
    until("Alice back", || listen(&b_out) > 0.1);

    // Bob deafened hears nothing, and is shown deafened.
    bob.set_self_deaf(true);
    until("Bob deafened", || listen(&b_out) < 0.01);
    until_store(&alice, "Bob shown deafened", |s| {
        s.instance(&key).unwrap().voice[&server.id].iter().any(|v| v.user_id == bob_id && v.self_deaf)
    });
    bob.set_self_deaf(false);
    until("Bob hearing again", || listen(&b_out) > 0.1);
    going.store(false, std::sync::atomic::Ordering::Relaxed);
    speaker.join().unwrap();

    // Leaving takes Bob out of the channel for Alice, right away.
    bob.leave_voice();
    assert!(bob.call().is_none());
    until_store(&alice, "Bob gone from the channel", |s| {
        let i = s.instance(&key).unwrap();
        fuwa_desktop::core::calls::in_channel(&i.voice, &server.id, &voice.id).len() == 1
    });

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
