//! A server's onboarding for its new members (docs/servers.md): a step at a
//! time under the server's banner, picking what they're into, agreeing to
//! the rules and saying hello, then where to go first. Like the web app's
//! `join/Onboarding.tsx`.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};

use crate::core::onboarding::{self, GoHere, PICK, RULES, SAY_HELLO};
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp};
use crate::ui::banner::{accent, on_accent};
use crate::ui::motion;
use crate::ui::overlay::{emoji_tile, section_title};
use crate::ui::text::{TIGHT, tracked};
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{icon, pal};

/// The web's wide dialog (`max-w-2xl`).
const WIDTH: f32 = 672.0;

/// One run through a server's onboarding.
pub struct Flow {
    pub key: String,
    pub server: String,
    loading: bool,
    error: Option<String>,
    steps: Vec<pb::OnboardingStep>,
    welcome: Option<pb::WelcomeScreen>,
    rules: Vec<String>,
    /// Which step is shown; `steps.len()` is the end.
    at: usize,
    /// Which way the last move went, for the slide: 1 forward, -1 back.
    came: f32,
    picked: Vec<String>,
    agreed: bool,
    /// What each hello step's box says, kept while going back and forth.
    hellos: HashMap<String, String>,
    busy: bool,
    /// Bumped on each refusal, so the buttons shake again.
    shakes: u32,
}

pub struct Onboarding {
    pub flow: Option<Flow>,
    pub hello: Entity<TextareaState>,
    /// Words for the hello box, set on the next draw (it needs the window).
    pending_hello: Option<String>,
}

impl Onboarding {
    pub fn new(window: &mut Window, cx: &mut Context<FuwaApp>) -> (Self, Vec<Subscription>) {
        let hello = cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 6));
        let subs = vec![cx.subscribe_in(&hello, window, |this: &mut FuwaApp, _, event: &InputEvent, _, cx| {
            if let InputEvent::Change = event {
                let text = this.onboarding.hello.read(cx).value().to_string();
                if let Some(flow) = this.onboarding.flow.as_mut()
                    && let Some(step) = flow.steps.get(flow.at)
                {
                    flow.hellos.insert(step.id.clone(), text);
                }
                cx.notify();
            }
        })];
        (Self { flow: None, hello, pending_hello: None }, subs)
    }
}

impl FuwaApp {
    /// Opens a server's onboarding and reads what it needs: its steps, the
    /// rules for someone who still has to agree, and the welcome screen's channels.
    pub(crate) fn open_onboarding(&mut self, key: &str, server_id: &str, cx: &mut Context<Self>) {
        let Some((must_agree, has_rules, has_welcome)) = self.core.shared.read(|s| {
            let i = s.instance(key)?;
            let server = i.server(server_id)?;
            Some((i.access(server_id).pending && server.has_rules, server.has_rules, server.has_welcome_screen))
        }) else {
            return;
        };
        self.dialog = Some(Dialog::Onboarding { key: key.to_owned(), server: server_id.to_owned() });
        self.onboarding.flow = Some(Flow {
            key: key.to_owned(),
            server: server_id.to_owned(),
            loading: true,
            error: None,
            steps: Vec::new(),
            welcome: None,
            rules: Vec::new(),
            at: 0,
            came: 1.0,
            picked: Vec::new(),
            agreed: false,
            hellos: HashMap::new(),
            busy: false,
            shakes: 0,
        });
        let (core, k, sid) = (self.core.clone(), key.to_owned(), server_id.to_owned());
        let load = async move {
            let onboarding = core.onboarding(&k, &sid).await?;
            let rules = if must_agree && has_rules {
                core.server_rules(&k, &sid).await.unwrap_or_default()
            } else {
                Vec::new()
            };
            let welcome = if has_welcome { core.welcome_screen(&k, &sid).await.ok() } else { None };
            Ok::<_, crate::core::api::Problem>((onboarding, rules, welcome))
        };
        let (k, sid) = (key.to_owned(), server_id.to_owned());
        self.run(cx, load, move |this, result, cx| {
            let visible = this.core.shared.read(|s| {
                s.instance(&k)
                    .map(|i| {
                        let access = i.access(&sid);
                        i.channels
                            .get(&sid)
                            .into_iter()
                            .flatten()
                            .filter(|c| access.has_in(&c.id, pb::Permission::ViewChannels))
                            .map(|c| c.id.clone())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            });
            let Some(flow) = this.onboarding.flow.as_mut().filter(|f| f.key == k && f.server == sid) else { return };
            flow.loading = false;
            match result {
                Ok((onboarding, rules, welcome)) => {
                    flow.steps = onboarding::steps_for(&onboarding, must_agree, |c| visible.iter().any(|v| v == c));
                    flow.rules = rules;
                    flow.welcome = welcome;
                }
                Err(err) => flow.error = Some(capitalized(&err.message)),
            }
            this.enter_step(cx);
            cx.notify();
        });
        cx.notify();
    }

    /// Fills the hello box for the step now shown.
    fn enter_step(&mut self, cx: &mut Context<Self>) {
        let Some(flow) = self.onboarding.flow.as_ref() else { return };
        let Some(step) = flow.steps.get(flow.at).filter(|s| s.kind == SAY_HELLO) else { return };
        let text = flow.hellos.get(&step.id).cloned().unwrap_or_else(|| {
            if step.hello.trim().is_empty() { onboarding::HELLO.to_owned() } else { step.hello.clone() }
        });
        self.onboarding.pending_hello = Some(text);
        cx.notify();
    }

    fn refuse(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some(flow) = self.onboarding.flow.as_mut() {
            flow.error = Some(text.to_owned());
            flow.shakes += 1;
        }
        cx.notify();
    }

    fn step_back(&mut self, cx: &mut Context<Self>) {
        if let Some(flow) = self.onboarding.flow.as_mut()
            && flow.at > 0
            && flow.at < flow.steps.len()
            && !flow.busy
        {
            flow.at -= 1;
            flow.came = -1.0;
            flow.error = None;
        }
        self.enter_step(cx);
    }

    /// Goes on from the step shown: after the last one, the choices are sent.
    fn step_on(&mut self, cx: &mut Context<Self>) {
        let Some(flow) = self.onboarding.flow.as_mut() else { return };
        flow.error = None;
        flow.came = 1.0;
        if flow.at + 1 < flow.steps.len() {
            flow.at += 1;
            self.enter_step(cx);
            return;
        }
        flow.busy = true;
        let (core, key, server, picked) =
            (self.core.clone(), flow.key.clone(), flow.server.clone(), flow.picked.clone());
        self.run(cx, async move { core.finish_onboarding(&key, &server, picked).await }, move |this, result, cx| {
            let Some(flow) = this.onboarding.flow.as_mut() else { return };
            flow.busy = false;
            match result {
                Ok(()) => flow.at = flow.steps.len(),
                Err(err) => {
                    flow.error = Some(capitalized(&err.message));
                    flow.shakes += 1;
                }
            }
            cx.notify();
        });
        cx.notify();
    }

    /// The primary button: Next, Agree or Send, each checking first.
    pub(crate) fn step_do(&mut self, skip: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(flow) = self.onboarding.flow.as_ref() else { return };
        if flow.busy || flow.loading {
            return;
        }
        let Some(step) = flow.steps.get(flow.at).cloned() else {
            // The end: off to look around.
            self.finish_onboarding_dialog(window, cx);
            return;
        };
        if skip {
            self.step_on(cx);
            return;
        }
        match step.kind {
            PICK if !step.skippable && !onboarding::has_pick(&flow.picked, &step) => {
                self.refuse("Pick at least one to go on.", cx)
            }
            RULES if !flow.agreed => self.refuse("Agree to the rules to go on.", cx),
            RULES => {
                let (core, key, server) = (self.core.clone(), flow.key.clone(), flow.server.clone());
                self.set_busy(true);
                self.run(cx, async move { core.agree_to_rules(&key, &server).await }, |this, result, cx| {
                    this.set_busy(false);
                    match result {
                        Ok(()) => this.step_on(cx),
                        Err(err) => this.refuse(&capitalized(&err.message), cx),
                    }
                });
            }
            SAY_HELLO => {
                let text = self.onboarding.hello.read(cx).value().trim().to_owned();
                if text.is_empty() {
                    self.step_on(cx);
                    return;
                }
                let (core, key, server, channel) =
                    (self.core.clone(), flow.key.clone(), flow.server.clone(), step.channel_id.clone());
                self.set_busy(true);
                self.run(
                    cx,
                    async move { core.send_message(&key, &server, &channel, &text).await },
                    |this, result, cx| {
                        this.set_busy(false);
                        match result {
                            Ok(()) => this.step_on(cx),
                            Err(err) => this.refuse(&capitalized(&err.message), cx),
                        }
                    },
                );
            }
            _ => self.step_on(cx),
        }
    }

    fn set_busy(&mut self, busy: bool) {
        if let Some(flow) = self.onboarding.flow.as_mut() {
            flow.busy = busy;
        }
    }

    /// Closes it; it doesn't come back by itself on this computer.
    pub(crate) fn finish_onboarding_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(flow) = self.onboarding.flow.take() {
            let seen = format!("{}/{}", flow.key, flow.server);
            self.core.set_prefs(|p| {
                p.welcomed.insert(seen);
            });
            self.prefs = self.core.prefs();
        }
        if matches!(self.dialog, Some(Dialog::Onboarding { .. })) {
            self.dialog = None;
        }
        cx.notify();
    }

    pub(crate) fn render_onboarding(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        // The hello box takes its words once a hello step shows.
        if let Some(text) = self.onboarding.pending_hello.take() {
            self.onboarding.hello.update(cx, |s, cx| s.set_value(text, window, cx));
        }
        let flow = self.onboarding.flow.as_ref()?;
        let p = pal(cx);
        let (key, sid) = (flow.key.clone(), flow.server.clone());
        let (server, roles, channels, look) = self.core.shared.read(|s| {
            let i = s.instance(&key)?;
            Some((
                i.server(&sid)?.clone(),
                i.roles.get(&sid).cloned().unwrap_or_default(),
                i.channels.get(&sid).cloned().unwrap_or_default(),
                crate::ui::mentions::Look::of(i, &sid),
            ))
        })?;
        let tint = accent(&server);
        let done = !flow.loading && flow.at >= flow.steps.len();
        let total = flow.steps.len();
        let eyebrow = if flow.loading {
            String::new()
        } else if done {
            "You're all set in".to_owned()
        } else {
            format!("Step {} of {}", flow.at + 1, total)
        };
        let mut body = div().flex().flex_col().px(px(24.0));
        // The web's `Dots`: `mt-3 gap-1.5`, each `h-1.5 w-1.5` on the muted color, the current
        // one `w-7` in the accent gliding along, the ones done the accent at 55%.
        if !flow.loading && !done && total > 1 {
            let at = flow.at;
            let lead = motion::follow("onb-dot", at as f32 * 12.0, window, cx);
            let mut dots = div().relative().mt(px(12.0)).flex().items_center().gap(px(6.0)).h(px(6.0));
            for n in 0..total {
                dots =
                    dots.child(div().w(px(if n == at { 28.0 } else { 6.0 })).h(px(6.0)).rounded_full().bg(if n < at {
                        Hsla { a: 0.55, ..tint }
                    } else {
                        p.muted.into()
                    }));
            }
            dots = dots.child(div().absolute().top_0().left(px(lead)).w(px(28.0)).h(px(6.0)).rounded_full().bg(tint));
            body = body.child(dots);
        }

        if flow.loading {
            body = body.child(crate::ui::search::skeleton(4, &p));
        } else if done {
            body = body.child(self.onboarding_done(&server, &channels, &look, tint, &p, cx));
        } else if let Some(step) = flow.steps.get(flow.at).cloned() {
            // The step's title and words, then what it asks, sliding in from the way it came.
            // `flex flex-col gap-3 pt-3 pb-1`: the title (`text-lg font-extrabold tracking-tight`) and its words together.
            let mut content = div().flex().flex_col().gap(px(12.0)).pt(px(12.0)).pb(px(4.0)).child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_lg()
                            .line_height(px(28.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(tracked(step.title.clone(), TIGHT).wraps()),
                    )
                    .when(!step.description.trim().is_empty(), |el| {
                        el.child(div().text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(
                            crate::ui::text::markdown(
                                SharedString::from(format!("onb-desc-{}", step.id)),
                                step.description.clone(),
                            ),
                        ))
                    }),
            );
            content = match step.kind {
                PICK => content.child(self.pick_cards(&step, &roles, &look, tint, &p, cx)),
                RULES => content.child(self.rules_box(&p, cx)),
                _ => {
                    let name = channels
                        .iter()
                        .find(|c| c.id == step.channel_id)
                        .map(|c| format!("#{}", c.name))
                        .unwrap_or_else(|| "#a channel".into());
                    content.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .text_xs()
                                    .line_height(px(16.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(p.muted_foreground)
                                    .child(icon("message-circle-heart").size(px(16.0)).text_color(tint))
                                    .child(format!("Goes to {name}")),
                            )
                            .child(div().relative().child(Textarea::new(&self.onboarding.hello)).child(
                                motion::ambient(
                                    div().absolute().top(px(-12.0)).right(px(-6.0)).text_size(px(22.0)).child("👋"),
                                    "onb-wave",
                                    Duration::from_secs(4),
                                    window,
                                    |el, t| {
                                        // A wiggle in the first fifth of every four seconds.
                                        let w =
                                            if t < 0.2 { (t / 0.2 * std::f32::consts::TAU * 2.0).sin() } else { 0.0 };
                                        el.mt(px(-3.0 * w.abs())).ml(px(3.0 * w))
                                    },
                                ),
                            )),
                    )
                }
            };
            body = body.child(motion::slide_in(
                content,
                SharedString::from(format!("onb-step-{}", flow.at)),
                48.0 * flow.came,
            ));
        } else if let Some(error) = &flow.error {
            body = body.child(div().text_sm().text_color(p.destructive).child(error.clone()));
        }

        // The error and the buttons (`mt-4`, the dialog's `pb-5`), under the part that scrolls.
        let mut foot = div().flex().flex_col().gap(px(12.0)).px(px(24.0)).pt(px(16.0)).pb(px(20.0));
        if let Some(error) = flow.error.clone().filter(|_| !flow.loading && flow.at < flow.steps.len()) {
            foot = foot.child(motion::rise(
                div().text_sm().font_weight(FontWeight::BOLD).text_color(p.destructive).child(error.clone()),
                SharedString::from(format!("onb-error-{}", flow.shakes)),
                Duration::ZERO,
                4.0,
            ));
        }
        if !flow.loading {
            foot = foot.child(self.onboarding_buttons(tint, &p, window, cx));
        }

        // The banner and the buttons always show; the step scrolls in what's left.
        let room = (f32::from(window.viewport_size().height) - 460.0).max(160.0);
        // The dialog frame every web dialog has (`rounded-3xl border bg-card shadow-2xl`).
        let hero = crate::ui::join::banner_hero_wide(
            &server,
            &eyebrow,
            Some(icon("sparkles").size(px(14.0)).into_any_element()),
            WIDTH - 2.0,
            &p,
            window,
            cx,
        );
        let panel = div()
            .w(px(WIDTH))
            .rounded(crate::ui::theme::radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .text_color(p.foreground)
            .shadow(crate::ui::overlay::shadow_2xl())
            .child(hero)
            .child(div().id("onb-body").max_h(px(room)).overflow_y_scroll().child(body))
            .child(foot);
        Some(crate::ui::overlay::dialog_layer("onboarding", panel, &p, |_, _, _| {}))
    }

    fn pick_cards(
        &self,
        step: &pb::OnboardingStep,
        roles: &[pb::Role],
        look: &crate::ui::mentions::Look,
        tint: Hsla,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let picked = self.onboarding.flow.as_ref().map(|f| f.picked.clone()).unwrap_or_default();
        let mut grid = div().flex().flex_wrap().gap(px(8.0));
        for (n, option) in step.options.iter().enumerate() {
            let on = picked.contains(&option.id);
            let (step_c, oid) = (step.clone(), option.id.clone());
            let chips: Vec<_> = option
                .role_ids
                .iter()
                .filter_map(|id| roles.iter().find(|r| r.id == *id))
                .map(|r| {
                    let color: Hsla =
                        r.color.map(|c| gpui_kit::rgb(c as u32).into()).unwrap_or(p.muted_foreground.into());
                    div()
                        .px(px(6.0))
                        .py(px(1.0))
                        .rounded_full()
                        .bg(p.muted)
                        .text_color(color)
                        .text_size(px(10.4))
                        .line_height(px(14.0))
                        .font_weight(FontWeight::BOLD)
                        .child(format!("@{}", r.name))
                })
                .collect();
            // `size-6 border-2`, `rounded-lg` for several, round for one.
            let mark = div()
                .size(px(24.0))
                .flex_none()
                .border_2()
                .border_color(if on { tint } else { alpha(p.muted_foreground, 0.3) })
                .when(step.multiple, |el| el.rounded(crate::ui::theme::radius_lg()))
                .when(!step.multiple, |el| el.rounded_full())
                .when(on, |el| el.bg(tint))
                .flex()
                .items_center()
                .justify_center()
                .when(on, |el| {
                    // The tick springs in from nothing, turning upright.
                    el.child(motion::pop(
                        icon("check").size(px(14.0)).text_color(gpui_kit::white()),
                        SharedString::from(format!("onb-tick-{}", option.id)),
                        0.0,
                        -45.0,
                        Duration::ZERO,
                    ))
                });
            grid = grid.child(motion::rise(
                div()
                    .id(SharedString::from(format!("onb-opt-{}", option.id)))
                    .w(px((WIDTH - 48.0 - 8.0) / 2.0 - 2.0))
                    // `flex items-center gap-3 rounded-2xl border p-3`, on the page at 60% until picked.
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .p(px(12.0))
                    .rounded(crate::ui::theme::radius_2xl())
                    .border_1()
                    .border_color(if on { tint } else { p.border.into() })
                    .bg(if on { Hsla { a: 0.12, ..tint } } else { alpha(p.background, 0.6) })
                    .cursor_pointer()
                    .when(!on, |el| el.hover(move |s| s.border_color(Hsla { a: 0.45, ..tint })))
                    // `whileTap={{ scale: 0.97 }}`.
                    .active(|s| s.scale(0.97))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(flow) = this.onboarding.flow.as_mut() {
                            onboarding::toggle(&mut flow.picked, &step_c, &oid);
                            flow.error = None;
                        }
                        cx.notify();
                    }))
                    .child(option_tile(&option.emoji, look, p))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .text_sm()
                                    .line_height(px(20.0))
                                    .truncate()
                                    .child(option.label.clone()),
                            )
                            .when(!option.description.is_empty(), |el| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .line_height(px(16.0))
                                        .text_color(p.muted_foreground)
                                        .child(option.description.clone()),
                                )
                            })
                            .when(!chips.is_empty(), |el| {
                                el.child(div().mt(px(4.0)).flex().flex_wrap().gap(px(4.0)).children(chips))
                            }),
                    )
                    .child(mark),
                SharedString::from(format!("onb-opt-in-{}", option.id)),
                Duration::from_millis(40 * n.min(8) as u64),
                8.0,
            ));
        }
        grid.into_any_element()
    }

    /// The rules step: the numbered rules (`RulesList`, `max-h-60`) and "I agree" (`AgreeCheck`).
    fn rules_box(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(flow) = self.onboarding.flow.as_ref() else { return div().into_any_element() };
        let list = if flow.rules.is_empty() {
            div()
                .text_sm()
                .text_color(p.muted_foreground)
                .child(crate::core::i18n::t("join.rules.none"))
                .into_any_element()
        } else {
            crate::ui::dialogs::rules_list(&flow.rules, 240.0, p).into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(list)
            .child(crate::ui::dialogs::agree_check("onb-agree", flow.agreed, p).on_click(cx.listener(
                |this, _, _, cx| {
                    if let Some(flow) = this.onboarding.flow.as_mut() {
                        flow.agreed = !flow.agreed;
                        flow.error = None;
                    }
                    cx.notify();
                },
            )))
            .into_any_element()
    }

    fn onboarding_done(
        &self,
        server: &pb::Server,
        channels: &[pb::Channel],
        look: &crate::ui::mentions::Look,
        tint: Hsla,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(flow) = self.onboarding.flow.as_ref() else { return div().into_any_element() };
        let go: Vec<GoHere> = onboarding::go_here_first(&flow.steps, &flow.picked, flow.welcome.as_ref(), |c| {
            channels.iter().any(|ch| ch.id == c)
        });
        let mut out = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(10.0))
            .child(motion::once(
                div()
                    .size(px(64.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(Hsla { a: 0.16, ..tint })
                    .text_size(px(30.0))
                    .child("🎉"),
                "onb-done-pop",
                Duration::from_millis(520),
                |el, t| {
                    let s = 1.0 - (1.0 - t).powi(3) * (1.0 + 2.6 * t);
                    el.size(px(64.0 * (0.4 + 0.6 * s)))
                },
            ))
            .child(
                div()
                    .flex()
                    .gap(px(4.0))
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child("That's everything. Have fun in")
                    .child(
                        div().font_weight(FontWeight::BOLD).text_color(p.foreground).child(format!("{}!", server.name)),
                    ),
            );
        if !go.is_empty() {
            let mut list = div().w_full().flex().flex_col().gap(px(8.0)).child(section_title("GO HERE FIRST", p));
            for (n, g) in go.into_iter().enumerate() {
                let Some(channel) = channels.iter().find(|c| c.id == g.channel_id) else { continue };
                let (k, sid, cid) = (flow.key.clone(), flow.server.clone(), channel.id.clone());
                let announcement = channel.r#type == pb::ChannelType::Announcement as i32;
                let group = SharedString::from(format!("onb-go-{cid}"));
                list = list.child(motion::rise(
                    div()
                        .id(group.clone())
                        .group(group.clone())
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .p(px(10.0))
                        .rounded(corner(14.0))
                        .border_1()
                        .border_color(p.border)
                        .bg(p.secondary)
                        .cursor_pointer()
                        // `whileHover={{ y: -3 }} whileTap={{ scale: 0.97 }}`.
                        .hover(move |s| {
                            s.border_color(Hsla { a: 0.55, ..tint }).bg(Hsla { a: 0.08, ..tint }).translate_y(px(-3.0))
                        })
                        .active(|s| s.scale(0.97))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.finish_onboarding_dialog(window, cx);
                            this.open_channel(&k, &sid, &cid, window, cx);
                        }))
                        // Its tile grows and tips, and the arrow leans on, as it's hovered.
                        .child(
                            div()
                                .id(SharedString::from(format!("{group}|tile")))
                                .group_hover(group.clone(), |s| {
                                    s.scale(1.1).rotate(gpui_kit::radians(-6f32.to_radians()))
                                })
                                .child(emoji_tile(
                                    &g.emoji,
                                    look,
                                    tint,
                                    if announcement { "megaphone" } else { "hash" },
                                )),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(
                                    div().font_weight(FontWeight::BOLD).text_sm().child(format!("#{}", channel.name)),
                                )
                                .when(!g.note.is_empty(), |el| {
                                    el.child(div().text_xs().text_color(p.muted_foreground).child(g.note.clone()))
                                }),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("{group}|arrow")))
                                .text_color(p.muted_foreground)
                                .group_hover(group.clone(), move |s| s.translate_x(px(4.0)).text_color(tint))
                                .child(icon("arrow-right").size(px(16.0))),
                        ),
                    SharedString::from(format!("onb-go-in-{n}")),
                    Duration::from_millis(220 + 60 * n as u64),
                    14.0,
                ));
            }
            out = out.child(list);
        }
        out.into_any_element()
    }

    fn onboarding_buttons(&self, tint: Hsla, p: &Palette, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(flow) = self.onboarding.flow.as_ref() else { return div().into_any_element() };
        let step = flow.steps.get(flow.at);
        let last = flow.at + 1 == flow.steps.len();
        let (glyph, label) = match step.map(|s| s.kind) {
            None => ("party-popper", "Start exploring"),
            Some(RULES) => ("check", "Agree"),
            Some(SAY_HELLO) => ("send", "Send"),
            Some(_) if last => ("arrow-right", "Finish"),
            Some(_) => ("arrow-right", "Next"),
        };
        let faint = step.is_some_and(|s| s.kind == PICK && !s.skippable && !onboarding::has_pick(&flow.picked, s));
        let back = flow.at > 0 && step.is_some();
        let skip = step.is_some_and(|s| s.skippable && s.kind != RULES);
        let busy = flow.busy;
        // The spinner turns while it works; otherwise the arrow leans on (the plane
        // flies off up and right) as the button's hovered.
        let glyph: AnyElement = if busy {
            motion::ambient(
                icon("loader-circle").size(px(16.0)),
                "onb-spin",
                Duration::from_millis(1000),
                window,
                |el, t| el.rotate(gpui_kit::radians(t * std::f32::consts::TAU)),
            )
        } else {
            let send = glyph == "send";
            div()
                .id("onb-go-glyph")
                .when(glyph == "arrow-right" || send, |el| {
                    el.group_hover("onb-go", move |s| {
                        let s = s.translate_x(px(2.0));
                        if send { s.translate_y(px(-2.0)) } else { s }
                    })
                })
                .child(icon(glyph).size(px(16.0)))
                .into_any_element()
        };
        let primary = div()
            .id("onb-go")
            .group("onb-go")
            // `h-10 min-w-32 rounded-xl font-bold text-white shadow-md`.
            .h(px(40.0))
            .min_w(px(128.0))
            .px(px(16.0))
            .rounded(crate::ui::theme::radius_xl())
            .flex()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .text_sm()
            .shadow_md()
            .bg(tint)
            .text_color(on_accent(tint))
            .font_weight(FontWeight::BOLD)
            .cursor_pointer()
            .when(faint || busy, |el| el.opacity(0.7))
            .hover(move |s| s.bg(Hsla { l: (tint.l * 1.08).min(0.95), ..tint }))
            .on_click(cx.listener(|this, _, window, cx| this.step_do(false, window, cx)))
            .child(motion::slide_in(div().child(label), SharedString::from(format!("onb-label-{label}")), 8.0))
            .child(glyph);
        let row = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .when(back, |el| {
                // A ghost `size-10 rounded-xl` button, popping in from the second step on.
                let hover = p.accent;
                el.child(motion::pop_in(
                    div()
                        .id("onb-back")
                        .group("onb-back")
                        .size(px(40.0))
                        .flex_none()
                        .rounded(crate::ui::theme::radius_xl())
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .on_click(cx.listener(|this, _, _, cx| this.step_back(cx)))
                        .child(
                            div()
                                .id("onb-back-arrow")
                                .group_hover("onb-back", |s| s.translate_x(px(-2.0)))
                                .child(icon("arrow-left").size(px(16.0))),
                        ),
                    "onb-back-in",
                    (0.5, 0.5),
                    0.8,
                    0.0,
                ))
            })
            .child(div().flex_1())
            .when(skip, |el| {
                el.child(
                    div()
                        .id("onb-skip")
                        .px(px(14.0))
                        .h(px(40.0))
                        .flex()
                        .items_center()
                        .rounded(corner(12.0))
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .cursor_pointer()
                        .hover(|s| s.bg(p.secondary))
                        .on_click(cx.listener(|this, _, window, cx| this.step_do(true, window, cx)))
                        .child("Skip"),
                )
            })
            .child(primary);
        // A refusal gives the row a little shake.
        if flow.error.is_some() && flow.shakes > 0 {
            motion::once(
                row,
                SharedString::from(format!("onb-shake-{}", flow.shakes)),
                Duration::from_millis(400),
                |el, t| el.ml(px((t * std::f32::consts::TAU * 3.0).sin() * 8.0 * (1.0 - t))),
            )
        } else {
            row.into_any_element()
        }
    }
}

/// The server's words with a capital letter first.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => "That didn't go through".into(),
    }
}

/// An option's emoji on its tile (`size-10 rounded-xl bg-muted/70 text-xl`), or a sparkle without one.
fn option_tile(emoji: &str, look: &crate::ui::mentions::Look, p: &Palette) -> AnyElement {
    let base = div()
        .size(px(40.0))
        .flex_none()
        .rounded(crate::ui::theme::radius_xl())
        .flex()
        .items_center()
        .justify_center()
        .bg(alpha(p.muted, 0.7));
    let own = emoji
        .strip_prefix('<')
        .and_then(|e| e.strip_suffix('>'))
        .and_then(|e| e.rsplit_once(':'))
        .and_then(|(_, id)| look.emojis.get(&id.to_uppercase()));
    match own {
        Some(url) => {
            use gpui_kit::StyledImage as _;
            base.child(
                gpui_kit::img(SharedString::from(url.clone())).size(px(24.0)).object_fit(gpui_kit::ObjectFit::Contain),
            )
            .into_any_element()
        }
        None if !emoji.is_empty() && !emoji.starts_with('<') => {
            base.text_xl().child(emoji.to_owned()).into_any_element()
        }
        None => base.child(icon("sparkles").size(px(16.0)).text_color(p.muted_foreground)).into_any_element(),
    }
}
