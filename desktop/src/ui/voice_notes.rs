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

use crate::core::vault::VoiceFile;
use crate::core::voice_notes::{self, Clip, Limits, Player, Progress, Recorder};
use crate::ui::app::{FuwaApp, Target};
use crate::ui::chat::Row;
use crate::ui::theme::{Palette, alpha};
use crate::ui::widgets::{icon, icon_button_in};

/// Bars in a voice message's bubble.
const BUBBLE_BARS: usize = 40;
/// Each bar's width and the gap after it.
const BAR: f32 = 3.0;
const GAP: f32 = 2.0;
/// Bars in the recording bar's live waveform.
const LIVE_BARS: usize = 48;
/// Decoded voice messages kept, so playing one again starts at once.
const SOUNDS_KEPT: usize = 6;

/// Something being recorded, for one conversation.
struct Recording {
    key: String,
    conversation: String,
    recorder: Recorder,
}

struct Playing {
    media_id: String,
    player: Player,
}

/// What voice messages are doing in the window.
#[derive(Default)]
pub struct VoiceState {
    recording: Option<Recording>,
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
    /// Where it's playing, read as it draws.
    fraction: Option<Arc<dyn Fn() -> f32 + Send + Sync>>,
}

impl VoiceCard {
    pub fn of(file: &VoiceFile) -> Self {
        Self {
            heights: voice_notes::heights(&file.waveform, BUBBLE_BARS),
            file: file.clone(),
            play: Play::Idle,
            fraction: None,
        }
    }

    /// What it was drawn from, for the list to know when to draw it again.
    pub fn digest(&self) -> (String, u8, u32, String) {
        let (state, at, why) = match &self.play {
            Play::Idle => (0, 0, String::new()),
            Play::Loading => (1, 0, String::new()),
            Play::Playing => (2, 0, String::new()),
            Play::Paused(f) => (3, (f * 1000.0) as u32, String::new()),
            Play::Failed(why) => (4, 0, why.clone()),
        };
        (self.file.media_id.clone(), state, at, why)
    }
}

impl FuwaApp {
    /// Voice messages go in private conversations (not secure channels) on an instance that has them.
    pub(crate) fn can_record(&self) -> bool {
        let Some(Target::Dm { key, conversation }) = self.target() else { return false };
        self.core.shared.read(|s| {
            s.instance(&key).is_some_and(|i| {
                let versions = i.node.as_ref().and_then(|n| n.versions.as_ref());
                crate::core::compat::instance_has(versions, "voice-messages", &crate::core::compat::FEATURES)
                    && !i.dms.blocked.contains_key(&conversation)
            })
        })
    }

    /// Whether this conversation has a recording going.
    pub(crate) fn recording_here(&self) -> bool {
        let Some(Target::Dm { key, conversation }) = self.target() else { return false };
        self.voice.recording.as_ref().is_some_and(|r| r.key == key && r.conversation == conversation)
    }

    pub(crate) fn start_recording(&mut self, cx: &mut Context<Self>) {
        let Some(Target::Dm { key, conversation }) = self.target() else { return };
        if self.voice.recording.is_some() || self.voice.sending {
            return;
        }
        // One sound at a time: what's playing stops for the microphone.
        self.voice.playing = None;
        let core = self.core.clone();
        let asked = key.clone();
        self.run(cx, async move { core.voice_limits(&asked).await }, move |this, limits: Limits, cx| {
            // Somewhere else by now, or already recording: nothing starts.
            let still =
                matches!(this.target(), Some(Target::Dm { key: k, conversation: c }) if k == key && c == conversation);
            if !still || this.voice.recording.is_some() {
                return;
            }
            this.voice.recording = Some(Recording { key, conversation, recorder: Recorder::start(limits.max_ms) });
            crate::core::reports::used("dm.voice.record");
            this.voice_tick(cx);
            this.sync_list(cx);
            cx.notify();
        });
    }

    /// Throws the recording away. True when there was one.
    pub(crate) fn discard_recording(&mut self, cx: &mut Context<Self>) -> bool {
        let had = self.voice.recording.take().is_some();
        if had {
            cx.notify();
        }
        had
    }

    pub(crate) fn send_recording(&mut self, cx: &mut Context<Self>) {
        let Some(recording) = self.voice.recording.take() else { return };
        let Recording { key, conversation, recorder } = recording;
        let Some(clip): Option<Clip> = recorder.finish() else {
            self.toast(
                "mic",
                "That's too short to send".into(),
                "Hold on a little longer next time.".into(),
                None,
                None,
                cx,
            );
            cx.notify();
            return;
        };
        self.voice.sending = true;
        cx.notify();
        let core = self.core.clone();
        self.run(cx, async move { core.send_voice(&key, &conversation, &clip, 0).await }, |this, result, cx| {
            this.voice.sending = false;
            if let Err(err) = result {
                this.toast("circle-alert", "Couldn't send that".into(), err.0, None, None, cx);
            }
            cx.notify();
        });
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
        self.voice.playing = Some(Playing { media_id, player: Player::play(sound, from_ms) });
        crate::core::reports::used("dm.voice.play");
        self.voice_tick(cx);
        self.sync_list(cx);
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
                        && r.recorder.progress().no_microphone
                    {
                        this.voice.recording = None;
                        this.toast(
                            "mic-off",
                            "Couldn't open a microphone".into(),
                            "Check that one is plugged in and that Fuwa may use it.".into(),
                            None,
                            None,
                            cx,
                        );
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
        for row in rows {
            let Row::Msg(m) = row else { continue };
            let Some(card) = &m.voice else { continue };
            let id = &card.file.media_id;
            let mut card = (**card).clone();
            if let Some(why) = self.voice.failed.get(id) {
                card.play = Play::Failed(why.clone());
            } else if self.voice.loading.as_ref() == Some(id) {
                card.play = Play::Loading;
            } else if let Some(p) = self.voice.playing.as_ref().filter(|p| p.media_id == *id && !p.player.is_done()) {
                let fraction = p.player.fraction();
                card.play = if p.player.is_paused() { Play::Paused(fraction()) } else { Play::Playing };
                card.fraction = Some(fraction);
            } else {
                continue;
            }
            let mut msg = (**m).clone();
            msg.voice = Some(Rc::new(card));
            *m = Rc::new(msg);
        }
    }

    /// The recording bar, in the composer's place while recording.
    pub(crate) fn recording_bar(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let progress: Progress = self.voice.recording.as_ref().map(|r| r.recorder.progress()).unwrap_or_default();
        let levels: Vec<f32> = progress.levels.iter().rev().take(LIVE_BARS * 2).rev().copied().collect();
        let loudest = levels.iter().copied().fold(0.05f32, f32::max);
        let mut wave = div().flex().items_center().gap(px(GAP)).h(px(28.0));
        // Two frames a bar, the newest on the right, quiet ones still showing a dot.
        let pairs: Vec<f32> = levels.chunks(2).map(|c| c.iter().copied().fold(0f32, f32::max)).collect();
        for n in 0..LIVE_BARS {
            let level = pairs.len().checked_sub(LIVE_BARS - n).and_then(|i| pairs.get(i)).copied().unwrap_or(0.0);
            let h = 3.0 + (level / loudest).min(1.0).sqrt() * 22.0;
            wave = wave.child(div().w(px(BAR)).h(px(h)).rounded_full().bg(alpha(p.destructive, 0.75)));
        }
        let limited = progress.limited;
        div()
            .flex()
            .flex_1()
            .items_center()
            .gap(px(12.0))
            .h(px(36.0))
            .child(div().size(px(10.0)).rounded_full().bg(p.destructive).with_animation(
                "voice-rec-dot",
                Animation::new(Duration::from_millis(1200)).repeat(),
                |el, t| el.opacity(0.35 + 0.65 * (1.0 - (t * std::f32::consts::TAU).cos()) / 2.0),
            ))
            .child(
                div()
                    .w(px(44.0))
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child(voice_notes::clock(progress.elapsed_ms)),
            )
            .child(div().flex_1().min_w_0().overflow_hidden().flex().justify_end().child(wave))
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(if limited { p.destructive } else { p.muted_foreground })
                    .child(if limited { "That's the longest here" } else { "Esc to throw away" }),
            )
            .child(
                icon_button_in("voice-discard", "trash", p, p.destructive)
                    .size(px(36.0))
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Throw away").build(window, cx))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.discard_recording(cx);
                    })),
            )
            .into_any_element()
    }

    /// The button at the end of the composer when nothing's typed: records,
    /// or sends what was recorded.
    pub(crate) fn voice_button(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let recording = self.recording_here();
        let sending = self.voice.sending;
        let (glyph, tip) = if recording { ("send", "Send voice message") } else { ("mic", "Record a voice message") };
        div()
            .id("voice-button")
            .size(px(36.0))
            .mb(px(2.0))
            .flex_none()
            .rounded(crate::ui::theme::radius_xl())
            .flex()
            .items_center()
            .justify_center()
            .when(recording, |el| el.bg(p.primary).text_color(p.primary_foreground))
            .when(!recording, |el| {
                let (bg, fg) = (p.muted, p.foreground);
                el.text_color(p.muted_foreground).hover(move |s| s.bg(bg).text_color(fg))
            })
            .cursor_pointer()
            .active(|s| s.top(px(1.0)))
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx))
            .on_click(cx.listener(move |this, _, _, cx| {
                if recording {
                    this.send_recording(cx);
                } else {
                    this.start_recording(cx);
                }
            }))
            .child(if sending {
                icon("loader-circle")
                    .size(px(18.0))
                    .with_animation("voice-sending", Animation::new(Duration::from_millis(900)).repeat(), |el, t| {
                        el.rotate(gpui_kit::percentage(t))
                    })
                    .into_any_element()
            } else {
                icon(glyph).size(px(18.0)).into_any_element()
            })
            .into_any_element()
    }
}

/// A voice message's bubble: play and pause, the waveform (click to jump),
/// and how long it is (or where it's at).
pub(crate) fn voice_card(mid: &str, card: &VoiceCard, p: &Palette, this: &WeakEntity<FuwaApp>) -> AnyElement {
    let duration = u64::from(card.file.duration_ms);
    let (glyph, label) = match &card.play {
        Play::Playing => ("pause", "Pause voice message"),
        Play::Loading => ("loader-circle", "Loading voice message"),
        _ => ("play", "Play voice message"),
    };
    let button = {
        let (this, file) = (this.clone(), card.file.clone());
        let inner = icon(glyph).size(px(16.0));
        div()
            .id(SharedString::from(format!("voice-play|{mid}")))
            .size(px(36.0))
            .flex_none()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(p.primary)
            .text_color(p.primary_foreground)
            .cursor_pointer()
            .active(|s| s.top(px(1.0)))
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(label).build(window, cx))
            .on_click(move |_, _, cx| {
                let _ = this.update(cx, |this, cx| this.play_voice(file.clone(), None, cx));
            })
            .child(if card.play == Play::Loading {
                inner
                    .with_animation(
                        SharedString::from(format!("voice-loading|{mid}")),
                        Animation::new(Duration::from_millis(900)).repeat(),
                        |el, t| el.rotate(gpui_kit::percentage(t)),
                    )
                    .into_any_element()
            } else {
                // The play triangle sits a touch right of centre, where it looks centred.
                div().when(glyph == "play", |el| el.ml(px(2.0))).child(inner).into_any_element()
            })
    };

    let width = BUBBLE_BARS as f32 * (BAR + GAP) - GAP;
    let bars = |color: gpui_kit::Hsla, clickable: bool| {
        let mut row = div().flex().items_center().gap(px(GAP)).h(px(28.0)).w(px(width)).flex_none();
        for (n, h) in card.heights.iter().enumerate() {
            let bar = div().w(px(BAR)).h(px(4.0 + h * 22.0)).rounded_full().bg(color);
            if clickable {
                // Each bar takes its slice of the height, so it's easy to hit.
                let (this, file) = (this.clone(), card.file.clone());
                let at = n as f32 / BUBBLE_BARS as f32;
                row = row.child(
                    div()
                        .id(SharedString::from(format!("voice-bar|{mid}|{n}")))
                        .h_full()
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            let _ = this.update(cx, |this, cx| this.play_voice(file.clone(), Some(at), cx));
                        })
                        .child(bar),
                );
            } else {
                row = row.child(bar);
            }
        }
        row
    };
    let fill = |fraction: f32| {
        div()
            .absolute()
            .top_0()
            .left_0()
            .h_full()
            .overflow_hidden()
            .w(px(width * fraction.clamp(0.0, 1.0)))
            .child(bars(p.primary.into(), false))
    };
    let filled: Option<AnyElement> = match (&card.play, &card.fraction) {
        (Play::Playing, Some(fraction)) => {
            let fraction = fraction.clone();
            Some(
                fill(0.0)
                    .with_animation(
                        SharedString::from(format!("voice-fill|{mid}")),
                        Animation::new(Duration::from_secs(1)).repeat(),
                        move |el, _| el.w(px(width * fraction().clamp(0.0, 1.0))),
                    )
                    .into_any_element(),
            )
        }
        (Play::Paused(at), _) => Some(fill(*at).into_any_element()),
        _ => None,
    };
    let wave = div().relative().flex_none().child(bars(alpha(p.muted_foreground, 0.45), true)).children(filled);

    let time: AnyElement = match (&card.play, &card.fraction) {
        (Play::Playing, Some(fraction)) => {
            let fraction = fraction.clone();
            div()
                .w(px(40.0))
                .with_animation(
                    SharedString::from(format!("voice-time|{mid}")),
                    Animation::new(Duration::from_secs(1)).repeat(),
                    move |el, _| el.child(voice_notes::clock((f64::from(fraction()) * duration as f64) as u64)),
                )
                .into_any_element()
        }
        (Play::Paused(at), _) => {
            div().w(px(40.0)).child(voice_notes::clock((f64::from(*at) * duration as f64) as u64)).into_any_element()
        }
        _ => div().w(px(40.0)).child(voice_notes::clock(duration)).into_any_element(),
    };

    div()
        .flex()
        .flex_col()
        .items_start()
        .gap(px(4.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .py(px(6.0))
                .pl(px(6.0))
                .pr(px(14.0))
                .max_w(px(360.0))
                .rounded_full()
                .bg(alpha(p.primary, 0.08))
                .border_1()
                .border_color(alpha(p.primary, 0.18))
                .child(button)
                .child(wave)
                .child(div().text_xs().font_weight(FontWeight::BOLD).text_color(p.muted_foreground).child(time)),
        )
        .when_some(
            match &card.play {
                Play::Failed(why) => Some(why.clone()),
                _ => None,
            },
            |el, why| el.child(div().text_xs().text_color(p.destructive).child(why)),
        )
        .into_any_element()
}
