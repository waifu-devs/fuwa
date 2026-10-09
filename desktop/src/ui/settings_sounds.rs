//! Choosing what each of the app's sounds plays (`core::sounds`): a row per
//! sound with a play button, its switch where it has one, and a menu of the
//! built-in tunes and "Choose a file…", for a sound file of your own that's
//! copied into the app's folder. Used by the Notifications page (messages,
//! mentions, direct messages, joins) and Voice & video (the ringtone and
//! each of a call's cues).

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::config::{Prefs, SoundPick, Sounds};
use crate::core::i18n::{Arg, t, t_with};
use crate::core::sounds::{self, Sound, TUNES};
use crate::ui::settings::SettingsView;
use crate::ui::settings_controls::switch;
use crate::ui::settings_menu::Item;
use crate::ui::theme::{Palette, alpha, radius_xl};
use crate::ui::widgets::icon;

/// A sound's switch in `Sounds`: whether it's on, and turning it on or off.
pub(crate) type Switch = (bool, fn(&mut Sounds, bool));

/// A built-in tune's name.
fn tune_name(id: &str) -> String {
    t(&format!("desktop.sounds.tune.{id}"))
}

/// What a sound plays now, as the menu's button says it.
fn picked_name(sound: Sound, prefs: &Prefs) -> String {
    match prefs.sound_picks.get(sound.id()) {
        Some(pick) if !pick.file.is_empty() => pick.name.clone(),
        Some(pick) if sounds::tune_by_id(&pick.tune).is_some() => tune_name(&pick.tune),
        _ => tune_name(sound.own_tune()),
    }
}

/// Whether any of `list` plays something other than its own tune.
pub(crate) fn picks_changed(prefs: &Prefs, list: &[Sound]) -> bool {
    list.iter().any(|s| prefs.sound_picks.contains_key(s.id()))
}

/// Puts every sound in `list` back to its own tune.
pub(crate) fn reset_picks(prefs: &mut Prefs, list: &[Sound]) {
    for s in list {
        prefs.sound_picks.remove(s.id());
    }
}

impl SettingsView {
    /// One sound: play it, its name and hint, the menu of what it plays and,
    /// when it has one, its switch. `dim` greys it while a switch over it is off.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn sound_row(
        &self,
        sound: Sound,
        label: String,
        hint: Option<String>,
        toggle: Option<Switch>,
        dim: bool,
        prefs: &Prefs,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (hover_bg, hover_fg) = (p.primary, p.primary_foreground);
        let (volume, device) = (prefs.volume, prefs.output_device.clone());
        let id = sound.id();
        let off = dim || toggle.is_some_and(|(on, _)| !on);
        let error = self.sound_error.as_ref().filter(|(s, _)| *s == sound).map(|(_, e)| e.clone());
        let row = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .child(
                div()
                    .id(SharedString::from(format!("play-sound-{id}")))
                    .group(SharedString::from(format!("play-sound-{id}")))
                    .size(px(36.0))
                    .flex_none()
                    .rounded_full()
                    .bg(p.muted)
                    .text_color(p.muted_foreground)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                    .active(|s| s.scale(0.9))
                    .on_click(move |_, _, _| sounds::play(sound, volume, &device))
                    .child(
                        div()
                            .id(SharedString::from(format!("play-sound-{id}-icon")))
                            .ml(px(1.0))
                            .group_hover(SharedString::from(format!("play-sound-{id}")), |s| s.scale(1.1))
                            .child(icon("play").size(px(16.0))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(off, |el| el.opacity(0.6))
                    .child(div().text_sm().line_height(px(20.0)).font_weight(FontWeight::BOLD).child(label))
                    .when_some(hint, |el, h| {
                        el.child(div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(h))
                    }),
            )
            // `Call` is only the cues' sample, standing for all of them: each picks its own.
            .when(sound != Sound::Call, |el| el.child(self.sound_picker(sound, prefs, p, cx)))
            .when_some(toggle, |el, (on, put)| {
                el.child(switch(
                    SharedString::from(format!("switch-sound-{id}")),
                    on,
                    false,
                    p,
                    window,
                    cx,
                    move |this, v, cx| this.set(cx, |pr| put(&mut pr.sounds, v)),
                ))
            });
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(row)
            .when_some(error, |el, e| el.child(div().pl(px(48.0)).text_xs().text_color(p.destructive).child(e)))
            .into_any_element()
    }

    /// The menu of what a sound plays: its own tune, another built-in one, or a file of yours.
    fn sound_picker(&self, sound: Sound, prefs: &Prefs, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = sound.id();
        let current = prefs.sound_picks.get(id).cloned();
        let file = current.as_ref().filter(|c| !c.file.is_empty()).cloned();
        let tune_now = current
            .as_ref()
            .filter(|c| c.file.is_empty())
            .map(|c| c.tune.clone())
            .unwrap_or(sound.own_tune().to_owned());
        let (volume, device) = (prefs.volume, prefs.output_device.clone());
        let mut items = vec![Item::Label(t("desktop.sounds.builtIn"))];
        for (tune, _) in TUNES {
            let own = tune == sound.own_tune();
            let label = if own {
                t_with("desktop.sounds.ownTune", &[("name", Arg::Str(&tune_name(tune)))])
            } else {
                tune_name(tune)
            };
            let on = file.is_none() && tune_now == tune;
            let device = device.clone();
            items.push(Item::action(label, Some(if on { "check" } else { "play" }), move |this, cx| {
                let pick = SoundPick { tune: tune.to_owned(), ..Default::default() };
                sounds::play_pick(sound, &pick, volume, &device);
                this.sound_error = None;
                this.set(cx, |pr| {
                    if own {
                        pr.sound_picks.remove(id);
                    } else {
                        pr.sound_picks.insert(id.to_owned(), pick);
                    }
                });
            }));
        }
        items.push(Item::Separator);
        items.push(Item::Label(t("desktop.sounds.yourOwn")));
        if let Some(file) = file {
            let device = device.clone();
            let name = file.name.clone();
            items.push(Item::action(name, Some("check"), move |_, _| sounds::play_pick(sound, &file, volume, &device)));
        }
        items.push(Item::action(t("desktop.sounds.chooseFile"), Some("folder-open"), move |this, cx| {
            this.choose_sound_file(sound, cx)
        }));
        let hover = alpha(p.primary, 0.5);
        let busy = self.sound_busy == Some(sound);
        let trigger = div()
            .id(SharedString::from(format!("sound-pick-{id}-trigger")))
            .h(px(32.0))
            .w(px(168.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .rounded(radius_xl())
            .border_1()
            .border_color(p.border)
            .bg(p.background)
            .px(px(10.0))
            .text_sm()
            .cursor_pointer()
            .hover(move |s| s.border_color(hover))
            .child(
                icon(if current.as_ref().is_some_and(|c| !c.file.is_empty()) { "audio-lines" } else { "sparkles" })
                    .size(px(14.0))
                    .text_color(p.muted_foreground),
            )
            .child(div().flex_1().min_w_0().truncate().child(if busy {
                t("desktop.sounds.copying")
            } else {
                picked_name(sound, prefs)
            }))
            .child(icon("chevron-down").size(px(14.0)).text_color(p.muted_foreground));
        div()
            .flex_none()
            .child(self.dropdown(format!("sound-pick-{id}"), trigger, items, true, 240.0, p, cx))
            .into_any_element()
    }

    /// Asks for a sound file, copies it into the app's folder once it's known
    /// to play, and makes it what `sound` plays.
    fn choose_sound_file(&mut self, sound: Sound, cx: &mut Context<Self>) {
        if self.sound_busy.is_some() {
            return;
        }
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("desktop.sounds.chooseFile").into()),
        });
        let (core, dir) = (self.core.clone(), self.core.paths.sounds());
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| {
                this.sound_busy = Some(sound);
                this.sound_error = None;
                cx.notify();
            });
            let rx = core.spawn(async move {
                tokio::task::spawn_blocking(move || sounds::import(&dir, &path))
                    .await
                    .unwrap_or_else(|_| Err(t("desktop.sounds.cantSave")))
            });
            let result = rx.await;
            let _ = this.update(cx, |this, cx| {
                this.sound_busy = None;
                match result {
                    Ok(Ok(pick)) => {
                        crate::core::reports::used("sounds.file");
                        let prefs = this.core.prefs();
                        sounds::play_pick(sound, &pick, prefs.volume, &prefs.output_device);
                        this.set(cx, |pr| {
                            pr.sound_picks.insert(sound.id().to_owned(), pick);
                        });
                    }
                    Ok(Err(error)) => {
                        this.sound_error = Some((sound, error));
                        cx.notify();
                    }
                    Err(_) => cx.notify(),
                }
            });
        })
        .detach();
    }
}
