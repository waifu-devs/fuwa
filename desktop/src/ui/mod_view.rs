//! Mod View (docs/mod-view.md): a server's cases and the people they're
//! about, full screen like server settings, for members with Use Mod View.
//! The queue (Open, Resolved, Dismissed, All) beside the case picked, and a
//! person's page (`mod_view/member.rs`). It shows things and never acts by
//! itself: time-outs, kicks and bans open the moderation dialog, which needs
//! their own permissions. Lists that grow draw only the rows in sight.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    UniformListScrollHandle, Window, div, px, uniform_list,
};

use crate::core::Core;
use crate::core::dms::now_ms;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::mod_view::{self as mv, TABS, Tab, kind_key, reason_key, status_key};
use crate::core::moderation::Action;
use crate::pb::{self, ModCaseKind as Kind, ModCaseStatus as Status, ModReportReason as Reason, Permission as P};
use crate::ui::motion;
use crate::ui::server_settings::spinner;
use crate::ui::settings_controls::{Look, button};
use crate::ui::text::{TIGHT, WIDE, ms_of, tracked};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_lg, radius_xl};
use crate::ui::widgets::{app_badge, avatar, error_line, icon, is_agent, pal};

mod member;

/// How tall a case is in the queue, its gap included.
const CASE_ROW: f32 = 104.0;
/// How long the screen takes to come and go.
const OPENING: Duration = Duration::from_millis(280);

pub enum ModViewEvent {
    Close,
    /// Open the moderation dialog over Mod View.
    Moderate {
        user_id: String,
        action: Action,
    },
    /// Close and show this message where it is.
    Jump(Box<pb::Message>),
    Toast {
        icon: &'static str,
        title: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Screen {
    Queue,
    Member,
}

pub struct ModViewView {
    core: Arc<Core>,
    pub key: String,
    pub server: String,
    screen: Screen,
    tab: Tab,
    /// Only the cases about this person.
    subject: Option<String>,
    /// The cases the instance listed for this tab, newest first.
    listed: Vec<String>,
    more: bool,
    loading: bool,
    /// Which listing is the latest asked, so an older answer is dropped.
    asked: u64,
    queue_scroll: UniformListScrollHandle,
    /// The case open beside the queue, and what the instance said of it.
    picked: Option<String>,
    case: Option<pb::GetModCaseResponse>,
    case_loading: bool,
    resolution: Entity<InputState>,
    /// A case's status being changed to this.
    acting: Option<Status>,
    member: Option<member::MemberPage>,
    note: Entity<TextareaState>,
    error: Option<String>,
    closing: Option<Instant>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ModViewEvent> for ModViewView {}

impl ModViewView {
    pub fn new(core: Arc<Core>, key: String, server: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let resolution = cx.new(|cx| InputState::new(window, cx).placeholder(t("desktop.modView.case.resolutionHint")));
        let note = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(2, 8).placeholder(t("desktop.modView.member.notePlaceholder"))
        });
        // Drawn again when the instance changes (a case arrives, a note, someone's roles).
        let mut changes = core.changes();
        let watching = core.clone();
        cx.spawn_in(window, async move |this, cx| {
            let mut seen = watching.shared.version();
            while changes.changed().await.is_ok() {
                let now = watching.shared.version();
                let changed = watching.shared.changed_since(seen, &[]);
                seen = now;
                let going = this.update(cx, |this, cx| {
                    if changed {
                        this.member_changed(cx);
                        cx.notify();
                    }
                });
                if going.is_err() {
                    break;
                }
            }
        })
        .detach();
        let subscriptions =
            vec![cx.subscribe(&note, |_: &mut Self, _, e: &gpui_kit::component::input::InputEvent, cx| {
                if let gpui_kit::component::input::InputEvent::Change = e {
                    cx.notify()
                }
            })];
        let mut this = Self {
            core,
            key,
            server,
            screen: Screen::Queue,
            tab: Tab::Open,
            subject: None,
            listed: Vec::new(),
            more: false,
            loading: false,
            asked: 0,
            queue_scroll: UniformListScrollHandle::new(),
            picked: None,
            case: None,
            case_loading: false,
            resolution,
            acting: None,
            member: None,
            note,
            error: None,
            closing: None,
            _subscriptions: subscriptions,
        };
        this.load_queue(false, cx);
        this
    }

    /// Runs `future` on the core and hands its result back to the view.
    fn run<T: Send + 'static>(
        &self,
        cx: &mut Context<Self>,
        future: impl Future<Output = T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        let rx = self.core.spawn(future);
        cx.spawn(async move |this, cx| {
            if let Ok(value) = rx.await {
                let _ = this.update(cx, |this, cx| done(this, value, cx));
            }
        })
        .detach();
    }

    /// The queue's first page for the tab and person picked, or the next with `older`.
    fn load_queue(&mut self, older: bool, cx: &mut Context<Self>) {
        if older && (self.loading || !self.more) {
            return;
        }
        let before = if older { self.listed.last().cloned().unwrap_or_default() } else { String::new() };
        if !older {
            self.listed.clear();
            self.more = false;
        }
        self.loading = true;
        self.asked += 1;
        let asked = self.asked;
        let (core, key, sid, tab) = (self.core.clone(), self.key.clone(), self.server.clone(), self.tab);
        let subject = self.subject.clone().unwrap_or_default();
        self.run(
            cx,
            async move { core.mod_cases(&key, &sid, tab, &subject, &before, mv::PAGE).await },
            move |this, result, cx| {
                if this.asked != asked {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(page) => {
                        this.listed.extend(page.ids);
                        this.more = page.more;
                    }
                    Err(err) => this.error = Some(err.message),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn pick_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.queue_scroll = UniformListScrollHandle::new();
        self.load_queue(false, cx);
    }

    /// The queue, of everyone or only one person's cases.
    pub(crate) fn show_queue(&mut self, subject: Option<String>, cx: &mut Context<Self>) {
        self.screen = Screen::Queue;
        self.error = None;
        if self.subject != subject {
            self.subject = subject;
            if self.subject.is_some() {
                self.tab = Tab::All;
            }
            self.queue_scroll = UniformListScrollHandle::new();
            self.load_queue(false, cx);
        }
        cx.notify();
    }

    /// Opens a case beside the queue, reading its reports and the message as it is now.
    fn pick_case(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.picked.as_deref() == Some(id.as_str()) {
            return;
        }
        self.picked = Some(id.clone());
        self.case = None;
        self.case_loading = true;
        self.resolution.update(cx, |s, cx| s.set_value("", window, cx));
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let asked = id.clone();
        self.run(cx, async move { core.mod_case(&key, &sid, &id).await }, move |this, result, cx| {
            if this.picked.as_deref() != Some(asked.as_str()) {
                return;
            }
            this.case_loading = false;
            match result {
                Ok(res) => this.case = Some(res),
                Err(err) => this.error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    /// Resolves or dismisses the open case (with what's typed), or opens it again.
    fn set_status(&mut self, status: Status, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.picked.clone() else { return };
        if self.acting.is_some() {
            return;
        }
        self.acting = Some(status);
        self.error = None;
        let resolution =
            if status == Status::Open { String::new() } else { self.resolution.read(cx).value().to_string() };
        self.resolution.update(cx, |s, cx| s.set_value("", window, cx));
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(
            cx,
            async move { core.set_case_status(&key, &sid, &id, status, &resolution).await },
            move |this, result, cx| {
                this.acting = None;
                match result {
                    Ok(case) => {
                        if let Some(open) = this.case.as_mut() {
                            open.mod_case = Some(case);
                        }
                        let title = match status {
                            Status::Resolved => t("desktop.modView.case.resolvedToast"),
                            Status::Dismissed => t("desktop.modView.case.dismissedToast"),
                            _ => t("desktop.modView.case.reopenedToast"),
                        };
                        let glyph = if status == Status::Open { "undo" } else { "circle-check" };
                        cx.emit(ModViewEvent::Toast { icon: glyph, title });
                    }
                    Err(err) => this.error = Some(err.message),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// Escape: closes a person's page first, then Mod View.
    pub fn escape(&mut self, cx: &mut Context<Self>) {
        if self.screen == Screen::Member {
            self.screen = Screen::Queue;
            cx.notify();
            return;
        }
        self.close(cx);
    }

    /// Closes with the screen fading and growing away.
    fn close(&mut self, cx: &mut Context<Self>) {
        if self.closing.is_some() {
            return;
        }
        if cx.reduce_motion() {
            cx.emit(ModViewEvent::Close);
            return;
        }
        self.closing = Some(Instant::now());
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(OPENING).await;
            let _ = this.update(cx, |_, cx| cx.emit(ModViewEvent::Close));
        })
        .detach();
    }

    /// What the page needs from the store about the server.
    fn facts(&self) -> Facts {
        self.core.shared.read(|s| {
            let Some(i) = s.instance(&self.key) else { return Facts::default() };
            let access = i.access(&self.server);
            Facts {
                name: i.server(&self.server).map(|s| s.name.clone()),
                allowed: i.can_mod_view(&self.server),
                manage: access.has(P::ManageServer),
                open: i.open_cases(&self.server),
                me: i.me.as_ref().map(|u| u.id.clone()).unwrap_or_default(),
            }
        })
    }

    /// Someone's name here (their nickname, else their name).
    fn name_of(&self, user_id: &str) -> String {
        self.core
            .shared
            .read(|s| s.instance(&self.key).map(|i| i.display_name(Some(&self.server), user_id)).unwrap_or_default())
    }

    fn user_of(&self, user_id: &str) -> Option<pb::User> {
        self.core.shared.read(|s| s.instance(&self.key).and_then(|i| i.users.get(user_id).cloned()))
    }

    fn channel_name(&self, channel_id: &str) -> Option<String> {
        self.core
            .shared
            .read(|s| s.instance(&self.key).and_then(|i| i.channel(&self.server, channel_id)).map(|c| c.name.clone()))
    }

    /// What you may do to someone from here (time out, kick, ban), as their card offers it.
    fn can_act_on(&self, user_id: &str) -> Vec<P> {
        self.core.shared.read(|s| {
            let Some(i) = s.instance(&self.key) else { return Vec::new() };
            let member = i
                .members
                .get(&self.server)
                .is_some_and(|l| l.iter().any(|m| m.user.as_ref().is_some_and(|u| u.id == user_id)));
            i.can_moderate(&self.server, user_id).into_iter().filter(|p| member || *p == P::BanMembers).collect()
        })
    }
}

#[derive(Default)]
struct Facts {
    name: Option<String>,
    allowed: bool,
    manage: bool,
    open: Option<i32>,
    me: String,
}

/// A case's look: its icon and tint.
fn kind_look(kind: Kind, p: &Palette) -> (&'static str, Hsla) {
    match kind {
        Kind::AutoMod => ("bot", gpui_kit::rgb(0xf59e0b).into()),
        Kind::MessageReport => ("flag", p.destructive.into()),
        Kind::MemberReport | Kind::Unspecified => ("user-round-search", gpui_kit::rgb(0x8b5cf6).into()),
    }
}

fn status_look(status: Status, p: &Palette) -> Hsla {
    match status {
        Status::Open | Status::Unspecified => p.primary.into(),
        Status::Resolved => gpui_kit::rgb(0x10b981).into(),
        Status::Dismissed => p.muted_foreground.into(),
    }
}

/// A small tinted pill.
fn chip(text: impl Into<SharedString>, tint: Hsla) -> gpui_kit::Div {
    div()
        .flex_none()
        .px(px(8.0))
        .h(px(20.0))
        .flex()
        .items_center()
        .gap(px(4.0))
        .rounded_full()
        .bg(tint.opacity(0.14))
        .text_color(tint)
        .text_size(px(11.0))
        .font_weight(FontWeight::BOLD)
        .child(text.into())
}

/// A case's reasons, its AutoMod rule, and how many reported it, as chips.
fn case_chips(case: &pb::ModCase, p: &Palette) -> Vec<gpui_kit::Div> {
    let mut out = Vec::new();
    if let Some(alert) = &case.auto_mod {
        let amber: Hsla = gpui_kit::rgb(0xf59e0b).into();
        if !alert.rule_name.is_empty() {
            out.push(chip(alert.rule_name.clone(), amber));
        }
        out.push(chip(
            if alert.blocked { t("desktop.modView.case.blocked") } else { t("desktop.modView.case.flagged") },
            if alert.blocked { p.destructive.into() } else { amber },
        ));
    }
    if case.report_count > 0 {
        out.push(chip(
            t_with("desktop.modView.case.reports", &[("count", Arg::Num(i64::from(case.report_count)))]),
            p.destructive.into(),
        ));
    }
    for reason in case.reasons.iter().filter_map(|r| Reason::try_from(*r).ok()).take(3) {
        out.push(chip(t(reason_key(reason)), p.muted_foreground.into()));
    }
    out
}

/// One line of a message, for rows.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl ModViewView {
    /// A case in a list: who it's about, where, a line of what was said, chips and when.
    fn case_row(&self, case: &pb::ModCase, picked: bool, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let kind = Kind::try_from(case.kind).unwrap_or(Kind::Unspecified);
        let status = Status::try_from(case.status).unwrap_or(Status::Open);
        let (glyph, tint) = kind_look(kind, p);
        let name = self.name_of(&case.subject_id);
        let place = match self.channel_name(&case.channel_id) {
            Some(c) => format!("#{c}"),
            None if case.channel_id.is_empty() => t(kind_key(kind)),
            None => t("desktop.modView.case.hiddenChannel"),
        };
        let snippet = if case.content.is_empty() { t(kind_key(kind)) } else { one_line(&case.content) };
        let ago = crate::ui::text::ago(ms_of(case.updated_at.as_ref()), now_ms());
        let id = case.id.clone();
        let hover = alpha(p.muted, 0.6);
        let group = SharedString::from(format!("mv-case-{}", case.id));
        div()
            .id(group.clone())
            .group(group.clone())
            .h(px(CASE_ROW - 8.0))
            .px(px(12.0))
            .py(px(10.0))
            .flex()
            .gap(px(12.0))
            .rounded(radius_xl())
            .border_1()
            .cursor_pointer()
            .map(|el| {
                if picked {
                    el.border_color(alpha(p.primary, 0.5)).bg(alpha(p.primary, 0.1))
                } else {
                    el.border_color(alpha(p.border, 0.6)).hover(move |s| s.bg(hover))
                }
            })
            .active(|s| s.scale(0.99))
            .on_click(cx.listener(move |this, _, window, cx| this.pick_case(id.clone(), window, cx)))
            .child(
                div()
                    .id(SharedString::from(format!("mv-case-icon-{}", case.id)))
                    .size(px(32.0))
                    .flex_none()
                    .rounded(radius_lg())
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(tint.opacity(0.15))
                    .text_color(tint)
                    .group_hover(group, |s| s.rotate(gpui_kit::radians(-0.18)).scale(1.08))
                    .child(icon(glyph).size(px(16.0))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_sm()
                            .child(div().font_weight(FontWeight::BOLD).truncate().child(name))
                            .child(div().flex_1().min_w_0().truncate().text_color(p.muted_foreground).child(place))
                            .child(div().flex_none().text_xs().text_color(p.muted_foreground).child(ago)),
                    )
                    .child(div().text_sm().truncate().text_color(alpha(p.foreground, 0.85)).child(snippet))
                    .child(
                        div()
                            .flex()
                            .gap(px(4.0))
                            .overflow_hidden()
                            .when(self.tab == Tab::All || self.subject.is_some(), |el| {
                                el.child(chip(t(status_key(status)), status_look(status, p)))
                            })
                            .children(case_chips(case, p)),
                    ),
            )
            .into_any_element()
    }

    /// The queue: tabs, the person it's narrowed to, and the cases (only those in sight drawn).
    fn queue(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.core.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.mod_view.get(&self.server))
                .map(|m| mv::queue(m, self.tab, self.subject.as_deref(), &self.listed, self.more))
                .unwrap_or_default()
        });
        // The tabs, the picked one's fill gliding between them.
        let at = TABS.iter().position(|t| *t == self.tab).unwrap_or(0);
        let width = 96.0;
        let x = motion::follow("mv-tab-x", at as f32 * width, window, cx);
        let mut tabs = div().relative().flex().p(px(3.0)).rounded(radius_xl()).bg(alpha(p.muted, 0.7)).child(
            div().absolute().top(px(3.0)).left(px(3.0 + x)).w(px(width)).h(px(30.0)).rounded(radius_lg()).bg(p.card),
        );
        for tab in TABS {
            let on = tab == self.tab;
            let fg = p.foreground;
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("mv-tab-{tab:?}")))
                    .relative()
                    .w(px(width))
                    .h(px(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(if on { p.foreground } else { p.muted_foreground })
                    .cursor_pointer()
                    .hover(move |s| s.text_color(fg))
                    .active(|s| s.scale(0.96))
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_tab(tab, cx)))
                    .child(t(tab.key())),
            );
        }
        let mut top = div().flex().flex_wrap().items_center().gap(px(8.0)).child(tabs);
        if let Some(subject) = self.subject.clone() {
            let name = self.name_of(&subject);
            let hover = alpha(p.primary, 0.2);
            top = top.child(motion::pop_in(
                div()
                    .id("mv-subject")
                    .h(px(30.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .rounded_full()
                    .bg(alpha(p.primary, 0.12))
                    .text_color(p.primary)
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(|this, _, _, cx| this.show_queue(None, cx)))
                    .child(t_with("desktop.modView.queue.about", &[("name", Arg::Str(&name))]))
                    .child(icon("x").size(px(14.0))),
                SharedString::from(format!("mv-subject-{subject}")),
                (0.0, 0.5),
                0.9,
                0.0,
            ));
        }
        let more = self.more;
        let list: AnyElement = if rows.is_empty() && self.loading {
            crate::ui::server_settings::shimmer_rows(5, p).into_any_element()
        } else if rows.is_empty() {
            empty(
                "inbox",
                &if self.tab == Tab::Open {
                    t("desktop.modView.queue.emptyOpen")
                } else {
                    t("desktop.modView.queue.empty")
                },
                p,
            )
        } else {
            let rows = std::rc::Rc::new(rows);
            let count = rows.len() + usize::from(more);
            let picked = self.picked.clone();
            uniform_list(
                "mv-cases",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                    let p = pal(cx);
                    range
                        .map(|n| match rows.get(n) {
                            Some(case) => {
                                let on = picked.as_deref() == Some(case.id.as_str());
                                div()
                                    .h(px(CASE_ROW))
                                    .pb(px(8.0))
                                    .child(this.case_row(case, on, &p, cx))
                                    .into_any_element()
                            }
                            None => this.more_row("mv-cases-more", window, cx, |this, cx| this.load_queue(true, cx)),
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.queue_scroll)
            .flex_1()
            .min_h_0()
            .into_any_element()
        };
        div()
            .w(px(420.0))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(top)
            .child(list)
            .into_any_element()
    }

    /// The last row of a list with more to read: a button, spinning while it reads.
    fn more_row(
        &self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        more: fn(&mut Self, &mut Context<Self>),
    ) -> AnyElement {
        let p = pal(cx);
        let loading = self.loading || self.member.as_ref().is_some_and(|m| m.loading_more());
        div()
            .h(px(CASE_ROW))
            .pt(px(10.0))
            .flex()
            .justify_center()
            .child(
                button(id, t("desktop.modView.more"), None, Look::Outline, false, &p)
                    .rounded(radius_xl())
                    .when(loading, |el| el.opacity(0.5).child(spinner(format!("{id}-spin"), 16.0, window)))
                    .on_click(cx.listener(move |this, _, _, cx| more(this, cx))),
            )
            .into_any_element()
    }

    /// The case open beside the queue.
    fn case_detail(&mut self, facts: &Facts, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(id) = self.picked.clone() else {
            return empty("mouse-pointer-click", &t("desktop.modView.case.pick"), p);
        };
        // The case as it is now (events change it), else as it was read.
        let case = self
            .core
            .shared
            .read(|s| s.instance(&self.key).and_then(|i| i.mod_view.get(&self.server)?.cases.get(&id).cloned()))
            .or_else(|| self.case.as_ref().and_then(|c| c.mod_case.clone()));
        let Some(case) = case else {
            return if self.case_loading {
                crate::ui::server_settings::shimmer_rows(3, p).into_any_element()
            } else {
                empty("search-x", &t("desktop.modView.case.gone"), p)
            };
        };
        let read = self.case.as_ref().filter(|c| c.mod_case.as_ref().is_some_and(|m| m.id == id));
        let kind = Kind::try_from(case.kind).unwrap_or(Kind::Unspecified);
        let status = Status::try_from(case.status).unwrap_or(Status::Open);
        let (glyph, tint) = kind_look(kind, p);
        let user = self.user_of(&case.subject_id);
        let name = self.name_of(&case.subject_id);

        // Who it's about, and the way to their page.
        let subject = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .p(px(12.0))
            .rounded(radius_2xl())
            .bg(alpha(p.muted, 0.6))
            .child(avatar(user.as_ref(), 40.0, p))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(div().font_weight(FontWeight::BOLD).truncate().child(name.clone()))
                            .when(is_agent(user.as_ref()), |el| el.child(app_badge("mv-case-agent", "AGENT", p))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .truncate()
                            .text_color(p.muted_foreground)
                            .child(format!("@{}", user.as_ref().map(|u| u.username.as_str()).unwrap_or(""))),
                    ),
            )
            .child({
                let uid = case.subject_id.clone();
                button(
                    "mv-view-member",
                    t("desktop.modView.case.viewMember"),
                    Some("user-round-search"),
                    Look::Outline,
                    true,
                    p,
                )
                .rounded(radius_xl())
                .on_click(cx.listener(move |this, _, window, cx| this.open_member(uid.clone(), window, cx)))
            });

        let mut col = div().flex().flex_col().gap(px(16.0));
        col = col.child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .size(px(36.0))
                        .rounded(radius_xl())
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(tint.opacity(0.15))
                        .text_color(tint)
                        .child(icon(glyph).size(px(18.0))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().text_lg().font_weight(FontWeight::EXTRA_BOLD).child(t(kind_key(kind))))
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.muted_foreground)
                                .child(crate::ui::text::full(ms_of(case.created_at.as_ref()))),
                        ),
                )
                .child(motion::swap_text(format!("mv-status-{}", case.id), t(status_key(status)), 12.0, window, cx))
                .child(div().size(px(10.0)).rounded_full().bg(status_look(status, p))),
        );
        col = col.child(subject);

        // Where it was said, and what.
        if !case.channel_id.is_empty() {
            let place = match self.channel_name(&case.channel_id) {
                Some(c) => format!("#{c}"),
                None => t("desktop.modView.case.hiddenChannel"),
            };
            let live = read.and_then(|r| r.message.clone());
            let mut said = div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .p(px(14.0))
                .rounded(radius_2xl())
                .border_1()
                .border_color(p.border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child(icon("hash").size(px(12.0)))
                        .child(place)
                        .when(!case.thread_id.is_empty(), |el| el.child(t("desktop.modView.case.inThread"))),
                )
                .child(div().text_sm().child(crate::ui::text::markdown(
                    SharedString::from(format!("mv-content-{}", case.id)),
                    crate::ui::text::images_as_links(&case.content),
                )));
            let blocked = case.auto_mod.as_ref().is_some_and(|a| a.blocked);
            let note = if blocked {
                Some(t("desktop.modView.case.neverSent"))
            } else if read.is_some() && live.is_none() && !case.message_id.is_empty() {
                Some(t("desktop.modView.case.deleted"))
            } else if live.as_ref().is_some_and(|m| m.content != case.content) {
                Some(t("desktop.modView.case.edited"))
            } else {
                None
            };
            said =
                said.when_some(note, |el, note| el.child(div().text_xs().text_color(p.muted_foreground).child(note)));
            if let Some(message) = live {
                said = said.child(
                    button("mv-jump", t("desktop.modView.case.jump"), Some("arrow-up-right"), Look::Outline, true, p)
                        .rounded(radius_xl())
                        .on_click(
                            cx.listener(move |_, _, _, cx| cx.emit(ModViewEvent::Jump(Box::new(message.clone())))),
                        ),
                );
            }
            col = col.child(said);
        }

        // AutoMod's rule and what set it off.
        if let Some(alert) = &case.auto_mod {
            let amber: Hsla = gpui_kit::rgb(0xf59e0b).into();
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .p(px(14.0))
                    .rounded(radius_2xl())
                    .bg(amber.opacity(0.08))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child(icon("bot").size(px(14.0)).text_color(amber))
                            .child(t_with("desktop.modView.case.rule", &[("rule", Arg::Str(&alert.rule_name))])),
                    )
                    .when(!alert.matched.is_empty(), |el| {
                        el.child(div().text_xs().text_color(p.muted_foreground).child(t_with(
                            "desktop.modView.case.matched",
                            &[("words", Arg::Str(&alert.matched.join(", ")))],
                        )))
                    })
                    .when(alert.timed_out_seconds > 0, |el| {
                        el.child(div().text_xs().text_color(p.muted_foreground).child(t_with(
                            "desktop.modView.case.timedOut",
                            &[(
                                "duration",
                                Arg::Str(&crate::ui::moderate::duration(i64::from(alert.timed_out_seconds))),
                            )],
                        )))
                    }),
            );
        }

        // The reports, oldest first.
        if let Some(read) = read.filter(|r| !r.reports.is_empty()) {
            let mut list = div().flex().flex_col().gap(px(8.0)).child(section_title(
                t_with("desktop.modView.case.reportsTitle", &[("count", Arg::Num(read.reports.len() as i64))]),
                p,
            ));
            for (n, report) in read.reports.iter().enumerate() {
                let who = self.user_of(&report.reporter_id);
                let reason = Reason::try_from(report.reason).unwrap_or(Reason::Other);
                list =
                    list.child(motion::rise(
                        div()
                            .flex()
                            .gap(px(10.0))
                            .p(px(10.0))
                            .rounded(radius_xl())
                            .bg(alpha(p.muted, 0.5))
                            .child(avatar(who.as_ref(), 28.0, p))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .text_sm()
                                            .child(
                                                div()
                                                    .font_weight(FontWeight::BOLD)
                                                    .truncate()
                                                    .child(self.name_of(&report.reporter_id)),
                                            )
                                            .child(chip(t(reason_key(reason)), p.destructive.into()))
                                            .child(div().flex_1())
                                            .child(div().text_xs().text_color(p.muted_foreground).child(
                                                crate::ui::text::ago(ms_of(report.created_at.as_ref()), now_ms()),
                                            )),
                                    )
                                    .when(!report.note.is_empty(), |el| {
                                        el.child(
                                            div()
                                                .text_sm()
                                                .text_color(alpha(p.foreground, 0.85))
                                                .child(report.note.clone()),
                                        )
                                    }),
                            ),
                        SharedString::from(format!("mv-report-{}-{n}", case.id)),
                        Duration::from_millis(30 * n as u64),
                        6.0,
                    ));
            }
            col = col.child(list);
        }

        // Who closed it and why.
        if status != Status::Open && !case.closed_by_id.is_empty() {
            let by = self.name_of(&case.closed_by_id);
            let when = crate::ui::text::ago(ms_of(case.closed_at.as_ref()), now_ms());
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .p(px(12.0))
                    .rounded(radius_xl())
                    .bg(status_look(status, p).opacity(0.08))
                    .child(div().text_sm().font_weight(FontWeight::BOLD).child(t_with(
                        if status == Status::Resolved {
                            "desktop.modView.case.resolvedBy"
                        } else {
                            "desktop.modView.case.dismissedBy"
                        },
                        &[("name", Arg::Str(&by)), ("when", Arg::Str(&when))],
                    )))
                    .when(!case.resolution.is_empty(), |el| {
                        el.child(div().text_sm().text_color(p.muted_foreground).child(case.resolution.clone()))
                    }),
            );
        }

        // What to do: close it (with a why) or open it again, and the moderation shortcuts.
        let acting = self.acting;
        let mut actions = div().flex().flex_col().gap(px(10.0));
        if status == Status::Open {
            actions = actions
                .child(
                    div()
                        .h(px(40.0))
                        .px(px(12.0))
                        .rounded(radius_xl())
                        .border_1()
                        .border_color(p.border)
                        .flex()
                        .items_center()
                        .text_sm()
                        .child(div().flex_1().child(Input::new(&self.resolution).appearance(false))),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .child(
                            button(
                                "mv-resolve",
                                t("desktop.modView.case.resolve"),
                                Some("circle-check"),
                                Look::Primary,
                                false,
                                p,
                            )
                            .rounded(radius_xl())
                            .when(acting == Some(Status::Resolved), |el| {
                                el.opacity(0.6).child(spinner("mv-resolve-spin", 14.0, window))
                            })
                            .on_click(cx.listener(|this, _, window, cx| this.set_status(Status::Resolved, window, cx))),
                        )
                        .child(
                            button(
                                "mv-dismiss",
                                t("desktop.modView.case.dismiss"),
                                Some("circle-x"),
                                Look::Outline,
                                false,
                                p,
                            )
                            .rounded(radius_xl())
                            .when(acting == Some(Status::Dismissed), |el| {
                                el.opacity(0.6).child(spinner("mv-dismiss-spin", 14.0, window))
                            })
                            .on_click(
                                cx.listener(|this, _, window, cx| this.set_status(Status::Dismissed, window, cx)),
                            ),
                        ),
                );
        } else {
            actions = actions.child(
                div().flex().child(
                    button("mv-reopen", t("desktop.modView.case.reopen"), Some("undo"), Look::Outline, false, p)
                        .rounded(radius_xl())
                        .when(acting == Some(Status::Open), |el| {
                            el.opacity(0.6).child(spinner("mv-reopen-spin", 14.0, window))
                        })
                        .on_click(cx.listener(|this, _, window, cx| this.set_status(Status::Open, window, cx))),
                ),
            );
        }
        if case.subject_id != facts.me {
            let shortcuts = self.moderation_buttons(&case.subject_id, "mv-case", p, cx);
            actions = actions.when_some(shortcuts, |el, row| el.child(row));
        }
        col = col.child(actions);
        motion::rise(col, SharedString::from(format!("mv-case-in-{}", case.id)), Duration::ZERO, 10.0)
            .into_any_element()
    }

    /// Time out, kick and ban, each only when you may (the moderation dialog asks for the rest).
    fn moderation_buttons(&self, user_id: &str, id: &str, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let allowed = self.can_act_on(user_id);
        if allowed.is_empty() {
            return None;
        }
        let mut row = div().flex().flex_wrap().gap(px(6.0));
        for permission in allowed {
            let (glyph, label, action) = match permission {
                P::TimeOutMembers => ("hourglass", t("workspace.popover.timeout"), Action::TimeOut(3_600)),
                P::KickMembers => ("door-open", t("workspace.popover.kick"), Action::Kick),
                _ => ("gavel", t("workspace.popover.ban"), Action::Ban(0)),
            };
            let uid = user_id.to_owned();
            row = row.child(
                button(SharedString::from(format!("{id}-{glyph}")), label, Some(glyph), Look::DangerOutline, true, p)
                    .rounded(radius_xl())
                    .on_click(
                        cx.listener(move |_, _, _, cx| {
                            cx.emit(ModViewEvent::Moderate { user_id: uid.clone(), action })
                        }),
                    ),
            );
        }
        Some(row.into_any_element())
    }
}

/// A heading over a part of a page.
fn section_title(text: String, p: &Palette) -> gpui_kit::Div {
    div()
        .text_size(px(11.2))
        .font_weight(FontWeight::BOLD)
        .text_color(p.muted_foreground)
        .child(tracked(text.to_uppercase(), WIDE))
}

/// Nothing to show: an icon that bobs in, and why.
fn empty(glyph: &'static str, text: &str, p: &Palette) -> AnyElement {
    motion::rise(
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.0))
            .py(px(48.0))
            .text_sm()
            .text_center()
            .text_color(p.muted_foreground)
            .child(motion::pop(
                div().child(icon(glyph).size(px(32.0))),
                SharedString::from(format!("mv-empty-{glyph}")),
                0.5,
                -12.0,
                Duration::from_millis(60),
            ))
            .child(text.to_owned()),
        SharedString::from(format!("mv-empty-in-{glyph}")),
        Duration::ZERO,
        8.0,
    )
    .into_any_element()
}

impl Render for ModViewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let facts = self.facts();
        let Some(server_name) = facts.name.clone().filter(|_| facts.allowed) else {
            // The server went away, or Use Mod View did.
            cx.defer_in(window, |_, _, cx| cx.emit(ModViewEvent::Close));
            return div().into_any_element();
        };
        let width = f32::from(window.viewport_size().width);
        let height = window.viewport_size().height;
        let aside = 240.0 + ((width - 1072.0).max(0.0) / 2.0);

        // The side: what it is, the server, and where you are.
        let open_badge = facts.open.filter(|n| *n > 0).map(|n| {
            crate::ui::widgets::counted(
                div()
                    .h(px(20.0))
                    .min_w(px(20.0))
                    .px(px(6.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(p.destructive)
                    .text_size(px(10.4))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(gpui_kit::white()),
                "mv-nav-open",
                n as u32,
                10.4,
                window,
                cx,
            )
        });
        let nav_row = |id: &'static str, glyph: AnyElement, label: String, on: bool, badge: Option<AnyElement>| {
            let hover = alpha(p.muted, 0.7);
            let group = SharedString::from(id);
            div()
                .id(id)
                .group(group.clone())
                .h(px(32.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .rounded(radius_lg())
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(if on { p.primary } else { p.muted_foreground })
                .cursor_pointer()
                .when(!on, |el| el.hover(move |s| s.bg(hover)))
                .active(|s| s.scale(0.97))
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-icon")))
                        .flex_none()
                        .group_hover(group, |s| s.rotate(gpui_kit::radians(-12f32.to_radians())).scale(1.1))
                        .child(glyph),
                )
                .child(div().flex_1().min_w_0().truncate().child(label))
                .when_some(badge, |el, b| el.child(b))
        };
        let on_queue = self.screen == Screen::Queue;
        let mut rows = div().relative().flex().flex_col().gap(px(2.0));
        let hl = motion::follow("mv-nav-hl", if on_queue { 0.0 } else { 34.0 }, window, cx);
        rows = rows.child(
            div().absolute().left_0().right_0().top(px(hl)).h(px(32.0)).rounded(radius_lg()).bg(alpha(p.primary, 0.15)),
        );
        rows = rows.child(
            nav_row(
                "mv-nav-cases",
                icon("inbox").size(px(16.0)).into_any_element(),
                t("desktop.modView.nav.cases"),
                on_queue,
                open_badge,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.screen = Screen::Queue;
                cx.notify();
            })),
        );
        if let Some(page) = &self.member {
            let user = self.user_of(&page.user_id);
            let name = self.name_of(&page.user_id);
            rows = rows.child(motion::slide_in(
                nav_row("mv-nav-member", avatar(user.as_ref(), 18.0, &p).into_any_element(), name, !on_queue, None)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.screen = Screen::Member;
                        cx.notify();
                    })),
                SharedString::from(format!("mv-nav-member-{}", page.user_id)),
                -10.0,
            ));
        }
        let menu = div()
            .w(px(240.0))
            .flex_none()
            .pt(px(64.0))
            .pr(px(12.0))
            .pl(px(20.0))
            .flex()
            .flex_col()
            .gap(px(20.0))
            .child(
                div()
                    .px(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(icon("shield-check").size(px(18.0)).text_color(p.primary))
                            .child(
                                div().text_lg().font_weight(FontWeight::EXTRA_BOLD).child(t("desktop.modView.title")),
                            ),
                    )
                    .child(div().text_xs().text_color(p.muted_foreground).truncate().child(server_name)),
            )
            .child(rows);

        let (title, about) = match self.screen {
            Screen::Queue => (t("desktop.modView.nav.cases"), t("desktop.modView.queue.about2")),
            Screen::Member => (t("desktop.modView.member.title"), t("desktop.modView.member.about")),
        };
        let body: AnyElement = match self.screen {
            Screen::Queue => {
                let queue = self.queue(&p, window, cx);
                let detail = self.case_detail(&facts, &p, window, cx);
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(px(24.0))
                    .child(queue)
                    .child(
                        div()
                            .id("mv-case-scroll")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_y_scroll()
                            .pr(px(8.0))
                            .pb(px(24.0))
                            .child(detail),
                    )
                    .into_any_element()
            }
            Screen::Member => self.member_page(&facts, &p, window, cx),
        };
        let content = div()
            .flex_1()
            .min_w_0()
            .h(height)
            .pl(px(40.0))
            .pr(px(40.0 + 64.0))
            .pt(px(64.0))
            .pb(px(24.0))
            .flex()
            .flex_col()
            .bg(p.chat_surface)
            .child(
                div()
                    .mb(px(20.0))
                    .child(
                        div()
                            .text_2xl()
                            .line_height(px(32.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(tracked(title, TIGHT)),
                    )
                    .child(div().mt(px(4.0)).text_sm().text_color(p.muted_foreground).child(about)),
            )
            .when_some(error_line(self.error.as_deref(), &p), |el, e| el.child(div().mb(px(12.0)).child(e)))
            .child(motion::rise(
                div().flex_1().min_h_0().flex().flex_col().child(body),
                SharedString::from(format!("mv-page-{:?}", self.screen)),
                Duration::ZERO,
                14.0,
            ));

        let (hover_bg, hover_fg, hover_ring) = (p.muted, p.foreground, alpha(p.foreground, 0.4));
        let close = div()
            .id("mv-close")
            .group("mv-close")
            .absolute()
            .top(px(64.0))
            .right(px(24.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.close(cx)))
            .child(
                div()
                    .id("mv-close-ring")
                    .size(px(40.0))
                    .rounded_full()
                    .border_2()
                    .border_color(p.border)
                    .text_color(p.muted_foreground)
                    .flex()
                    .items_center()
                    .justify_center()
                    .group_hover("mv-close", move |s| s.bg(hover_bg).text_color(hover_fg).border_color(hover_ring))
                    .active(|s| s.scale(0.9))
                    .child(
                        div()
                            .id("mv-close-x")
                            .group_hover("mv-close", |s| s.rotate(gpui_kit::radians(std::f32::consts::FRAC_PI_2)))
                            .child(icon("x").size(px(20.0))),
                    ),
            )
            .child(div().text_size(px(10.4)).font_weight(FontWeight::BOLD).text_color(p.muted_foreground).child("ESC"));

        let behind = crate::ui::backdrop::layers(&crate::ui::theme::backdrop(cx), &p, window, cx);
        let screen = div()
            .id("mod-view")
            .absolute()
            .inset_0()
            // At least the window, as server settings is, so nothing behind shows past it.
            .min_w(px(width))
            .min_h(height)
            .occlude()
            .flex()
            .bg(p.background)
            .text_color(p.foreground)
            .line_height(gpui_kit::relative(1.5))
            .when_some(behind, |el, behind| el.child(behind))
            .child(
                div()
                    .flex_none()
                    .w(px(aside))
                    .h(height)
                    .flex()
                    .justify_end()
                    .items_start()
                    .bg(p.side_surface)
                    .border_r_1()
                    .border_color(p.border)
                    .child(menu),
            )
            .child(content)
            .child(close);
        // It comes in as server settings do, from a little larger, fading up, and goes the same way.
        use gpui_kit::{Animation, AnimationExt as _};
        let (id, out) = match self.closing {
            Some(at) => (SharedString::from(format!("mv-closing-{at:?}")), true),
            None => (SharedString::from("mv-opening"), false),
        };
        let (w, h) = (width, f32::from(height));
        screen
            .with_animation(id, Animation::new(OPENING).with_easing(gpui_kit::ease_out_quint()), move |el, t| {
                let k = if out { 1.0 - t } else { t };
                let (dx, dy) = (w * 0.02 * (1.0 - k), h * 0.02 * (1.0 - k));
                el.opacity(k).top(px(-dy)).bottom(px(-dy)).left(px(-dx)).right(px(-dx))
            })
            .into_any_element()
    }
}
