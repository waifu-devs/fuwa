//! Usage, Limits, Transfer ownership and Delete server, as in the web's
//! `ServerSettingsDialog.tsx` and `settings/server/Ownership.tsx`: what the
//! server holds against its caps, an instance admin's caps for this server
//! alone, handing the server to another member, and deleting it once its
//! name is typed.

use super::pages::{boxed, focused, shimmers};
use super::*;
use crate::core::instance_admin::{UNITS, format_bytes, parse_cap, split_bytes};
use crate::ui::settings_controls::{Look, button, switch};
use crate::ui::text::{WIDE, tracked};
use crate::ui::theme::{radius_2xl, radius_3xl, radius_lg, radius_md, radius_xl};

/// The caps a server has of its own, as (field, label key, a size).
const CAPS: [(Cap, &str, bool); 5] = [
    (Cap::Members, "serversettings.nav.members", false),
    (Cap::Channels, "serversettings.nav.channels", false),
    (Cap::Storage, "serversettings.usage.storage", true),
    (Cap::Files, "serversettings.limits.files", true),
    (Cap::Emoji, "serversettings.nav.emoji", false),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cap {
    Members,
    Channels,
    Storage,
    Files,
    Emoji,
}

impl Cap {
    fn get(self, l: &pb::ServerLimits) -> Option<i64> {
        match self {
            Cap::Members => l.members,
            Cap::Channels => l.channels,
            Cap::Storage => l.storage_bytes,
            Cap::Files => l.attachment_bytes,
            Cap::Emoji => l.emojis,
        }
    }

    fn set(self, l: &mut pb::ServerLimits, v: Option<i64>) {
        match self {
            Cap::Members => l.members = v,
            Cap::Channels => l.channels = v,
            Cap::Storage => l.storage_bytes = v,
            Cap::Files => l.attachment_bytes = v,
            Cap::Emoji => l.emojis = v,
        }
    }
}

pub(super) struct Usage {
    data: Option<pb::GetServerUsageResponse>,
    error: Option<String>,
    loading: bool,
    /// Limits: the server's own caps, the draft, the instance's defaults.
    own: Option<pb::ServerLimits>,
    draft: pb::ServerLimits,
    defaults: pb::ServerLimits,
    limits_error: Option<String>,
    limits_loading: bool,
    saving: bool,
    save_error: Option<String>,
    boxes: Vec<Entity<InputState>>,
    units: [usize; 5],
    /// A cap just switched on with an empty box: what to put in it.
    pending_text: Option<(usize, &'static str)>,
    /// Transfer ownership: who's picked, and the boxes.
    find: Entity<InputState>,
    picked: Option<String>,
    confirm: Entity<InputState>,
    transferring: bool,
    transfer_error: Option<String>,
    /// Delete server.
    delete_confirm: Entity<InputState>,
    deleting: bool,
    delete_error: Option<String>,
}

impl Usage {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ServerSettingsView>) -> (Self, Vec<Subscription>) {
        let notify = |cx: &mut Context<ServerSettingsView>, s: &Entity<InputState>| {
            cx.subscribe(s, |_: &mut ServerSettingsView, _, e: &InputEvent, cx| {
                if let InputEvent::Change = e {
                    cx.notify()
                }
            })
        };
        let find = cx.new(|cx| InputState::new(window, cx).placeholder(t("serversettings.ownership.find")));
        let confirm = cx.new(|cx| InputState::new(window, cx));
        let delete_confirm = cx.new(|cx| InputState::new(window, cx));
        let mut subscriptions = vec![notify(cx, &find), notify(cx, &confirm), notify(cx, &delete_confirm)];
        let mut boxes = Vec::new();
        for (n, _) in CAPS.iter().enumerate() {
            let b = cx.new(|cx| InputState::new(window, cx));
            subscriptions.push(cx.subscribe(&b, move |this: &mut ServerSettingsView, s, e: &InputEvent, cx| {
                if let InputEvent::Change = e {
                    let text = s.read(cx).value().to_string();
                    this.cap_typed(n, &text);
                    cx.notify();
                }
            }));
            boxes.push(b);
        }
        let usage = Self {
            data: None,
            error: None,
            loading: false,
            own: None,
            draft: Default::default(),
            defaults: Default::default(),
            limits_error: None,
            limits_loading: false,
            saving: false,
            save_error: None,
            boxes,
            units: [1; 5],
            pending_text: None,
            find,
            picked: None,
            confirm,
            transferring: false,
            transfer_error: None,
            delete_confirm,
            deleting: false,
            delete_error: None,
        };
        (usage, subscriptions)
    }
}

/// A count with its thousands grouped, as the web's `lang.number`.
fn number(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

impl ServerSettingsView {
    fn load_usage(&mut self, cx: &mut Context<Self>) {
        let u = &mut self.pages.usage;
        if u.loading || u.data.is_some() || u.error.is_some() {
            return;
        }
        u.loading = true;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.server_usage(&key, &sid).await }, |this, result, cx| {
            let u = &mut this.pages.usage;
            u.loading = false;
            match result {
                Ok(data) => u.data = Some(data),
                Err(err) => u.error = Some(err.message),
            }
            cx.notify();
        });
    }

    pub(super) fn usage_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.load_usage(cx);
        let u = &self.pages.usage;
        if let Some(e) = &u.error {
            return super::pages::problem(e, p);
        }
        let Some(data) = u.data.clone() else {
            return div()
                .flex()
                .flex_wrap()
                .gap(px(12.0))
                .children((0..4).map(|n| {
                    div().w(px((self.column - 12.0) / 2.0)).child(crate::ui::instance_home::shimmer(
                        96.0,
                        radius_2xl(),
                        p,
                        window,
                        n,
                    ))
                }))
                .into_any_element();
        };
        let usage = data.usage.unwrap_or_default();
        let l = data.limits.unwrap_or_default();
        let checks = usage.automod_checks_today;
        let check_cap = l.automod_checks_per_day;
        struct Row {
            label: String,
            value: i64,
            limit: Option<i64>,
            bytes: bool,
            sub: Option<String>,
            note: Option<String>,
            warn: bool,
        }
        let row = |label: String, value: i64, limit: Option<i64>, bytes: bool, sub: Option<String>| Row {
            label,
            value,
            limit,
            bytes,
            sub,
            note: None,
            warn: false,
        };
        let rows = vec![
            row(t("serversettings.nav.members"), usage.members, l.members, false, None),
            row(t("serversettings.nav.channels"), usage.channels, l.channels, false, None),
            row(
                t("serversettings.usage.messages"),
                usage.messages,
                None,
                false,
                Some(t_with("serversettings.usage.sentAllTime", &[("count", Arg::Num(usage.messages_sent))])),
            ),
            row(t("serversettings.usage.storage"), usage.storage_bytes, l.storage_bytes, true, None),
            row(
                t("serversettings.usage.attachments"),
                usage.attachment_bytes,
                l.attachment_bytes,
                true,
                Some(t_with("serversettings.usage.files", &[("count", Arg::Num(usage.attachments))])),
            ),
            row(t("serversettings.nav.emoji"), usage.emojis, l.emojis, false, None),
            row(t("serversettings.usage.events"), usage.events, None, false, Some(t("serversettings.usage.inLog"))),
            Row {
                label: t("serversettings.usage.smartChecks"),
                value: checks,
                limit: check_cap,
                bytes: false,
                sub: Some(t("serversettings.usage.noDailyLimit")),
                warn: check_cap.is_some_and(|c| checks >= c),
                note: check_cap.map(|c| {
                    if checks >= c {
                        t_with("serversettings.usage.allUsed", &[("count", Arg::Num(c))])
                    } else {
                        t_with(
                            "serversettings.usage.checksLeft",
                            &[("cap", Arg::Str(&number(c))), ("left", Arg::Str(&number(c - checks)))],
                        )
                    }
                }),
            },
        ];
        let card_w = (self.column - 12.0) / 2.0;
        let amber = amber(p);
        let warn_fg = if p.dark { gpui_kit::rgb(0xfbbf24) } else { gpui_kit::rgb(0xb45309) };
        let mut grid = div().flex().flex_wrap().gap(px(12.0));
        for (n, r) in rows.into_iter().enumerate() {
            let share = r.limit.filter(|l| *l > 0).map(|l| (r.value as f32 / l as f32).min(1.0));
            let delay = Duration::from_millis(100 + 50 * n as u64);
            let fill = if r.warn { Hsla::from(gpui_kit::rgb(0xf59e0b)) } else { p.primary.into() };
            let bar = div().mt(px(8.0)).h(px(6.0)).rounded_full().overflow_hidden().bg(p.muted).child(motion::once(
                div().h_full().rounded_full().bg(fill),
                SharedString::from(format!("usage-bar-{n}")),
                Duration::from_millis(900) + delay,
                move |el, t| {
                    let start = delay.as_secs_f32() / (0.9 + delay.as_secs_f32());
                    let k = if t <= start { 0.0 } else { (t - start) / (1.0 - start) };
                    let e = 1.0 - (1.0 - k).powi(4);
                    let width = share.unwrap_or(1.0) * card_w;
                    el.w(px(width * e.max(0.0) - 0.0)).opacity(if share.is_some() { 1.0 } else { 0.25 })
                },
            ));
            let foot = r.note.clone().unwrap_or_else(|| match r.limit {
                Some(limit) => t_with(
                    "serversettings.usage.of",
                    &[("limit", Arg::Str(&if r.bytes { format_bytes(limit) } else { number(limit) }))],
                ),
                None => r.sub.clone().unwrap_or_else(|| t("serversettings.usage.noLimit")),
            });
            let bytes = r.bytes;
            grid = grid.child(motion::rise(
                div()
                    .w(px(card_w))
                    .p(px(16.0))
                    .rounded(radius_2xl())
                    .border_1()
                    .border_color(p.border)
                    .bg(alpha(p.background, 0.5))
                    .child(
                        div()
                            .text_xs()
                            .line_height(px(16.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.muted_foreground)
                            .child(tracked(r.label.to_uppercase(), WIDE)),
                    )
                    .child(
                        div().mt(px(4.0)).text_2xl().line_height(px(32.0)).font_weight(FontWeight::EXTRA_BOLD).child(
                            motion::count_up(
                                SharedString::from(format!("usage-n-{n}")),
                                r.value as f64,
                                delay,
                                if bytes { |v| format_bytes(v.round() as i64) } else { |v| number(v.round() as i64) },
                            ),
                        ),
                    )
                    .child(bar)
                    .child(
                        div()
                            .mt(px(6.0))
                            .text_xs()
                            .line_height(px(16.0))
                            .map(|el| {
                                if r.warn {
                                    el.font_weight(FontWeight::BOLD).text_color(warn_fg)
                                } else {
                                    el.text_color(p.muted_foreground)
                                }
                            })
                            .child(foot),
                    ),
                SharedString::from(format!("usage-card-{n}")),
                Duration::from_millis(50 * n as u64),
                10.0,
            ));
        }
        let _ = amber;
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(grid)
            .child(
                div()
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(t("serversettings.usage.note")),
            )
            .into_any_element()
    }

    // ───────────────────────── Limits ─────────────────────────

    fn load_limits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let u = &mut self.pages.usage;
        if u.limits_loading || u.own.is_some() || u.limits_error.is_some() {
            return;
        }
        u.limits_loading = true;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let rx = self.core.spawn(async move {
            let usage = core.server_usage(&key, &sid).await?;
            let node = core.node_usage(&key).await?;
            Ok::<_, Problem>((usage.own_limits.unwrap_or_default(), node.default_limits.unwrap_or_default()))
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.pages.usage.limits_loading = false;
                match result {
                    Ok((own, defaults)) => {
                        this.pages.usage.defaults = defaults;
                        this.take_limits(own, window, cx);
                    }
                    Err(err) => this.pages.usage.limits_error = Some(err.message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows `own` as saved and in the boxes.
    fn take_limits(&mut self, own: pb::ServerLimits, window: &mut Window, cx: &mut Context<Self>) {
        for (n, (cap, _, bytes)) in CAPS.iter().enumerate() {
            let v = cap.get(&own);
            let text = if *bytes {
                let (amount, unit) = split_bytes(v);
                self.pages.usage.units[n] = unit;
                amount
            } else {
                v.map(|v| v.to_string()).unwrap_or_default()
            };
            self.pages.usage.boxes[n].update(cx, |s, cx| s.set_value(text, window, cx));
        }
        let u = &mut self.pages.usage;
        u.draft = own;
        u.own = Some(own);
    }

    /// A cap's box changed: the draft follows while it reads as a number.
    fn cap_typed(&mut self, n: usize, text: &str) {
        let (cap, _, bytes) = CAPS[n];
        if cap.get(&self.pages.usage.draft).is_none() {
            return;
        }
        if let Some(v) = parse_cap(text, bytes.then_some(self.pages.usage.units[n])) {
            cap.set(&mut self.pages.usage.draft, Some(v));
        }
        self.pages.usage.save_error = None;
    }

    pub(super) fn limits_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.load_limits(window, cx);
        let u = &self.pages.usage;
        if let Some(e) = &u.limits_error {
            return super::pages::problem(e, p);
        }
        let Some(own) = u.own else { return shimmers(1, 192.0, radius_2xl(), p, window) };
        let draft = u.draft;
        let changed = CAPS.iter().filter(|(cap, _, _)| cap.get(&draft) != cap.get(&own)).count();
        if changed > 0 {
            let own2 = own;
            self.bar = Some(bar_with_error(
                "limits-bar",
                changed,
                self.pages.usage.saving,
                self.pages.usage.save_error.as_deref(),
                p,
                cx,
                move |this, window, cx| {
                    this.take_limits(own2, window, cx);
                    cx.notify();
                },
                move |this, window, cx| {
                    let draft = this.pages.usage.draft;
                    this.pages.usage.saving = true;
                    let (core, key, sid) = (this.core.clone(), this.key.clone(), this.server.clone());
                    let rx =
                        this.core.spawn(async move { core.set_server_limits(&key, &sid, draft).await.map(|_| draft) });
                    cx.spawn_in(window, async move |this, cx| {
                        let Ok(result) = rx.await else { return };
                        let _ = this.update_in(cx, |this, window, cx| {
                            this.pages.usage.saving = false;
                            match result {
                                Ok(saved) => this.take_limits(saved, window, cx),
                                Err(err) => this.pages.usage.save_error = Some(err.message),
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                    cx.notify();
                },
            ));
        }
        let defaults = self.pages.usage.defaults;
        let mut col = div().flex().flex_col().gap(px(16.0)).child(
            div()
                .text_sm()
                .line_height(px(20.0))
                .text_color(p.muted_foreground)
                .child(t("serversettings.limits.intro")),
        );
        for (n, (cap, key, bytes)) in CAPS.iter().copied().enumerate() {
            let on = cap.get(&draft).is_some();
            let fallback = {
                let d = cap.get(&defaults);
                let value = match d {
                    None => t("serversettings.usage.noLimit"),
                    Some(v) if bytes => format_bytes(v),
                    Some(v) => number(v),
                };
                t_with("serversettings.limits.instanceDefault", &[("value", Arg::Str(&value))])
            };
            let state = self.pages.usage.boxes[n].clone();
            let unit = self.pages.usage.units[n];
            let row = div()
                .min_h(px(36.0))
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(switch(
                    SharedString::from(format!("cap-switch-{n}")),
                    on,
                    false,
                    p,
                    window,
                    cx,
                    move |this: &mut Self, on, cx| {
                        let u = &mut this.pages.usage;
                        if !on {
                            cap.set(&mut u.draft, None);
                        } else {
                            let text = u.boxes[n].read(cx).value().to_string();
                            let parsed = parse_cap(&text, bytes.then_some(u.units[n]));
                            let start = parsed.unwrap_or(if bytes { UNITS[1].1 } else { 100 });
                            cap.set(&mut u.draft, Some(start));
                            if text.trim().is_empty() {
                                if bytes {
                                    u.units[n] = 1;
                                }
                                u.pending_text = Some((n, if bytes { "1" } else { "100" }));
                            }
                        }
                        cx.notify();
                    },
                ))
                .child(div().w(px(96.0)).flex_none().text_sm().font_weight(FontWeight::BOLD).child(t(key)))
                .child(if on {
                    motion::slide_in(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div().w(px(112.0)).child(
                                    boxed(Input::new(&state).appearance(false), 36.0, focused(&state, window, cx), p)
                                        .rounded(radius_lg()),
                                ),
                            )
                            .when(bytes, |el| {
                                el.child(div().flex().p(px(2.0)).rounded(radius_lg()).bg(p.muted).children(
                                    UNITS.iter().enumerate().map(|(k, (label, _))| {
                                        let chosen = k == unit;
                                        div()
                                            .id(SharedString::from(format!("cap-unit-{n}-{k}")))
                                            .px(px(8.0))
                                            .py(px(4.0))
                                            .rounded(radius_md())
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .cursor_pointer()
                                            .map(|el| {
                                                if chosen {
                                                    el.bg(p.background)
                                                        .text_color(p.foreground)
                                                        .shadow(crate::ui::settings_controls::shadow_sm())
                                                } else {
                                                    el.text_color(p.muted_foreground)
                                                }
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                let u = &mut this.pages.usage;
                                                u.units[n] = k;
                                                let text = u.boxes[n].read(cx).value().to_string();
                                                if let Some(v) = parse_cap(&text, Some(k)) {
                                                    cap.set(&mut u.draft, Some(v));
                                                }
                                                cx.notify();
                                            }))
                                            .child(*label)
                                    }),
                                ))
                            }),
                        SharedString::from(format!("cap-on-{n}")),
                        -12.0,
                    )
                    .into_any_element()
                } else {
                    motion::slide_in(
                        div().text_sm().text_color(p.muted_foreground).child(fallback),
                        SharedString::from(format!("cap-off-{n}")),
                        12.0,
                    )
                    .into_any_element()
                });
            col = col.child(row);
        }
        if let Some((n, value)) = self.pages.usage.pending_text.take() {
            self.pages.usage.boxes[n].update(cx, |s, cx| s.set_value(value, window, cx));
        }
        col.into_any_element()
    }

    // ───────────────────────── Transfer ownership ─────────────────────────

    pub(super) fn ownership_page(
        &mut self,
        server: &pb::Server,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (members, me) = self.core.shared.read(|s| {
            let i = s.instance(&self.key);
            (
                i.and_then(|i| i.members.get(&self.server).cloned()).unwrap_or_default(),
                i.and_then(|i| i.me.as_ref().map(|m| m.id.clone())).unwrap_or_default(),
            )
        });
        let name_of = |m: &pb::Member| {
            if m.nickname.is_empty() { m.user.as_ref().map(user_name).unwrap_or_default() } else { m.nickname.clone() }
        };
        let mine = members.iter().find(|m| m.user.as_ref().is_some_and(|u| u.id == me)).cloned();
        let query = self.pages.usage.find.read(cx).value().trim().to_lowercase();
        let others: Vec<pb::Member> = members
            .iter()
            .filter(|m| m.user.as_ref().is_some_and(|u| u.id != me))
            .filter(|m| {
                query.is_empty()
                    || format!("{} {}", name_of(m), m.user.as_ref().map(|u| u.username.as_str()).unwrap_or_default())
                        .to_lowercase()
                        .contains(&query)
            })
            .take(8)
            .cloned()
            .collect();
        let picked = self
            .pages
            .usage
            .picked
            .as_ref()
            .and_then(|id| members.iter().find(|m| m.user.as_ref().is_some_and(|u| &u.id == id)))
            .cloned();
        let confirm = self.pages.usage.confirm.read(cx).value().trim().to_lowercase();
        let armed = picked
            .as_ref()
            .and_then(|m| m.user.as_ref())
            .is_some_and(|u| !confirm.is_empty() && confirm == u.username.to_lowercase());
        let amber_c = gpui_kit::rgb(0xf59e0b);
        let amber_ring = gpui_kit::rgba(0xf59e0b66);
        let seat = |member: Option<&pb::Member>, label: String, crowned: bool, id: &str| {
            div()
                .w(px(112.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .relative()
                        .child(match member {
                            Some(m) => motion::once(
                                avatar(m.user.as_ref(), 64.0, p),
                                SharedString::from(format!(
                                    "seat-{id}-{}",
                                    m.user.as_ref().map(|u| u.id.as_str()).unwrap_or("")
                                )),
                                Duration::from_millis(300),
                                |el, t| el.opacity(0.5 + 0.5 * t),
                            ),
                            None => div()
                                .size(px(64.0))
                                .rounded_full()
                                .border_2()
                                .border_dashed()
                                .border_color(p.border)
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(p.muted_foreground)
                                .child("?")
                                .into_any_element(),
                        })
                        .when(crowned, |el| {
                            el.child(div().absolute().top(px(-16.0)).left(px(-8.0)).child(motion::once(
                                div().text_color(gpui_kit::rgb(0xfbbf24)).child(icon("crown").size(px(24.0))),
                                SharedString::from(format!(
                                    "crown-{id}-{}",
                                    member.and_then(|m| m.user.as_ref()).map(|u| u.id.as_str()).unwrap_or("")
                                )),
                                Duration::from_millis(420),
                                |el, t| {
                                    let k = 1.0 - (1.0 - t).powi(3);
                                    el.opacity(k).relative().top(px(-24.0 * (1.0 - k)))
                                },
                            )))
                        }),
                )
                .child(div().w_full().truncate().text_center().text_sm().font_weight(FontWeight::BOLD).child(label))
        };
        let arrow = if picked.is_some() {
            motion::ambient(
                div().text_color(amber_c).child(icon("arrow-right").size(px(20.0))),
                "ownership-arrow",
                Duration::from_millis(1200),
                window,
                |el, t| el.relative().left(px(6.0 * (t * std::f32::consts::PI).sin())),
            )
        } else {
            div()
                .text_color(alpha(p.muted_foreground, 0.4))
                .child(icon("arrow-right").size(px(20.0)))
                .into_any_element()
        };
        let stage = div()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(16.0))
            .px(px(16.0))
            .py(px(24.0))
            .rounded(radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.background, 0.4))
            .child(seat(mine.as_ref(), t("serversettings.shared.you"), picked.is_none(), "me"))
            .child(arrow)
            .child(seat(
                picked.as_ref(),
                picked.as_ref().map(name_of).unwrap_or_else(|| t("serversettings.ownership.pick")),
                picked.is_some(),
                "them",
            ));
        let find = self.pages.usage.find.clone();
        let search = div()
            .relative()
            .child(
                div()
                    .absolute()
                    .left(px(12.0))
                    .top(px(12.0))
                    .text_color(p.muted_foreground)
                    .child(icon("search").size(px(16.0))),
            )
            .child(boxed(Input::new(&find).appearance(false), 40.0, focused(&find, window, cx), p).pl(px(28.0)));
        let mut list = div().flex().flex_col().gap(px(4.0));
        if others.is_empty() {
            list = list.child(
                div()
                    .px(px(12.0))
                    .py(px(16.0))
                    .text_center()
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(t("serversettings.ownership.nobody")),
            );
        }
        for m in &others {
            let Some(user) = m.user.clone() else { continue };
            let on = picked.as_ref().and_then(|x| x.user.as_ref()).is_some_and(|u| u.id == user.id);
            let (hover, fg) = (alpha(p.muted, 0.6), p.foreground);
            let uid = user.id.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("owner-pick-{}", user.id)))
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .rounded(radius_xl())
                    .cursor_pointer()
                    .map(|el| {
                        // The web's ring sits outside the row; a border inside takes its room from the padding.
                        if on {
                            el.px(px(10.0))
                                .py(px(6.0))
                                .text_color(p.foreground)
                                .bg(gpui_kit::rgba(0xf59e0b1a))
                                .border_2()
                                .border_color(amber_ring)
                        } else {
                            el.px(px(12.0))
                                .py(px(8.0))
                                .text_color(p.muted_foreground)
                                .hover(move |s| s.bg(hover).text_color(fg))
                        }
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let u = &mut this.pages.usage;
                        u.picked = Some(uid.clone());
                        u.transfer_error = None;
                        u.confirm.update(cx, |s, cx| s.set_value("", window, cx));
                        cx.notify();
                    }))
                    .child(avatar(Some(&user), 32.0, p))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .line_height(px(20.0))
                                    .font_weight(FontWeight::BOLD)
                                    .child(name_of(m)),
                            )
                            .child(
                                div().truncate().text_xs().line_height(px(16.0)).child(format!("@{}", user.username)),
                            ),
                    ),
            );
        }
        let confirm_box = picked.as_ref().map(|m| {
            let user = m.user.clone().unwrap_or_default();
            let state = self.pages.usage.confirm.clone();
            let line = t_with(
                "serversettings.ownership.confirm",
                &[
                    ("username", Arg::Str(&strong(&user.username))),
                    ("server", Arg::Str(&server.name)),
                    ("name", Arg::Str(&name_of(m))),
                ],
            );
            let pending = self.pages.usage.transferring;
            let target = user.clone();
            let server_name = server.name.clone();
            let shown_name = name_of(m);
            super::pages::slide_in(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .p(px(16.0))
                    .rounded(radius_2xl())
                    .border_1()
                    .border_color(amber_ring)
                    .bg(gpui_kit::rgba(0xf59e0b0d))
                    .child(div().text_sm().child(marked(&line, p)))
                    .child(
                        boxed(Input::new(&state).appearance(false), 40.0, focused(&state, window, cx), p)
                            .when(armed, |el| el.border_color(amber_c)),
                    )
                    .when_some(self.pages.usage.transfer_error.clone(), |el, e| {
                        el.child(
                            div().text_sm().text_color(p.destructive).child(crate::ui::instance_home::capitalized(&e)),
                        )
                    })
                    .child(
                        div().flex().justify_end().child(
                            div()
                                .id("transfer-go")
                                .h(px(36.0))
                                .px(px(12.0))
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .rounded(radius_xl())
                                .bg(amber_c)
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .text_color(gpui_kit::white())
                                .when(!armed || pending, |el| el.opacity(0.5))
                                .when(armed && !pending, |el| {
                                    el.cursor_pointer().hover(|s| s.opacity(0.9)).on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.pages.usage.transferring = true;
                                            let (core, key, sid, uid) = (
                                                this.core.clone(),
                                                this.key.clone(),
                                                this.server.clone(),
                                                target.id.clone(),
                                            );
                                            let (server_name, shown_name) = (server_name.clone(), shown_name.clone());
                                            this.run(
                                                cx,
                                                async move { core.transfer_ownership(&key, &sid, &uid).await },
                                                move |this, result, cx| {
                                                    this.pages.usage.transferring = false;
                                                    match result {
                                                        Ok(_) => {
                                                            this.pages.usage.picked = None;
                                                            cx.emit(ServerSettingsEvent::Toast {
                                                                icon: "crown",
                                                                title: t_with(
                                                                    "serversettings.ownership.done",
                                                                    &[
                                                                        ("name", Arg::Str(&shown_name)),
                                                                        ("server", Arg::Str(&server_name)),
                                                                    ],
                                                                ),
                                                            });
                                                            this.page = None;
                                                        }
                                                        Err(err) => this.pages.usage.transfer_error = Some(err.message),
                                                    }
                                                    cx.notify();
                                                },
                                            );
                                            cx.notify();
                                        },
                                    ))
                                })
                                .child(if pending {
                                    spinner("transfer-spin", 16.0, window)
                                } else {
                                    icon("crown").size(px(16.0)).into_any_element()
                                })
                                .child(t("serversettings.nav.ownership")),
                        ),
                    ),
                "ownership-confirm",
            )
        });
        div()
            .flex()
            .flex_col()
            .gap(px(20.0))
            .child(
                div()
                    .text_sm()
                    .line_height(px(20.0))
                    .text_color(p.muted_foreground)
                    .child(t("serversettings.ownership.intro")),
            )
            .child(stage)
            .child(div().flex().flex_col().gap(px(8.0)).child(search).child(list))
            .children(confirm_box)
            .into_any_element()
    }

    // ───────────────────────── Delete server ─────────────────────────

    pub(super) fn danger_page(
        &mut self,
        server: &pb::Server,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.pages.usage.delete_confirm.clone();
        let armed = state.read(cx).value() == server.name;
        let pending = self.pages.usage.deleting;
        let line = t_with("serversettings.danger.confirm", &[("name", Arg::Str(&strong(&server.name)))]);
        let on = focused(&state, window, cx);
        let field = boxed(Input::new(&state).appearance(false), 40.0, on, p).when(armed, |el| {
            el.border_color(p.destructive).shadow(vec![gpui_kit::BoxShadow {
                color: alpha(p.destructive, 0.2),
                offset: gpui_kit::point(px(0.0), px(0.0)),
                blur_radius: px(0.0),
                spread_radius: px(2.0),
                inset: false,
            }])
        });
        let go = button("delete-server-go", t("serversettings.nav.danger"), None, Look::Destructive, false, p)
            .rounded(radius_xl())
            .font_weight(FontWeight::BOLD)
            .when(pending, |el| el.child(spinner("delete-spin", 16.0, window)))
            .when(!armed || pending, |el| el.opacity(0.5))
            .when(armed && !pending, |el| {
                el.on_click(cx.listener(|this, _, _, cx| {
                    this.pages.usage.deleting = true;
                    this.pages.usage.delete_error = None;
                    let (core, key, sid) = (this.core.clone(), this.key.clone(), this.server.clone());
                    this.run(cx, async move { core.delete_any_server(&key, &sid).await }, |this, result, cx| {
                        this.pages.usage.deleting = false;
                        match result {
                            Ok(()) => cx.emit(ServerSettingsEvent::Close),
                            Err(err) => this.pages.usage.delete_error = Some(err.message),
                        }
                        cx.notify();
                    });
                    cx.notify();
                }))
            });
        let go = motion::once(
            div().child(go),
            SharedString::from(format!("delete-armed-{armed}")),
            Duration::from_millis(400),
            move |el, t| {
                if armed {
                    let s = (t * std::f32::consts::PI).sin();
                    el.relative().top(px(-2.0 * s))
                } else {
                    el
                }
            },
        );
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(16.0))
            .rounded(radius_2xl())
            .border_1()
            .border_color(alpha(p.destructive, 0.4))
            .bg(alpha(p.destructive, 0.05))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.destructive)
                    .child(icon("triangle-alert").size(px(16.0)))
                    .child(t_with("serversettings.danger.title", &[("server", Arg::Str(&server.name))])),
            )
            .child(
                div()
                    .text_sm()
                    .line_height(px(20.0))
                    .text_color(p.muted_foreground)
                    .child(t("serversettings.danger.hint")),
            )
            .child(div().text_sm().line_height(px(14.0)).child(marked(&line, p)))
            .child(field)
            .when_some(self.pages.usage.delete_error.clone(), |el, e| {
                el.child(div().text_sm().text_color(p.destructive).child(crate::ui::instance_home::capitalized(&e)))
            })
            .child(div().flex().justify_end().child(go))
            .into_any_element()
    }
}
