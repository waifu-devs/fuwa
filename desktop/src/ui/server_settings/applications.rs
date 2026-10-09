//! Applications, the web's `settings/server/Applications.tsx`: people asking
//! to join, oldest first, with their answers. Letting someone in sends their
//! card off to the right; turning them down (with a reason they'll see)
//! sends it off to the left.

use std::collections::{HashMap, HashSet};

use super::*;
use crate::core::server_pages::{NEW_ACCOUNT_MS, REASON_MAX};
use crate::ui::settings_controls::{Look, button};
use crate::ui::theme::{radius_2xl, radius_3xl, radius_xl};

#[derive(Default)]
pub(super) struct Applications {
    list: Option<Vec<pb::Application>>,
    loading: bool,
    /// Asked once each time settings open; events don't carry new applications here.
    asked: bool,
    error: Option<String>,
    /// Cards whose reason box is open, and the boxes.
    declining: HashSet<String>,
    reasons: HashMap<String, Entity<TextareaState>>,
    /// A card on its way: let in (true) or turned down, and since when.
    leaving: HashMap<String, (bool, Instant)>,
    busy: Option<String>,
}

impl Applications {
    /// How many are waiting, for the menu's badge.
    pub(super) fn waiting(&self) -> usize {
        self.list.as_ref().map_or(0, |l| l.len())
    }
}

/// How long a decided card takes to fly off.
const LEAVE: Duration = Duration::from_millis(350);

impl ServerSettingsView {
    /// Reads the applications (once, unless `again`).
    pub(super) fn load_applications(&mut self, again: bool, cx: &mut Context<Self>) {
        let a = &mut self.pages.applications;
        if a.loading || (a.asked && !again) {
            return;
        }
        a.asked = true;
        a.loading = true;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.list_applications(&key, &sid).await }, |this, result, cx| {
            let a = &mut this.pages.applications;
            a.loading = false;
            match result {
                Ok(list) => {
                    a.list = Some(list);
                    a.error = None;
                }
                Err(err) => a.error = Some(err.message),
            }
            cx.notify();
        });
    }

    fn decide(&mut self, user: pb::User, approve: bool, cx: &mut Context<Self>) {
        let reason = if approve {
            String::new()
        } else {
            self.pages
                .applications
                .reasons
                .get(&user.id)
                .map(|r| r.read(cx).value().trim().chars().take(REASON_MAX).collect())
                .unwrap_or_default()
        };
        let a = &mut self.pages.applications;
        a.busy = Some(user.id.clone());
        a.leaving.insert(user.id.clone(), (approve, Instant::now()));
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let uid = user.id.clone();
        self.run(
            cx,
            async move { core.review_application(&key, &sid, &uid, approve, &reason).await },
            move |this, result, cx| {
                let a = &mut this.pages.applications;
                a.busy = None;
                let name = user_name(&user);
                match result {
                    Ok(()) => {
                        let title = if approve {
                            t_with("serversettings.applications.letInDone", &[("name", Arg::Str(&name))])
                        } else {
                            t_with("serversettings.applications.turnedDownDone", &[("name", Arg::Str(&name))])
                        };
                        cx.emit(ServerSettingsEvent::Toast {
                            icon: if approve { "user-check" } else { "user-x" },
                            title,
                        });
                        // The card finishes flying, then leaves the list.
                        let id = user.id.clone();
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(LEAVE).await;
                            let _ = this.update(cx, |this, cx| {
                                let a = &mut this.pages.applications;
                                if let Some(list) = &mut a.list {
                                    list.retain(|x| x.user.as_ref().is_none_or(|u| u.id != id));
                                }
                                a.leaving.remove(&id);
                                a.declining.remove(&id);
                                a.reasons.remove(&id);
                                cx.notify();
                            });
                        })
                        .detach();
                    }
                    Err(err) => {
                        a.leaving.remove(&user.id);
                        cx.emit(ServerSettingsEvent::Toast { icon: "circle-alert", title: err.message });
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    pub(super) fn applications_page(
        &mut self,
        server: &pb::Server,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.load_applications(false, cx);
        let a = &self.pages.applications;
        let Some(list) = a.list.clone() else {
            if let Some(e) = &a.error {
                return super::pages::problem(e, p);
            }
            return super::pages::shimmers(2, 160.0, radius_3xl(), p, window);
        };
        if list.is_empty() {
            return motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(12.0))
                    .py(px(48.0))
                    .rounded(radius_3xl())
                    .border_1()
                    .border_dashed()
                    .border_color(p.border)
                    .child(motion::ambient(
                        div()
                            .size(px(56.0))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(alpha(p.primary, 0.15))
                            .text_color(p.primary)
                            .child(icon("inbox").size(px(28.0))),
                        "applications-float",
                        Duration::from_millis(3000),
                        window,
                        |el, t| el.relative().top(px(-3.0 * (t * std::f32::consts::TAU).sin())),
                    ))
                    .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("serversettings.applications.none")))
                    .child(
                        div()
                            .max_w(px(384.0))
                            .text_sm()
                            .line_height(px(20.0))
                            .text_center()
                            .text_color(p.muted_foreground)
                            .child(if server.applications {
                                t("serversettings.applications.noneHint")
                            } else {
                                t("serversettings.applications.noneHintOff")
                            }),
                    ),
                "applications-empty",
                Duration::ZERO,
                8.0,
            )
            .into_any_element();
        }
        // "3 people" in bold, the number rolling as applications come and go, then the rest of
        // the sentence.
        let intro = t_with("serversettings.applications.waiting", &[("people", Arg::Str("\u{E002}"))]);
        let (before, after) = intro.split_once('\u{E002}').unwrap_or((intro.as_str(), ""));
        let people = div().flex_none().font_weight(FontWeight::BOLD).text_color(p.foreground).child(motion::counted(
            "applications-count",
            "serversettings.applications.people",
            list.len() as u64,
            14.0,
        ));
        let intro = div()
            .flex()
            .text_sm()
            .line_height(px(20.0))
            .text_color(p.muted_foreground)
            .when(!before.is_empty(), |el| el.child(div().flex_none().child(before.to_owned())))
            .child(people)
            .child(div().flex_1().min_w_0().child(after.to_owned()));
        let mut col = div().flex().flex_col().gap(px(12.0)).child(intro);
        let now = now_ms();
        for (n, app) in list.iter().enumerate() {
            col = col.child(self.application_card(app, n, now, p, window, cx));
        }
        col.into_any_element()
    }

    fn application_card(
        &mut self,
        app: &pb::Application,
        n: usize,
        now: i64,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(user) = app.user.clone() else { return div().into_any_element() };
        let uid = user.id.clone();
        let made = crate::ui::text::ms_of(app.account_created_at.as_ref());
        let applied = crate::ui::text::ms_of(app.created_at.as_ref());
        let fresh = now - made < NEW_ACCOUNT_MS;
        let declining = self.pages.applications.declining.contains(&uid);
        let busy = self.pages.applications.busy.is_some();
        let leaving = self.pages.applications.leaving.get(&uid).copied();
        let amber = amber(p);
        let line = t_with(
            "serversettings.applications.line",
            &[
                ("username", Arg::Str(&user.username)),
                ("age", Arg::Str(&super::pages::roughly(now - made))),
                ("when", Arg::Str(&crate::ui::text::ago(applied, now))),
            ],
        );
        let header = div().flex().items_center().gap(px(12.0)).child(avatar(Some(&user), 44.0, p)).child(
            div()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .child(div().truncate().child(user_name(&user)))
                        .when(fresh, |el| {
                            el.child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .rounded_full()
                                    .bg(amber.opacity(0.15))
                                    .px(px(8.0))
                                    .py(px(2.0))
                                    .text_size(px(10.4))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(amber)
                                    .child(icon("sparkles").size(px(12.0)))
                                    .child(t("serversettings.applications.newAccount")),
                            )
                        }),
                )
                .child(div().truncate().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(line)),
        );
        let answers = (!app.answers.is_empty()).then(|| {
            div().mt(px(16.0)).flex().flex_col().gap(px(12.0)).children(app.answers.iter().map(|answer| {
                div()
                    .p(px(12.0))
                    .rounded(radius_2xl())
                    .bg(alpha(p.muted, 0.5))
                    .child(
                        div()
                            .text_xs()
                            .line_height(px(16.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.muted_foreground)
                            .child(answer.question.clone()),
                    )
                    .child(
                        div()
                            .mt(px(4.0))
                            .text_sm()
                            .line_height(px(20.0))
                            .when(answer.answer.is_empty(), |el| el.italic().text_color(p.muted_foreground))
                            .child(if answer.answer.is_empty() {
                                t("serversettings.applications.noAnswer")
                            } else {
                                answer.answer.clone()
                            }),
                    )
            }))
        });
        let reason_box = declining.then(|| {
            let state = self.reason_box(&uid, window, cx);
            let on = super::pages::focused(&state, window, cx);
            super::pages::slide_in(
                div()
                    .child(
                        div()
                            .mt(px(16.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child(t("serversettings.applications.whyNot"))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(p.muted_foreground)
                                    .child(t("serversettings.applications.whyNotHint")),
                            ),
                    )
                    .child(crate::ui::instance_home::focus_ring(
                        div()
                            .mt(px(6.0))
                            .min_h(px(64.0))
                            .px(px(12.0))
                            .py(px(8.0))
                            .rounded(radius_xl())
                            .border_1()
                            .border_color(p.border)
                            .text_sm()
                            .child(Textarea::new(&state).appearance(false)),
                        on,
                        p,
                    )),
                SharedString::from(format!("reason-in-{uid}")),
            )
        });
        let emerald = gpui_kit::rgb(0x10b981);
        let emerald_hover = gpui_kit::rgb(0x059669);
        let (u1, u2, u3) = (user.clone(), user.clone(), uid.clone());
        let footer = div().mt(px(16.0)).flex().flex_wrap().items_center().justify_end().gap(px(8.0)).map(|el| {
            if declining {
                el.child(
                    button(
                        SharedString::from(format!("cancel-{uid}")),
                        t("common.cancel"),
                        None,
                        Look::Ghost,
                        false,
                        p,
                    )
                    .rounded(radius_xl())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pages.applications.declining.remove(&u3);
                        cx.notify();
                    })),
                )
                .child(
                    button(
                        SharedString::from(format!("decline-{uid}")),
                        t("serversettings.applications.turnDown"),
                        Some("user-x"),
                        Look::Destructive,
                        false,
                        p,
                    )
                    .rounded(radius_xl())
                    .font_weight(FontWeight::BOLD)
                    .when(busy, |el| el.opacity(0.5))
                    .when(!busy, |el| {
                        el.on_click(cx.listener(move |this, _, _, cx| this.decide(u1.clone(), false, cx)))
                    }),
                )
            } else {
                let (border, red) = (alpha(p.destructive, 0.5), p.destructive);
                let open = uid.clone();
                el.child(
                    super::pages::hover_button(
                        SharedString::from(format!("turn-down-{uid}")),
                        t("serversettings.applications.turnDown"),
                        Some("user-x"),
                        true,
                        p,
                        move |s| s.border_color(border).text_color(red),
                    )
                    .when(!busy, |el| {
                        el.on_click(cx.listener(move |this, _, _, cx| {
                            this.pages.applications.declining.insert(open.clone());
                            cx.notify();
                        }))
                    }),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("let-in-{uid}")))
                        .h(px(36.0))
                        .px(px(12.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(radius_xl())
                        .bg(emerald)
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(gpui_kit::white())
                        .cursor_pointer()
                        .hover(move |s| s.bg(emerald_hover))
                        .active(|s| s.scale(0.97))
                        .when(busy, |el| el.opacity(0.5))
                        .when(!busy, |el| {
                            el.on_click(cx.listener(move |this, _, _, cx| this.decide(u2.clone(), true, cx)))
                        })
                        .child(icon(if leaving.is_some_and(|l| l.0) { "check" } else { "user-check" }).size(px(16.0)))
                        .child(t("serversettings.applications.letIn")),
                )
            }
        });
        let card = div()
            .relative()
            .overflow_hidden()
            .rounded(radius_3xl())
            .border_1()
            .p(px(20.0))
            .map(|el| match leaving {
                Some((true, _)) => el.border_color(gpui_kit::rgba(0x10b98199)).bg(gpui_kit::rgba(0x10b9810d)),
                Some((false, _)) => el.border_color(alpha(p.destructive, 0.5)).bg(alpha(p.destructive, 0.05)),
                None => el.border_color(p.border).bg(p.card),
            })
            .child(header)
            .children(answers)
            .children(reason_box)
            .child(footer);
        match leaving {
            Some((approve, at)) => {
                let dir = if approve { 1.0 } else { -1.0 };
                motion::once(card, SharedString::from(format!("app-leave-{uid}-{at:?}")), LEAVE, move |el, t| {
                    let k = t * t;
                    el.opacity(1.0 - k).relative().left(px(120.0 * dir * k))
                })
            }
            None => motion::rise(
                card,
                SharedString::from(format!("app-in-{uid}")),
                Duration::from_millis(30 * n.min(8) as u64),
                16.0,
            )
            .into_any_element(),
        }
    }

    /// The reason box for turning someone down, made the first time it's needed.
    fn reason_box(&mut self, uid: &str, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextareaState> {
        if let Some(s) = self.pages.applications.reasons.get(uid) {
            return s.clone();
        }
        let state = cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 6));
        state.update(cx, |s, cx| s.focus(window, cx));
        self.pages.applications.reasons.insert(uid.to_owned(), state.clone());
        state
    }
}
