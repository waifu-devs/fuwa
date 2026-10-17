//! The soundboard in a voice channel's call (docs/soundboard.md), the web's
//! `components/calls/Soundboard.tsx`: a button beside the call's other
//! controls for people with Use Soundboard there, opening a grid of the
//! server's sounds. Pressing one plays it for everyone in the call; the
//! little play button beside it is a preview for you alone. While you're
//! muted or deafened (or can't talk there) the grid says why it can't play.
//! Who played what shows on their tile for a moment.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, px,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::core::soundboard::{self, Standing};
use crate::core::voice::{CallView, Status};
use crate::pb;
use crate::ui::app::FuwaApp;
use crate::ui::call_parts::{CallPop, Side, Size, button_glyph, call_button_frame, pop_card};
use crate::ui::emoji::InColor as _;
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, corner, radius_xl};
use crate::ui::widgets::{icon, pal};

/// How wide the grid's card is (what `Side::AboveCenter` centers on).
const CARD: f32 = 288.0;
/// Sounds in a row of the grid.
const COLS: usize = 3;
/// The grid's height before it scrolls.
const GRID_MOST: f32 = 264.0;

/// Whether you get the soundboard in this call: a voice channel (never a
/// direct message's call, which is end-to-end encrypted) on an instance that
/// has one, with Use Soundboard there.
pub(crate) fn may_use(app: &FuwaApp, call: &CallView) -> bool {
    if call.is_dm() {
        return false;
    }
    app.core.shared.read(|s| {
        s.instance(&call.instance).is_some_and(|i| {
            i.has("soundboard") && i.access(&call.server_id).has_in(&call.channel_id, pb::Permission::UseSoundboard)
        })
    })
}

/// The id in a server emoji written `<:name:id>` (or `<a:name:id>`).
pub(crate) fn custom_emoji_id(emoji: &str) -> Option<&str> {
    let inner = emoji.strip_prefix('<')?.strip_suffix('>')?;
    let rest = inner.strip_prefix("a:").or_else(|| inner.strip_prefix(':'))?;
    let (_, id) = rest.rsplit_once(':')?;
    (!id.is_empty()).then_some(id)
}

/// A sound's emoji at `size`: a Unicode one as it is, one of the server's
/// own as its picture, a speaker for none (or one that's gone).
pub(crate) fn sound_emoji(emoji: &str, emojis: &[pb::Emoji], size: f32, p: &Palette) -> AnyElement {
    let base = div().size(px(size + 2.0)).flex_none().flex().items_center().justify_center();
    if let Some(id) = custom_emoji_id(emoji) {
        if let Some(e) = emojis.iter().find(|e| e.id == id) {
            return base
                .child(
                    crate::ui::widgets::picture(e.url.clone()).size(px(size)).object_fit(gpui_kit::ObjectFit::Contain),
                )
                .into_any_element();
        }
    } else if !emoji.trim().is_empty() {
        return base.in_color().text_size(px(size * 0.85)).child(emoji.trim().to_owned()).into_any_element();
    }
    base.text_color(p.muted_foreground).child(icon("volume-2").size(px(size * 0.8))).into_any_element()
}

impl FuwaApp {
    /// The soundboard button, with the call's other controls, when you may use it here.
    pub(crate) fn soundboard_button(
        &mut self,
        from: &str,
        size: Size,
        grow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let call = self.core.call()?;
        if !may_use(self, &call) {
            return None;
        }
        let p = pal(cx);
        let pop = CallPop::Soundboard { from: from.to_owned() };
        let open = self.calls.pop.as_ref() == Some(&pop);
        let (bg, fg, hover_bg, hover_fg) = match (open, size) {
            (true, _) => (Some(alpha(p.primary, 0.15)), p.primary.into(), alpha(p.primary, 0.25), p.primary.into()),
            (false, Size::Sm) => (None, p.muted_foreground.into(), p.muted.into(), p.foreground.into()),
            (false, Size::Lg) => (Some(p.muted.into()), p.foreground.into(), alpha(p.muted, 0.7), p.foreground.into()),
        };
        let id = SharedString::from(format!("soundboard|{from}"));
        let button =
            call_button_frame(id.clone(), size, bg, fg, hover_bg, hover_fg, t("dms-calls.calls.soundboard.button"))
                .when(grow, |el| el.flex_1());
        let side = if size == Size::Sm { 18.0 } else { 20.0 };
        let glyph =
            button_glyph(&id, format!("{id}|glyph|{open}"), icon("audio-lines").size(px(side)), 0.4, 0.0, |s| {
                s.scale(1.1)
            });
        let pop_open = pop.clone();
        let button = button.child(div().relative().child(glyph)).on_click(cx.listener(move |this, _, _, cx| {
            if this.calls.pop.as_ref() != Some(&pop_open)
                && let Some(call) = this.core.call()
            {
                this.core.want_sounds(&call.instance, &call.server_id, false);
            }
            this.toggle_call_pop(pop_open.clone(), cx)
        }));
        let holder = div().relative().flex().when(grow, |el| el.flex_1()).child(button).children(self.pop_card(
            &pop,
            Side::AboveCenter,
            window,
            cx,
            |this, window, cx| this.soundboard_card(&call, window, cx),
        ));
        Some(holder.into_any_element())
    }

    /// Where you stand in the call, for whether a sound may play for everyone.
    fn standing(&self, call: &CallView) -> Standing {
        let (server_muted, may_speak) = self.core.shared.read(|s| {
            let Some(i) = s.instance(&call.instance) else { return (false, false) };
            let me = i.me.as_ref().map(|m| m.id.as_str()).unwrap_or_default();
            let server_muted = i
                .voice
                .get(&call.server_id)
                .and_then(|list| list.iter().find(|v| v.user_id == me))
                .is_some_and(|v| v.server_mute || v.server_deaf);
            (server_muted, i.access(&call.server_id).has_in(&call.channel_id, pb::Permission::Speak))
        });
        Standing {
            connected: call.status == Status::Connected,
            muted: call.self_mute,
            deafened: call.self_deaf,
            server_muted,
            may_speak,
        }
    }

    /// The grid of the server's sounds.
    fn soundboard_card(&mut self, call: &CallView, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let (key, server) = (call.instance.clone(), call.server_id.clone());
        let sounds = self.core.sounds(&key, &server);
        let failed = sounds.is_none() && self.core.sounds_failed(&key, &server);
        if sounds.is_none() && !failed {
            self.core.want_sounds(&key, &server, false);
        }
        let emojis: Vec<pb::Emoji> = self
            .core
            .shared
            .read(|s| s.instance(&key).and_then(|i| i.emojis.get(&server).cloned()).unwrap_or_default());
        let blocked = soundboard::blocked(self.standing(call));
        let head = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .px(px(4.0))
            .child(div().font_weight(FontWeight::BOLD).text_sm().child(t("dms-calls.calls.soundboard.title")))
            .child(
                div()
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(t("dms-calls.calls.soundboard.hint")),
            );
        let mut card = pop_card(CARD, &p).flex().flex_col().gap(px(10.0)).child(head);
        if let Some(why) = blocked {
            card = card.child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(corner(10.0))
                    .bg(alpha(p.muted, 0.6))
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(icon(if call.self_deaf { "headphone-off" } else { "mic-off" }).size(px(14.0)).flex_none())
                    .child(div().min_w_0().child(t(why))),
                SharedString::from(format!("soundboard-why|{why}")),
                std::time::Duration::ZERO,
                4.0,
            ));
        }
        let body: AnyElement = match sounds {
            None if failed => div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.0))
                .py(px(16.0))
                .text_sm()
                .text_color(p.muted_foreground)
                .child(t("dms-calls.calls.soundboard.failed"))
                .child(
                    crate::ui::widgets::soft_button("soundboard-retry", t("dms-calls.calls.soundboard.retry"), &p)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.core.want_sounds(&key, &server, true);
                            cx.notify();
                        })),
                )
                .into_any_element(),
            None => div()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .py(px(20.0))
                .text_sm()
                .text_color(p.muted_foreground)
                .child(motion::ambient(
                    icon("loader-circle").size(px(16.0)),
                    "soundboard-loading",
                    std::time::Duration::from_millis(900),
                    window,
                    |el, t| el.rotate(gpui_kit::radians(t * std::f32::consts::TAU)),
                ))
                .child(t("dms-calls.calls.soundboard.loading"))
                .into_any_element(),
            Some(list) if list.is_empty() => div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.0))
                .py(px(16.0))
                .text_center()
                .child(div().text_size(px(28.0)).in_color().child("🔇"))
                .child(div().font_weight(FontWeight::BOLD).text_sm().child(t("dms-calls.calls.soundboard.empty")))
                .child(
                    div()
                        .text_xs()
                        .line_height(px(16.0))
                        .text_color(p.muted_foreground)
                        .child(t("dms-calls.calls.soundboard.emptyText")),
                )
                .into_any_element(),
            Some(list) => {
                let cell = (CARD - 24.0 - 8.0 * (COLS - 1) as f32) / COLS as f32;
                let mut grid = div().flex().flex_wrap().gap(px(8.0));
                for (n, sound) in list.iter().enumerate() {
                    grid = grid.child(self.sound_cell(call, sound, &emojis, blocked.is_none(), cell, n, &p, cx));
                }
                let rows = list.len().div_ceil(COLS) as f32;
                let tall = rows * (cell * 0.8 + 8.0) > GRID_MOST;
                // The card floats over the page and occludes, so its grid scrolls by itself.
                div()
                    .id("soundboard-grid")
                    .when(tall, |el| el.h(px(GRID_MOST)))
                    .overflow_y_scroll()
                    .child(grid)
                    .into_any_element()
            }
        };
        card.child(body).into_any_element()
    }

    /// One sound in the grid: press it to play it for everyone; its little
    /// play button previews it for you alone.
    #[allow(clippy::too_many_arguments)]
    fn sound_cell(
        &self,
        call: &CallView,
        sound: &pb::Sound,
        emojis: &[pb::Emoji],
        may_play: bool,
        w: f32,
        n: usize,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = sound.id.clone();
        let group = SharedString::from(format!("sound|{id}"));
        let hover_bg = alpha(p.primary, 0.12);
        let hover_border = alpha(p.primary, 0.4);
        let (play_call, play_sound) = (call.clone(), sound.clone());
        let label = t_with("dms-calls.calls.soundboard.play", &[("name", Arg::Str(&sound.name))]);
        let main = div()
            .id(SharedString::from(format!("sound-play|{id}")))
            .group(group.clone())
            .relative()
            .w(px(w))
            .h(px(w * 0.8))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(2.0))
            .px(px(6.0))
            .rounded(radius_xl())
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.muted, 0.35))
            .when(!may_play, |el| el.opacity(0.5))
            .when(may_play, |el| {
                el.cursor_pointer()
                    .hover(move |s| s.bg(hover_bg).border_color(hover_border))
                    .active(|s| s.scale(0.93))
                    .on_click(cx.listener(move |this, _, _, cx| this.play_for_everyone(&play_call, &play_sound, cx)))
            })
            .tooltip(move |window, cx| crate::ui::overlay::Tip::new(label.clone()).build(window, cx))
            .child(
                div()
                    .id(SharedString::from(format!("sound-emoji|{id}")))
                    .group_hover(group.clone(), |s| s.scale(1.15))
                    .child(sound_emoji(&sound.emoji, emojis, 24.0, p)),
            )
            .child(
                div()
                    .w_full()
                    .text_center()
                    .truncate()
                    .text_xs()
                    .line_height(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .child(sound.name.clone()),
            );
        let (key, preview) = (call.instance.clone(), sound.clone());
        let preview_label = t_with("dms-calls.calls.soundboard.preview", &[("name", Arg::Str(&sound.name))]);
        let preview_button = div()
            .id(SharedString::from(format!("sound-preview|{id}")))
            .absolute()
            .top(px(3.0))
            .right(px(3.0))
            .size(px(20.0))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(p.card)
            .border_1()
            .border_color(p.border)
            .text_color(p.muted_foreground)
            .cursor_pointer()
            .opacity(0.0)
            .group_hover(group, |s| s.opacity(1.0))
            .hover(|s| s.scale(1.1))
            .active(|s| s.scale(0.9))
            .tooltip(move |window, cx| crate::ui::overlay::Tip::new(preview_label.clone()).build(window, cx))
            .child(icon("play").size(px(11.0)))
            .on_click(cx.listener(move |this, _, _, cx| {
                // Not the sound under it too.
                cx.stop_propagation();
                this.preview_sound(&key, &preview, cx);
            }));
        motion::rise(
            div().relative().child(main).child(preview_button),
            SharedString::from(format!("sound-in|{id}")),
            std::time::Duration::from_millis((n.min(12) * 20) as u64),
            6.0,
        )
        .into_any_element()
    }

    /// Plays a sound for everyone in the call (you hear it when the instance says it played).
    fn play_for_everyone(&mut self, call: &CallView, sound: &pb::Sound, cx: &mut Context<Self>) {
        let core = self.core.clone();
        let (key, server, channel, id) =
            (call.instance.clone(), call.server_id.clone(), call.channel_id.clone(), sound.id.clone());
        self.run(cx, async move { core.play_sound(&key, &server, &channel, &id).await }, |this, result, cx| {
            if let Err(err) = result {
                this.toast("volume-x", t("dms-calls.calls.soundboard.cantPlay"), err.message, None, None, cx);
            }
        });
    }

    /// Plays a sound for you alone.
    fn preview_sound(&mut self, key: &str, sound: &pb::Sound, cx: &mut Context<Self>) {
        let (core, key, sound) = (self.core.clone(), key.to_owned(), sound.clone());
        self.run(cx, async move { core.preview_sound(&key, &sound).await }, |this, result, cx| {
            if let Err(err) = result {
                this.toast("volume-x", t("dms-calls.calls.soundboard.cantPlay"), err.message, None, None, cx);
            }
        });
    }

    /// What someone just played, beside their name (`tile` on their stage tile, in its corner).
    pub(crate) fn sound_played_badge(
        &self,
        key: &str,
        server: &str,
        channel: &str,
        user_id: &str,
        tile: bool,
        p: &Palette,
    ) -> Option<AnyElement> {
        let (played, emojis) = self.core.shared.read(|s| {
            let i = s.instance(key)?;
            let played = soundboard::played_by(i, channel, user_id)?.clone();
            Some((played, i.emojis.get(server).cloned().unwrap_or_default()))
        })?;
        let label = div().min_w_0().truncate().child(played.name.clone());
        let pill = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .rounded_full()
            .bg(alpha(p.primary, 0.15))
            .text_color(p.primary)
            .font_weight(FontWeight::BOLD)
            .text_xs()
            .line_height(px(16.0))
            .child(sound_emoji(&played.emoji, &emojis, 12.0, p));
        let pill = if tile {
            pill.absolute().top(px(8.0)).left(px(8.0)).max_w(px(160.0)).px(px(8.0)).py(px(2.0)).child(label)
        } else {
            pill.flex_none().max_w(px(72.0)).px(px(6.0)).child(label)
        };
        Some(
            motion::pop(
                pill,
                SharedString::from(format!("played|{channel}|{user_id}|{:?}", played.at)),
                0.6,
                -8.0,
                std::time::Duration::ZERO,
            )
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_emoji_are_told_by_their_id() {
        assert_eq!(custom_emoji_id("<:party:01ABC>"), Some("01ABC"));
        assert_eq!(custom_emoji_id("<a:spin:01XYZ>"), Some("01XYZ"));
        assert_eq!(custom_emoji_id("🎉"), None);
        assert_eq!(custom_emoji_id("<:broken>"), None);
        assert_eq!(custom_emoji_id(""), None);
    }
}
