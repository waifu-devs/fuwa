//! The live preview at the top of Welcome & onboarding, the web's `Preview`
//! in `WelcomeAndOnboarding.tsx`: the welcome screen, applying or onboarding
//! as new members get them, over a sketch of the app at a desktop's or a
//! phone's width, switched with two segmented controls.

use super::*;
use crate::ui::theme::{radius_2xl, radius_3xl, radius_lg, radius_xl};

/// What the preview shows.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(super) enum View {
    #[default]
    Welcome,
    Apply,
    Onboarding,
}

/// The widths the preview draws at: a desktop's and a phone's, as the web's `DEVICES`.
const DESKTOP: (f32, f32) = (1280.0, 800.0);
const PHONE: (f32, f32) = (390.0, 844.0);

impl ServerSettingsView {
    /// Segmented choices with an icon each (the web's `Segments`: `h-8 px-3`, a highlight that glides).
    #[allow(clippy::too_many_arguments)]
    fn segments(
        &mut self,
        id: &'static str,
        options: Vec<(String, &'static str)>,
        chosen: usize,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
        pick: impl Fn(&mut Self, usize, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let widths: Vec<f32> =
            options.iter().map(|(l, _)| 24.0 + 16.0 + 6.0 + l.chars().count() as f32 * 7.6).collect();
        let x = motion::follow(SharedString::from(format!("{id}-x")), widths[..chosen].iter().sum::<f32>(), window, cx);
        let w = motion::follow(SharedString::from(format!("{id}-w")), widths[chosen], window, cx);
        let pick = std::rc::Rc::new(pick);
        let mut row = div().relative().flex().p(px(4.0)).rounded(radius_xl()).bg(p.muted).child(
            div()
                .absolute()
                .top(px(4.0))
                .bottom(px(4.0))
                .left(px(4.0 + x))
                .w(px(w))
                .rounded(radius_lg())
                .bg(p.background)
                .shadow(crate::ui::settings_controls::shadow_sm()),
        );
        for (n, (label, glyph)) in options.into_iter().enumerate() {
            let on = n == chosen;
            let fg = p.foreground;
            let pick = pick.clone();
            row = row.child(
                div()
                    .id(SharedString::from(format!("{id}-{n}")))
                    .relative()
                    .w(px(widths[n]))
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .whitespace_nowrap()
                    .text_color(if on { p.foreground } else { p.muted_foreground })
                    .cursor_pointer()
                    .hover(move |s| s.text_color(fg))
                    .on_click(cx.listener(move |this, _, _, cx| pick(this, n, cx)))
                    .child(icon(glyph).size(px(16.0)))
                    .child(label),
            );
        }
        row.into_any_element()
    }

    /// The preview: its two switches, then the frame with the app sketched in and the card over it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn welcome_stage(
        &mut self,
        server: &pb::Server,
        screen: &pb::WelcomeScreen,
        channels: &[pb::Channel],
        look: &crate::ui::mentions::Look,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.load_join_form(window, cx);
        let view = self.pages.stage_view;
        let phone = self.pages.stage_phone;
        let views = vec![
            (t("serversettings.welcome.viewWelcome"), "party-popper"),
            (t("serversettings.welcome.viewApplying"), "clipboard-pen"),
            (t("serversettings.nav.onboarding"), "sparkles"),
        ];
        let chosen = match view {
            View::Welcome => 0,
            View::Apply => 1,
            View::Onboarding => 2,
        };
        let view_tabs = self.segments("stage-view", views, chosen, p, window, cx, |this, n, cx| {
            this.pages.stage_view = [View::Welcome, View::Apply, View::Onboarding][n];
            cx.notify();
        });
        let devices = vec![("1280".to_owned(), "monitor"), ("390".to_owned(), "smartphone")];
        let device_tabs = self.segments("stage-device", devices, usize::from(phone), p, window, cx, |this, n, cx| {
            this.pages.stage_phone = n == 1;
            cx.notify();
        });

        let (fw, fh) = if phone { PHONE } else { DESKTOP };
        let scale = if phone { (self.column / fw).min(0.62) } else { (self.column / fw).min(1.0) };
        let (w, h) = (fw * scale, fh * scale);
        let card = self.stage_card(server, screen, channels, look, view, phone, scale, p, window, cx);
        let overlay = div()
            .absolute()
            .inset_0()
            // GPUI doesn't clip to rounded corners, so the shade rounds its own.
            .map(|el| if phone { el.rounded(px(31.0)) } else { el.rounded(radius_3xl()) })
            .bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.5))
            .flex()
            .map(|el| if phone { el.items_end() } else { el.items_center().justify_center().p(px(32.0 * scale)) })
            .child(motion::rise(
                div().child(card),
                SharedString::from(format!("stage-card-{view:?}-{phone}")),
                Duration::ZERO,
                24.0,
            ));
        let frame = div()
            .relative()
            .mx_auto()
            .w(px(w))
            .h(px(h))
            .overflow_hidden()
            .bg(p.background)
            .shadow(crate::ui::settings_controls::shadow_xl())
            .map(|el| {
                if phone {
                    el.rounded(px(35.0)).border_4().border_color(alpha(p.foreground, 0.8))
                } else {
                    el.rounded(radius_3xl()).border_1().border_color(p.border)
                }
            })
            .child(fake_app(server, phone, scale, p))
            .child(overlay);
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(view_tabs)
                    .child(device_tabs),
            )
            .child(frame)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn stage_card(
        &mut self,
        server: &pb::Server,
        screen: &pb::WelcomeScreen,
        channels: &[pb::Channel],
        look: &crate::ui::mentions::Look,
        view: View,
        phone: bool,
        scale: f32,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let width = if phone { PHONE.0 * scale } else { (672.0 * scale).max(320.0) };
        let body: AnyElement = match view {
            View::Welcome if screen.enabled || !screen.description.is_empty() || !screen.channels.is_empty() => {
                self.welcome_card(server, screen, channels, look, width, p, window, cx)
            }
            View::Welcome => empty(server, &t("serversettings.welcome.emptyWelcome"), width, p, window, cx),
            View::Apply => self.apply_card(server, width, p, window, cx),
            View::Onboarding => match self.onboarding_card(server, p, cx) {
                Some(card) => card,
                None => empty(server, &t("serversettings.welcome.emptyOnboarding"), width, p, window, cx),
            },
        };
        div()
            .id("stage-card")
            .w(px(width))
            .max_h(px(if phone { PHONE.1 * scale * 0.92 } else { DESKTOP.1 * scale * 0.92 }))
            .overflow_y_scroll()
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .shadow(crate::ui::settings_controls::shadow_xl())
            .map(|el| if phone { el.rounded_t(radius_3xl()) } else { el.rounded(radius_3xl()) })
            .child(body)
            .into_any_element()
    }

    /// Applying, as someone sees it: the banner, the rules and questions, and the button.
    fn apply_card(
        &mut self,
        server: &pb::Server,
        width: f32,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let form = self.pages.join.saved_form().unwrap_or_default();
        let tint = crate::ui::banner::accent(server);
        let hero = crate::ui::banner::banner_hero(server, &t("join.apply"), width, p, window, cx);
        let mut body = div().flex().flex_col().gap(px(16.0)).px(px(24.0)).pb(px(24.0)).child(
            div()
                .text_sm()
                .line_height(px(20.0))
                .text_color(p.muted_foreground)
                .child(t("join.applyDialog.description")),
        );
        let caps = |text: String| {
            div()
                .text_xs()
                .line_height(px(16.0))
                .font_weight(FontWeight::BOLD)
                .text_color(p.muted_foreground)
                .child(text.to_uppercase())
        };
        if !form.rules.is_empty() {
            let mut list = div().flex().flex_col().gap(px(8.0));
            for (n, rule) in form.rules.iter().take(4).enumerate() {
                list = list.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(10.0))
                        .text_sm()
                        .child(
                            div()
                                .size(px(24.0))
                                .flex_none()
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(alpha(p.primary, 0.15))
                                .text_color(p.primary)
                                .text_xs()
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .child((n + 1).to_string()),
                        )
                        .child(div().flex_1().min_w_0().child(rule.clone())),
                );
            }
            body =
                body.child(div().flex().flex_col().gap(px(8.0)).child(caps(t("join.applyDialog.rules"))).child(list));
        }
        if form.questions.is_empty() {
            body = body.child(
                div()
                    .rounded(radius_2xl())
                    .bg(alpha(p.muted, 0.5))
                    .p(px(12.0))
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(t("serversettings.welcome.noQuestions")),
            );
        } else {
            let mut list = div().flex().flex_col().gap(px(12.0)).child(caps(t("join.applyDialog.questions")));
            for q in &form.questions {
                list = list.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(
                            div()
                                .flex()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child(q.prompt.clone())
                                .when(q.required, |el| el.child(div().text_color(p.destructive).child("\u{a0}*"))),
                        )
                        .child(
                            div()
                                .h(px(if q.paragraph { 76.0 } else { 44.0 }))
                                .rounded(radius_xl())
                                .border_1()
                                .border_color(p.border),
                        ),
                );
            }
            body = body.child(list);
        }
        body = body.child(
            div()
                .h(px(44.0))
                .rounded(radius_xl())
                .flex()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .bg(tint)
                .text_color(gpui_kit::white())
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .child(icon("send").size(px(16.0)))
                .child(t("join.applyDialog.send")),
        );
        div().flex().flex_col().gap(px(16.0)).child(hero).child(body).into_any_element()
    }
}

/// Nothing to show yet: the banner, and a word about what would be here.
fn empty(
    server: &pb::Server,
    text: &str,
    width: f32,
    p: &Palette,
    window: &mut Window,
    cx: &mut Context<ServerSettingsView>,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(16.0))
        .pb(px(24.0))
        .child(crate::ui::banner::banner_hero(server, &t("settings.controls.preview"), width, p, window, cx))
        .child(
            div()
                .mx(px(24.0))
                .rounded(radius_2xl())
                .bg(alpha(p.muted, 0.5))
                .p(px(16.0))
                .text_center()
                .text_sm()
                .line_height(px(20.0))
                .text_color(p.muted_foreground)
                .child(text.to_owned()),
        )
        .into_any_element()
}

/// The app behind the preview's card, as shapes: rail, channels and a few message lines.
fn fake_app(server: &pb::Server, phone: bool, s: f32, p: &Palette) -> AnyElement {
    let shade: Hsla = p.muted.into();
    let bar = |w: f32, h: f32| div().w(px(w * s)).h(px(h * s)).rounded(px(4.0 * s)).bg(shade);
    let mut app = div().absolute().inset_0().flex().opacity(0.85);
    if !phone {
        let mut rail = div()
            .w(px(72.0 * s))
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.0 * s))
            .py(px(12.0 * s))
            .rounded_l(radius_3xl())
            .bg(alpha(p.muted, 0.6));
        for _ in 0..4 {
            rail = rail.child(div().size(px(48.0 * s)).rounded(px(16.0 * s)).bg(shade));
        }
        let mut side = div()
            .w(px(240.0 * s))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.0 * s))
            .p(px(12.0 * s))
            .bg(alpha(p.muted, 0.3));
        side = side.child(bar(128.0, 20.0).mb(px(8.0 * s)));
        for w in [40.0, 28.0, 34.0, 22.0, 30.0] {
            side = side.child(bar(w * 4.0, 14.0));
        }
        app = app.child(rail).child(side);
    }
    let tint = crate::ui::banner::accent(server);
    let mut main = div().flex_1().flex().flex_col().gap(px(16.0 * s)).p(px(24.0 * s)).child(bar(160.0, 20.0));
    for w in [0.7, 0.5, 0.85, 0.4, 0.6, 0.75] {
        main = main.child(
            div()
                .flex()
                .items_start()
                .gap(px(12.0 * s))
                .child(div().size(px(40.0 * s)).flex_none().rounded_full().bg(tint.opacity(0.35)))
                .child(div().flex_1().flex().flex_col().gap(px(6.0 * s)).child(bar(96.0, 12.0)).child(
                    div().w(gpui_kit::relative(w)).h(px(12.0 * s)).rounded(px(4.0 * s)).bg(alpha(p.muted, 0.7)),
                )),
        );
    }
    app.child(main).into_any_element()
}
