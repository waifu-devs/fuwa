//! A person's page in Mod View: who they are here (roles, when they joined,
//! how old their account is, a time-out, the rules, a ban), how many cases
//! they've had, the moderators' notes about them, and beside it their
//! messages, their cases and what moderators did to them, drawing only the
//! rows in sight.

use super::*;
use crate::core::moderation::timed_out_until;

/// How tall a row is in the person's lists, its gap included.
const ROW: f32 = 76.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum MemberTab {
    #[default]
    Messages,
    Cases,
    History,
}

pub(super) struct MemberPage {
    pub(super) user_id: String,
    overview: Option<pb::GetMemberOverviewResponse>,
    tab: MemberTab,
    messages: Vec<pb::Message>,
    messages_more: bool,
    messages_asked: bool,
    cases: Vec<String>,
    cases_more: bool,
    cases_asked: bool,
    loading: bool,
    scroll: UniformListScrollHandle,
    saving_note: bool,
    deleting: Option<String>,
    /// Whether they were a member when last looked, so leaving or a ban reads the page again.
    present: Option<bool>,
}

impl MemberPage {
    pub(super) fn loading_more(&self) -> bool {
        self.loading
    }
}

impl ModViewView {
    /// Opens someone's page.
    pub(crate) fn open_member(&mut self, user_id: String, _window: &mut Window, cx: &mut Context<Self>) {
        self.screen = Screen::Member;
        self.error = None;
        if self.member.as_ref().is_some_and(|m| m.user_id == user_id) {
            cx.notify();
            return;
        }
        self.member = Some(MemberPage {
            user_id,
            overview: None,
            tab: MemberTab::Messages,
            messages: Vec::new(),
            messages_more: false,
            messages_asked: false,
            cases: Vec::new(),
            cases_more: false,
            cases_asked: false,
            loading: false,
            scroll: UniformListScrollHandle::new(),
            saving_note: false,
            deleting: None,
            present: None,
        });
        self.load_overview(cx);
        self.load_tab(false, cx);
    }

    fn load_overview(&mut self, cx: &mut Context<Self>) {
        let Some(page) = &self.member else { return };
        let (core, key, sid, uid) = (self.core.clone(), self.key.clone(), self.server.clone(), page.user_id.clone());
        let asked = uid.clone();
        self.run(cx, async move { core.member_overview(&key, &sid, &uid).await }, move |this, result, cx| {
            let Some(page) = this.member.as_mut().filter(|m| m.user_id == asked) else { return };
            match result {
                Ok(res) => page.overview = Some(res),
                Err(err) => this.error = Some(err.message),
            }
            cx.notify();
        });
    }

    /// The store changed: someone who left or came back has their page read again.
    pub(super) fn member_changed(&mut self, cx: &mut Context<Self>) {
        let Some(page) = &self.member else { return };
        let present = self.core.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.members.get(&self.server))
                .is_some_and(|l| l.iter().any(|m| m.user.as_ref().is_some_and(|u| u.id == page.user_id)))
        });
        let was = page.present;
        if let Some(page) = self.member.as_mut() {
            page.present = Some(present);
        }
        if was.is_some_and(|w| w != present) {
            self.load_overview(cx);
        }
    }

    fn pick_member_tab(&mut self, tab: MemberTab, cx: &mut Context<Self>) {
        let Some(page) = self.member.as_mut() else { return };
        if page.tab == tab {
            return;
        }
        page.tab = tab;
        page.scroll = UniformListScrollHandle::new();
        self.load_tab(false, cx);
        cx.notify();
    }

    /// The open tab's first page (once), or its next with `older`.
    fn load_tab(&mut self, older: bool, cx: &mut Context<Self>) {
        let Some(page) = self.member.as_mut() else { return };
        if page.loading {
            return;
        }
        let (core, key, sid, uid) = (self.core.clone(), self.key.clone(), self.server.clone(), page.user_id.clone());
        match page.tab {
            MemberTab::History => {}
            MemberTab::Messages => {
                if (older && !page.messages_more) || (!older && page.messages_asked) {
                    return;
                }
                page.messages_asked = true;
                page.loading = true;
                let before =
                    if older { page.messages.last().map(|m| m.id.clone()).unwrap_or_default() } else { String::new() };
                let asked = uid.clone();
                self.run(
                    cx,
                    async move { core.member_messages(&key, &sid, &uid, &before).await },
                    move |this, result, cx| {
                        let Some(page) = this.member.as_mut().filter(|m| m.user_id == asked) else { return };
                        page.loading = false;
                        match result {
                            Ok(more) => {
                                page.messages.extend(more.messages);
                                page.messages_more = more.more;
                            }
                            Err(err) => this.error = Some(err.message),
                        }
                        cx.notify();
                    },
                );
            }
            MemberTab::Cases => {
                if (older && !page.cases_more) || (!older && page.cases_asked) {
                    return;
                }
                page.cases_asked = true;
                page.loading = true;
                let before = if older { page.cases.last().cloned().unwrap_or_default() } else { String::new() };
                let asked = uid.clone();
                self.run(
                    cx,
                    async move { core.mod_cases(&key, &sid, Tab::All, &uid, &before, mv::PAGE).await },
                    move |this, result, cx| {
                        let Some(page) = this.member.as_mut().filter(|m| m.user_id == asked) else { return };
                        page.loading = false;
                        match result {
                            Ok(more) => {
                                page.cases.extend(more.ids);
                                page.cases_more = more.more;
                            }
                            Err(err) => this.error = Some(err.message),
                        }
                        cx.notify();
                    },
                );
            }
        }
    }

    fn add_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = self.member.as_mut() else { return };
        let text = self.note.read(cx).value().trim().to_owned();
        if page.saving_note || text.is_empty() || text.chars().count() > mv::MAX_NOTE {
            return;
        }
        page.saving_note = true;
        let (core, key, sid, uid) = (self.core.clone(), self.key.clone(), self.server.clone(), page.user_id.clone());
        self.run(cx, async move { core.add_mod_note(&key, &sid, &uid, &text).await }, |this, result, cx| {
            if let Some(page) = this.member.as_mut() {
                page.saving_note = false;
            }
            match result {
                Ok(()) => this.error = None,
                Err(err) => this.error = Some(err.message),
            }
            cx.notify();
        });
        self.note.update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    fn delete_note(&mut self, note_id: String, cx: &mut Context<Self>) {
        let Some(page) = self.member.as_mut() else { return };
        if page.deleting.is_some() {
            return;
        }
        page.deleting = Some(note_id.clone());
        let (core, key, sid, uid) = (self.core.clone(), self.key.clone(), self.server.clone(), page.user_id.clone());
        self.run(cx, async move { core.delete_mod_note(&key, &sid, &uid, &note_id).await }, |this, result, cx| {
            if let Some(page) = this.member.as_mut() {
                page.deleting = None;
            }
            if let Err(err) = result {
                this.error = Some(err.message);
            }
            cx.notify();
        });
        cx.notify();
    }

    pub(super) fn member_page(
        &mut self,
        facts: &Facts,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(page) = &self.member else { return empty("user-round-search", "", p) };
        let user_id = page.user_id.clone();
        let Some(overview) = page.overview.clone() else {
            return crate::ui::server_settings::shimmer_rows(4, p).into_any_element();
        };
        let left = self.member_side(&user_id, &overview, facts, p, window, cx);
        let right = self.member_lists(&user_id, &overview, p, window, cx);
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .gap(px(24.0))
            .child(
                div()
                    .id("mv-member-side")
                    .w(px(340.0))
                    .flex_none()
                    .h_full()
                    .overflow_y_scroll()
                    .pr(px(8.0))
                    .pb(px(24.0))
                    .child(left),
            )
            .child(right)
            .into_any_element()
    }

    /// Who they are, where they stand, the counts and the notes.
    fn member_side(
        &mut self,
        user_id: &str,
        o: &pb::GetMemberOverviewResponse,
        facts: &Facts,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Their membership as the store has it now (time-outs and roles change live), else as read.
        let (member, roles) = self.core.shared.read(|s| {
            let i = s.instance(&self.key);
            let member = i
                .and_then(|i| i.members.get(&self.server))
                .and_then(|l| l.iter().find(|m| m.user.as_ref().is_some_and(|u| u.id == user_id)).cloned());
            let roles = i.and_then(|i| i.roles.get(&self.server).cloned()).unwrap_or_default();
            (member, roles)
        });
        let member = member.or_else(|| o.member.clone());
        let user = o.user.clone().or_else(|| self.user_of(user_id));
        let name = self.name_of(user_id);
        let now = now_ms();

        let mut col = div().flex().flex_col().gap(px(16.0));
        col = col.child(
            div()
                .flex()
                .items_center()
                .gap(px(14.0))
                .child(motion::pop(
                    avatar(user.as_ref(), 64.0, p),
                    SharedString::from(format!("mv-member-avatar-{user_id}")),
                    0.7,
                    -8.0,
                    Duration::ZERO,
                ))
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
                                .child(
                                    div().text_xl().font_weight(FontWeight::EXTRA_BOLD).truncate().child(name.clone()),
                                )
                                .when(is_agent(user.as_ref()), |el| el.child(app_badge("mv-member-agent", "AGENT", p))),
                        )
                        .child(
                            div()
                                .text_sm()
                                .truncate()
                                .text_color(p.muted_foreground)
                                .child(format!("@{}", user.as_ref().map(|u| u.username.as_str()).unwrap_or(""))),
                        )
                        .when_some(member.as_ref().filter(|m| !m.nickname.is_empty()), |el, m| {
                            el.child(div().text_xs().text_color(p.muted_foreground).child(t_with(
                                "desktop.modView.member.nickname",
                                &[("nickname", Arg::Str(&m.nickname))],
                            )))
                        }),
                ),
        );

        // Where they stand: timed out, still to agree to the rules, banned, or gone.
        let amber = crate::ui::moderate::amber(p);
        let mut standing = div().flex().flex_wrap().gap(px(6.0));
        if let Some(until) = member.as_ref().and_then(|m| timed_out_until(m, now)) {
            standing = standing.child(chip(
                t_with("desktop.modView.member.timedOut", &[("time", Arg::Str(&crate::ui::moderate::stamp(until)))]),
                amber,
            ));
        }
        if member.as_ref().is_some_and(|m| m.pending) {
            standing = standing.child(chip(t("desktop.modView.member.pending"), p.muted_foreground.into()));
        }
        if o.ban.is_some() {
            standing = standing.child(chip(t("desktop.modView.member.banned"), p.destructive.into()));
        } else if member.is_none() {
            standing = standing.child(chip(t("desktop.modView.member.notMember"), p.muted_foreground.into()));
        }
        col = col.child(standing);
        if let Some(ban) = &o.ban {
            let by = self.name_of(&ban.banned_by_id);
            col = col.child(
                div()
                    .p(px(12.0))
                    .rounded(radius_xl())
                    .bg(alpha(p.destructive, 0.08))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(div().text_sm().font_weight(FontWeight::BOLD).child(t_with(
                        "desktop.modView.member.bannedBy",
                        &[
                            ("name", Arg::Str(&by)),
                            ("when", Arg::Str(&crate::ui::text::ago(ms_of(ban.created_at.as_ref()), now))),
                        ],
                    )))
                    .child(div().text_sm().text_color(p.muted_foreground).child(if ban.reason.is_empty() {
                        t("desktop.modView.member.noReason")
                    } else {
                        ban.reason.clone()
                    })),
            );
        }

        // The facts, two to a row.
        let (n, unit) = mv::age(ms_of(o.account_created_at.as_ref()), now);
        let fact = |label: String, value: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .p(px(10.0))
                .rounded(radius_xl())
                .bg(alpha(p.muted, 0.5))
                .child(div().text_xs().text_color(p.muted_foreground).child(label))
                .child(div().text_sm().font_weight(FontWeight::BOLD).child(value))
        };
        let joined = member
            .as_ref()
            .and_then(|m| m.joined_at.as_ref())
            .map(|t| crate::ui::text::day(ms_of(Some(t))))
            .unwrap_or_else(|| "—".to_owned());
        let uid = user_id.to_owned();
        let cases = div()
            .id("mv-member-cases")
            .cursor_pointer()
            .hover(|s| s.opacity(0.8))
            .on_click(cx.listener(move |this, _, _, cx| this.show_queue(Some(uid.clone()), cx)))
            .child(fact(
                t("desktop.modView.member.cases"),
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(t_with(
                        "desktop.modView.member.caseCounts",
                        &[("open", Arg::Num(i64::from(o.open_cases))), ("total", Arg::Num(i64::from(o.total_cases)))],
                    ))
                    .child(icon("chevron-right").size(px(14.0)))
                    .into_any_element(),
            ));
        col = col.child(
            div()
                .grid()
                .grid_cols(2)
                .gap(px(8.0))
                .child(fact(t("desktop.modView.member.joined"), div().child(joined).into_any_element()))
                .child(fact(
                    t("desktop.modView.member.accountAge"),
                    div().child(t_with(unit, &[("count", Arg::Num(n))])).into_any_element(),
                ))
                .child(fact(
                    t("desktop.modView.member.messages"),
                    motion::count("mv-member-messages", o.message_count.max(0) as u64, None, 14.0, window, cx),
                ))
                .child(cases),
        );

        // Their roles.
        if let Some(m) = &member {
            let held: Vec<&pb::Role> = roles.iter().filter(|r| m.role_ids.contains(&r.id)).collect();
            if !held.is_empty() {
                let mut chips = div().flex().flex_wrap().gap(px(6.0));
                for role in held {
                    chips = chips.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .px(px(8.0))
                            .h(px(24.0))
                            .rounded_full()
                            .border_1()
                            .border_color(p.border)
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .child(crate::ui::profile_card::role_dot(role, p))
                            .child(role.name.clone()),
                    );
                }
                col = col.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(section_title(t("desktop.modView.member.roles"), p))
                        .child(chips),
                );
            }
        }

        if user_id != facts.me
            && let Some(row) = self.moderation_buttons(user_id, "mv-member", p, cx)
        {
            col = col.child(row);
        }

        // The notes: write one, and those written, newest first.
        let notes = self.core.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.mod_view.get(&self.server))
                .and_then(|m| m.notes.get(user_id).cloned())
                .unwrap_or_default()
        });
        let typed = self.note.read(cx).value().chars().count();
        let saving = self.member.as_ref().is_some_and(|m| m.saving_note);
        let deleting = self.member.as_ref().and_then(|m| m.deleting.clone());
        let mut notes_col = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(section_title(t("desktop.modView.member.notes"), p))
            .child(div().text_xs().text_color(p.muted_foreground).child(t("desktop.modView.member.notesAbout")))
            .child(
                div()
                    .min_h(px(60.0))
                    .rounded(radius_xl())
                    .border_1()
                    .border_color(p.border)
                    .px(px(12.0))
                    .text_sm()
                    .child(
                        gpui_kit::component::input::Textarea::new(&self.note)
                            .appearance(false)
                            .text_size(px(14.0))
                            .line_height(px(20.0)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(if typed > mv::MAX_NOTE { p.destructive } else { p.muted_foreground })
                            .child(format!("{typed}/{}", mv::MAX_NOTE)),
                    )
                    .child(
                        button(
                            "mv-note-add",
                            t("desktop.modView.member.addNote"),
                            Some("notebook-pen"),
                            Look::Primary,
                            true,
                            p,
                        )
                        .rounded(radius_xl())
                        .when(typed == 0 || typed > mv::MAX_NOTE || saving, |el| el.opacity(0.5))
                        .when(saving, |el| el.child(spinner("mv-note-spin", 14.0, window)))
                        .on_click(cx.listener(|this, _, window, cx| this.add_note(window, cx))),
                    ),
            );
        if notes.is_empty() {
            notes_col = notes_col
                .child(div().text_sm().text_color(p.muted_foreground).child(t("desktop.modView.member.noNotes")));
        }
        for note in notes {
            let author = self.user_of(&note.author_id);
            let mine = mv::can_delete_note(&note, &facts.me, facts.manage);
            let id = note.id.clone();
            let gone = deleting.as_deref() == Some(note.id.as_str());
            notes_col = notes_col.child(motion::rise(
                div()
                    .flex()
                    .gap(px(10.0))
                    .p(px(10.0))
                    .rounded(radius_xl())
                    .bg(alpha(p.muted, 0.5))
                    .when(gone, |el| el.opacity(0.5))
                    .child(avatar(author.as_ref(), 24.0, p))
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
                                    .text_xs()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::BOLD)
                                            .truncate()
                                            .child(self.name_of(&note.author_id)),
                                    )
                                    .child(
                                        div()
                                            .text_color(p.muted_foreground)
                                            .child(crate::ui::text::ago(ms_of(note.created_at.as_ref()), now)),
                                    ),
                            )
                            .child(div().text_sm().child(note.content.clone())),
                    )
                    .when(mine, |el| {
                        el.child(
                            crate::ui::widgets::icon_button_in(
                                SharedString::from(format!("mv-note-del-{}", note.id)),
                                "trash",
                                p,
                                p.destructive,
                            )
                            .size(px(26.0))
                            .on_click(cx.listener(move |this, _, _, cx| this.delete_note(id.clone(), cx))),
                        )
                    }),
                SharedString::from(format!("mv-note-{}", note.id)),
                Duration::ZERO,
                6.0,
            ));
        }
        col = col.child(notes_col);
        col.into_any_element()
    }

    /// Their messages, cases and history, as tabs over one list.
    fn member_lists(
        &mut self,
        user_id: &str,
        o: &pb::GetMemberOverviewResponse,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(page) = &self.member else { return div().into_any_element() };
        let tab = page.tab;
        let tabs_list = [
            (MemberTab::Messages, t("desktop.modView.member.messagesTab")),
            (MemberTab::Cases, t("desktop.modView.member.casesTab")),
            (MemberTab::History, t("desktop.modView.member.historyTab")),
        ];
        let at = tabs_list.iter().position(|(k, _)| *k == tab).unwrap_or(0);
        let width = 110.0;
        let x = motion::follow("mv-member-tab-x", at as f32 * width, window, cx);
        let mut tabs =
            div().relative().flex().flex_none().p(px(3.0)).rounded(radius_xl()).bg(alpha(p.muted, 0.7)).child(
                div()
                    .absolute()
                    .top(px(3.0))
                    .left(px(3.0 + x))
                    .w(px(width))
                    .h(px(30.0))
                    .rounded(radius_lg())
                    .bg(p.card),
            );
        for (k, label) in tabs_list {
            let fg = p.foreground;
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("mv-member-tab-{k:?}")))
                    .relative()
                    .w(px(width))
                    .h(px(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(if k == tab { p.foreground } else { p.muted_foreground })
                    .cursor_pointer()
                    .hover(move |s| s.text_color(fg))
                    .active(|s| s.scale(0.96))
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_member_tab(k, cx)))
                    .child(label),
            );
        }

        enum Row {
            Message(Box<pb::Message>),
            Case(Box<pb::ModCase>),
            History(pb::AuditEntry),
            More,
        }
        let rows: Vec<Row> = match tab {
            MemberTab::Messages => {
                let mut rows: Vec<Row> = page.messages.iter().cloned().map(|m| Row::Message(Box::new(m))).collect();
                if page.messages_more {
                    rows.push(Row::More);
                }
                rows
            }
            MemberTab::Cases => {
                let cases = self.core.shared.read(|s| {
                    s.instance(&self.key)
                        .and_then(|i| i.mod_view.get(&self.server))
                        .map(|m| mv::queue(m, Tab::All, Some(user_id), &page.cases, page.cases_more))
                        .unwrap_or_default()
                });
                let mut rows: Vec<Row> = cases.into_iter().map(|c| Row::Case(Box::new(c))).collect();
                if page.cases_more {
                    rows.push(Row::More);
                }
                rows
            }
            MemberTab::History => o.history.iter().cloned().map(Row::History).collect(),
        };
        let loading = page.loading;
        let list: AnyElement = if rows.is_empty() && loading {
            crate::ui::server_settings::shimmer_rows(4, p).into_any_element()
        } else if rows.is_empty() {
            let text = match tab {
                MemberTab::Messages => t("desktop.modView.member.noMessages"),
                MemberTab::Cases => t("desktop.modView.member.noCases"),
                MemberTab::History => t("desktop.modView.member.noHistory"),
            };
            empty("inbox", &text, p)
        } else {
            let rows = std::rc::Rc::new(rows);
            let people: crate::core::server_admin::People = o.users.iter().map(|u| (u.id.clone(), u.clone())).collect();
            let channels = self.core.shared.read(|s| {
                s.instance(&self.key).and_then(|i| i.channels.get(&self.server).cloned()).unwrap_or_default()
            });
            let scroll = page.scroll.clone();
            uniform_list(
                "mv-member-rows",
                rows.len(),
                cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                    let p = pal(cx);
                    range
                        .map(|n| match &rows[n] {
                            Row::Message(m) => this.message_row(m, n, &p, cx),
                            Row::Case(c) => {
                                let id = c.id.clone();
                                let subject = c.subject_id.clone();
                                div()
                                    .id(SharedString::from(format!("mv-member-case-{id}")))
                                    .h(px(CASE_ROW))
                                    .pb(px(8.0))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.show_queue(Some(subject.clone()), cx);
                                        this.pick_case(id.clone(), window, cx);
                                    }))
                                    .child(this.case_row(c, false, &p, cx))
                                    .into_any_element()
                            }
                            Row::History(e) => history_row(e, &people, &channels, n, &p),
                            Row::More => {
                                this.more_row("mv-member-more", window, cx, |this, cx| this.load_tab(true, cx))
                            }
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&scroll)
            .flex_1()
            .min_h_0()
            .into_any_element()
        };
        div().flex_1().min_w_0().h_full().flex().flex_col().gap(px(12.0)).child(tabs).child(list).into_any_element()
    }

    /// One of their messages: where and when, a line of it, and a jump there.
    fn message_row(&self, m: &pb::Message, n: usize, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let place = self.channel_name(&m.channel_id).map(|c| format!("#{c}")).unwrap_or_default();
        let text = if m.content.is_empty() && !m.attachments.is_empty() {
            t_with("desktop.modView.member.files", &[("count", Arg::Num(m.attachments.len() as i64))])
        } else {
            one_line(&m.content)
        };
        let message = m.clone();
        let hover = alpha(p.muted, 0.6);
        let group = SharedString::from(format!("mv-msg-{}", m.id));
        div()
            .h(px(ROW))
            .pb(px(8.0))
            .child(
                div()
                    .id(group.clone())
                    .group(group.clone())
                    .size_full()
                    .px(px(12.0))
                    .py(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(radius_xl())
                    .border_1()
                    .border_color(alpha(p.border, 0.6))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .active(|s| s.scale(0.99))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(ModViewEvent::Jump(Box::new(message.clone())))))
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
                                    .gap(px(6.0))
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .child(div().font_weight(FontWeight::BOLD).child(place))
                                    .child(crate::ui::text::when(ms_of(m.created_at.as_ref())))
                                    .when(!m.thread_id.is_empty(), |el| el.child(t("desktop.modView.case.inThread"))),
                            )
                            .child(div().text_sm().truncate().child(text)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("mv-msg-go-{}", m.id)))
                            .flex_none()
                            .opacity(0.4)
                            .group_hover(group, |s| s.opacity(1.0).translate_x(px(2.0)))
                            .child(icon("arrow-up-right").size(px(16.0))),
                    ),
            )
            .with_animation_rise(n)
    }
}

/// What a moderator did to them, as the audit log says it.
fn history_row(
    e: &pb::AuditEntry,
    people: &crate::core::server_admin::People,
    channels: &[pb::Channel],
    n: usize,
    p: &Palette,
) -> AnyElement {
    let action = pb::AuditAction::try_from(e.action).unwrap_or(pb::AuditAction::Unspecified);
    let (glyph, bg, fg, _) = crate::ui::server_settings::audit_kind(action, p);
    let said = crate::ui::server_settings::sentence(e, people, channels);
    div()
        .h(px(ROW))
        .pb(px(8.0))
        .child(
            div()
                .size_full()
                .px(px(12.0))
                .py(px(8.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .rounded(radius_xl())
                .bg(alpha(p.muted, 0.4))
                .child(
                    div()
                        .size(px(32.0))
                        .flex_none()
                        .rounded(radius_lg())
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(bg)
                        .text_color(fg)
                        .child(icon(glyph).size(px(16.0))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(
                            div()
                                .text_sm()
                                .max_h(px(20.0))
                                .overflow_hidden()
                                .child(crate::ui::text::markdown(SharedString::from(format!("mv-h-{}", e.id)), said)),
                        )
                        .child(div().text_xs().truncate().text_color(p.muted_foreground).child(
                            if e.reason.is_empty() {
                                crate::ui::text::when(ms_of(e.created_at.as_ref()))
                            } else {
                                format!("{} · {}", crate::ui::text::when(ms_of(e.created_at.as_ref())), e.reason)
                            },
                        )),
                ),
        )
        .with_animation_rise(n)
}

/// Rows rise into place a beat after the one above (only the first few wait).
trait RiseRow {
    fn with_animation_rise(self, n: usize) -> AnyElement;
}

impl RiseRow for gpui_kit::Div {
    fn with_animation_rise(self, n: usize) -> AnyElement {
        motion::rise(
            self,
            SharedString::from(format!("mv-row-in-{n}")),
            Duration::from_millis(25 * n.min(8) as u64),
            6.0,
        )
        .into_any_element()
    }
}
