//! The Soundboard page (docs/soundboard.md), the web's
//! `settings/server/Soundboard.tsx`: the server's sounds, each with a
//! preview, its name, emoji and volume edited in place, and deleted after a
//! confirmation. Adding one picks any sound file, shows how long it is and
//! a window of up to 10 seconds to keep (its start can be moved, and it can
//! be heard first), a name from the file's, an emoji (a Unicode one, or one
//! of the server's written `<:name:id>`) and a volume; it's made into Ogg
//! Opus here (`core::soundboard`) and uploaded.

use std::path::PathBuf;

use gpui_kit::base::slider::{SliderEvent, SliderState};
use gpui_kit::base::{Slider as BaseSlider, SliderIndicator, SliderThumb, SliderTrack};
use gpui_kit::{AnimationExt as _, ExternalPaths};

use super::*;
use crate::core::soundboard::{self as board, Made, Picked};
use crate::core::voice_notes::clock;
use crate::ui::settings_controls::shadow_sm;
use crate::ui::soundboard::sound_emoji;

/// The biggest file read to make a sound from.
const MOST_FILE: u64 = 200 * 1024 * 1024;
/// How much a press of a sound's volume buttons changes it.
const VOLUME_STEP: f32 = 0.1;

/// A sound being made from a picked file.
struct Adding {
    picked: Picked,
    /// Where the kept window starts.
    start_ms: u64,
    /// What it's doing once Add is pressed: a locale key.
    stage: Option<&'static str>,
}

pub(super) struct Sounds {
    adding: Option<Adding>,
    /// The file being read, by name.
    reading: Option<String>,
    name: Entity<InputState>,
    emoji: Entity<InputState>,
    /// The window's start and the new sound's volume (0 to 100).
    start: Entity<SliderState>,
    volume: Entity<SliderState>,
    /// The sound being renamed or given another emoji, and the box it's typed in.
    renaming: Option<String>,
    rename: Entity<InputState>,
    emoji_of: Option<String>,
    emoji_edit: Entity<InputState>,
    confirming: Option<String>,
    busy: Option<String>,
    /// When a file that isn't a sound was refused, so the drop zone shakes.
    refused: Option<Instant>,
    /// A file was just read: its name and the sliders are set on the next draw.
    fresh: bool,
}

impl Sounds {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ServerSettingsView>) -> (Self, Vec<Subscription>) {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(t("serversettings.soundboard.namePlaceholder")));
        let emoji = cx.new(|cx| InputState::new(window, cx).placeholder("🎺"));
        let rename = cx.new(|cx| InputState::new(window, cx));
        let emoji_edit = cx.new(|cx| InputState::new(window, cx).placeholder("🎺"));
        // Where the window starts, as a share of where it can.
        let start = cx.new(|_| SliderState::new().min(0.0).max(1.0).step(0.001).default_value(0.0));
        let volume = cx.new(|_| SliderState::new().min(0.0).max(100.0).step(5.0).default_value(100.0));
        let notify = |_: &mut ServerSettingsView, _, e: &InputEvent, cx: &mut Context<ServerSettingsView>| {
            if let InputEvent::Change = e {
                cx.notify()
            }
        };
        let subscriptions = vec![
            cx.subscribe(&name, notify),
            cx.subscribe(&emoji, notify),
            cx.subscribe(&rename, |this: &mut ServerSettingsView, _, e: &InputEvent, cx| match e {
                InputEvent::PressEnter { .. } | InputEvent::Blur => this.finish_sound_rename(cx),
                InputEvent::Change => cx.notify(),
                InputEvent::Focus => {}
            }),
            cx.subscribe(&emoji_edit, |this: &mut ServerSettingsView, _, e: &InputEvent, cx| match e {
                InputEvent::PressEnter { .. } | InputEvent::Blur => this.finish_sound_emoji(cx),
                InputEvent::Change => cx.notify(),
                InputEvent::Focus => {}
            }),
            cx.subscribe(&start, |this: &mut ServerSettingsView, _, e: &SliderEvent, cx| {
                let v = match e {
                    SliderEvent::Change(v) | SliderEvent::Release(v) => v.end(),
                };
                if let Some(adding) = this.sounds.adding.as_mut() {
                    let total = adding.picked.duration_ms();
                    let most = total.saturating_sub(board::MAX_MS);
                    let at = (f64::from(v.clamp(0.0, 1.0)) * most as f64 / 100.0).round() as u64 * 100;
                    adding.start_ms = board::window_start(total, at);
                }
                cx.notify();
            }),
            cx.subscribe(&volume, |_: &mut ServerSettingsView, _, _: &SliderEvent, cx| cx.notify()),
        ];
        let sounds = Self {
            adding: None,
            reading: None,
            name,
            emoji,
            start,
            volume,
            renaming: None,
            rename,
            emoji_of: None,
            emoji_edit,
            confirming: None,
            busy: None,
            refused: None,
            fresh: false,
        };
        (sounds, subscriptions)
    }
}

/// A volume as the page shows it.
fn percent(v: f32) -> String {
    format!("{}%", (v * 100.0).round() as i64)
}

impl ServerSettingsView {
    fn pick_sound(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("serversettings.soundboard.drop").into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let _ = this.update(cx, |this, cx| this.open_sound(paths, cx));
        })
        .detach();
    }

    /// Reads a picked file as sound, to make one of.
    fn open_sound(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(path) = paths.into_iter().next() else { return };
        let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.error = None;
        self.core.stop_preview();
        self.sounds.reading = Some(file.clone());
        cx.notify();
        self.run(
            cx,
            async move {
                let unreadable = |_: std::io::Error| t("desktop.look.cantRead");
                let size = tokio::fs::metadata(&path).await.map_err(unreadable)?.len();
                if size > MOST_FILE {
                    return Err(t("desktop.server.soundboard.notAudio"));
                }
                let bytes = tokio::fs::read(&path).await.map_err(unreadable)?;
                tokio::task::spawn_blocking(move || board::open_file(&bytes, &file))
                    .await
                    .map_err(|_| t("desktop.server.soundboard.notAudio"))?
                    .map_err(|_| t("desktop.server.soundboard.notAudio"))
            },
            |this, result, cx| {
                this.sounds.reading = None;
                match result {
                    Ok(picked) => {
                        this.sounds.adding = Some(Adding { picked, start_ms: 0, stage: None });
                        this.sounds.refused = None;
                        this.sounds.confirming = None;
                        // The boxes and sliders are set where a window is at hand.
                        this.sounds.fresh = true;
                    }
                    Err(why) => {
                        this.sounds.refused = Some(Instant::now());
                        this.error = Some(why);
                    }
                }
                cx.notify();
            },
        );
    }

    /// The new sound's boxes and sliders, set from the file once it's read.
    fn fill_new_sound(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.sounds.fresh) {
            return;
        }
        let Some(adding) = &self.sounds.adding else { return };
        let name = adding.picked.name.clone();
        self.sounds.name.update(cx, |s, cx| s.set_value(name, window, cx));
        self.sounds.emoji.update(cx, |s, cx| s.set_value("", window, cx));
        self.sounds.start.update(cx, |s, cx| s.set_value(0.0, window, cx));
        self.sounds.volume.update(cx, |s, cx| s.set_value(100.0, window, cx));
    }

    fn cancel_new_sound(&mut self, cx: &mut Context<Self>) {
        self.core.stop_preview();
        self.sounds.adding = None;
        self.error = None;
        cx.notify();
    }

    /// The part of the picked file kept, heard first.
    fn hear_cut(&mut self, cx: &mut Context<Self>) {
        let Some(adding) = &self.sounds.adding else { return };
        let gain = self.sounds.volume.read(cx).value().end() / 100.0;
        let cut = board::window(&adding.picked.samples, adding.start_ms);
        self.core.preview_samples(cut, gain * self.core.shared.prefs().soundboard_volume);
    }

    /// Makes the sound and sends it up.
    fn add_sound(&mut self, cx: &mut Context<Self>) {
        let Some(adding) = self.sounds.adding.as_mut() else { return };
        if adding.stage.is_some() {
            return;
        }
        let name = self.sounds.name.read(cx).value().trim().to_owned();
        if !board::name_ok(&name) {
            self.error = Some(t("serversettings.soundboard.badName"));
            cx.notify();
            return;
        }
        let emoji = self.sounds.emoji.read(cx).value().trim().to_owned();
        let volume = (self.sounds.volume.read(cx).value().end() / 100.0).clamp(0.0, 1.0);
        let cut: Vec<f32> = board::window(&adding.picked.samples, adding.start_ms).to_vec();
        adding.stage = Some("serversettings.soundboard.encoding");
        self.error = None;
        self.core.stop_preview();
        cx.notify();
        self.run(
            cx,
            async move {
                tokio::task::spawn_blocking(move || board::encode(&cut))
                    .await
                    .map_err(|_| t("serversettings.soundboard.uploadFailed"))?
                    .map_err(|_| t("serversettings.soundboard.tooBig"))
            },
            move |this, made: Result<Made, String>, cx| match made {
                Ok(made) => this.send_sound(name, emoji, volume, made, cx),
                Err(why) => {
                    if let Some(a) = this.sounds.adding.as_mut() {
                        a.stage = None;
                    }
                    this.error = Some(why);
                    cx.notify();
                }
            },
        );
    }

    fn send_sound(&mut self, name: String, emoji: String, volume: f32, made: Made, cx: &mut Context<Self>) {
        if let Some(a) = self.sounds.adding.as_mut() {
            a.stage = Some("serversettings.soundboard.uploading");
        }
        cx.notify();
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(
            cx,
            async move { core.create_sound(&key, &sid, &name, &emoji, volume, made).await },
            |this, result, cx| {
                match result {
                    Ok(_) => {
                        this.sounds.adding = None;
                        this.flash_saved(cx);
                    }
                    Err(err) => {
                        if let Some(a) = this.sounds.adding.as_mut() {
                            a.stage = None;
                        }
                        this.error = Some(match err.code {
                            tonic::Code::ResourceExhausted => t("serversettings.soundboard.full"),
                            _ if err.message.is_empty() => t("serversettings.soundboard.uploadFailed"),
                            _ => err.message,
                        });
                    }
                }
                cx.notify();
            },
        );
    }

    fn sound_by_id(&self, id: &str) -> Option<pb::Sound> {
        self.core.sounds(&self.key, &self.server).and_then(|list| list.into_iter().find(|s| s.id == id))
    }

    /// Changes one of a sound's things, kept in its row until the instance says yes.
    fn change_sound(
        &mut self,
        id: String,
        name: Option<String>,
        emoji: Option<String>,
        volume: Option<f32>,
        cx: &mut Context<Self>,
    ) {
        self.error = None;
        self.sounds.busy = Some(id.clone());
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(
            cx,
            async move { core.update_sound(&key, &sid, &id, name, emoji, volume).await },
            |this, result, cx| {
                this.sounds.busy = None;
                match result {
                    Ok(_) => this.flash_saved(cx),
                    Err(err) => this.error = Some(err.message),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn start_sound_rename(&mut self, sound: &pb::Sound, window: &mut Window, cx: &mut Context<Self>) {
        self.sounds.renaming = Some(sound.id.clone());
        self.sounds.emoji_of = None;
        self.sounds.confirming = None;
        let name = sound.name.clone();
        self.sounds.rename.update(cx, |s, cx| {
            s.set_value(name, window, cx);
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        cx.notify();
    }

    fn finish_sound_rename(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.sounds.renaming.take() else { return };
        let name = self.sounds.rename.read(cx).value().trim().to_owned();
        cx.notify();
        if self.sound_by_id(&id).is_none_or(|s| s.name == name) {
            return;
        }
        if !board::name_ok(&name) {
            self.error = Some(t("serversettings.soundboard.badName"));
            return;
        }
        self.change_sound(id, Some(name), None, None, cx);
    }

    fn start_sound_emoji(&mut self, sound: &pb::Sound, window: &mut Window, cx: &mut Context<Self>) {
        self.sounds.emoji_of = Some(sound.id.clone());
        self.sounds.renaming = None;
        self.sounds.confirming = None;
        let emoji = sound.emoji.clone();
        self.sounds.emoji_edit.update(cx, |s, cx| {
            s.set_value(emoji, window, cx);
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        cx.notify();
    }

    fn finish_sound_emoji(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.sounds.emoji_of.take() else { return };
        let emoji = self.sounds.emoji_edit.read(cx).value().trim().to_owned();
        cx.notify();
        if self.sound_by_id(&id).is_none_or(|s| s.emoji == emoji) {
            return;
        }
        self.change_sound(id, None, Some(emoji), None, cx);
    }

    fn remove_sound(&mut self, id: String, cx: &mut Context<Self>) {
        self.sounds.busy = Some(id.clone());
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.delete_sound(&key, &sid, &id).await }, |this, result, cx| {
            this.sounds.busy = None;
            this.sounds.confirming = None;
            if let Err(err) = result {
                this.error = Some(err.message);
            }
            cx.notify();
        });
        cx.notify();
    }

    fn preview_listed(&mut self, sound: pb::Sound, cx: &mut Context<Self>) {
        let (core, key) = (self.core.clone(), self.key.clone());
        self.run(cx, async move { core.preview_sound(&key, &sound).await }, |this, result, cx| {
            if let Err(err) = result {
                this.error = Some(err.message);
                cx.notify();
            }
        });
    }

    pub(super) fn soundboard_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.fill_new_sound(window, cx);
        self.core.want_sounds(&self.key, &self.server, false);
        let (sounds, emojis) = self.core.shared.read(|s| {
            let Some(i) = s.instance(&self.key) else { return (None, Vec::new()) };
            (i.sounds.get(&self.server).cloned(), i.emojis.get(&self.server).cloned().unwrap_or_default())
        });
        let failed = sounds.is_none() && self.core.sounds_failed(&self.key, &self.server);
        let count = sounds.as_ref().map_or(0, Vec::len);
        let mut page = div().flex().flex_col().gap(px(20.0));
        page = match self.sounds.adding.is_some() {
            true => page.child(self.new_sound(&emojis, p, window, cx)),
            false => page.child(self.sound_drop(count, p, window, cx)),
        };
        match sounds {
            None if failed => {
                page = page.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child(t("dms-calls.calls.soundboard.failed"))
                        .child(soft_button("sounds-retry", t("dms-calls.calls.soundboard.retry"), p).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.core.want_sounds(&this.key, &this.server, true);
                                cx.notify();
                            }),
                        )),
                )
            }
            None => {
                page = page.child(div().flex().justify_center().py(px(24.0)).text_color(p.muted_foreground).child(
                    motion::ambient(
                        icon("loader-circle").size(px(20.0)),
                        "sounds-loading",
                        Duration::from_millis(900),
                        window,
                        |el, t| el.rotate(gpui_kit::radians(t * std::f32::consts::TAU)),
                    ),
                ))
            }
            Some(list) if list.is_empty() => {
                page = page.child(motion::rise(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(8.0))
                        .py(px(32.0))
                        .child(div().text_size(px(36.0)).line_height(px(40.0)).child("🔇"))
                        .child(div().font_weight(FontWeight::BOLD).child(t("serversettings.soundboard.none")))
                        .child(
                            div()
                                .text_sm()
                                .line_height(px(20.0))
                                .text_color(p.muted_foreground)
                                .child(t("serversettings.soundboard.noneHint")),
                        ),
                    "sounds-empty",
                    Duration::from_millis(80),
                    8.0,
                ))
            }
            Some(list) => {
                let mut rows = div().flex().flex_col().gap(px(8.0));
                for (n, sound) in list.iter().enumerate() {
                    rows = rows.child(self.sound_row(sound, &emojis, n, p, window, cx));
                }
                page = page.child(rows);
            }
        }
        page.into_any_element()
    }

    fn sound_drop(&self, count: usize, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (primary, hover_bg, drag_bg) = (p.primary, alpha(p.primary, 0.05), alpha(p.primary, 0.12));
        let hover_border = alpha(p.primary, 0.5);
        let reading = self.sounds.reading.clone();
        let zone = div()
            .id("sound-drop")
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.0))
            .p(px(24.0))
            .rounded(corner(24.0))
            .border_2()
            .border_dashed()
            .border_color(p.border)
            .text_center()
            .when(reading.is_none(), |el| {
                el.cursor_pointer()
                    .hover(move |s| s.bg(hover_bg).border_color(hover_border).translate_y(px(-2.0)))
                    .active(|s| s.scale(0.98))
                    .on_click(cx.listener(|this, _, _, cx| this.pick_sound(cx)))
                    .drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(drag_bg).border_color(primary))
                    .on_drop(
                        cx.listener(|this, paths: &ExternalPaths, _, cx| this.open_sound(paths.paths().to_vec(), cx)),
                    )
            })
            .child(
                div()
                    .size(px(48.0))
                    .rounded(corner(16.0))
                    .bg(alpha(p.primary, 0.15))
                    .text_color(p.primary)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(match &reading {
                        Some(_) => motion::ambient(
                            icon("loader-circle").size(px(24.0)),
                            "sound-reading",
                            Duration::from_millis(900),
                            window,
                            |el, t| el.rotate(gpui_kit::radians(t * std::f32::consts::TAU)),
                        ),
                        None => motion::once(
                            icon("audio-lines").size(px(24.0)),
                            "sound-drop-wiggle",
                            Duration::from_millis(900),
                            |el, t| {
                                el.rotate(gpui_kit::radians((t * std::f32::consts::TAU * 2.0).sin() * 0.2 * (1.0 - t)))
                            },
                        )
                        .into_any_element(),
                    }),
            )
            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(match &reading {
                Some(file) => t_with("serversettings.soundboard.reading", &[("file", Arg::Str(file))]),
                None => t("serversettings.soundboard.drop"),
            }))
            .child(
                div()
                    .max_w(px(384.0))
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(t("desktop.server.soundboard.dropHint")),
            )
            .child(
                div()
                    .mt(px(4.0))
                    .px(px(10.0))
                    .py(px(2.0))
                    .rounded_full()
                    .bg(p.muted)
                    .text_xs()
                    .line_height(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .child(t_with("serversettings.soundboard.count", &[("count", Arg::Num(count as i64))])),
            );
        match self.sounds.refused.filter(|at| at.elapsed() < Duration::from_millis(500)) {
            Some(at) => zone
                .with_animation(
                    SharedString::from(format!("sound-refused-{at:?}")),
                    gpui_kit::Animation::new(Duration::from_millis(400)),
                    |el, t| el.relative().left(px((t * std::f32::consts::TAU * 2.5).sin() * 8.0 * (1.0 - t))),
                )
                .into_any_element(),
            None => zone.into_any_element(),
        }
    }

    /// The web's slider look, over `state` from `min` to `max`.
    fn sound_slider(state: &Entity<SliderState>, (min, max): (f32, f32), p: &Palette, cx: &mut Context<Self>) -> Div {
        let now = state.read(cx).value().end();
        let frac = if max > min { ((now - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
        let thumb = SliderThumb::new(state)
            .absolute()
            .top(px(-6.0))
            .left(gpui_kit::relative(frac))
            .ml(px(-10.0))
            .size(px(20.0))
            .rounded_full()
            .border(px(3.0))
            .border_color(p.primary)
            .bg(p.background)
            .shadow(shadow_sm())
            .cursor_pointer();
        let track =
            SliderTrack::new(state).relative().w_full().h(px(20.0)).flex().items_center().cursor_pointer().child(
                SliderIndicator::new(state)
                    .relative()
                    .w_full()
                    .h(px(8.0))
                    .rounded_full()
                    .bg(p.muted)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(gpui_kit::relative(frac))
                            .rounded_full()
                            .bg(p.primary),
                    )
                    .child(thumb),
            );
        div().child(BaseSlider::new(state).relative().w_full().child(track))
    }

    /// The sound being made: its length and the window kept, name, emoji and volume.
    fn new_sound(
        &mut self,
        emojis: &[pb::Emoji],
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(adding) = &self.sounds.adding else { return div().into_any_element() };
        let total = adding.picked.duration_ms();
        let start = adding.start_ms;
        let end = (start + board::MAX_MS).min(total);
        let stage = adding.stage;
        let typed_emoji = self.sounds.emoji.read(cx).value().trim().to_owned();
        let name_bad = !board::name_ok(self.sounds.name.read(cx).value().trim());
        let volume = self.sounds.volume.read(cx).value().end();
        let label = |text: String| {
            div()
                .text_xs()
                .line_height(px(16.0))
                .font_weight(FontWeight::BOLD)
                .text_color(p.muted_foreground)
                .child(text)
        };
        // The whole file, and the part kept lit.
        let (from, width) =
            if total > 0 { (start as f32 / total as f32, (end - start) as f32 / total as f32) } else { (0.0, 1.0) };
        let bar = div()
            .relative()
            .h(px(36.0))
            .w_full()
            .rounded(corner(10.0))
            .bg(alpha(p.muted, 0.6))
            .overflow_hidden()
            .child(motion::glide(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .w(gpui_kit::relative(width))
                    .rounded(corner(10.0))
                    .bg(alpha(p.primary, 0.3))
                    .border_2()
                    .border_color(p.primary),
                "sound-window".to_owned(),
                from,
                cx,
                |el, x| el.left(gpui_kit::relative(x)),
            ));
        let mut window_part = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .child(label(t("serversettings.soundboard.window")))
                    .child(div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(t_with(
                        "serversettings.soundboard.windowRange",
                        &[("start", Arg::Str(&clock(start))), ("end", Arg::Str(&clock(end)))],
                    ))),
            )
            .child(bar);
        if total > board::MAX_MS {
            window_part = window_part.child(Self::sound_slider(&self.sounds.start.clone(), (0.0, 1.0), p, cx)).child(
                div()
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(t("serversettings.soundboard.windowHint")),
            );
        }
        let busy = stage.is_some();
        let head = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .child(
                div()
                    .size(px(40.0))
                    .rounded(corner(12.0))
                    .bg(alpha(p.primary, 0.15))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(sound_emoji(&typed_emoji, emojis, 22.0, p)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_weight(FontWeight::BOLD).child(t("serversettings.soundboard.newSound")))
                    .child(
                        div()
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .child(t_with("serversettings.soundboard.length", &[("length", Arg::Str(&clock(total)))])),
                    ),
            )
            .child(
                soft_button("sound-hear", t("serversettings.soundboard.previewCut"), p)
                    .on_click(cx.listener(|this, _, _, cx| this.hear_cut(cx))),
            );
        let fields = div()
            .flex()
            .gap(px(12.0))
            .child(div().flex_1().min_w_0().child(labeled(
                &t("serversettings.soundboard.name"),
                div().when(name_bad, |el| el.text_color(p.destructive)).child(Input::new(&self.sounds.name)),
                p,
            )))
            .child(div().w(px(140.0)).child(labeled(
                &t("serversettings.soundboard.emoji"),
                Input::new(&self.sounds.emoji),
                p,
            )));
        let volume_part = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(label(t("serversettings.soundboard.volume")))
                    .child(label(percent(volume / 100.0))),
            )
            .child(Self::sound_slider(&self.sounds.volume.clone(), (0.0, 100.0), p, cx));
        let mut add = primary_button(
            "sound-add",
            match stage {
                Some(key) => t(key),
                None => t("serversettings.soundboard.upload"),
            },
            p,
        );
        add = if busy || name_bad {
            add.opacity(0.6)
        } else {
            add.on_click(cx.listener(|this, _, _, cx| this.add_sound(cx)))
        };
        let buttons = div()
            .flex()
            .justify_end()
            .gap(px(8.0))
            .child(
                soft_button("sound-cancel", t("serversettings.soundboard.cancel"), p)
                    .when(!busy, |el| el.on_click(cx.listener(|this, _, _, cx| this.cancel_new_sound(cx)))),
            )
            .child(add);
        let _ = window;
        motion::rise(
            div()
                .flex()
                .flex_col()
                .gap(px(16.0))
                .p(px(16.0))
                .rounded(corner(20.0))
                .border_1()
                .border_color(alpha(p.primary, 0.4))
                .bg(alpha(p.primary, 0.04))
                .child(head)
                .child(window_part)
                .child(fields)
                .child(volume_part)
                .child(buttons),
            "sound-new",
            Duration::ZERO,
            8.0,
        )
        .into_any_element()
    }

    fn sound_row(
        &self,
        sound: &pb::Sound,
        emojis: &[pb::Emoji],
        n: usize,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = sound.id.clone();
        let group = SharedString::from(format!("sound-{id}"));
        let busy = self.sounds.busy.as_deref() == Some(id.as_str());
        let hover_border = alpha(p.primary, 0.3);
        let hover = alpha(p.primary, 0.08);
        let preview = {
            let s = sound.clone();
            icon_button(SharedString::from(format!("sound-preview-{id}")), "play", p)
                .tooltip({
                    let tip = t_with("serversettings.soundboard.preview", &[("name", Arg::Str(&sound.name))]);
                    move |window, cx| crate::ui::overlay::Tip::new(tip.clone()).build(window, cx)
                })
                .on_click(cx.listener(move |this, _, _, cx| this.preview_listed(s.clone(), cx)))
        };
        let emoji = if self.sounds.emoji_of.as_deref() == Some(id.as_str()) {
            div().w(px(96.0)).child(Input::new(&self.sounds.emoji_edit).small()).into_any_element()
        } else {
            let s = sound.clone();
            div()
                .id(SharedString::from(format!("sound-emoji-{id}")))
                .size(px(40.0))
                .flex_none()
                .rounded(corner(10.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(move |st| st.bg(hover).scale(1.1))
                .tooltip(|window, cx| {
                    crate::ui::overlay::Tip::new(t("serversettings.soundboard.pickEmoji")).build(window, cx)
                })
                .on_click(cx.listener(move |this, _, window, cx| this.start_sound_emoji(&s, window, cx)))
                .child(sound_emoji(&sound.emoji, emojis, 24.0, p))
                .into_any_element()
        };
        let name = if self.sounds.renaming.as_deref() == Some(id.as_str()) {
            let bad = !board::name_ok(self.sounds.rename.read(cx).value().trim());
            div()
                .w(px(220.0))
                .when(bad, |el| el.text_color(p.destructive))
                .child(Input::new(&self.sounds.rename).small())
                .into_any_element()
        } else {
            let s = sound.clone();
            div()
                .id(SharedString::from(format!("sound-name-{id}")))
                .flex()
                .items_center()
                .gap(px(4.0))
                .px(px(4.0))
                .ml(px(-4.0))
                .rounded(corner(6.0))
                .cursor_text()
                .font_weight(FontWeight::BOLD)
                .hover(move |st| st.bg(hover))
                .on_click(cx.listener(move |this, _, window, cx| this.start_sound_rename(&s, window, cx)))
                .child(div().min_w_0().truncate().child(sound.name.clone()))
                .when(busy, |el| {
                    el.child(motion::ambient(
                        icon("loader-circle").size(px(14.0)).text_color(p.muted_foreground),
                        SharedString::from(format!("sound-saving-{n}")),
                        Duration::from_millis(900),
                        window,
                        |el, t| el.rotate(gpui_kit::radians(t * std::f32::consts::TAU)),
                    ))
                })
                .into_any_element()
        };
        let about = format!("{} · {}", clock(u64::try_from(sound.duration_ms).unwrap_or(0)), percent(sound.volume));
        // Its volume, a step at a time.
        let step = |dir: f32, glyph: &str| {
            let (sid, v) = (id.clone(), sound.volume);
            let next = ((v + dir * VOLUME_STEP) * 10.0).round() / 10.0;
            let next = next.clamp(0.0, 1.0);
            let same = (next - v).abs() < 0.001;
            icon_button(SharedString::from(format!("sound-vol-{glyph}-{id}")), glyph, p)
                .when(same || busy, |el| el.opacity(0.4))
                .when(!same && !busy, |el| {
                    el.on_click(
                        cx.listener(move |this, _, _, cx| this.change_sound(sid.clone(), None, None, Some(next), cx)),
                    )
                })
        };
        let volume_label = t_with("serversettings.soundboard.volumeOf", &[("name", Arg::Str(&sound.name))]);
        let volume = div()
            .id(SharedString::from(format!("sound-volume-{id}")))
            .flex()
            .items_center()
            .gap(px(2.0))
            .tooltip(move |window, cx| crate::ui::overlay::Tip::new(volume_label.clone()).build(window, cx))
            .child(step(-1.0, "minus"))
            .child(div().w(px(40.0)).text_center().text_xs().font_weight(FontWeight::BOLD).child(motion::swapping(
                SharedString::from(format!("sound-vol-{id}")),
                percent(sound.volume),
                12.0,
            )))
            .child(step(1.0, "plus"));
        let end = if self.sounds.confirming.as_deref() == Some(id.as_str()) {
            let yes = id.clone();
            motion::slide_in(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.muted_foreground)
                            .child(t("serversettings.soundboard.deleteQuestion")),
                    )
                    .child(
                        danger_button(
                            SharedString::from(format!("sound-delete-yes-{id}")),
                            t("serversettings.shared.delete"),
                            p,
                        )
                        .h(px(32.0))
                        .px(px(12.0))
                        .text_xs()
                        .line_height(px(16.0))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.sounds.busy.is_none() {
                                this.remove_sound(yes.clone(), cx)
                            }
                        })),
                    )
                    .child(icon_button(SharedString::from(format!("sound-keep-{id}")), "x", p).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.sounds.confirming = None;
                            cx.notify();
                        },
                    ))),
                SharedString::from(format!("sound-confirm-{id}")),
                8.0,
            )
            .into_any_element()
        } else {
            let ask = id.clone();
            let red = p.destructive;
            let tip = t_with("serversettings.soundboard.deleteNamed", &[("name", Arg::Str(&sound.name))]);
            div()
                .opacity(0.5)
                .id(SharedString::from(format!("{group}-tools")))
                .group_hover(group.clone(), |s| s.opacity(1.0))
                .child(
                    icon_button_in(SharedString::from(format!("sound-delete-{id}")), "trash", p, red)
                        .tooltip(move |window, cx| crate::ui::overlay::Tip::new(tip.clone()).build(window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sounds.confirming = Some(ask.clone());
                            cx.notify();
                        })),
                )
                .into_any_element()
        };
        motion::rise(
            div()
                .id(SharedString::from(format!("sound-row-{id}")))
                .group(group)
                .flex()
                .items_center()
                .gap(px(12.0))
                .p(px(12.0))
                .rounded(corner(16.0))
                .border_1()
                .border_color(p.border)
                .bg(alpha(p.background, 0.5))
                .hover(move |s| s.border_color(hover_border))
                .child(preview)
                .child(emoji)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(div().flex().min_w_0().child(name))
                        .child(div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(about)),
                )
                .child(volume)
                .child(end),
            SharedString::from(format!("sound-in-{id}")),
            Duration::from_millis((n.min(12) * 25) as u64),
            8.0,
        )
        .into_any_element()
    }
}
