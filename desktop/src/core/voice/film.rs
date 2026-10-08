//! Turning your camera and shared screen on and off in the call you're in
//! (the web engine's `setCamera` and `setScreen`), and what the instance
//! says about them: VIDEO taken away in the channel, or a moderator turning
//! them off, which turns them off here too and says why. A camera goes out
//! at the lowest of your choice, the instance's ceiling and the server's,
//! and opens again when one of them, or the camera picked, changes.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::capture::{FAKE_VIDEO, Failure, Sending, Source};
use super::ceiling::Ceiling;
use super::video::feed_of;
use super::{CAMERA_TEST, CallView, Cue};
use crate::core::Core;
use crate::core::i18n::t;
use crate::pb;

impl Core {
    /// Whether a test pattern stands in for the camera and screen.
    fn pattern(&self) -> bool {
        self.voice.pattern.load(Ordering::Relaxed) || std::env::var_os(FAKE_VIDEO).is_some()
    }

    /// Sends a moving test pattern instead of the camera and screen: for the
    /// tests, and machines without either.
    #[doc(hidden)]
    pub fn use_test_pattern(&self) {
        self.voice.pattern.store(true, Ordering::Relaxed);
    }

    /// The pattern's tint, so two people's look different.
    fn seed(me: &str) -> u32 {
        me.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b))) % 16384
    }

    /// Your own choice of camera quality (0s for the best).
    fn my_ceiling(&self) -> Ceiling {
        let prefs = self.prefs();
        Ceiling { height: prefs.camera_height, fps: prefs.camera_fps }
    }

    /// The most your camera sends in this call: the lowest of your choice,
    /// the instance's ceiling and, in a voice channel, its server's.
    fn camera_ceiling(&self, view: &CallView) -> Ceiling {
        let server = if view.is_dm() {
            Ceiling { height: 0, fps: 0 }
        } else {
            self.shared.read(|s| {
                let server = s.instance(&view.instance).and_then(|i| i.server(&view.server_id));
                Ceiling {
                    height: server.map_or(0, |s| s.camera_max_height.max(0) as u32),
                    fps: server.map_or(0, |s| s.camera_max_fps.max(0) as u32),
                }
            })
        };
        Ceiling::lowest(self.my_ceiling(), view.instance_camera, server)
    }

    /// Where your camera's pictures come from now.
    fn camera_source(&self, me: &str) -> Source {
        if self.pattern() {
            Source::Pattern { screen: false, seed: Self::seed(me) }
        } else {
            Source::Camera(self.prefs().video_device)
        }
    }

    /// Opens your camera again when what it should go out at changed (your
    /// choice, the instance's or the server's ceiling, or the camera picked)
    /// while it's on: in the call, or the check in settings.
    pub fn watch_camera_ceiling(self: &Arc<Self>) {
        if let Some(view) = self.call().filter(|v| v.self_video) {
            let me = self.shared.read(|s| s.instance(&view.instance).and_then(|i| i.me.as_ref().map(|m| m.id.clone())));
            let ceiling = self.camera_ceiling(&view);
            if let Some(me) = me {
                let source = self.camera_source(&me);
                let stale = self
                    .voice
                    .active
                    .lock()
                    .as_ref()
                    .and_then(|a| a.camera.as_ref())
                    .is_some_and(|c| c.ceiling != ceiling || c.source != source);
                if stale {
                    self.open_video(&me, false, None, ceiling);
                }
            }
        }
        let wanted = (self.test_source(), Ceiling::lowest(self.my_ceiling(), Ceiling::NONE, Ceiling::NONE));
        let stale = self.voice.camera_test.lock().as_ref().is_some_and(|c| (c.source.clone(), c.ceiling) != wanted);
        if stale {
            self.camera_test(true);
        }
    }

    /// Turns your camera on or off in the call you're in. Its place in the
    /// connection stays either way, so it needs no new offer.
    pub fn set_camera(self: &Arc<Self>, on: bool) {
        self.set_video(false, on, None);
    }

    /// Shares a screen or window (`target`, from `capture::screens`; None
    /// for the main screen) in the call you're in, or stops.
    pub fn set_screen(self: &Arc<Self>, on: bool, target: Option<u32>) {
        self.set_video(true, on, target);
    }

    fn set_video(self: &Arc<Self>, screen: bool, on: bool, target: Option<u32>) {
        let Some(view) = self.call() else { return };
        if (if screen { view.self_stream } else { view.self_video }) == on {
            return;
        }
        if on && view.video_suppress {
            self.notice(t(if screen { "workspace.calls.noScreenHere" } else { "workspace.calls.noCameraHere" }));
            return;
        }
        if on && view.video_off {
            self.notice(t(if screen { "workspace.calls.modScreenHere" } else { "workspace.calls.modCameraHere" }));
            return;
        }
        let me = self.shared.read(|s| s.instance(&view.instance).and_then(|i| i.me.as_ref().map(|m| m.id.clone())));
        let Some(me) = me else { return };
        self.voice.mirror.store(self.prefs().mirror_video, Ordering::Relaxed);
        let sending = if on {
            if !self.open_video(&me, screen, target, self.camera_ceiling(&view)) {
                return;
            }
            true
        } else {
            let mut active = self.voice.active.lock();
            let Some(active) = active.as_mut() else { return };
            *(if screen { &mut active.screen } else { &mut active.camera }) = None;
            false
        };
        if on {
            crate::core::reports::used(if screen { "call.screen_share" } else { "call.camera" });
        }
        if let Some(view) = self.voice.view.lock().as_mut() {
            view.video_trouble = None;
        }
        self.set_selves(|s| if screen { s.stream = sending } else { s.video = sending });
        self.cue(if on { Cue::Unmute } else { Cue::Mute });
    }

    /// Starts (or starts again) your camera or screen in the call, giving
    /// whether it's going.
    fn open_video(self: &Arc<Self>, me: &str, screen: bool, target: Option<u32>, ceiling: Ceiling) -> bool {
        let mut active = self.voice.active.lock();
        let Some(active) = active.as_mut() else { return false };
        let slot = if screen { &mut active.screen } else { &mut active.camera };
        *slot = None;
        let source = if !screen {
            self.camera_source(me)
        } else if self.pattern() {
            Source::Pattern { screen, seed: Self::seed(me) }
        } else {
            Source::Screen(target)
        };
        let (core, id) = (Arc::downgrade(self), active.id);
        *slot = Some(Sending::start(
            source,
            ceiling,
            feed_of(me, screen),
            self.voice.videos.clone(),
            self.voice.mirror.clone(),
            Some(active.filmed.clone()),
            move |failure| {
                if let Some(core) = core.upgrade() {
                    core.video_failed(id, screen, failure);
                }
            },
        ));
        true
    }

    /// The camera or screen couldn't start, or stopped: it goes off, and
    /// says why (with the way to the system's settings when they're why).
    fn video_failed(&self, id: u64, screen: bool, failure: Failure) {
        {
            let mut active = self.voice.active.lock();
            let Some(active) = active.as_mut().filter(|a| a.id == id) else { return };
            if screen {
                active.screen = None;
            } else {
                active.camera = None;
            }
        }
        self.set_selves(|s| if screen { s.stream = false } else { s.video = false });
        if failure == Failure::Blocked {
            if let Some(view) = self.voice.view.lock().as_mut() {
                view.video_trouble = Some((screen, failure));
            }
            self.shared.update(|_| ());
            return;
        }
        self.notice(t(match (screen, failure) {
            (true, _) => "workspace.calls.screen.failed",
            (false, Failure::NotFound) => "workspace.calls.camera.notFound",
            (false, Failure::Busy) => "workspace.calls.camera.busy",
            (false, _) => "workspace.calls.camera.failed",
        }));
    }

    /// How the instance has you in the call now: VIDEO taken away turns the
    /// camera and screen off, and so does a moderator, each saying so once.
    pub(super) fn video_state(self: &Arc<Self>, id: u64, state: &pb::VoiceState) {
        let Some(view) = self.call() else { return };
        if self.voice.active.lock().as_ref().is_none_or(|a| a.id != id) {
            return;
        }
        let moderated = state.server_video_off != view.video_off;
        if let Some(v) = self.voice.view.lock().as_mut() {
            v.video_suppress = state.video_suppress;
            v.video_off = state.server_video_off;
        }
        if moderated {
            if !state.server_video_off {
                self.notice(t("workspace.calls.mod.cameraBack"));
            } else {
                self.notice(t(match (view.self_video, view.self_stream) {
                    (true, true) => "workspace.calls.mod.bothOff",
                    (false, true) => "workspace.calls.mod.screenOff",
                    (true, false) => "workspace.calls.mod.cameraOff",
                    (false, false) => "workspace.calls.mod.allOff",
                }));
                self.set_camera(false);
                self.set_screen(false, None);
            }
        }
        if state.video_suppress && view.self_video && !moderated {
            self.notice(t("workspace.calls.noCameraAnyMore"));
            self.set_camera(false);
        }
        if state.video_suppress && view.self_stream && !moderated {
            self.notice(t("workspace.calls.noScreenAnyMore"));
            self.set_screen(false, None);
        }
    }

    /// Notices a moderator's doing the moment its event comes, not at the
    /// next keep (the web's `watchModeration`).
    pub fn watch_video_moderation(self: &Arc<Self>) {
        let Some(view) = self.call().filter(|c| !c.is_dm()) else { return };
        let mine = self.shared.read(|s| {
            let i = s.instance(&view.instance)?;
            let me = i.me.as_ref()?.id.clone();
            i.voice.get(&view.server_id)?.iter().find(|v| v.user_id == me && v.channel_id == view.channel_id).cloned()
        });
        let id = self.voice.active.lock().as_ref().map(|a| a.id);
        if let (Some(mine), Some(id)) = (mine, id)
            && mine.server_video_off != view.video_off
        {
            self.video_state(id, &mine);
        }
    }

    /// Shows your camera in Voice & video settings, outside a call, or stops.
    pub fn camera_test(self: &Arc<Self>, on: bool) {
        let mut test = self.voice.camera_test.lock();
        *test = None;
        if !on {
            return;
        }
        self.voice.mirror.store(self.prefs().mirror_video, Ordering::Relaxed);
        let core = Arc::downgrade(self);
        *test = Some(Sending::start(
            self.test_source(),
            // Your own choice: no instance or server has a say outside a call.
            Ceiling::lowest(self.my_ceiling(), Ceiling::NONE, Ceiling::NONE),
            CAMERA_TEST.into(),
            self.voice.videos.clone(),
            self.voice.mirror.clone(),
            None,
            move |failure| {
                if let Some(core) = core.upgrade() {
                    *core.voice.camera_test.lock() = None;
                    *core.voice.camera_test_failed.lock() = Some(failure);
                    core.shared.update(|_| ());
                }
            },
        ));
        *self.voice.camera_test_failed.lock() = None;
    }

    /// Where the camera check's pictures come from.
    fn test_source(&self) -> Source {
        if self.pattern() {
            Source::Pattern { screen: false, seed: 1 }
        } else {
            Source::Camera(self.prefs().video_device)
        }
    }

    /// Whether the camera check is on, and why it stopped if it did.
    pub fn camera_testing(&self) -> (bool, Option<Failure>) {
        (self.voice.camera_test.lock().is_some(), self.voice.camera_test_failed.lock().clone())
    }

    /// The mirror setting changed: your preview follows at once.
    pub fn apply_mirror(&self) {
        self.voice.mirror.store(self.prefs().mirror_video, Ordering::Relaxed);
    }
}
