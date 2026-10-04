//! Voice channels in the desktop app, sound only for now: join for a place
//! and a session on the media part, keep the place every few seconds, and
//! rejoin with the same session when the connection drops or the media
//! part restarts, the way web/src/calls/engine.ts does. One call at a time;
//! joining another channel leaves the one you're in.
//!
//! [`link`] is the connection, [`sound`] the Opus and mixing between it
//! and the devices, [`devices`] the microphone and speakers.

pub mod devices;
pub mod link;
pub mod sound;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::sync::{mpsc, watch};
use tonic::Code;

use self::devices::{Devices, Trouble};
use self::link::{Happened, Link, Signal};
use self::sound::{FRAME, Microphone, Mixer, Pipe};
use super::Core;
use super::api::{Api, Problem};
use super::reports;
use crate::{pb, rpc};

/// How often the place in the call is kept: the media part forgets it after 15 s.
const KEEP_EVERY: Duration = Duration::from_secs(5);
/// How long a dropped connection gets to come back on its own.
const GRACE: Duration = Duration::from_millis(2500);
/// How long the first connection may take before the call gives up.
const FIRST_CONNECT: Duration = Duration::from_secs(15);
/// The longest wait between tries to get back into the call.
const MOST_BACKOFF: Duration = Duration::from_secs(8);
/// How long a connection must last before the next drop starts the waits
/// between tries afresh.
const STAYED_UP: Duration = Duration::from_secs(30);

/// Where a call is at, for the call bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Connecting,
    Connected,
    Reconnecting,
}

/// The call you're in, as the window shows it.
#[derive(Clone, Debug)]
pub struct CallView {
    pub instance: String,
    pub server_id: String,
    pub channel_id: String,
    pub status: Status,
    pub self_mute: bool,
    pub self_deaf: bool,
    /// Who's speaking now, by account id (yours too).
    pub speaking: HashSet<String>,
    /// The microphone or speakers that couldn't open.
    pub trouble: Vec<Trouble>,
}

/// Why a call ended, when it wasn't you leaving.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ended {
    Replaced,
    Disconnected,
    Unreachable,
    Refused(String),
}

impl Ended {
    pub fn message(&self) -> String {
        match self {
            Self::Replaced => "You joined this voice channel somewhere else.".into(),
            Self::Disconnected => "You were disconnected from the voice channel.".into(),
            Self::Unreachable => "Couldn't connect to the voice channel. Your network may be blocking calls.".into(),
            Self::Refused(message) => message.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Selves {
    mute: bool,
    deaf: bool,
}

enum Command {
    Leave,
}

struct Active {
    id: u64,
    commands: mpsc::UnboundedSender<Command>,
    selves: watch::Sender<Selves>,
}

/// The app's call, if any.
#[derive(Default)]
pub struct Voice {
    active: Mutex<Option<Active>>,
    view: Mutex<Option<CallView>>,
    ended: Mutex<Option<Ended>>,
    next_id: Mutex<u64>,
}

impl Core {
    /// The call you're in, if any.
    pub fn call(&self) -> Option<CallView> {
        self.voice.view.lock().clone()
    }

    /// Why the last call ended on its own, once: for a notice.
    pub fn take_call_ended(&self) -> Option<Ended> {
        self.voice.ended.lock().take()
    }

    /// Joins a voice channel, leaving the call you're in.
    pub fn join_voice(self: &Arc<Self>, instance: &str, server_id: &str, channel_id: &str) {
        self.join_voice_with(instance, server_id, channel_id, None);
    }

    /// [`Core::join_voice`] with sound from and to these instead of the
    /// devices: for the tests.
    #[doc(hidden)]
    pub fn join_voice_with(
        self: &Arc<Self>,
        instance: &str,
        server_id: &str,
        channel_id: &str,
        pipes: Option<(Arc<Pipe>, Arc<Pipe>)>,
    ) {
        let here = |c: &CallView| c.instance == instance && c.server_id == server_id && c.channel_id == channel_id;
        if self.voice.view.lock().as_ref().is_some_and(here) {
            return;
        }
        let Some(api) = self.api(instance) else { return };
        let me = self.shared.read(|s| s.instances.get(instance).and_then(|i| i.me.as_ref().map(|u| u.id.clone())));
        let Some(me) = me else { return };
        let selves = self.voice.view.lock().as_ref().map(|v| Selves { mute: v.self_mute, deaf: v.self_deaf });
        self.leave_voice();
        let selves = selves.unwrap_or_default();
        let id = {
            let mut next = self.voice.next_id.lock();
            *next += 1;
            *next
        };
        let (commands, commands_rx) = mpsc::unbounded_channel();
        let (selves_tx, selves_rx) = watch::channel(selves);
        *self.voice.active.lock() = Some(Active { id, commands, selves: selves_tx });
        *self.voice.ended.lock() = None;
        *self.voice.view.lock() = Some(CallView {
            instance: instance.to_string(),
            server_id: server_id.to_string(),
            channel_id: channel_id.to_string(),
            status: Status::Connecting,
            self_mute: selves.mute,
            self_deaf: selves.deaf,
            speaking: HashSet::new(),
            trouble: vec![],
        });
        self.shared.update(|_| ());
        reports::used("call.join_voice");
        let target = Target { server_id: server_id.into(), channel_id: channel_id.into(), me };
        let sound = match pipes {
            Some((microphone, speakers)) => Sound { microphone, speakers, devices: None },
            None => {
                let (microphone, speakers) = (Arc::new(Pipe::microphone()), Arc::new(Pipe::speakers()));
                let devices = Devices::open(microphone.clone(), speakers.clone(), !selves.mute && !selves.deaf);
                Sound { microphone, speakers, devices: Some(devices) }
            }
        };
        let core = Arc::downgrade(self);
        self.runtime.spawn(async move {
            let ended = run(core.clone(), id, api, target, sound, commands_rx, selves_rx).await;
            if let Some(core) = core.upgrade() {
                core.finish(id, ended);
            }
        });
    }

    /// Leaves the call you're in.
    pub fn leave_voice(&self) {
        if let Some(active) = self.voice.active.lock().take() {
            let _ = active.commands.send(Command::Leave);
        }
        if self.voice.view.lock().take().is_some() {
            self.shared.update(|_| ());
        }
    }

    pub fn set_self_mute(&self, mute: bool) {
        self.set_selves(|s| {
            s.mute = mute;
            // Unmuting while deafened undeafens too, as it does on the web.
            if !mute {
                s.deaf = false;
            }
        });
    }

    pub fn set_self_deaf(&self, deaf: bool) {
        self.set_selves(|s| s.deaf = deaf);
    }

    fn set_selves(&self, f: impl FnOnce(&mut Selves)) {
        let Some(active) = self.voice.active.lock().as_ref().map(|a| a.selves.clone()) else { return };
        active.send_modify(f);
        let selves = *active.borrow();
        if let Some(view) = self.voice.view.lock().as_mut() {
            view.self_mute = selves.mute;
            view.self_deaf = selves.deaf;
        }
        self.shared.update(|_| ());
    }

    /// Changes the view of call `id`, if it's still the one going.
    fn call_view(&self, id: u64, f: impl FnOnce(&mut CallView)) {
        if self.voice.active.lock().as_ref().is_none_or(|a| a.id != id) {
            return;
        }
        let changed = match self.voice.view.lock().as_mut() {
            Some(view) => {
                let before = view.clone();
                f(view);
                before.status != view.status || before.speaking != view.speaking || before.trouble != view.trouble
            }
            None => false,
        };
        if changed {
            self.shared.update(|_| ());
        }
    }

    fn finish(&self, id: u64, ended: Option<Ended>) {
        {
            let mut active = self.voice.active.lock();
            if active.as_ref().is_none_or(|a| a.id != id) {
                return;
            }
            *active = None;
        }
        *self.voice.view.lock() = None;
        *self.voice.ended.lock() = ended;
        self.shared.update(|_| ());
    }
}

struct Target {
    server_id: String,
    channel_id: String,
    me: String,
}

struct Sound {
    microphone: Arc<Pipe>,
    speakers: Arc<Pipe>,
    devices: Option<Devices>,
}

/// The place in the call was lost for good (taken over, disconnected,
/// CONNECT taken away, signed out).
fn gone(problem: &Problem) -> bool {
    matches!(problem.code, Code::FailedPrecondition | Code::NotFound | Code::PermissionDenied | Code::Unauthenticated)
}

enum Outcome {
    Left,
    Rejoin { soon: bool },
    Ended(Ended),
}

async fn run(
    core: std::sync::Weak<Core>,
    id: u64,
    api: Api,
    target: Target,
    mut sound: Sound,
    mut commands: mpsc::UnboundedReceiver<Command>,
    selves: watch::Receiver<Selves>,
) -> Option<Ended> {
    let view = |f: &dyn Fn(&mut CallView)| {
        if let Some(core) = core.upgrade() {
            core.call_view(id, f);
        }
    };
    let mut session = String::new();
    let mut backoff = Duration::from_millis(500);
    // Rejoins straight after a restart without a connection that lasted in
    // between: only the first is quick.
    let mut quick_rejoins = 0u32;
    let started = Instant::now();
    let mut ever_connected = false;
    let ended = loop {
        let (mut link, offer) = Link::offer();
        let now = *selves.borrow();
        let request = pb::JoinVoiceRequest {
            server_id: target.server_id.clone(),
            channel_id: target.channel_id.clone(),
            offer,
            self_mute: now.mute || now.deaf,
            self_deaf: now.deaf,
            session_id: session.clone(),
            ..Default::default()
        };
        let joined = tokio::select! {
            joined = rpc!(api.calls(), join_voice(request)) => joined,
            Some(Command::Leave) = commands.recv() => break None,
        };
        let soon = match joined {
            Ok(joined) => {
                session = joined.session_id;
                let connecting = tokio::select! {
                    connected = link.connect(&joined.answer) => connected,
                    Some(Command::Leave) = commands.recv() => break None,
                };
                let (outcome, connected) = match connecting {
                    Ok(()) => {
                        drive(&mut link, &api, &target, &session, &mut sound, &mut commands, selves.clone(), &view)
                            .await
                    }
                    Err(_) => {
                        reports::error("call_connect", "address");
                        (Outcome::Rejoin { soon: false }, None)
                    }
                };
                if let Some(at) = connected {
                    ever_connected = true;
                    if at.elapsed() >= STAYED_UP {
                        backoff = Duration::from_millis(500);
                        quick_rejoins = 0;
                    }
                }
                match outcome {
                    Outcome::Left => break None,
                    Outcome::Ended(ended) => break Some(ended),
                    Outcome::Rejoin { soon } => soon,
                }
            }
            // The first try says why it can't (calls off, no CONNECT); a
            // rejoin only stops when the place is gone.
            Err(problem) if session.is_empty() && !problem.retryable() => {
                break Some(Ended::Refused(problem.message));
            }
            Err(problem) if gone(&problem) => break Some(Ended::Disconnected),
            Err(_) => false,
        };
        // Never got through at all: the network is in the way.
        if !ever_connected && started.elapsed() > FIRST_CONNECT {
            reports::error("call_connect", "ice");
            break Some(Ended::Unreachable);
        }
        drop(link);
        sound.speakers.clear();
        view(&|v| {
            v.status = if ever_connected { Status::Reconnecting } else { Status::Connecting };
            v.speaking.clear();
        });
        // Jitter keeps everyone from arriving at the next media part at once.
        let wait = if soon && quick_rejoins == 0 {
            Duration::from_millis(150 + (rand_unit() * 450.0) as u64)
        } else {
            backoff.mul_f64(0.75 + rand_unit() / 2.0)
        };
        if soon {
            quick_rejoins += 1;
        }
        backoff = (backoff * 2).min(MOST_BACKOFF);
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            Some(Command::Leave) = commands.recv() => break None,
        }
    };
    drop(sound.devices.take());
    if !session.is_empty() && ended.is_none() {
        let request = pb::LeaveVoiceRequest { server_id: target.server_id.clone(), session_id: session.clone() };
        let _ = rpc!(api.calls(), leave_voice(request)).await;
    }
    ended
}

/// A number in [0, 1) for jitter.
fn rand_unit() -> f64 {
    let mut bytes = [0u8; 4];
    let _ = getrandom::fill(&mut bytes);
    f64::from(u32::from_le_bytes(bytes)) / (f64::from(u32::MAX) + 1.0)
}

/// Runs one connection until it ends, you leave, or the call does.
#[allow(clippy::too_many_arguments)]
async fn drive(
    link: &mut Link,
    api: &Api,
    target: &Target,
    session: &str,
    sound: &mut Sound,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    selves: watch::Receiver<Selves>,
    view: &ViewFn<'_>,
) -> (Outcome, Option<Instant>) {
    let mut connected = None;
    let outcome = drive_until(link, api, target, session, sound, commands, selves, view, &mut connected).await;
    (outcome, connected)
}

#[allow(clippy::too_many_arguments)]
async fn drive_until(
    link: &mut Link,
    api: &Api,
    target: &Target,
    session: &str,
    sound: &mut Sound,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    selves: watch::Receiver<Selves>,
    view: &ViewFn<'_>,
    connected: &mut Option<Instant>,
) -> Outcome {
    let started = Instant::now();
    let Ok(mut microphone) = Microphone::new() else {
        return Outcome::Ended(Ended::Refused("Opus didn't start.".into()));
    };
    let mut mixer = Mixer::default();
    let (kept_tx, mut kept) = mpsc::channel(4);
    let keeper = tokio::spawn(keep(
        api.clone(),
        target.server_id.clone(),
        target.channel_id.clone(),
        session.to_string(),
        selves.clone(),
        kept_tx,
    ));
    let _keeper = AbortOnDrop(keeper);
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
    let mut happened = Vec::new();
    let mut broken_since: Option<Instant> = None;
    let mut trouble_seen = vec![];
    let mut out = [0.0f32; FRAME];
    loop {
        let wake = link.poll(&mut happened).await;
        for event in happened.drain(..) {
            match event {
                Happened::Heard(who, packet) => {
                    if !selves.borrow().deaf {
                        mixer.hear(&who, &packet);
                    }
                }
                Happened::Gone(who) => mixer.forget(&who),
                Happened::Connected => {
                    broken_since = None;
                    if connected.is_none() {
                        *connected = Some(Instant::now());
                        reports::timing("call.connect", started.elapsed());
                        view(&|v| v.status = Status::Connected);
                    }
                }
                Happened::Restored => broken_since = None,
                Happened::Broken { for_good: true } => return Outcome::Rejoin { soon: false },
                Happened::Broken { for_good: false } => {
                    broken_since.get_or_insert_with(Instant::now);
                }
                Happened::Signal(Signal::Restarting) => return Outcome::Rejoin { soon: true },
                Happened::Signal(Signal::Replaced) => return Outcome::Ended(Ended::Replaced),
                Happened::Signal(Signal::Closed) => return Outcome::Ended(Ended::Disconnected),
            }
        }
        if broken_since.is_some_and(|at| at.elapsed() > GRACE) {
            return Outcome::Rejoin { soon: false };
        }
        if connected.is_none() && started.elapsed() > FIRST_CONNECT {
            return Outcome::Rejoin { soon: false };
        }
        let sleep = tokio::time::sleep_until(tokio::time::Instant::from_std(wake));
        tokio::select! {
            packet = link.packet() => link.receive(packet),
            _ = sleep => link.tick(),
            _ = tick.tick() => {
                let now = *selves.borrow();
                // The microphone is closed while muted or deafened, not just ignored.
                if let Some(devices) = &sound.devices {
                    devices.listen(!now.mute && !now.deaf);
                }
                let mut speaking = HashSet::new();
                // A microphone running a touch faster than this clock would
                // pile up delay: past 160 ms behind (more than a device hands
                // over at once), the oldest goes.
                while sound.microphone.len() > FRAME * 8 {
                    let _ = sound.microphone.frame();
                }
                if let Some(frame) = sound.microphone.frame() {
                    if now.mute || now.deaf || !link.is_connected() {
                        microphone.speaking.stop();
                        link.skip();
                    } else if let Some(packet) = microphone.encode(&frame) {
                        link.speak(packet);
                        if microphone.speaking.is_on() {
                            speaking.insert(target.me.clone());
                        }
                    }
                }
                if now.deaf {
                    mixer.clear();
                    sound.speakers.clear();
                } else {
                    for who in mixer.mix(&mut out) {
                        // A shared screen's sound isn't its sharer speaking.
                        if !who.ends_with("-screen") {
                            speaking.insert(who);
                        }
                    }
                    sound.speakers.push(&out);
                }
                let trouble = sound.devices.as_ref().map(|d| d.trouble()).unwrap_or_default();
                if trouble != trouble_seen {
                    for t in trouble.iter().filter(|t| !trouble_seen.contains(t)) {
                        reports::error("call_device", match t { Trouble::NoMicrophone => "microphone", Trouble::NoSpeakers => "speakers" });
                    }
                    trouble_seen = trouble.clone();
                }
                view(&|v| {
                    v.speaking = speaking.clone();
                    v.trouble = trouble.clone();
                });
            }
            kept = kept.recv() => match kept {
                Some(Kept::Gone) => return Outcome::Ended(Ended::Disconnected),
                Some(Kept::Lost) => return Outcome::Rejoin { soon: true },
                None => {}
            },
            Some(Command::Leave) = commands.recv() => return Outcome::Left,
        }
    }
}

/// Changes the call's view, if the call is still the one going.
type ViewFn<'a> = dyn Fn(&dyn Fn(&mut CallView)) + Sync + 'a;

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

enum Kept {
    /// The place is gone for good.
    Gone,
    /// The media part lost the place (it restarted): join again.
    Lost,
}

/// Keeps the place every few seconds, and at once when mute or deafen change.
async fn keep(
    api: Api,
    server_id: String,
    channel_id: String,
    session_id: String,
    mut selves: watch::Receiver<Selves>,
    kept: mpsc::Sender<Kept>,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(KEEP_EVERY) => {}
            changed = selves.changed() => if changed.is_err() { return },
        }
        let now = *selves.borrow_and_update();
        let request = pb::KeepVoiceRequest {
            server_id: server_id.clone(),
            session_id: session_id.clone(),
            channel_id: channel_id.clone(),
            self_mute: now.mute || now.deaf,
            self_deaf: now.deaf,
            ..Default::default()
        };
        match rpc!(api.calls(), keep_voice(request)).await {
            Ok(_) => {}
            Err(problem) if gone(&problem) => {
                let _ = kept.send(Kept::Gone).await;
                return;
            }
            Err(problem) if problem.code == Code::Unavailable && problem.message.contains("join again") => {
                let _ = kept.send(Kept::Lost).await;
                return;
            }
            // Anything else (the instance restarting, a blip): the next keep tries again.
            Err(_) => {}
        }
    }
}
