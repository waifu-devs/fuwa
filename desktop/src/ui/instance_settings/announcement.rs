//! The Announcement page: the banner across the top of every app on this
//! instance, for news and maintenance notices, with a live preview. Changing
//! only the tone or the end keeps it closed for people who closed it; new
//! words bring it back for everyone. As in the web's `Announcement.tsx`.

use crate::ui::instance_home::{focus_ring, has_focus};
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use super::controls::{Opt, area_box, choice_chip};
use super::{InstanceSettingsEvent, InstanceSettingsView};
use crate::core::dms::now_ms;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::instance_manage::{self as manage, LENGTHS, TEXT_MAX};
use crate::pb::{self, AnnouncementTone as Tone};
use crate::ui::announcement::banner_in;
use crate::ui::motion;
use crate::ui::server_settings::{amber, spinner};
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{error_line, icon, primary_button};

/// How long it stays up. `Keep` leaves the end of the one that's up where it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Ends {
    Keep,
    Never,
    For(i64),
}

/// The page's own state; it doesn't wait for the save bar.
pub(super) struct Announce {
    pub text: Entity<TextareaState>,
    pub tone: Tone,
    pub ends: Ends,
    pub busy: bool,
    pub error: Option<String>,
    /// Which announcement the boxes were filled from, so a new one fills them again.
    pub filled: Option<String>,
    /// Goes up with each put-up, so the megaphone shouts once.
    pub shouts: u32,
}

impl Announce {
    pub fn new(window: &mut Window, cx: &mut Context<InstanceSettingsView>) -> (Self, gpui_kit::Subscription) {
        let text = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(2, 5).placeholder(t("instancesettings.announcement.placeholder"))
        });
        let sub =
            cx.subscribe_in(&text, window, |this: &mut InstanceSettingsView, state, e: &InputEvent, window, cx| {
                if !matches!(e, InputEvent::Change) {
                    return;
                }
                let value = state.read(cx).value().to_string();
                if value.chars().count() > TEXT_MAX {
                    let cut: String = value.chars().take(TEXT_MAX).collect();
                    state.update(cx, |s, cx| s.set_value(cut, window, cx));
                }
                this.announce.error = None;
                cx.notify();
            });
        (Self { text, tone: Tone::Info, ends: Ends::Never, busy: false, error: None, filled: None, shouts: 0 }, sub)
    }
}

impl InstanceSettingsView {
    fn live_announcement(&self, now: i64) -> Option<pb::Announcement> {
        self.core
            .shared
            .read(|s| s.instance(&self.key).and_then(|i| i.node.as_ref()).and_then(|n| n.announcement.clone()))
            .filter(|a| manage::is_live(Some(a), now))
    }

    /// Fills the boxes from what's up, once for each announcement.
    fn fill_announcement(&mut self, live: Option<&pb::Announcement>, window: &mut Window, cx: &mut Context<Self>) {
        let id = live.map(|a| a.id.clone()).unwrap_or_default();
        if self.announce.filled.as_ref() == Some(&id) {
            return;
        }
        self.announce.filled = Some(id);
        let text = live.map(|a| a.text.clone()).unwrap_or_default();
        self.announce.text.update(cx, |s, cx| s.set_value(text, window, cx));
        self.announce.tone = live.map(manage::tone_of).unwrap_or(Tone::Info);
        self.announce.ends = if live.is_some_and(|a| a.ends_at.is_some()) { Ends::Keep } else { Ends::Never };
    }

    fn put_up(&mut self, take_down: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.announce.busy {
            return;
        }
        let now = now_ms();
        let live = self.live_announcement(now);
        let text = if take_down { String::new() } else { self.announce.text.read(cx).value().trim().to_owned() };
        let ends_at = match self.announce.ends {
            Ends::Keep => live.as_ref().and_then(manage::ends_ms),
            Ends::Never => None,
            Ends::For(ms) => Some(now + ms),
        };
        self.announce.busy = true;
        self.announce.error = None;
        if !take_down {
            self.announce.shouts += 1;
        }
        let (core, key, tone) = (self.core.clone(), self.key.clone(), self.announce.tone);
        let was_up = live.is_some();
        self.run(
            window,
            cx,
            async move { core.set_announcement(&key, text, tone, if take_down { None } else { ends_at }).await },
            move |this, result, window, cx| {
                this.announce.busy = false;
                match result {
                    Ok(now_up) => {
                        let title = match (take_down, was_up) {
                            (true, _) => &t("instancesettings.announcement.down"),
                            (false, true) => &t("instancesettings.announcement.updated"),
                            (false, false) => &t("instancesettings.announcement.up"),
                        };
                        // What came back is what the boxes now show.
                        this.announce.filled = None;
                        this.fill_announcement(now_up.as_ref(), window, cx);
                        cx.emit(InstanceSettingsEvent::Toast { icon: "megaphone", title: title.to_owned() });
                    }
                    Err(problem) => this.announce.error = Some(problem.message),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    pub(super) fn announcement_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let now = now_ms();
        let live = self.live_announcement(now);
        self.fill_announcement(live.as_ref(), window, cx);
        let typed = self.announce.text.read(cx).value().to_string();
        let trimmed = typed.trim().to_owned();
        let tone = self.announce.tone;
        let ends = self.announce.ends;
        let ends_at = match ends {
            Ends::Keep => live.as_ref().and_then(manage::ends_ms),
            Ends::Never => None,
            Ends::For(ms) => Some(now + ms),
        };
        let changed = live.as_ref().is_none_or(|l| {
            trimmed != l.text
                || tone != manage::tone_of(l)
                || match ends {
                    Ends::Keep => false,
                    Ends::Never => l.ends_at.is_some(),
                    Ends::For(_) => true,
                }
        });
        let draft = pb::Announcement {
            id: live.as_ref().filter(|l| l.text == trimmed).map_or_else(|| "draft".to_owned(), |l| l.id.clone()),
            text: if trimmed.is_empty() { t("instancesettings.announcement.previewText") } else { trimmed.clone() },
            tone: tone as i32,
            ends_at: ends_at.map(|ms| prost_types::Timestamp { seconds: ms / 1000, nanos: 0 }),
            ..Default::default()
        };

        // The preview: the banner over a sketch of the app.
        let close = (tone != Tone::Critical).then(|| {
            div()
                .size(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .child(icon("x").size(px(16.0)))
                .into_any_element()
        });
        let sketch = div()
            .h(px(80.0))
            .flex()
            .gap(px(8.0))
            .p(px(12.0))
            .opacity(0.5)
            .child(div().w(px(40.0)).rounded(corner(12.0)).bg(p.muted))
            .child(div().w(px(112.0)).rounded(corner(12.0)).bg(alpha(p.muted, 0.7)))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(div().h(px(12.0)).w(px(320.0)).rounded_full().bg(p.muted))
                    .child(div().h(px(12.0)).w(px(240.0)).rounded_full().bg(alpha(p.muted, 0.7))),
            );
        let status = match &live {
            Some(l) => {
                let since = manage::created_ms(l).map(|at| manage::stamp_label(at, now)).unwrap_or_default();
                match manage::ends_ms(l) {
                    Some(at) => t_with(
                        "instancesettings.announcement.upSinceUntil",
                        &[("time", Arg::Str(&since)), ("end", Arg::Str(&manage::ends_label(at, now)))],
                    ),
                    None => t_with("instancesettings.announcement.upSince", &[("time", Arg::Str(&since))]),
                }
            }
            None => t("instancesettings.announcement.nothingUp"),
        };
        let green = p.success;
        let dot = div()
            .relative()
            .size(px(8.0))
            .rounded_full()
            .bg(if live.is_some() { green.into() } else { alpha(p.muted_foreground, 0.4) })
            .when(live.is_some(), |el| {
                el.child(motion::ambient(
                    div().absolute().rounded_full().bg(alpha(green, 0.6)),
                    "announcement-live-ping",
                    Duration::from_millis(1400),
                    window,
                    |el, t| {
                        let grow = 8.0 + 10.0 * t;
                        el.size(px(grow)).top(px(4.0 - grow / 2.0)).left(px(4.0 - grow / 2.0)).opacity(1.0 - t)
                    },
                ))
            });
        let preview = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .overflow_hidden()
                    .rounded(corner(16.0))
                    .border_1()
                    .border_color(p.border)
                    .bg(alpha(p.background, 0.4))
                    .child(banner_in("announcement-preview", &draft, now, close, Some(corner(15.0)), p, window))
                    .child(sketch),
            )
            .child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_xs()
                    .text_color(p.muted_foreground)
                    .child(dot)
                    .child(status),
                SharedString::from(format!(
                    "announcement-status-{}",
                    live.as_ref().map_or(&t("instancesettings.shared.none"), |l| &l.id)
                )),
                Duration::ZERO,
                6.0,
            ));

        // The message, with how much of its room is used.
        let used = typed.chars().count();
        let left = TEXT_MAX.saturating_sub(used);
        let fill = (used as f32 / TEXT_MAX as f32).min(1.0);
        let warm = if left == 0 {
            p.destructive.into()
        } else if left <= 30 {
            amber(p)
        } else {
            p.primary.into()
        };
        let share = motion::follow("announcement-room", fill, window, cx);
        // `min-h-20 rounded-xl pr-12`, with the ring of room used in its corner (`LengthRing`).
        let message = div()
            .relative()
            .child(
                focus_ring(
                    area_box(Textarea::new(&self.announce.text).appearance(false), None, p),
                    has_focus(&self.announce.text, window, cx),
                    p,
                )
                .min_h(px(80.0))
                .pr(px(48.0)),
            )
            .child(
                div()
                    .absolute()
                    .right(px(10.0))
                    .bottom(px(10.0))
                    .size(px(28.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(length_ring(share, alpha(warm.to_rgb(), 0.15), warm))
                    .when(left <= 30, |el| {
                        el.child(
                            div()
                                .relative()
                                .text_size(px(9.6))
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .text_color(warm)
                                .child(left.to_string()),
                        )
                    }),
            );

        let tones = self.choice(
            "announcement-tone",
            tone as i32,
            vec![
                Opt::new(
                    Tone::Info as i32,
                    t("instancesettings.announcement.info"),
                    t("instancesettings.announcement.infoHint"),
                    "megaphone",
                ),
                Opt::new(
                    Tone::Warning as i32,
                    t("instancesettings.announcement.warning"),
                    t("instancesettings.announcement.warningHint"),
                    "triangle-alert",
                ),
                Opt::new(
                    Tone::Critical as i32,
                    t("instancesettings.announcement.critical"),
                    t("instancesettings.announcement.criticalHint"),
                    "siren",
                ),
            ],
            p,
            window,
            cx,
            |this, value, _, cx| {
                this.announce.tone = Tone::try_from(value).unwrap_or(Tone::Info);
                cx.notify();
            },
        );

        let mut lengths: Vec<(String, Ends)> = Vec::new();
        if let Some(at) = live.as_ref().and_then(manage::ends_ms) {
            lengths.push((
                t_with("instancesettings.announcement.until", &[("time", Arg::Str(&manage::ends_label(at, now)))]),
                Ends::Keep,
            ));
        }
        // `LENGTHS` in order: until taken down, an hour, four hours, a day, a week.
        const LENGTH_KEYS: [&str; 5] = [
            "instancesettings.announcement.untilDown",
            "instancesettings.announcement.hour",
            "instancesettings.announcement.fourHours",
            "instancesettings.announcement.day",
            "instancesettings.announcement.week",
        ];
        lengths
            .extend(LENGTHS.iter().zip(LENGTH_KEYS).map(|((_, ms), key)| (t(key), ms.map_or(Ends::Never, Ends::For))));
        let mut chips = div().flex().flex_wrap().gap(px(6.0));
        for (n, (label, value)) in lengths.into_iter().enumerate() {
            chips = chips.child(
                choice_chip(SharedString::from(format!("announcement-ends-{n}")), &label, ends == value, p).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.announce.ends = value;
                        cx.notify();
                    }),
                ),
            );
        }
        let comes_down = div().flex().flex_col().gap(px(8.0)).child(chips).when_some(
            ends_at.filter(|_| ends != Ends::Keep),
            |el, at| {
                el.child(motion::rise(
                    div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(
                        crate::ui::text::hint_line(
                            &t_with("instancesettings.announcement.comesDownAt", &[("time", Arg::Str("{time}"))]),
                            &[("time", manage::stamp_label(at, now).as_str())],
                            p,
                        ),
                    ),
                    SharedString::from(format!("announcement-ends-at-{ends:?}")),
                    Duration::ZERO,
                    4.0,
                ))
            },
        );

        let can_send = !trimmed.is_empty() && changed && !self.announce.busy;
        let shout = self.announce.shouts;
        let megaphone = motion::once(
            div().child(icon("megaphone").size(px(16.0))),
            SharedString::from(format!("announcement-shout-{shout}")),
            Duration::from_millis(500),
            move |el, t| {
                if shout == 0 {
                    return el;
                }
                // A shout: it swings and settles.
                let swing = (t * 3.0 * std::f32::consts::PI).sin() * (1.0 - t);
                el.relative().left(px(-3.0 * swing)).top(px(-2.0 * swing.abs()))
            },
        );
        // The icon goes before the words, so the button's own label stays empty.
        let send = primary_button("announcement-send", "", p)
            .child(if self.announce.busy { spinner("announcement-busy", 16.0, window) } else { megaphone })
            .child(t(if live.is_some() {
                "instancesettings.announcement.update"
            } else {
                "instancesettings.announcement.putUp"
            }))
            .when(!can_send, |el| el.opacity(0.5).cursor_default())
            .when(can_send, |el| el.on_click(cx.listener(|this, _, window, cx| this.put_up(false, window, cx))));
        let destructive = p.destructive;
        let actions = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .pt(px(18.0))
            .when(live.is_some(), |el| {
                el.child(motion::slide_in(
                    div()
                        .id("announcement-down")
                        .h(px(40.0))
                        .px(px(14.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(corner(12.0))
                        .text_color(destructive)
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .hover(move |s| s.bg(alpha(destructive, 0.1)))
                        .active(|s| s.top(px(1.0)))
                        .on_click(cx.listener(|this, _, window, cx| this.put_up(true, window, cx)))
                        .child(icon("megaphone-off").size(px(16.0)))
                        .child(t("instancesettings.announcement.takeDown")),
                    "announcement-down-in",
                    10.0,
                ))
            })
            .child(div().flex_1())
            .child(send);

        div()
            .flex()
            .flex_col()
            .child(self.setting(
                "announcement-preview",
                &t("settings.controls.preview"),
                Some(&t("instancesettings.announcement.previewHint")),
                &[],
                "",
                0,
                preview,
                p,
                cx,
            ))
            .child(self.setting(
                "announcement-text",
                &t("instancesettings.announcement.message"),
                Some(&t("instancesettings.announcement.messageHint")),
                &[],
                "",
                1,
                message,
                p,
                cx,
            ))
            .child(self.setting(
                "announcement-tone",
                &t("instancesettings.announcement.tone"),
                None,
                &[],
                "",
                2,
                tones,
                p,
                cx,
            ))
            .child(self.setting(
                "announcement-ends",
                &t("instancesettings.announcement.comesDown"),
                Some(&t("instancesettings.announcement.comesDownHint")),
                &[],
                "",
                3,
                comes_down,
                p,
                cx,
            ))
            .when_some(error_line(self.announce.error.as_deref(), p), |el, e| el.child(div().pt(px(12.0)).child(e)))
            .child(actions)
            .into_any_element()
    }
}

/// How much of the limit is used, as a ring (the web's 24-unit SVG, r=9, a 2.5 stroke) filling
/// clockwise from the top.
fn length_ring(share: f32, track: gpui_kit::Hsla, color: gpui_kit::Hsla) -> AnyElement {
    use gpui_kit::{PathBuilder, canvas, point};
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let side = f32::from(bounds.size.width).min(f32::from(bounds.size.height));
            let scale = side / 24.0;
            let (cx, cy) = (
                f32::from(bounds.origin.x) + f32::from(bounds.size.width) / 2.0,
                f32::from(bounds.origin.y) + f32::from(bounds.size.height) / 2.0,
            );
            let r = 9.0 * scale;
            let arc = |sweep: f32| {
                let steps = ((sweep.abs() * 64.0).ceil() as usize).max(2);
                let mut path = PathBuilder::stroke(px(2.5 * scale));
                for n in 0..=steps {
                    let a = (sweep * n as f32 / steps as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                    let at = point(px(cx + r * a.cos()), px(cy + r * a.sin()));
                    if n == 0 { path.move_to(at) } else { path.line_to(at) }
                }
                path.build().ok()
            };
            if let Some(path) = arc(1.0) {
                window.paint_path(path, track);
            }
            let share = share.clamp(0.0, 1.0);
            if share > 0.001
                && let Some(path) = arc(share)
            {
                window.paint_path(path, color);
            }
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}
