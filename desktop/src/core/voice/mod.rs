//! Calls in the desktop app: voice channels and calls in direct messages,
//! with sound, cameras and shared screens. Join for a place and a session on the media part, keep
//! the place every few seconds, and rejoin with the same session when the
//! connection drops or the media part restarts, the way
//! web/src/calls/engine.ts does. One call at a time; joining another leaves
//! the one you're in. A direct message's call seals every frame end to end
//! ([`Frames`]) under a secret from the conversation's MLS group.
//!
//! [`link`] is the connection, [`sound`] the Opus and mixing between it
//! and the devices, [`devices`] the microphone and speakers, [`quality`]
//! how the connection is doing, [`video`] the cameras and screens coming in,
//! [`capture`] yours going out, [`vp8`] their pictures, and
//! [`screen_sound`] what your shared screen plays.

pub mod access;
pub mod capture;
pub mod ceiling;
pub mod devices;
mod film;
#[cfg(any(windows, feature = "system-libvpx"))]
mod libvpx;
pub mod link;
pub mod processing;
pub mod quality;
pub mod screen_sound;
pub mod sound;
pub mod video;
pub mod vp8;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::sync::{mpsc, watch};
use tonic::Code;

use self::capture::{Failure, Filmed, Sending};
use self::devices::{Devices, Listener, Trouble};
use self::link::{Happened, Link, Signal};
use self::processing::{Choice, Processing};
use self::quality::Quality;
use self::screen_sound::ScreenSound;
use self::sound::{FRAME, Microphone, Mixer, Pipe};
use self::video::{Videos, Watcher, is_screen, layer_for, owner_of};
use super::Core;
use super::api::{Api, Problem};
use super::calls::{Frames, Opened};
use super::i18n::t;
use super::reports;
use super::sounds::Sound as Cue;
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
/// How often a direct message's call looks for a new epoch (a device joined
/// or left the conversation), as the web's does.
const SECRET_EVERY: Duration = Duration::from_secs(4);
/// How long after connecting newcomers start getting a cue (the first
/// sound from everyone already there isn't anyone joining).
const SETTLED: Duration = Duration::from_millis(1500);
/// How long the sizes asked of cameras wait, so a burst of resizes is one message.
const LAYERS_AFTER: Duration = Duration::from_millis(120);
/// How often the instance's call settings are read again during a call, for
/// a new ceiling on cameras.
const SETTINGS_EVERY: Duration = Duration::from_secs(60);
/// How often a feed whose decoder lost its place asks for a keyframe.
const KEYFRAME_ASK: Duration = Duration::from_millis(500);
/// How long a shared screen's sound may go unheard before it counts as
/// stopped: its track stays open after the share ends, but nothing comes.
const SCREEN_SOUND_GONE: Duration = Duration::from_millis(1500);

/// Where a call is at, for the call panel.
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
    /// A voice channel's server and channel; empty in a direct message's call.
    pub server_id: String,
    pub channel_id: String,
    /// A direct message's conversation; empty in a voice channel.
    pub conversation_id: String,
    pub status: Status,
    pub self_mute: bool,
    pub self_deaf: bool,
    /// Your camera is on.
    pub self_video: bool,
    /// You're sharing your screen.
    pub self_stream: bool,
    /// You're recording the call's sound to a file on this computer.
    pub self_record: bool,
    /// You're recording the voice channel on the server.
    pub server_record: bool,
    /// The instance records voice channels on the server.
    pub server_recordings: bool,
    /// The instance's ceiling on cameras (0s for none), read when the call
    /// starts and every minute after.
    pub instance_camera: ceiling::Ceiling,
    /// When sound first went through.
    pub since: Option<Instant>,
    /// Who's speaking now, by account id (yours too).
    pub speaking: HashSet<String>,
    /// The microphone or speakers that couldn't open.
    pub trouble: Vec<Trouble>,
    pub quality: Quality,
    /// The channel doesn't allow cameras or shared screens (no VIDEO there).
    pub video_suppress: bool,
    /// A moderator turned your camera and screen off in this server.
    pub video_off: bool,
    /// Why your camera or screen couldn't start, when the system's settings
    /// are in the way: (a screen, why).
    pub video_trouble: Option<(bool, Failure)>,
    /// The instance passes a shared screen's sound on (read as the call starts).
    pub screen_sound_offered: bool,
    /// Your shared screen brings this computer's sound: whether it goes out
    /// now (its sound button turns it off for everyone). None without sound.
    pub screen_sound: Option<bool>,
    /// Whose shared screens' sound is coming, by account id.
    pub screen_sounds: HashSet<String>,
    /// Shared screens you turned the sound off for, for yourself, by account id.
    pub quiet_screens: HashSet<String>,
}

impl CallView {
    pub fn is_dm(&self) -> bool {
        !self.conversation_id.is_empty()
    }

    /// Whether this is the call in that voice channel.
    pub fn in_channel(&self, instance: &str, channel_id: &str) -> bool {
        !self.is_dm() && self.instance == instance && self.channel_id == channel_id
    }

    /// Whether this is the call in that conversation.
    pub fn in_conversation(&self, instance: &str, conversation_id: &str) -> bool {
        self.instance == instance && self.conversation_id == conversation_id
    }
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

    /// The same, for a call in a direct message.
    pub fn dm_message(&self) -> String {
        match self {
            Self::Replaced => t("workspace.calls.joinedElsewhere"),
            Self::Disconnected => t("workspace.calls.ended"),
            Self::Unreachable => "Couldn't connect to the call. Your network may be blocking calls.".into(),
            Self::Refused(message) => message.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Selves {
    mute: bool,
    deaf: bool,
    /// The camera is on.
    video: bool,
    /// Sharing a screen.
    stream: bool,
    /// Recording on this computer.
    record: bool,
    /// Recording the voice channel on the server.
    server_record: bool,
}

enum Command {
    Leave,
}

struct Active {
    id: u64,
    /// The microphone, opened and closed with mute and deafen whatever the
    /// call is doing (connecting, or waiting to rejoin).
    microphone: Listener,
    commands: mpsc::UnboundedSender<Command>,
    selves: watch::Sender<Selves>,
    /// Your camera and shared screen while they're on, and where their frames go.
    camera: Option<Sending>,
    screen: Option<Sending>,
    /// This computer's sound, while your shared screen brings it.
    screen_sound: Option<Arc<ScreenSound>>,
    filmed: mpsc::Sender<Filmed>,
}

/// Where a call is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Place {
    Voice { server_id: String, channel_id: String },
    Dm { conversation_id: String },
}

/// How loud things are, set from the app's settings: each person by
/// account id, the call as a whole, and the microphone.
#[derive(Clone, Debug, PartialEq)]
struct Volumes {
    people: HashMap<String, f32>,
    output: f32,
    input: f32,
    /// Push to talk: the microphone goes out only while its key is held,
    /// and for `release` after.
    ptt: bool,
    release: Duration,
    /// Shared screens whose sound you turned off, by account id.
    quiet_screens: HashSet<String>,
    /// Echo cancellation, noise suppression and automatic gain.
    processing: Choice,
}

impl Default for Volumes {
    fn default() -> Self {
        Self {
            people: HashMap::new(),
            output: 1.0,
            input: 1.0,
            ptt: false,
            release: Duration::ZERO,
            quiet_screens: HashSet::new(),
            processing: Choice { echo: true, noise: true, gain: true },
        }
    }
}

impl Volumes {
    /// How loud each stream plays: people as set, quiet screens not at all.
    fn gains(&self) -> HashMap<String, f32> {
        let mut gains = self.people.clone();
        for id in &self.quiet_screens {
            gains.insert(video::feed_of(id, true), 0.0);
        }
        gains
    }
}

/// Push to talk's key: held now, or when it came up.
#[derive(Clone, Copy, Debug, Default)]
struct Push {
    down: bool,
    released: Option<Instant>,
}

impl Push {
    fn open(&self, release: Duration) -> bool {
        self.down || self.released.is_some_and(|at| at.elapsed() < release)
    }
}

/// The app's call, if any.
#[derive(Default)]
pub struct Voice {
    active: Mutex<Option<Active>>,
    view: Mutex<Option<CallView>>,
    ended: Mutex<Option<Ended>>,
    next_id: Mutex<u64>,
    /// Mute and deafen outside a call: kept for the next one (the web's call store).
    idle: Mutex<Selves>,
    /// Things worth a notice, said once: recordings saved or stopped.
    notices: Mutex<Vec<String>>,
    volumes: Arc<Mutex<Volumes>>,
    push: Arc<Mutex<Push>>,
    /// Cameras and shared screens: their pictures, and what the windows want.
    videos: Arc<Videos>,
    /// Your own camera shows mirrored to you.
    mirror: Arc<AtomicBool>,
    /// A test pattern stands in for the camera and screen.
    pattern: AtomicBool,
    /// The camera checked in settings, outside a call.
    camera_test: Mutex<Option<Sending>>,
    /// Why the camera check stopped by itself, if it did.
    camera_test_failed: Mutex<Option<Failure>>,
}

/// The feed the camera check in settings shows.
pub const CAMERA_TEST: &str = "camera-test";

impl Core {
    /// Cameras and shared screens in the call, for the window.
    pub fn videos(&self) -> &Arc<Videos> {
        &self.voice.videos
    }

    /// The call you're in, if any.
    pub fn call(&self) -> Option<CallView> {
        self.voice.view.lock().clone()
    }

    /// Whether you're muted and deafened: in the call, or for the next one.
    pub fn selves(&self) -> (bool, bool) {
        let s = self.voice.view.lock().as_ref().map(|v| (v.self_mute, v.self_deaf));
        s.unwrap_or_else(|| {
            let idle = *self.voice.idle.lock();
            (idle.mute, idle.deaf)
        })
    }

    /// Whether the call's microphone is meant to be open: for the tests.
    #[doc(hidden)]
    pub fn microphone_open(&self) -> Option<bool> {
        self.voice.active.lock().as_ref().map(|a| a.microphone.is_listening())
    }

    /// Why the last call ended on its own, once: for a notice.
    pub fn take_call_ended(&self) -> Option<Ended> {
        self.voice.ended.lock().take()
    }

    /// What the call wants said, once each.
    pub fn take_call_notices(&self) -> Vec<String> {
        std::mem::take(&mut *self.voice.notices.lock())
    }

    fn notice(&self, text: String) {
        self.voice.notices.lock().push(text);
        self.shared.update(|_| ());
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
        let place = Place::Voice { server_id: server_id.into(), channel_id: channel_id.into() };
        self.join(instance, place, pipes);
    }

    /// Starts the call in a direct message's conversation, or joins the one going on.
    pub fn join_dm_call(self: &Arc<Self>, instance: &str, conversation_id: &str) {
        self.join_dm_call_with(instance, conversation_id, None);
    }

    /// [`Core::join_dm_call`] with sound from and to these instead of the
    /// devices: for the tests.
    #[doc(hidden)]
    pub fn join_dm_call_with(
        self: &Arc<Self>,
        instance: &str,
        conversation_id: &str,
        pipes: Option<(Arc<Pipe>, Arc<Pipe>)>,
    ) {
        self.join(instance, Place::Dm { conversation_id: conversation_id.into() }, pipes);
    }

    fn join(self: &Arc<Self>, instance: &str, place: Place, pipes: Option<(Arc<Pipe>, Arc<Pipe>)>) {
        let here = |c: &CallView| {
            c.instance == instance
                && match &place {
                    Place::Voice { channel_id, .. } => c.channel_id == *channel_id && !c.is_dm(),
                    Place::Dm { conversation_id } => c.conversation_id == *conversation_id,
                }
        };
        if self.voice.view.lock().as_ref().is_some_and(here) {
            return;
        }
        let Some(api) = self.api(instance) else { return };
        let me = self.shared.read(|s| s.instances.get(instance).and_then(|i| i.me.as_ref().map(|u| u.id.clone())));
        let Some(me) = me else { return };
        let selves = self.voice.view.lock().as_ref().map(|v| Selves {
            mute: v.self_mute,
            deaf: v.self_deaf,
            ..Default::default()
        });
        self.leave_voice();
        // Mute and deafen carry over; recording starts afresh with every call.
        let selves = selves.unwrap_or_else(|| *self.voice.idle.lock());
        let selves = Selves { mute: selves.mute, deaf: selves.deaf, ..Default::default() };
        let id = {
            let mut next = self.voice.next_id.lock();
            *next += 1;
            *next
        };
        let (commands, commands_rx) = mpsc::unbounded_channel();
        let (selves_tx, selves_rx) = watch::channel(selves);
        // A few frames of each size; more waits for the next keyframe.
        let (filmed, filmed_rx) = mpsc::channel(32);
        let sound = match pipes {
            Some((microphone, speakers)) => Sound { microphone, speakers, devices: None },
            None => {
                let (microphone, speakers) = (Arc::new(Pipe::microphone()), Arc::new(Pipe::speakers()));
                let devices = Devices::open(microphone.clone(), speakers.clone(), !selves.mute && !selves.deaf);
                Sound { microphone, speakers, devices: Some(devices) }
            }
        };
        let microphone = match &sound.devices {
            Some(devices) => devices.listener(),
            None => Listener::detached(!selves.mute && !selves.deaf),
        };
        *self.voice.active.lock() = Some(Active {
            id,
            microphone,
            commands,
            selves: selves_tx,
            camera: None,
            screen: None,
            screen_sound: None,
            filmed,
        });
        *self.voice.ended.lock() = None;
        let (server_id, channel_id, conversation_id) = match &place {
            Place::Voice { server_id, channel_id } => (server_id.clone(), channel_id.clone(), String::new()),
            Place::Dm { conversation_id } => (String::new(), String::new(), conversation_id.clone()),
        };
        *self.voice.view.lock() = Some(CallView {
            instance: instance.to_string(),
            server_id,
            channel_id,
            conversation_id,
            status: Status::Connecting,
            self_mute: selves.mute,
            self_deaf: selves.deaf,
            self_video: false,
            self_stream: false,
            self_record: false,
            server_record: false,
            server_recordings: false,
            instance_camera: ceiling::Ceiling::NONE,
            since: None,
            speaking: HashSet::new(),
            trouble: vec![],
            quality: Quality::default(),
            video_suppress: false,
            video_off: false,
            video_trouble: None,
            screen_sound_offered: false,
            screen_sound: None,
            screen_sounds: HashSet::new(),
            quiet_screens: HashSet::new(),
        });
        self.voice.volumes.lock().quiet_screens.clear();
        self.apply_volumes();
        self.shared.update(|_| ());
        reports::used(if matches!(place, Place::Dm { .. }) { "call.join_dm" } else { "call.join_voice" });
        let target = Target { instance: instance.to_string(), place, me };
        let core = Arc::downgrade(self);
        let volumes = (self.voice.volumes.clone(), self.voice.push.clone());
        let videos = self.voice.videos.clone();
        // The instance's ceiling on cameras can change during the call.
        let watcher = Arc::downgrade(self);
        self.runtime.spawn(async move {
            loop {
                tokio::time::sleep(SETTINGS_EVERY).await;
                let Some(core) = watcher.upgrade() else { return };
                if core.voice.active.lock().as_ref().is_none_or(|a| a.id != id) {
                    return;
                }
                let Some(api) = core.call().and_then(|v| core.api(&v.instance)) else { return };
                drop(core);
                let Ok(settings) = rpc!(api.calls(), get_call_settings(pb::GetCallSettingsRequest {})).await else {
                    continue;
                };
                let Some(core) = watcher.upgrade() else { return };
                core.call_view(id, |v| v.instance_camera = instance_camera(&settings));
            }
        });
        self.runtime.spawn(async move {
            let ended =
                run(core.clone(), id, api, target, sound, commands_rx, selves_rx, volumes, (videos, filmed_rx)).await;
            if let Some(core) = core.upgrade() {
                core.finish(id, ended);
            }
        });
    }

    /// Leaves the call you're in.
    pub fn leave_voice(&self) {
        let active = self.voice.active.lock().take();
        if let Some(active) = active {
            // Closed now, not once the call has wound down.
            active.microphone.listen(false);
            let _ = active.commands.send(Command::Leave);
            self.cue(Cue::Disconnect);
        }
        if let Some(view) = self.voice.view.lock().take() {
            *self.voice.idle.lock() = Selves { mute: view.self_mute, deaf: view.self_deaf, ..Default::default() };
            self.shared.update(|_| ());
        }
    }

    pub fn set_self_mute(&self, mute: bool) {
        self.cue(if mute { Cue::Mute } else { Cue::Unmute });
        self.set_selves(|s| {
            s.mute = mute;
            // Unmuting while deafened undeafens too, as it does on the web.
            if !mute {
                s.deaf = false;
            }
        });
    }

    /// Mutes you without a cue: the microphone didn't open (the web's
    /// engine does the same when the browser gives it none).
    pub fn mute_for_no_microphone(&self) {
        self.set_selves(|s| s.mute = true);
    }

    pub fn set_self_deaf(&self, deaf: bool) {
        self.cue(if deaf { Cue::Deafen } else { Cue::Undeafen });
        self.set_selves(|s| s.deaf = deaf);
    }

    /// Starts or stops recording the call's sound on this computer (the web's
    /// `setRecording`): everyone in it sees that you are, and stopping saves
    /// an Ogg Opus file in your downloads.
    pub fn set_recording(&self, on: bool) {
        if self.voice.view.lock().as_ref().is_none_or(|v| v.self_record == on) {
            return;
        }
        self.set_selves(|s| s.record = on);
        self.cue(if on { Cue::Recording } else { Cue::Mute });
    }

    /// Plays one of a call's cues, when call sounds are on (the web's `cue`).
    pub fn cue(&self, sound: Cue) {
        let p = self.prefs();
        if !p.sounds.call || (p.streamer_mode && p.streamer_mute_sounds) {
            return;
        }
        super::sounds::play(sound, p.volume, &p.output_device);
    }

    /// Starts or stops recording the voice channel you're in on the server
    /// (the web's `setServerRecording`).
    pub fn set_server_recording(&self, on: bool) {
        let Some(view) = self.call() else { return };
        if view.is_dm() || view.server_record == on {
            return;
        }
        if on && !view.server_recordings {
            self.notice(t("workspace.calls.noServerRecord"));
            return;
        }
        self.set_selves(|s| s.server_record = on);
        self.cue(if on { Cue::Recording } else { Cue::Mute });
        if !on {
            self.notice(t("workspace.calls.serverRecordStopped"));
        }
    }

    /// Someone's volume, or the call's, changed in the settings: the call hears it at once.
    pub fn apply_volumes(&self) {
        let Some(instance) = self.voice.view.lock().as_ref().map(|v| v.instance.clone()) else { return };
        let prefs = self.prefs();
        let prefix = format!("{instance}/");
        let people = prefs
            .user_volumes
            .iter()
            .filter_map(|(key, v)| key.strip_prefix(&prefix).map(|id| (id.to_owned(), f32::from(*v) / 100.0)))
            .collect();
        let mut volumes = self.voice.volumes.lock();
        *volumes = Volumes {
            people,
            output: f32::from(prefs.output_volume) / 100.0,
            input: f32::from(prefs.input_volume) / 100.0,
            ptt: prefs.input_mode == super::config::InputMode::Ptt,
            release: Duration::from_millis(u64::from(prefs.ptt_release)),
            quiet_screens: std::mem::take(&mut volumes.quiet_screens),
            processing: Choice::of(&prefs),
        };
    }

    /// Turns someone's shared screen's sound off for you, or back on (the
    /// web's `toggleScreenQuiet`): for this call only.
    pub fn toggle_screen_quiet(&self, user_id: &str) {
        let quiet = {
            let mut volumes = self.voice.volumes.lock();
            if !volumes.quiet_screens.remove(user_id) {
                volumes.quiet_screens.insert(user_id.to_owned());
            }
            volumes.quiet_screens.clone()
        };
        if let Some(view) = self.voice.view.lock().as_mut() {
            view.quiet_screens = quiet;
        }
        self.shared.update(|_| ());
    }

    /// Push to talk's key went down or up (the web's `setPushing`).
    pub fn set_pushing(&self, down: bool) {
        let mut push = self.voice.push.lock();
        if push.down == down {
            return;
        }
        push.down = down;
        if !down {
            push.released = Some(Instant::now());
        }
    }

    /// Whether you're talking through push to talk right now: for the user panel.
    pub fn pushing(&self) -> bool {
        self.voice.push.lock().down
    }

    fn set_selves(&self, f: impl FnOnce(&mut Selves)) {
        let Some((active, microphone)) =
            self.voice.active.lock().as_ref().map(|a| (a.selves.clone(), a.microphone.clone()))
        else {
            f(&mut self.voice.idle.lock());
            self.shared.update(|_| ());
            return;
        };
        active.send_modify(f);
        let selves = *active.borrow();
        microphone.listen(!selves.mute && !selves.deaf);
        if let Some(view) = self.voice.view.lock().as_mut() {
            view.self_mute = selves.mute;
            view.self_deaf = selves.deaf;
            view.self_video = selves.video;
            view.self_stream = selves.stream;
            view.self_record = selves.record;
            view.server_record = selves.server_record;
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
                before.status != view.status
                    || before.speaking != view.speaking
                    || before.trouble != view.trouble
                    || before.quality != view.quality
                    || before.server_recordings != view.server_recordings
                    || before.instance_camera != view.instance_camera
                    || before.since != view.since
                    || before.screen_sound_offered != view.screen_sound_offered
                    || before.screen_sounds != view.screen_sounds
            }
            None => false,
        };
        if changed {
            self.shared.update(|_| ());
        }
    }

    /// The server stopped recording for you (RECORD went away, the
    /// recordings filled up): the button goes off, and says why.
    fn server_record_stopped(&self, id: u64, why: String) {
        if self.voice.active.lock().as_ref().is_none_or(|a| a.id != id) {
            return;
        }
        self.set_selves(|s| s.server_record = false);
        self.notice(why);
    }

    fn finish(&self, id: u64, ended: Option<Ended>) {
        {
            let mut active = self.voice.active.lock();
            if active.as_ref().is_none_or(|a| a.id != id) {
                return;
            }
            *active = None;
        }
        if let Some(view) = self.voice.view.lock().take() {
            *self.voice.idle.lock() = Selves { mute: view.self_mute, deaf: view.self_deaf, ..Default::default() };
        }
        *self.voice.ended.lock() = ended;
        self.shared.update(|_| ());
    }
}

struct Target {
    instance: String,
    place: Place,
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

/// A direct message's call keeps its secret current: every few seconds,
/// and at once when someone's frames say there's a newer epoch.
struct Secrets {
    latest: watch::Receiver<(u64, Vec<u8>)>,
    want: mpsc::UnboundedSender<u64>,
    _task: AbortOnDrop,
}

async fn first_secret(core: &std::sync::Weak<Core>, target: &Target) -> Result<Option<Secrets>, Ended> {
    let Place::Dm { conversation_id } = &target.place else { return Ok(None) };
    let engine = core.upgrade().and_then(|c| c.dm_engine(&target.instance));
    let Some(engine) = engine else { return Err(Ended::Refused(t("workspace.calls.dmsNotReady"))) };
    let first = engine.call_secret(conversation_id, None).await.map_err(|e| Ended::Refused(e.0))?;
    let (latest_tx, latest) = watch::channel(first);
    let (want, mut wanted) = mpsc::unbounded_channel::<u64>();
    let conversation = conversation_id.clone();
    let task = tokio::spawn(async move {
        loop {
            let seen = tokio::select! {
                _ = tokio::time::sleep(SECRET_EVERY) => None,
                Some(epoch) = wanted.recv() => Some(epoch),
            };
            if let Ok(got) = engine.call_secret(&conversation, seen).await
                && *latest_tx.borrow() != got
            {
                let _ = latest_tx.send(got);
            }
        }
    });
    Ok(Some(Secrets { latest, want, _task: AbortOnDrop(task) }))
}

/// The instance's ceiling on cameras, from its call settings.
fn instance_camera(settings: &pb::GetCallSettingsResponse) -> ceiling::Ceiling {
    ceiling::Ceiling { height: settings.camera_max_height, fps: settings.camera_max_fps }
}

/// The call's sound, recorded on this computer while you ask: what you hear
/// and what you say, together, as Opus.
struct Recorder {
    encoder: opus::Encoder,
    packets: Vec<Vec<u8>>,
    started: chrono::DateTime<chrono::Local>,
}

impl Recorder {
    fn new() -> Option<Self> {
        let encoder = opus::Encoder::new(sound::RATE, opus::Channels::Mono, opus::Application::Audio).ok()?;
        Some(Self { encoder, packets: Vec::new(), started: chrono::Local::now() })
    }

    fn add(&mut self, frame: &[f32; FRAME]) {
        if let Ok(packet) = self.encoder.encode_vec_float(frame, 4000) {
            self.packets.push(packet);
        }
    }

    /// Writes the file into the downloads folder, named for when it started.
    fn save(mut self) -> Option<String> {
        if self.packets.is_empty() {
            return None;
        }
        let pre_skip = self.encoder.get_lookahead().ok().and_then(|n| u16::try_from(n).ok()).unwrap_or(312);
        let bytes = super::voice_notes::write_ogg(&self.packets, pre_skip);
        let when = self.started.format("%Y-%m-%d %H.%M").to_string();
        let name = super::i18n::t_with("workspace.calls.fileName", &[("when", super::i18n::Arg::Str(&when))]);
        let dir = dirs::download_dir().or_else(dirs::home_dir).unwrap_or_default();
        let name: String = name.chars().map(|c| if "\\/:*?\"<>|".contains(c) { '-' } else { c }).collect();
        let path = dir.join(format!("{name}.ogg"));
        std::fs::write(&path, bytes).ok()?;
        Some(t("workspace.calls.recordingSaved"))
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(
    core: std::sync::Weak<Core>,
    id: u64,
    api: Api,
    target: Target,
    mut sound: Sound,
    mut commands: mpsc::UnboundedReceiver<Command>,
    selves: watch::Receiver<Selves>,
    (volumes, push): (Arc<Mutex<Volumes>>, Arc<Mutex<Push>>),
    (videos, mut filmed): (Arc<Videos>, mpsc::Receiver<Filmed>),
) -> Option<Ended> {
    let view = |f: &dyn Fn(&mut CallView)| {
        if let Some(core) = core.upgrade() {
            core.call_view(id, f);
        }
    };
    let stopped = |why: String| {
        if let Some(core) = core.upgrade() {
            core.server_record_stopped(id, why);
        }
    };
    let video_state = |state: pb::VoiceState| {
        if let Some(core) = core.upgrade() {
            core.video_state(id, &state);
        }
    };
    let say = |what: Option<Cue>, text: Option<String>| {
        if let Some(core) = core.upgrade() {
            if let Some(sound) = what {
                core.cue(sound);
            }
            if let Some(text) = text {
                core.notice(text);
            }
        }
    };
    // Calls switched off on the instance, and whether it records on the server.
    let settings = tokio::select! {
        settings = rpc!(api.calls(), get_call_settings(pb::GetCallSettingsRequest {})) => settings.ok(),
        Some(Command::Leave) = commands.recv() => return None,
    };
    if let Some(settings) = &settings {
        if !settings.enabled {
            return Some(Ended::Refused(t("workspace.calls.switchedOff")));
        }
        let (recordings, camera, screen_sound) =
            (settings.recordings, instance_camera(settings), settings.screen_sound);
        view(&|v| {
            v.server_recordings = recordings;
            v.instance_camera = camera;
            v.screen_sound_offered = screen_sound;
        });
    }
    // An instance that doesn't take a screen's sound gets no place for it in the offer.
    let screen_sound = settings.as_ref().is_some_and(|s| s.screen_sound);
    let secrets = tokio::select! {
        secrets = first_secret(&core, &target) => secrets,
        Some(Command::Leave) = commands.recv() => return None,
    };
    let mut secrets = match secrets {
        Ok(secrets) => secrets,
        Err(ended) => return Some(ended),
    };
    let mut frames = secrets.as_ref().map(|s| {
        let (epoch, secret) = s.latest.borrow().clone();
        Frames::new(&target.me, epoch, secret)
    });
    let mut recorder: Option<Recorder> = None;
    let mut session = String::new();
    let mut backoff = Duration::from_millis(500);
    // Rejoins straight after a restart without a connection that lasted in
    // between: only the first is quick.
    let mut quick_rejoins = 0u32;
    let started = Instant::now();
    let mut ever_connected = false;
    let ended = loop {
        let (mut link, offer) = Link::offer(screen_sound);
        let now = *selves.borrow();
        let joined = match &target.place {
            Place::Voice { server_id, channel_id } => {
                let request = pb::JoinVoiceRequest {
                    server_id: server_id.clone(),
                    channel_id: channel_id.clone(),
                    offer,
                    self_mute: now.mute || now.deaf,
                    self_deaf: now.deaf,
                    self_video: now.video,
                    self_stream: now.stream,
                    self_record: now.record,
                    server_record: now.server_record,
                    session_id: session.clone(),
                };
                tokio::select! {
                    joined = rpc!(api.calls(), join_voice(request)) => joined.map(|j| {
                        if j.recordings_full && now.server_record {
                            stopped(t("workspace.calls.recordingsFull"));
                        }
                        (j.session_id, j.answer, j.state)
                    }),
                    Some(Command::Leave) = commands.recv() => break None,
                }
            }
            Place::Dm { conversation_id } => {
                let request = pb::JoinDmCallRequest {
                    conversation_id: conversation_id.clone(),
                    offer,
                    self_mute: now.mute || now.deaf,
                    self_deaf: now.deaf,
                    self_video: now.video,
                    self_stream: now.stream,
                    self_record: now.record,
                    session_id: session.clone(),
                };
                tokio::select! {
                    joined = rpc!(api.calls(), join_dm_call(request)) => joined.map(|j| (j.session_id, j.answer, j.state)),
                    Some(Command::Leave) = commands.recv() => break None,
                }
            }
        };
        let soon = match joined {
            Ok((session_id, answer, state)) => {
                session = session_id;
                if let Some(state) = state {
                    video_state(state);
                }
                let connecting = tokio::select! {
                    connected = link.connect(&answer) => connected,
                    Some(Command::Leave) = commands.recv() => break None,
                };
                let (outcome, connected) = match connecting {
                    Ok(()) => {
                        let mut call = Running {
                            api: &api,
                            target: &target,
                            session: &session,
                            sound: &mut sound,
                            commands: &mut commands,
                            selves: selves.clone(),
                            view: &view,
                            stopped: &stopped,
                            say: &say,
                            volumes: &volumes,
                            push: &push,
                            frames: &mut frames,
                            secrets: &mut secrets,
                            recorder: &mut recorder,
                            videos: &videos,
                            filmed: &mut filmed,
                            video_state: &video_state,
                            core: &core,
                        };
                        call.drive(&mut link).await
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
            v.screen_sounds.clear();
            v.quality = Quality::default();
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
    // Hanging up ends a recording and saves it.
    if let Some(saved) = recorder.take().and_then(Recorder::save)
        && let Some(core) = core.upgrade()
    {
        core.notice(saved);
    }
    if !session.is_empty() && ended.is_none() {
        match &target.place {
            Place::Voice { server_id, .. } => {
                let request = pb::LeaveVoiceRequest { server_id: server_id.clone(), session_id: session.clone() };
                let _ = rpc!(api.calls(), leave_voice(request)).await;
            }
            Place::Dm { conversation_id } => {
                let request =
                    pb::LeaveDmCallRequest { conversation_id: conversation_id.clone(), session_id: session.clone() };
                let _ = rpc!(api.calls(), leave_dm_call(request)).await;
            }
        }
    }
    ended
}

/// A number in [0, 1) for jitter.
fn rand_unit() -> f64 {
    let mut bytes = [0u8; 4];
    let _ = getrandom::fill(&mut bytes);
    f64::from(u32::from_le_bytes(bytes)) / (f64::from(u32::MAX) + 1.0)
}

/// Changes the call's view, if the call is still the one going.
type ViewFn<'a> = dyn Fn(&dyn Fn(&mut CallView)) + Sync + 'a;
/// The server stopped recording for you, and why.
type StoppedFn<'a> = dyn Fn(String) + Sync + 'a;
/// A cue to play, or a notice to show.
type SayFn<'a> = dyn Fn(Option<Cue>, Option<String>) + Sync + 'a;

/// What one connection runs with.
struct Running<'a> {
    api: &'a Api,
    target: &'a Target,
    session: &'a str,
    sound: &'a mut Sound,
    commands: &'a mut mpsc::UnboundedReceiver<Command>,
    selves: watch::Receiver<Selves>,
    view: &'a ViewFn<'a>,
    stopped: &'a StoppedFn<'a>,
    say: &'a SayFn<'a>,
    volumes: &'a Arc<Mutex<Volumes>>,
    push: &'a Arc<Mutex<Push>>,
    frames: &'a mut Option<Frames>,
    secrets: &'a mut Option<Secrets>,
    recorder: &'a mut Option<Recorder>,
    videos: &'a Arc<Videos>,
    filmed: &'a mut mpsc::Receiver<Filmed>,
    video_state: &'a (dyn Fn(pb::VoiceState) + Sync),
    core: &'a std::sync::Weak<Core>,
}

impl Running<'_> {
    /// Runs one connection until it ends, you leave, or the call does.
    async fn drive(&mut self, link: &mut Link) -> (Outcome, Option<Instant>) {
        let mut connected = None;
        let mut watchers = HashMap::new();
        let outcome = self.drive_until(link, &mut connected, &mut watchers).await;
        // The others' cameras go with the connection; yours stay.
        drop(watchers);
        let me = self.target.me.clone();
        self.videos.forget_all_but(|feed| owner_of(feed) == me);
        (outcome, connected)
    }

    /// A frame of someone's camera or screen: opened in a direct message's
    /// call, then handed to its feed's decoder.
    fn seen(
        &mut self,
        link: &mut Link,
        watchers: &mut HashMap<String, (Watcher, Option<Instant>)>,
        feed: String,
        frame: Vec<u8>,
        contiguous: bool,
    ) -> bool {
        let frame = match self.frames.as_mut() {
            None => frame,
            Some(frames) => match frames.open_video(owner_of(&feed), &frame) {
                Opened::Plain(plain) => plain,
                Opened::Newer(epoch) => {
                    if let Some(s) = self.secrets.as_ref() {
                        let _ = s.want.send(u64::from(epoch));
                    }
                    return false;
                }
                Opened::Dropped => return false,
            },
        };
        let new = !watchers.contains_key(&feed);
        if new {
            let Some(watcher) = Watcher::start(feed.clone(), self.videos.clone()) else { return false };
            watchers.insert(feed.clone(), (watcher, None));
        }
        let Some((watcher, asked)) = watchers.get_mut(&feed) else { return false };
        let kept = watcher.give(frame, contiguous);
        if (!kept || watcher.waiting()) && asked.is_none_or(|at| at.elapsed() >= KEYFRAME_ASK) {
            *asked = Some(Instant::now());
            link.ask_keyframe(&feed);
        }
        new
    }

    async fn drive_until(
        &mut self,
        link: &mut Link,
        connected: &mut Option<Instant>,
        watchers: &mut HashMap<String, (Watcher, Option<Instant>)>,
    ) -> Outcome {
        let started = Instant::now();
        let Ok(mut microphone) = Microphone::new() else {
            return Outcome::Ended(Ended::Refused("Opus didn't start.".into()));
        };
        let mut screen_sound = screen_sound::Encoder::new().ok();
        let mut mixer = Mixer::default();
        let mut processing = Processing::new(self.volumes.lock().processing);
        let (kept_tx, mut kept) = mpsc::channel(4);
        let keeper = tokio::spawn(keep(
            self.api.clone(),
            self.target.place.clone(),
            self.session.to_string(),
            self.selves.clone(),
            kept_tx,
        ));
        let _keeper = AbortOnDrop(keeper);
        let mut tick = tokio::time::interval(Duration::from_millis(20));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
        let mut happened = Vec::new();
        let mut broken_since: Option<Instant> = None;
        let mut trouble_seen = vec![];
        let mut out = [0.0f32; FRAME];
        let mut quality = Quality::default();
        let mut heard: HashSet<String> = HashSet::new();
        // Shared screens' sound, by its stream, and when a packet last came.
        let mut screens_heard: HashMap<String, Instant> = HashMap::new();
        let mut wanted = self.videos.wanted_changes();
        // Until it's said, the media part sends the smallest size.
        let mut layers_due: Option<Instant> = Some(Instant::now());
        loop {
            let wake = link.poll(&mut happened).await;
            for event in happened.drain(..) {
                match event {
                    Happened::Seen { feed, frame, contiguous } => {
                        // A new feed: say what size of it to send.
                        if self.seen(link, watchers, feed, frame, contiguous) {
                            layers_due.get_or_insert_with(|| Instant::now() + LAYERS_AFTER);
                        }
                    }
                    Happened::Unseen(feed) => {
                        watchers.remove(&feed);
                        self.videos.forget(&feed);
                    }
                    Happened::KeyframeAsked { screen, rid } => self.keyframe(screen, rid.as_deref()),
                    Happened::Heard(who, packet) => {
                        if is_screen(&who) {
                            // A shared screen's sound coming: its sound button shows.
                            if screens_heard.insert(who.clone(), Instant::now()).is_none() {
                                let owner = owner_of(&who).to_owned();
                                (self.view)(&|v| {
                                    v.screen_sounds.insert(owner.clone());
                                });
                            }
                        } else if heard.insert(who.clone()) && connected.is_some_and(|at| at.elapsed() > SETTLED) {
                            // Someone new in a call that's been going a moment: a cue.
                            (self.say)(Some(Cue::SomeoneJoined), None);
                        }
                        if self.selves.borrow().deaf {
                            continue;
                        }
                        match self.frames.as_mut() {
                            None => mixer.hear(&who, &packet),
                            // A screen's sound is sealed by its sharer, like their voice.
                            Some(frames) => match frames.open(owner_of(&who), &packet) {
                                Opened::Plain(plain) => mixer.hear(&who, &plain),
                                Opened::Newer(epoch) => {
                                    if let Some(s) = self.secrets.as_ref() {
                                        let _ = s.want.send(u64::from(epoch));
                                    }
                                }
                                Opened::Dropped => {}
                            },
                        }
                    }
                    Happened::Gone(who) => {
                        mixer.forget(&who);
                        if screens_heard.remove(&who).is_some() {
                            let owner = owner_of(&who).to_owned();
                            (self.view)(&|v| {
                                v.screen_sounds.remove(&owner);
                            });
                        } else if heard.remove(&who) {
                            (self.say)(Some(Cue::SomeoneLeft), None);
                        }
                    }
                    Happened::Connected => {
                        broken_since = None;
                        // Every camera starts each viewer on a keyframe of each size.
                        self.keyframe(false, None);
                        self.keyframe(true, None);
                        if connected.is_none() {
                            *connected = Some(Instant::now());
                            reports::timing("call.connect", started.elapsed());
                            (self.view)(&|v| {
                                v.status = Status::Connected;
                                v.since.get_or_insert_with(Instant::now);
                            });
                            (self.say)(Some(Cue::Connect), None);
                        }
                    }
                    Happened::Restored => broken_since = None,
                    Happened::Broken { for_good: true } => return Outcome::Rejoin { soon: false },
                    Happened::Broken { for_good: false } => {
                        broken_since.get_or_insert_with(Instant::now);
                    }
                    Happened::Stats(sample) => {
                        quality.take(sample);
                        let q = quality.clone();
                        (self.view)(&|v| v.quality = q.clone());
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
            // New cameras, or windows showing them at another size.
            let mut layers_wake = wake;
            if let Some(due) = layers_due {
                if Instant::now() >= due {
                    let videos = self.videos.clone();
                    layers_due = (!link.say_layers(|feed| layer_for(videos.wanted(feed))))
                        .then(|| Instant::now() + LAYERS_AFTER);
                } else {
                    layers_wake = wake.min(due);
                }
            }
            let sleep = tokio::time::sleep_until(tokio::time::Instant::from_std(layers_wake.min(wake)));
            tokio::select! {
                packet = link.packet() => link.receive(packet),
                _ = sleep => link.tick(),
                Ok(()) = wanted.changed() => {
                    layers_due.get_or_insert_with(|| Instant::now() + LAYERS_AFTER);
                }
                _ = tick.tick() => {
                    self.tick(link, &mut microphone, &mut processing, &mut mixer, &mut out, &mut trouble_seen);
                    self.share_sound(link, screen_sound.as_mut());
                    // Screens whose sound stopped coming: their buttons go.
                    let quiet: Vec<String> = screens_heard
                        .iter()
                        .filter(|(_, at)| at.elapsed() > SCREEN_SOUND_GONE)
                        .map(|(who, _)| who.clone())
                        .collect();
                    for who in quiet {
                        screens_heard.remove(&who);
                        mixer.forget(&who);
                        let owner = owner_of(&who).to_owned();
                        (self.view)(&|v| {
                            v.screen_sounds.remove(&owner);
                        });
                    }
                }
                Some(filmed) = self.filmed.recv() => self.film(link, filmed),
                kept = kept.recv() => match kept {
                    Some(Kept::State(state)) => (self.video_state)(*state),
                    Some(Kept::Gone) => return Outcome::Ended(Ended::Disconnected),
                    Some(Kept::Lost) => return Outcome::Rejoin { soon: true },
                    Some(Kept::Stopped(why)) => (self.stopped)(why),
                    None => {}
                },
                Some(Command::Leave) = self.commands.recv() => return Outcome::Left,
            }
        }
    }

    /// Asks your camera (or screen) for a keyframe of one size, or of all.
    fn keyframe(&self, screen: bool, rid: Option<&str>) {
        let Some(core) = self.core.upgrade() else { return };
        let active = core.voice.active.lock();
        let sending = active.as_ref().and_then(|a| if screen { a.screen.as_ref() } else { a.camera.as_ref() });
        if let Some(sending) = sending {
            sending.keyframe(rid);
        }
    }

    /// A frame of your camera or screen goes out, sealed in a direct message's call.
    fn film(&mut self, link: &mut Link, filmed: Filmed) {
        let now = *self.selves.borrow();
        if !link.is_connected() || !(if filmed.screen { now.stream } else { now.video }) {
            return;
        }
        let frame = match self.frames.as_mut() {
            Some(frames) => match frames.seal_video(&filmed.frame) {
                Some(sealed) => sealed,
                None => return,
            },
            None => filmed.frame,
        };
        link.film(filmed.screen, filmed.rid, filmed.taken, frame);
    }

    /// 20 ms of your shared screen's sound out while it brings sound,
    /// sealed in a direct message's call: silence while it's turned off or
    /// nothing plays (as a browser's disabled track sends), so the others
    /// know it's still there; skipped without sound.
    fn share_sound(&mut self, link: &mut Link, encoder: Option<&mut screen_sound::Encoder>) {
        if !link.has_screen_sound() {
            return;
        }
        let sound = self.core.upgrade().and_then(|c| c.voice.active.lock().as_ref()?.screen_sound.clone());
        let packet = sound
            .filter(|_| self.selves.borrow().stream && link.is_connected())
            .and_then(|sound| {
                // Taken whether it goes or not, so it never falls behind.
                let frame = sound.frame().filter(|_| sound.is_on());
                encoder?.encode(&frame.unwrap_or([0.0; FRAME]))
            })
            .and_then(|packet| match self.frames.as_mut() {
                Some(frames) => frames.seal(&packet),
                None => Some(packet),
            });
        link.share_sound(packet);
    }

    /// 20 ms of the call: the microphone out, everyone else mixed in, who's speaking.
    fn tick(
        &mut self,
        link: &mut Link,
        microphone: &mut Microphone,
        processing: &mut Processing,
        mixer: &mut Mixer,
        out: &mut [f32; FRAME],
        trouble_seen: &mut Vec<Trouble>,
    ) {
        let now = *self.selves.borrow();
        let volumes = self.volumes.lock().clone();
        mixer.set_gains(&volumes.gains());
        processing.set(volumes.processing);
        if let (Some(frames), Some(secrets)) = (self.frames.as_mut(), self.secrets.as_mut())
            && secrets.latest.has_changed().unwrap_or(false)
        {
            let (epoch, secret) = secrets.latest.borrow_and_update().clone();
            frames.set_secret(epoch, secret);
        }
        let mut speaking = HashSet::new();
        // A microphone running a touch faster than this clock would pile up
        // delay: past 160 ms behind (more than a device hands over at once),
        // the oldest goes.
        while self.sound.microphone.len() > FRAME * 8 {
            let _ = self.sound.microphone.frame();
        }
        let mut said = None;
        // What's waiting at both ends: echo cancellation's first guess at
        // how long the speakers take to reach the microphone.
        let delay = processing::ms_of(self.sound.microphone.len() + FRAME + self.sound.speakers.len());
        if let Some(mut frame) = self.sound.microphone.frame() {
            processing.clean(&mut frame, delay);
            if volumes.input != 1.0 {
                for s in frame.iter_mut() {
                    *s = (*s * volumes.input).clamp(-1.0, 1.0);
                }
            }
            let held = !volumes.ptt || self.push.lock().open(volumes.release);
            if now.mute || now.deaf || !held || !link.is_connected() {
                microphone.speaking.stop();
                link.skip();
            } else if let Some(packet) = microphone.encode(&frame) {
                let packet = match self.frames.as_mut() {
                    Some(frames) => frames.seal(&packet),
                    None => Some(packet),
                };
                match packet {
                    Some(packet) => link.speak(packet),
                    None => link.skip(),
                }
                if microphone.speaking.is_on() {
                    speaking.insert(self.target.me.clone());
                }
                said = Some(frame);
            }
        }
        if now.deaf {
            mixer.clear();
            self.sound.speakers.clear();
            out.fill(0.0);
        } else {
            for who in mixer.mix(out) {
                // A shared screen's sound isn't its sharer speaking.
                if !who.ends_with("-screen") {
                    speaking.insert(who);
                }
            }
            if volumes.output != 1.0 {
                for s in out.iter_mut() {
                    *s = (*s * volumes.output).clamp(-1.0, 1.0);
                }
            }
            self.sound.speakers.push(out);
        }
        processing.heard(out);
        self.record(now.record, out, said.as_ref());
        let trouble = self.sound.devices.as_ref().map(|d| d.trouble()).unwrap_or_default();
        if trouble != *trouble_seen {
            for t in trouble.iter().filter(|t| !trouble_seen.contains(t)) {
                reports::error(
                    "call_device",
                    match t {
                        Trouble::NoMicrophone => "microphone",
                        Trouble::MicrophoneBlocked => "microphone_blocked",
                        Trouble::NoSpeakers => "speakers",
                    },
                );
            }
            *trouble_seen = trouble.clone();
        }
        (self.view)(&|v| {
            v.speaking = speaking.clone();
            v.trouble = trouble.clone();
        });
    }

    /// Recording on this computer: the call as you hear it, with your own voice.
    fn record(&mut self, on: bool, heard: &[f32; FRAME], said: Option<&[f32; FRAME]>) {
        if !on {
            if let Some(saved) = self.recorder.take().and_then(Recorder::save) {
                (self.say)(None, Some(saved));
            }
            return;
        }
        if self.recorder.is_none() {
            *self.recorder = Recorder::new();
        }
        let Some(recorder) = self.recorder.as_mut() else { return };
        let mut frame = *heard;
        if let Some(said) = said {
            for (a, b) in frame.iter_mut().zip(said) {
                *a = (*a + b).clamp(-1.0, 1.0);
            }
        }
        recorder.add(&frame);
    }
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

enum Kept {
    /// How the instance has you now (VIDEO taken away, a moderator's doing).
    State(Box<pb::VoiceState>),
    /// The place is gone for good.
    Gone,
    /// The media part lost the place (it restarted): join again.
    Lost,
    /// The server stopped recording for you, and why.
    Stopped(String),
}

/// Keeps the place every few seconds, and at once when how you are changes.
async fn keep(
    api: Api,
    place: Place,
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
        let told = kept.clone();
        let result = match &place {
            Place::Voice { server_id, channel_id } => {
                let request = pb::KeepVoiceRequest {
                    server_id: server_id.clone(),
                    session_id: session_id.clone(),
                    channel_id: channel_id.clone(),
                    self_mute: now.mute || now.deaf,
                    self_deaf: now.deaf,
                    self_video: now.video,
                    self_stream: now.stream,
                    self_record: now.record,
                    server_record: now.server_record,
                };
                rpc!(api.calls(), keep_voice(request)).await.map(|kept| {
                    let state = kept.state.unwrap_or_default();
                    let _ = told.try_send(Kept::State(Box::new(state.clone())));
                    // RECORD went away, or the instance stopped recording on the server.
                    (now.server_record && !state.server_record).then(|| {
                        if state.record_suppress {
                            t("workspace.calls.noRecordAnyMore")
                        } else if kept.recordings_full {
                            t("workspace.calls.recordingsFull")
                        } else if kept.recording_ended {
                            t("workspace.calls.recordingEnded")
                        } else {
                            t("workspace.calls.serverStopped")
                        }
                    })
                })
            }
            Place::Dm { conversation_id } => {
                let request = pb::KeepDmCallRequest {
                    conversation_id: conversation_id.clone(),
                    session_id: session_id.clone(),
                    self_mute: now.mute || now.deaf,
                    self_deaf: now.deaf,
                    self_video: now.video,
                    self_stream: now.stream,
                    self_record: now.record,
                };
                rpc!(api.calls(), keep_dm_call(request)).await.map(|kept| {
                    let _ = told.try_send(Kept::State(Box::new(kept.state.unwrap_or_default())));
                    None
                })
            }
        };
        match result {
            Ok(None) => {}
            Ok(Some(why)) => {
                let _ = kept.send(Kept::Stopped(why)).await;
            }
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
