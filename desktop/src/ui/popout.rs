//! Popped-out cameras and screens (the web's `PopOutWindow`): one person's
//! camera, or their shared screen, in a window of its own, edge to edge, or
//! their avatar on their color while it's off. Nothing else shows unless
//! the mouse moves: the name, a glow while they talk, filling or fitting the
//! window, each remembered. The window is titled for the person, so
//! streaming apps (OBS's Window Capture) find it again as a clean feed.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    BoxShadow, Context, FontWeight, InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, point, px,
};

use crate::core::Core;
use crate::core::config::PopoutFit;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::voice::video::feed_of;
use crate::pb;
use crate::ui::call_parts::{green, voice_avatar};
use crate::ui::motion;
use crate::ui::theme::{alpha, radius_xl};
use crate::ui::video;
use crate::ui::widgets::{hue_gradient, icon};

/// How long the controls stay after the mouse last moved.
const AWAKE: Duration = Duration::from_millis(1600);

/// Whose feed a pop-out shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Popped {
    pub instance: String,
    pub user: String,
    /// The voice channel's server, for the name people have there.
    pub server: Option<String>,
    pub screen: bool,
}

impl Popped {
    pub fn key(&self) -> String {
        format!("{}/{}", self.instance, feed_of(&self.user, self.screen))
    }
}

pub(crate) struct PopOut {
    core: Arc<Core>,
    pub popped: Popped,
    moved: Instant,
    /// The cursor was hidden with the controls, until the mouse moves again.
    cursor_hidden: bool,
    title: String,
}

impl PopOut {
    pub fn new(core: Arc<Core>, popped: Popped, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Pictures and who's talking.
        for mut changes in [core.changes(), core.videos().changes()] {
            cx.spawn_in(window, async move |this, cx| {
                while changes.changed().await.is_ok() {
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        Self { core, popped, moved: Instant::now(), cursor_hidden: false, title: String::new() }
    }

    /// The person's name where the call is.
    fn name(&self) -> String {
        let p = &self.popped;
        self.core
            .shared
            .read(|s| s.instance(&p.instance).map(|i| i.display_name(p.server.as_deref(), &p.user)).unwrap_or_default())
    }

    /// Whether their camera (or screen) is on, in the call you're in.
    fn on(&self) -> bool {
        let p = &self.popped;
        let Some(call) = self.core.call().filter(|c| c.instance == p.instance) else { return false };
        let me = self.core.shared.read(|s| s.instance(&p.instance).and_then(|i| i.me.as_ref().map(|m| m.id.clone())));
        if me.as_deref() == Some(p.user.as_str()) {
            return if p.screen { call.self_stream } else { call.self_video };
        }
        let state: Option<pb::VoiceState> = self.core.shared.read(|s| {
            let i = s.instance(&p.instance)?;
            if call.is_dm() {
                i.dms.calls.get(&call.conversation_id)?.participants.iter().find(|v| v.user_id == p.user).cloned()
            } else {
                i.voice.get(&call.server_id)?.iter().find(|v| v.user_id == p.user).cloned()
            }
        });
        state.is_some_and(|v| if p.screen { v.self_stream } else { v.self_video })
    }
}

/// A round button in the pop-out's corner: brighter while its setting is on.
fn toggle(id: &str, glyph: &str, on: bool, label: String) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(SharedString::from(id.to_owned()))
        .size(px(36.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius_xl())
        .cursor_pointer()
        .text_color(if on { gpui_kit::white() } else { gpui_kit::hsla(0.0, 0.0, 1.0, 0.7) })
        .bg(if on { gpui_kit::hsla(0.0, 0.0, 1.0, 0.25) } else { gpui_kit::hsla(0.0, 0.0, 0.0, 0.5) })
        .backdrop_blur(px(8.0))
        .hover(|s| s.text_color(gpui_kit::white()).scale(1.05))
        .active(|s| s.scale(0.9))
        .tooltip(move |window, cx| crate::ui::overlay::Tip::new(label.clone()).build(window, cx))
        .child(icon(glyph).size(px(16.0)))
}

impl Render for PopOut {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prefs = self.core.prefs();
        let p = self.popped.clone();
        let name = self.name();
        let title = t_with(
            if p.screen { "dms-calls.calls.video.screenWindow" } else { "dms-calls.calls.video.cameraWindow" },
            &[("name", Arg::Str(&name))],
        );
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }
        let on = self.on();
        let speaking = self.core.call().is_some_and(|c| c.speaking.contains(&p.user));
        let awake = self.moved.elapsed() < AWAKE;
        // The cursor goes with the controls (the web's `cursor-none`), once each time.
        if !awake && !self.cursor_hidden {
            self.cursor_hidden = true;
            cx.hide_cursor_until_mouse_moves();
        }
        // The controls and the glow fade in and out (`transition-opacity duration-300`).
        let shown = motion::follow("pop-awake", if awake { 1.0 } else { 0.0 }, window, cx).clamp(0.0, 1.0);
        let glowing = prefs.popout_glow && speaking;
        let glow = motion::follow("pop-glow", if glowing { 1.0 } else { 0.0 }, window, cx).clamp(0.0, 1.0);
        let grow = motion::follow("pop-grow", if glowing { 1.06 } else { 1.0 }, window, cx);
        let fit =
            if p.screen || prefs.popout_fit == PopoutFit::Contain { ObjectFit::Contain } else { ObjectFit::Cover };
        let user = self.core.shared.read(|s| s.instance(&p.instance).and_then(|i| i.users.get(&p.user).cloned()));

        let mut root = div()
            .id("popout")
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(gpui_kit::black())
            .text_color(gpui_kit::white())
            .font_family(crate::ui::theme::FONT)
            .on_mouse_move(cx.listener(|this, _, _, cx| {
                let was = this.moved.elapsed() < AWAKE;
                this.moved = Instant::now();
                this.cursor_hidden = false;
                if !was {
                    cx.notify();
                }
                // And again once the controls should go.
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(AWAKE).await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
            }))
            .child(hue_gradient(&p.user, 0.0, div().absolute().inset_0()).opacity(0.25))
            // Their avatar while the feed's off, ringed and a little bigger while they talk.
            .child(div().absolute().inset_0().flex().items_center().justify_center().child(voice_avatar(
                user.as_ref(),
                &p.user,
                112.0 * grow,
                36.0 * grow,
                4.0,
                glowing,
                "popout",
                window,
                cx,
            )));
        if on {
            root = root.child(video::feed_view(&self.core, &feed_of(&p.user, p.screen), fit, px(0.0), window, cx));
        }
        if glow > 0.01 {
            // The web's `inset 0 0 0 4px #3ba55d, inset 0 0 40px` green glow.
            root =
                root.child(div().absolute().inset_0().border(px(4.0)).border_color(alpha(green(), glow)).shadow(vec![
                    BoxShadow {
                        color: gpui_kit::hsla(136.0 / 360.0, 0.47, 0.44, 0.45 * glow),
                        offset: point(px(0.0), px(0.0)),
                        blur_radius: px(40.0),
                        spread_radius: px(0.0),
                        inset: true,
                    },
                ]));
        }
        if prefs.popout_name {
            root = root.child(
                div()
                    .absolute()
                    .bottom(px(12.0))
                    .left(px(12.0))
                    .max_w(relative_width())
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .rounded(radius_xl())
                    .bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.6))
                    .px(px(12.0))
                    .py(px(4.0))
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child(name),
            );
        }
        if shown > 0.01 {
            let core = self.core.clone();
            let (n, g, f) = (prefs.popout_name, prefs.popout_glow, prefs.popout_fit);
            let mut controls = div().absolute().top(px(12.0)).right(px(12.0)).flex().gap(px(6.0)).opacity(shown).child(
                toggle(
                    "pop-name",
                    "tag",
                    n,
                    t(if n { "dms-calls.calls.video.hideName" } else { "dms-calls.calls.video.showName" }),
                )
                .on_click({
                    let core = core.clone();
                    move |_, _, _| core.set_prefs(|pr| pr.popout_name = !n)
                }),
            );
            controls = controls.child(
                toggle(
                    "pop-glow",
                    "sparkles",
                    g,
                    t(if g { "dms-calls.calls.video.noGlow" } else { "dms-calls.calls.video.glow" }),
                )
                .on_click({
                    let core = core.clone();
                    move |_, _, _| core.set_prefs(|pr| pr.popout_glow = !g)
                }),
            );
            // A screen always shows whole: cropping it would cut off what's shown.
            if !p.screen {
                let cover = f == PopoutFit::Cover;
                controls = controls.child(
                    toggle(
                        "pop-fit",
                        if cover { "crop" } else { "expand" },
                        cover,
                        t(if cover { "dms-calls.calls.video.fit" } else { "dms-calls.calls.video.fill" }),
                    )
                    .on_click({
                        let core = core.clone();
                        move |_, _, _| {
                            core.set_prefs(|pr| {
                                pr.popout_fit = if cover { PopoutFit::Contain } else { PopoutFit::Cover }
                            })
                        }
                    }),
                );
            }
            controls = controls.child(
                toggle("pop-close", "x", false, t("common.close")).on_click(|_, window, _| window.remove_window()),
            );
            root = root.child(controls);
        }
        root.child(video::commit(self.core.clone()))
    }
}

/// The name's chip takes at most most of the width.
fn relative_width() -> gpui_kit::DefiniteLength {
    gpui_kit::relative(0.7)
}
