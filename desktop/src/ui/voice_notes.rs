//! Voice messages in private conversations, as the web app has them: the
//! microphone takes the send button's place while nothing's typed, a tap
//! records (Esc or the bin throws it away), and each voice message plays
//! from its bubble, its waveform doubling as the place to jump to.

use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, div, px,
};

use crate::core::i18n::{t, t_with};
use crate::core::vault::VoiceFile;
use crate::core::voice::processing::Choice;
use crate::core::voice_notes::{self, Clip, Limits, Player, Progress, Recorder};
use crate::pb;
use crate::ui::app::{FuwaApp, Target};
use crate::ui::chat::Row;
use crate::ui::theme::{Palette, alpha};
use crate::ui::widgets::icon;

/// Bars in a voice message's bubble.
const BUBBLE_BARS: usize = 40;
/// Each bar's width and the gap after it.
const BAR: f32 = 3.0;
const GAP: f32 = 2.0;
/// Bars in the recording bar's live waveform.
const LIVE_BARS: usize = 64;
/// Decoded voice messages kept, so playing one again starts at once.
const SOUNDS_KEPT: usize = 6;

/// Held this long, the mic records only while held; a quicker tap starts and stops it.
const HOLD_MS: u128 = 280;
/// Dragged this far left while holding, the recording is thrown away.
const CANCEL_PX: f32 = 96.0;

/// Something being recorded, for one conversation or channel.
struct Recording {
    target: Target,
    recorder: Recorder,
}

/// The mic held down: when, where the pointer started, and how far it has slid left.
#[derive(Clone, Copy)]
struct Hold {
    since: std::time::Instant,
    x: f32,
    drag: f32,
}

struct Playing {
    media_id: String,
    player: Player,
}

/// What voice messages are doing in the window.
#[derive(Default)]
pub struct VoiceState {
    recording: Option<Recording>,
    /// The mic is held down (recording while held), rather than tapped.
    hold: Option<Hold>,
    /// Waiting for the microphone to open.
    starting: bool,
    sending: bool,
    playing: Option<Playing>,
    loading: Option<String>,
    /// Why one couldn't be played, by media id.
    failed: HashMap<String, String>,
    sounds: VecDeque<(String, Arc<Vec<f32>>)>,
    ticking: bool,
}

/// How a voice message's bubble is.
#[derive(Clone, PartialEq)]
pub enum Play {
    Idle,
    Loading,
    Playing,
    /// Paused this far through (0 to 1).
    Paused(f32),
    Failed(String),
}

/// A voice message as its bubble draws it.
#[derive(Clone)]
pub struct VoiceCard {
    pub file: VoiceFile,
    pub heights: Vec<f32>,
    pub play: Play,
    /// How fast voice messages play (the speed button).
    pub rate: f32,
    /// Where it's playing, read as it draws.
    fraction: Option<Arc<dyn Fn() -> f32 + Send + Sync>>,
}

impl VoiceCard {
    pub fn of(file: &VoiceFile) -> Self {
        Self {
            heights: voice_notes::heights(&file.waveform, BUBBLE_BARS),
            file: file.clone(),
            play: Play::Idle,
            rate: 1.0,
            fraction: None,
        }
    }

    /// What it was drawn from, for the list to know when to draw it again.
    pub fn digest(&self) -> (String, u8, u32, String, u32) {
        let (state, at, why) = match &self.play {
            Play::Idle => (0, 0, String::new()),
            Play::Loading => (1, 0, String::new()),
            Play::Playing => (2, 0, String::new()),
            Play::Paused(f) => (3, (f * 1000.0) as u32, String::new()),
            Play::Failed(why) => (4, 0, why.clone()),
        };
        (self.file.media_id.clone(), state, at, why, self.rate.to_bits())
    }
}

impl FuwaApp {
    /// Voice messages go in private conversations (not secure channels) on an
    /// instance that has them, and in a server's channels where it takes them
    /// there ("voice-messages-in-channels") and you may attach files.
    pub(crate) fn can_record(&self) -> bool {
        match self.target() {
            Some(Target::Dm { key, conversation }) => self.core.shared.read(|s| {
                s.instance(&key).is_some_and(|i| {
                    let versions = i.node.as_ref().and_then(|n| n.versions.as_ref());
                    crate::core::compat::instance_has(versions, "voice-messages", &crate::core::compat::FEATURES)
                        && !i.dms.blocked.contains_key(&conversation)
                })
            }),
            Some(Target::Channel { key, .. }) => {
                self.core.shared.read(|s| s.instance(&key).is_some_and(|i| i.has("voice-messages-in-channels")))
                    && self.send_gate().is_some_and(|g| g.can_attach)
            }
            _ => false,
        }
    }

    /// Whether this conversation or channel has a recording going (or one opening the microphone).
    pub(crate) fn recording_here(&self) -> bool {
        let Some(target) = self.target() else { return false };
        self.voice.recording.as_ref().is_some_and(|r| r.target == target)
            || (self.voice.starting && self.voice.hold.is_some())
    }

    /// The mic pressed: starts recording, held until let go (or a tap that keeps going).
    pub(crate) fn press_mic(&mut self, x: f32, cx: &mut Context<Self>) {
        if self.voice.recording.is_some() {
            // Tapped to start, so this press sends.
            if self.voice.hold.is_none() {
                self.send_recording(cx);
            }
            return;
        }
        self.voice.hold = Some(Hold { since: std::time::Instant::now(), x, drag: 0.0 });
        self.start_recording(cx);
        cx.notify();
    }

    /// The mic let go: a quick tap keeps recording until sent; a hold sends.
    pub(crate) fn release_mic(&mut self, cx: &mut Context<Self>) {
        let Some(hold) = self.voice.hold.take() else { return };
        if hold.since.elapsed().as_millis() >= HOLD_MS && self.voice.recording.is_some() {
            self.send_recording(cx);
        }
        cx.notify();
    }

    /// The pointer moving while the mic is held: sliding left far enough throws it away.
    pub(crate) fn slide_mic(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some(hold) = &mut self.voice.hold else { return };
        hold.drag = (x - hold.x).min(0.0);
        if hold.drag < -CANCEL_PX {
            self.voice.hold = None;
            self.discard_recording(cx);
        }
        cx.notify();
    }

    pub(crate) fn start_recording(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.target() else { return };
        if self.voice.recording.is_some() || self.voice.sending || self.voice.starting {
            return;
        }
        // One sound at a time: what's playing stops for the microphone.
        self.voice.playing = None;
        self.voice.starting = true;
        self.set_voice_problem(None);
        let core = self.core.clone();
        let asked = target.key().to_owned();
        self.run(cx, async move { core.voice_limits(&asked).await }, move |this, limits: Limits, cx| {
            this.voice.starting = false;
            // Somewhere else by now, or already recording: nothing starts.
            if this.target().as_ref() != Some(&target) || this.voice.recording.is_some() {
                this.voice.hold = None;
                cx.notify();
                return;
            }
            let dm = matches!(target, Target::Dm { .. });
            this.voice.recording =
                Some(Recording { target, recorder: Recorder::start(limits.max_ms, Choice::of(&this.core.prefs())) });
            crate::core::reports::used(if dm { "dm.voice.record" } else { "message.voice.record" });
            this.voice_tick(cx);
            this.sync_list(cx);
            cx.notify();
        });
    }

    /// Throws the recording away. True when there was one.
    pub(crate) fn discard_recording(&mut self, cx: &mut Context<Self>) -> bool {
        self.voice.hold = None;
        let had = self.voice.recording.take().is_some();
        if had {
            cx.notify();
        }
        had
    }

    /// Lets the composer follow the pointer while the mic is held (sliding left throws it away).
    pub(crate) fn mic_slide<E: gpui_kit::InteractiveElement + 'static>(&self, el: E, cx: &mut Context<Self>) -> E {
        if self.voice.hold.is_none() {
            return el;
        }
        el.on_mouse_move(
            cx.listener(|this, e: &gpui_kit::MouseMoveEvent, _, cx| this.slide_mic(f32::from(e.position.x), cx)),
        )
    }

    /// A recording started with a tap: Enter sends it, as on the web.
    pub(crate) fn tapped_recording(&self) -> bool {
        self.voice.recording.is_some() && self.voice.hold.is_none()
    }

    pub(crate) fn send_recording(&mut self, cx: &mut Context<Self>) {
        self.voice.hold = None;
        let Some(recording) = self.voice.recording.take() else { return };
        let Recording { target, recorder } = recording;
        let Some(clip): Option<Clip> = recorder.finish() else {
            self.set_voice_problem(Some(t("dms-calls.voice.recorder.tooShort")));
            cx.notify();
            return;
        };
        self.voice.sending = true;
        cx.notify();
        let core = self.core.clone();
        match target {
            Target::Dm { key, conversation } => {
                self.run(
                    cx,
                    async move { core.send_voice(&key, &conversation, &clip, 0).await },
                    |this, result, cx| {
                        this.voice.sending = false;
                        if let Err(err) = result {
                            this.toast("circle-alert", "Couldn't send that".into(), err.0, None, None, cx);
                        }
                        cx.notify();
                    },
                );
            }
            Target::Channel { key, server, channel } => {
                // A recorded voice message goes up as its message's only file, then the message is sent.
                let upload = {
                    let (core, key, server) = (core.clone(), key.clone(), server.clone());
                    async move { core.upload_voice(&key, &server, &clip).await }
                };
                self.run(cx, upload, move |this, result, cx| {
                    this.voice.sending = false;
                    match result {
                        Ok(file) => {
                            let core = this.core.clone();
                            this.run(
                                cx,
                                async move { core.send_message_with(&key, &server, &channel, "", vec![file]).await },
                                |_, _, cx| cx.notify(),
                            );
                        }
                        Err(problem) => {
                            let error =
                                if problem.message.is_empty() { t("chat.composer.tryAgain") } else { problem.message };
                            this.set_voice_problem(Some(t_with(
                                "chat.composer.voiceFailed",
                                &[("error", crate::core::i18n::Arg::Str(&error))],
                            )));
                        }
                    }
                    cx.notify();
                });
            }
            Target::Secure { .. } => {
                self.voice.sending = false;
            }
        }
    }

    /// Plays or pauses a voice message, or (`at`, 0 to 1) jumps to a place in it.
    pub(crate) fn play_voice(&mut self, file: VoiceFile, at: Option<f32>, cx: &mut Context<Self>) {
        if let Some(playing) = &self.voice.playing
            && playing.media_id == file.media_id
            && !playing.player.is_done()
        {
            match at {
                Some(at) => {
                    playing.player.seek(at);
                    playing.player.pause(false);
                }
                None => playing.player.pause(!playing.player.is_paused()),
            }
            self.sync_list(cx);
            return;
        }
        if self.voice.recording.is_some() || self.voice.loading.as_deref() == Some(&file.media_id) {
            return;
        }
        self.voice.playing = None;
        self.voice.failed.remove(&file.media_id);
        let from_ms = at.map_or(0, |at| (f64::from(at) * f64::from(file.duration_ms)) as u64);
        if let Some(sound) = self.voice.sounds.iter().find(|(id, _)| *id == file.media_id).map(|(_, s)| s.clone()) {
            self.start_playing(file.media_id, sound, from_ms, cx);
            return;
        }
        let Some(key) = self.target().map(|t| t.key().to_owned()) else { return };
        self.voice.loading = Some(file.media_id.clone());
        self.sync_list(cx);
        let core = self.core.clone();
        let media_id = file.media_id.clone();
        self.run(cx, async move { core.voice_sound(&key, &file).await }, move |this, result, cx| {
            if this.voice.loading.as_deref() == Some(&media_id) {
                this.voice.loading = None;
            }
            match result {
                Ok(sound) => {
                    this.voice.sounds.push_back((media_id.clone(), sound.clone()));
                    while this.voice.sounds.len() > SOUNDS_KEPT {
                        this.voice.sounds.pop_front();
                    }
                    if this.voice.recording.is_none() {
                        this.start_playing(media_id, sound, from_ms, cx);
                    }
                }
                Err(problem) => {
                    this.voice.failed.insert(media_id, problem.message);
                }
            }
            this.sync_list(cx);
        });
    }

    fn start_playing(&mut self, media_id: String, sound: Arc<Vec<f32>>, from_ms: u64, cx: &mut Context<Self>) {
        let rate = self.core.prefs().voice_rate;
        self.voice.playing = Some(Playing { media_id, player: Player::play(sound, from_ms, rate) });
        crate::core::reports::used("dm.voice.play");
        self.voice_tick(cx);
        self.sync_list(cx);
    }

    /// Steps voice messages to the next speed (1×, 1.5×, 2×), kept for next time.
    pub(crate) fn next_voice_rate(&mut self, cx: &mut Context<Self>) {
        let rate = next_rate(self.core.prefs().voice_rate);
        self.core.set_prefs(|prefs| prefs.voice_rate = rate);
        if let Some(playing) = &self.voice.playing {
            playing.player.set_rate(rate);
        }
        self.sync_list(cx);
        cx.notify();
    }

    /// Stops what's playing (leaving a conversation does).
    pub(crate) fn stop_voice(&mut self) {
        self.voice.playing = None;
    }

    /// While something records or plays: the recording bar redraws, and a
    /// voice message that played to the end goes back to the start.
    fn voice_tick(&mut self, cx: &mut Context<Self>) {
        if self.voice.ticking {
            return;
        }
        self.voice.ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(80)).await;
                let more = this.update(cx, |this, cx| {
                    let mut changed = false;
                    if let Some(playing) = this.voice.playing.take_if(|p| p.player.is_done()) {
                        if playing.player.had_no_speakers() {
                            this.voice.failed.insert(playing.media_id, "Couldn't open the speakers.".into());
                        }
                        changed = true;
                    }
                    if let Some(r) = &this.voice.recording
                        && let progress = r.recorder.progress()
                        && progress.no_microphone
                    {
                        this.voice.recording = None;
                        let body = if progress.microphone_blocked {
                            t("desktop.voice.micBlocked")
                        } else {
                            "Check that one is plugged in and that Fuwa may use it.".into()
                        };
                        this.toast("mic-off", "Couldn't open a microphone".into(), body, None, None, cx);
                    }
                    if changed {
                        this.sync_list(cx);
                    }
                    if this.voice.recording.is_some() {
                        cx.notify();
                    }
                    let more = this.voice.recording.is_some() || this.voice.playing.is_some();
                    this.voice.ticking = more;
                    more
                });
                if !matches!(more, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Gives the conversation's voice messages how they're playing.
    pub(crate) fn dress_voice(&self, rows: &mut [Row]) {
        let rate = self.core.prefs().voice_rate;
        for row in rows {
            let Row::Msg(m) = row else { continue };
            let Some(card) = &m.voice else { continue };
            let id = &card.file.media_id;
            let mut card = (**card).clone();
            let rated = (card.rate - rate).abs() > 0.001;
            card.rate = rate;
            if let Some(why) = self.voice.failed.get(id) {
                card.play = Play::Failed(why.clone());
            } else if self.voice.loading.as_ref() == Some(id) {
                card.play = Play::Loading;
            } else if let Some(p) = self.voice.playing.as_ref().filter(|p| p.media_id == *id && !p.player.is_done()) {
                let fraction = p.player.fraction();
                card.play = if p.player.is_paused() { Play::Paused(fraction()) } else { Play::Playing };
                card.fraction = Some(fraction);
            } else if !rated {
                continue;
            }
            let mut msg = (**m).clone();
            msg.voice = Some(Rc::new(card));
            *m = Rc::new(msg);
        }
    }

    /// The recording bar, in the composer's place while recording (the web's
    /// `VoiceRecorder` bar): throw away, the pulsing dot, the time, the live
    /// waveform and what the keys do.
    pub(crate) fn recording_bar(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let progress: Progress = self.voice.recording.as_ref().map(|r| r.recorder.progress()).unwrap_or_default();
        let levels: Vec<f32> = progress.levels.iter().rev().take(LIVE_BARS * 2).rev().copied().collect();
        let drag = self.voice.hold.map_or(0.0, |h| h.drag);
        let cancelling = (-drag / CANCEL_PX).min(1.0);
        let mut wave = div()
            .flex()
            .flex_1()
            .min_w_0()
            .h(px(28.0))
            .items_center()
            .justify_end()
            .gap(px(GAP + 1.0))
            .overflow_hidden()
            .opacity(1.0 - cancelling * 0.7);
        // Two levels a bar, the newest on the right, quiet ones still showing a sliver.
        let pairs: Vec<f32> = levels.chunks(2).map(|c| c.iter().copied().fold(0f32, f32::max)).collect();
        for n in 0..LIVE_BARS {
            let level = pairs.len().checked_sub(LIVE_BARS - n).and_then(|i| pairs.get(i)).copied().unwrap_or(0.0);
            let h = 28.0 * (level.sqrt() * 1.4).clamp(0.1, 1.0);
            wave = wave.child(div().flex_none().w(px(BAR)).h(px(h)).rounded_full().bg(p.primary));
        }
        let limited = progress.limited;
        let held = self.voice.hold.is_some();
        let hint = if limited {
            t("dms-calls.voice.recorder.longest")
        } else if held {
            t("dms-calls.voice.recorder.slide")
        } else {
            t("dms-calls.voice.recorder.esc")
        };
        let amber = if p.dark { gpui_kit::rgb(0xfbbf24) } else { gpui_kit::rgb(0xd97706) };
        let (bin_bg, bin_fg) = (alpha(p.destructive, 0.1), p.destructive);
        div()
            .id("voice-recording")
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(px(12.0))
            .h(px(36.0))
            .mb(px(2.0))
            .ml(px(-4.0))
            .child(
                div()
                    .id("voice-discard")
                    .size(px(36.0))
                    .flex_none()
                    .rounded(crate::ui::theme::radius_xl())
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(p.muted_foreground)
                    .cursor_pointer()
                    .hover(move |s| s.bg(bin_bg).text_color(bin_fg))
                    .active(|s| s.top(px(1.0)))
                    .tooltip(|window, cx| {
                        crate::ui::overlay::Tip::new(t("dms-calls.voice.recorder.throwAwayTitle")).build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.discard_recording(cx);
                    }))
                    .child(icon("trash").size(px(18.0))),
            )
            .child(
                div()
                    .relative()
                    .size(px(12.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(!limited, |el| {
                        el.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full()
                                .rounded_full()
                                .bg(p.destructive)
                                .with_animation(
                                    "voice-rec-ping",
                                    Animation::new(Duration::from_millis(1400)).repeat(),
                                    |el, t| {
                                        // The web's ping: a ring growing to 1.9 times while it fades out.
                                        let grow = 1.0 + 0.9 * (1.0 - (2.0 * t - 1.0).abs());
                                        let off = 6.0 * (grow - 1.0);
                                        el.top(px(-off))
                                            .left(px(-off))
                                            .size(px(12.0 * grow))
                                            .opacity(0.6 * (2.0 * t - 1.0).abs())
                                    },
                                ),
                        )
                    })
                    .child(div().size(px(10.0)).rounded_full().bg(p.destructive)),
            )
            .child(
                div()
                    .w(px(40.0))
                    .flex_none()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child(voice_notes::clock(progress.elapsed_ms)),
            )
            .child(wave)
            .child(
                div()
                    .flex_none()
                    .relative()
                    .left(px(drag * 0.4))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(if limited { amber } else { p.muted_foreground })
                    .when(held, |el| el.opacity(1.0 - cancelling))
                    .child(hint),
            )
            .into_any_element()
    }

    /// The button at the end of the composer when there's nothing to send:
    /// hold it to record and let go to send (sliding left throws it away), or
    /// tap it to start and again to send.
    pub(crate) fn voice_button(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let recording = self.recording_here();
        let sending = self.voice.sending;
        let held = self.voice.hold.is_some();
        let tapped = recording && !held;
        let drag = self.voice.hold.map_or(0.0, |h| h.drag);
        let (glyph, tip) = if tapped {
            ("send-horizontal", t("dms-calls.voice.recorder.sendTitle"))
        } else {
            ("mic", t("dms-calls.voice.recorder.recordTitle"))
        };
        let lit = recording || held;
        div()
            .id("voice-button")
            .size(px(36.0))
            .mb(px(2.0))
            .flex_none()
            .relative()
            .left(px(drag))
            .rounded(crate::ui::theme::radius_xl())
            .flex()
            .items_center()
            .justify_center()
            .when(lit, |el| {
                el.bg(p.primary).text_color(p.primary_foreground).shadow(vec![gpui_kit::BoxShadow {
                    color: p.primary.into(),
                    offset: gpui_kit::point(px(0.0), px(6.0)),
                    blur_radius: px(18.0),
                    spread_radius: px(-8.0),
                    inset: false,
                }])
            })
            .when(!lit, |el| {
                let (bg, fg) = (p.muted, p.foreground);
                el.text_color(p.muted_foreground).hover(move |s| s.bg(bg).text_color(fg))
            })
            .cursor_pointer()
            .tooltip(move |window, cx| crate::ui::overlay::Tip::new(tip.clone()).build(window, cx))
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, e: &gpui_kit::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.press_mic(f32::from(e.position.x), cx)
                }),
            )
            .on_mouse_up(gpui_kit::MouseButton::Left, cx.listener(|this, _, _, cx| this.release_mic(cx)))
            .on_mouse_up_out(gpui_kit::MouseButton::Left, cx.listener(|this, _, _, cx| this.release_mic(cx)))
            .child(if sending {
                icon("loader-circle")
                    .size(px(18.0))
                    .with_animation("voice-sending", Animation::new(Duration::from_millis(900)).repeat(), |el, t| {
                        el.rotate(gpui_kit::percentage(t))
                    })
                    .into_any_element()
            } else {
                icon(glyph)
                    .size(px(if held { 18.0 * 1.12 } else { 18.0 }))
                    .with_animation(
                        SharedString::from(format!("voice-glyph|{glyph}")),
                        Animation::new(Duration::from_millis(260)).with_easing(gpui_kit::ease_out_quint()),
                        |el, t| el.rotate(gpui_kit::radians((1.0 - t) * -25f32.to_radians())),
                    )
                    .into_any_element()
            })
            .into_any_element()
    }
}

/// A voice message sent in a server's channel: the card for its file (the
/// message's only one), and the other files to show as usual.
pub(crate) fn channel_voice(files: &[pb::Attachment]) -> (Option<Rc<VoiceCard>>, Vec<pb::Attachment>) {
    let Some(at) = files.iter().position(|f| f.voice.is_some() && f.content_type.starts_with("audio/")) else {
        return (None, files.to_vec());
    };
    let file = &files[at];
    let voice = file.voice.as_ref().cloned().unwrap_or_default();
    let media_id =
        if file.id.is_empty() { file.url.rsplit('/').next().unwrap_or_default().to_owned() } else { file.id.clone() };
    let card = VoiceCard::of(&VoiceFile {
        media_id,
        key: Vec::new(),
        sha256: Vec::new(),
        size: file.size,
        duration_ms: voice.duration_ms,
        waveform: voice.waveform,
    });
    let rest = files.iter().enumerate().filter(|(n, _)| *n != at).map(|(_, f)| f.clone()).collect();
    (Some(Rc::new(card)), rest)
}

/// How fast voice messages play, in steps (the web's `RATES`).
const RATES: [f32; 3] = [1.0, 1.5, 2.0];

fn next_rate(rate: f32) -> f32 {
    let at = RATES.iter().position(|r| (r - rate).abs() < 0.01).unwrap_or(RATES.len() - 1);
    RATES[(at + 1) % RATES.len()]
}

/// "1×", "1.5×", "2×".
fn rate_label(rate: f32) -> String {
    let n = if rate.fract() == 0.0 { format!("{}", rate as i32) } else { format!("{rate}") };
    t_with("dms-calls.voice.message.rate", &[("rate", crate::core::i18n::Arg::Str(&n))])
}

/// A voice message (the web's `VoiceMessage`): play and pause, its waveform
/// (click to jump), how long it is (or where it's at), and how fast it plays.
pub(crate) fn voice_card(mid: &str, card: &VoiceCard, p: &Palette, this: &WeakEntity<FuwaApp>) -> AnyElement {
    let duration = u64::from(card.file.duration_ms);
    let failed = matches!(card.play, Play::Failed(_));
    let (glyph, label) = match &card.play {
        Play::Failed(_) => ("circle-alert", t("dms-calls.voice.message.play")),
        Play::Playing => ("pause", t("dms-calls.voice.message.pause")),
        Play::Loading => ("loader-circle", t("dms-calls.voice.message.play")),
        _ => ("play", t("dms-calls.voice.message.play")),
    };
    let button = {
        let (this, file) = (this.clone(), card.file.clone());
        let inner = match glyph {
            "play" | "pause" => gpui_kit::component::Icon::default()
                .path(SharedString::from(format!("fuwa/{glyph}-filled.svg")))
                .flex_none(),
            _ => icon(glyph),
        }
        .size(px(18.0));
        let face: AnyElement = if card.play == Play::Loading {
            inner
                .with_animation(
                    SharedString::from(format!("voice-loading|{mid}")),
                    Animation::new(Duration::from_millis(900)).repeat(),
                    |el, t| el.rotate(gpui_kit::percentage(t)),
                )
                .into_any_element()
        } else {
            // The play triangle sits a pixel right of centre, where it looks centred.
            div().when(glyph == "play", |el| el.ml(px(1.0))).child(inner).into_any_element()
        };
        div()
            .id(SharedString::from(format!("voice-play|{mid}")))
            .size(px(36.0))
            .flex_none()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .when(!failed, |el| {
                el.bg(p.primary).text_color(p.primary_foreground).shadow(vec![gpui_kit::BoxShadow {
                    color: p.primary.into(),
                    offset: gpui_kit::point(px(0.0), px(6.0)),
                    blur_radius: px(16.0),
                    spread_radius: px(-8.0),
                    inset: false,
                }])
            })
            .when(failed, |el| el.bg(p.muted).text_color(p.muted_foreground))
            .cursor_pointer()
            .active(|s| s.opacity(0.85))
            .tooltip(move |window, cx| crate::ui::overlay::Tip::new(label.clone()).build(window, cx))
            .on_click(move |_, _, cx| {
                let _ = this.update(cx, |this, cx| this.play_voice(file.clone(), None, cx));
            })
            .child(face)
    };

    // The bars: as tall as they're loud, centred, filling the track.
    let bars = |color: gpui_kit::Hsla, clickable: bool| {
        let mut row = div().flex().items_center().gap(px(2.0)).h(px(32.0)).w_full();
        for (n, h) in card.heights.iter().enumerate() {
            let bar = div().w_full().h(px(32.0 * h.max(0.12))).rounded_full().bg(color);
            let slot = div().flex_1().min_w(px(2.0)).h_full().flex().items_center();
            if clickable {
                // Each bar takes its slice of the track, so it's easy to hit.
                let (this, file) = (this.clone(), card.file.clone());
                let at = n as f32 / BUBBLE_BARS as f32;
                row = row.child(
                    slot.id(SharedString::from(format!("voice-bar|{mid}|{n}")))
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            let _ = this.update(cx, |this, cx| this.play_voice(file.clone(), Some(at), cx));
                        })
                        .child(bar),
                );
            } else {
                row = row.child(slot.child(bar));
            }
        }
        row
    };
    // The played part, uncovered from the left over the unplayed bars.
    let fill = |fraction: f32| {
        let f = fraction.clamp(0.001, 1.0);
        div()
            .absolute()
            .top_0()
            .left_0()
            .h_full()
            .overflow_hidden()
            .w(gpui_kit::relative(f))
            .child(div().h_full().w(gpui_kit::relative(1.0 / f)).child(bars(p.primary.into(), false)))
    };
    let filled: Option<AnyElement> =
        match (&card.play, &card.fraction) {
            (Play::Playing, Some(fraction)) => {
                let fraction = fraction.clone();
                let primary = p.primary;
                let heights = card.heights.clone();
                Some(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .with_animation(
                            SharedString::from(format!("voice-fill|{mid}")),
                            Animation::new(Duration::from_secs(1)).repeat(),
                            move |el, _| {
                                let f = fraction().clamp(0.001, 1.0);
                                let mut row = div().flex().items_center().gap(px(2.0)).h(px(32.0)).w_full();
                                for h in &heights {
                                    row =
                                        row.child(div().flex_1().min_w(px(2.0)).h_full().flex().items_center().child(
                                            div().w_full().h(px(32.0 * h.max(0.12))).rounded_full().bg(primary),
                                        ));
                                }
                                el.child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .h_full()
                                        .overflow_hidden()
                                        .w(gpui_kit::relative(f))
                                        .child(div().h_full().w(gpui_kit::relative(1.0 / f)).child(row)),
                                )
                            },
                        )
                        .into_any_element(),
                )
            }
            (Play::Paused(at), _) if *at > 0.0 => Some(fill(*at).into_any_element()),
            _ => None,
        };
    let track =
        div().relative().flex_1().min_w_0().h(px(32.0)).child(bars(alpha(p.foreground, 0.2), true)).children(filled);

    let time: AnyElement = {
        let base = div()
            .w(px(36.0))
            .flex_none()
            .flex()
            .justify_end()
            .text_xs()
            .font_weight(FontWeight::BOLD)
            .text_color(p.muted_foreground);
        match (&card.play, &card.fraction) {
            (Play::Playing, Some(fraction)) => {
                let fraction = fraction.clone();
                base.with_animation(
                    SharedString::from(format!("voice-time|{mid}")),
                    Animation::new(Duration::from_secs(1)).repeat(),
                    move |el, _| el.child(voice_notes::clock((f64::from(fraction()) * duration as f64) as u64)),
                )
                .into_any_element()
            }
            (Play::Paused(at), _) if *at > 0.0 => {
                base.child(voice_notes::clock((f64::from(*at) * duration as f64) as u64)).into_any_element()
            }
            _ => base.child(voice_notes::clock(duration)).into_any_element(),
        }
    };

    let rate = {
        let this = this.clone();
        let (bg, fg) = (alpha(p.muted, 0.7), p.foreground);
        div()
            .id(SharedString::from(format!("voice-rate|{mid}")))
            .h(px(24.0))
            .w(px(40.0))
            .flex_none()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(p.muted)
            .text_size(px(11.2))
            .font_weight(FontWeight::BOLD)
            .text_color(p.muted_foreground)
            .cursor_pointer()
            .hover(move |s| s.bg(bg).text_color(fg))
            .active(|s| s.opacity(0.85))
            .tooltip(|window, cx| {
                crate::ui::overlay::Tip::new(t("dms-calls.voice.message.speedTitle")).build(window, cx)
            })
            .on_click(move |_, _, cx| {
                let _ = this.update(cx, |this, cx| this.next_voice_rate(cx));
            })
            .child(rate_label(card.rate))
    };

    div()
        .flex()
        .flex_col()
        .items_start()
        .child(
            div()
                .mt(px(4.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .w_full()
                .max_w(px(352.0))
                .py(px(6.0))
                .pl(px(6.0))
                .pr(px(8.0))
                .rounded(crate::ui::theme::radius_2xl())
                .bg(alpha(p.card, 0.7))
                .border_1()
                .border_color(p.border)
                .shadow(vec![gpui_kit::BoxShadow {
                    color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.05),
                    offset: gpui_kit::point(px(0.0), px(1.0)),
                    blur_radius: px(2.0),
                    spread_radius: px(0.0),
                    inset: false,
                }])
                .child(button)
                .child(track)
                .child(time)
                .child(rate),
        )
        .when_some(
            match &card.play {
                Play::Failed(why) => Some(why.clone()),
                _ => None,
            },
            |el, why| el.child(div().mt(px(4.0)).text_xs().text_color(p.destructive).child(why)),
        )
        .into_any_element()
}
