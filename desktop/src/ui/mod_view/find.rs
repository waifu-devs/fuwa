//! Finding someone in Mod View, as the web's `modview/FindPeople.tsx`:
//! everyone the server knows by name, username, nickname or id, members
//! first, and people who left or were banned too, which the member list
//! can't reach. Picking one opens their page.

use gpui_kit::Focusable as _;

use super::*;

/// How long typing rests before the instance is asked.
const REST: Duration = Duration::from_millis(200);

#[derive(Default)]
pub(super) struct Found {
    /// What's typed, as asked (no @), bumped on every change so older answers are dropped.
    asked: String,
    typed: u64,
    /// What came back, and for which query.
    people: Option<(String, Vec<pb::ModPerson>)>,
    error: Option<String>,
}

impl ModViewView {
    pub(super) fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.screen = Screen::Find;
        self.error = None;
        self.finder.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    /// The search changed: ask once typing rests.
    pub(super) fn find_typed(&mut self, cx: &mut Context<Self>) {
        let typed = self.finder.read(cx).value().to_string();
        let asked = typed.trim().trim_start_matches('@').trim().to_owned();
        let found = &mut self.found;
        if asked == found.asked {
            return;
        }
        found.asked.clone_from(&asked);
        found.typed += 1;
        found.error = None;
        let turn = found.typed;
        cx.notify();
        if asked.is_empty() {
            found.people = None;
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REST).await;
            let _ = this.update(cx, |this, cx| {
                if this.found.typed != turn {
                    return;
                }
                let (core, key, sid) = (this.core.clone(), this.key.clone(), this.server.clone());
                let query = asked.clone();
                this.run(cx, async move { core.find_people(&key, &sid, &query).await }, move |this, result, cx| {
                    if this.found.typed != turn {
                        return;
                    }
                    match result {
                        Ok(people) => this.found.people = Some((asked, people)),
                        Err(err) => this.found.error = Some(err.message),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(super) fn find_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.finder.read(cx).focus_handle(cx).is_focused(window);
        let found = &self.found;
        let waiting = !found.asked.is_empty()
            && found.error.is_none()
            && found.people.as_ref().is_none_or(|(q, _)| *q != found.asked);
        let field = crate::ui::dialogs::focus_ring(
            div()
                .h(px(44.0))
                .px(px(12.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .rounded(radius_xl())
                .border_1()
                .border_color(p.border)
                .bg(p.card)
                .text_sm()
                .child(icon("search").size(px(16.0)).text_color(p.muted_foreground))
                .child(div().flex_1().min_w_0().child(Input::new(&self.finder).appearance(false)))
                .when(waiting, |el| el.child(spinner("mv-find-spinner", 14.0, window))),
            focused,
            p,
        );
        let mut col = div().max_w(px(672.0)).flex().flex_col().gap(px(12.0)).child(field);
        if let Some(err) = error_line(found.error.as_deref(), p) {
            col = col.child(err);
        }
        let shown = found.people.as_ref().filter(|_| !found.asked.is_empty()).map(|(_, people)| people.clone());
        match shown {
            _ if found.asked.is_empty() => {
                return col.child(empty("user-round-search", &t("chattools.modview.findStart"), p)).into_any_element();
            }
            Some(people) if people.is_empty() && !waiting => {
                col = col.child(empty("search-x", &t("chattools.modview.findNone"), p));
            }
            Some(people) => {
                let rows =
                    people.iter().enumerate().filter_map(|(n, person)| self.person_row(person, n, waiting, p, cx));
                col = col.child(div().flex().flex_col().gap(px(6.0)).children(rows));
            }
            None => {}
        }
        div()
            .id("mv-find-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .pr(px(8.0))
            .pb(px(24.0))
            .child(col)
            .into_any_element()
    }

    /// One person found: who they are here, and whether they're still in or banned.
    fn person_row(
        &self,
        person: &pb::ModPerson,
        n: usize,
        waiting: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let user = person.user.clone()?;
        let id = user.id.clone();
        let real = crate::core::store::user_name(&user);
        let name = if person.nickname.is_empty() { real.clone() } else { person.nickname.clone() };
        let mut handle = format!("@{}", user.username);
        if !person.nickname.is_empty() && person.nickname != real {
            handle = format!("{handle} · {real}");
        }
        let tag = if person.banned {
            Some(chip(t("desktop.modView.member.banned"), p.destructive.into()).child(icon("ban").size(px(11.0))))
        } else if !person.member {
            Some(chip(t("desktop.modView.member.notMember"), p.muted_foreground.into()))
        } else {
            None
        };
        let group = SharedString::from(format!("mv-person-{id}"));
        let hover = (alpha(p.primary, 0.3), alpha(p.muted, 0.3));
        let row = div()
            .id(group.clone())
            .group(group.clone())
            .p(px(12.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .rounded(radius_2xl())
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.background, 0.4))
            .when(waiting, |el| el.opacity(0.6))
            .cursor_pointer()
            .hover(move |s| s.border_color(hover.0).bg(hover.1))
            .active(|s| s.scale(0.99))
            .on_click(cx.listener(move |this, _, window, cx| this.open_member(id.clone(), window, cx)))
            .child(
                div()
                    .id(SharedString::from(format!("mv-person-avatar-{}", user.id)))
                    .flex_none()
                    .when(!person.member, |el| el.opacity(0.6))
                    .group_hover(group, |s| s.scale(1.1))
                    .child(avatar(Some(&user), 36.0, p)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(div().text_sm().font_weight(FontWeight::BOLD).truncate().child(name))
                            .when(is_agent(Some(&user)), |el| {
                                el.child(app_badge(
                                    SharedString::from(format!("mv-person-agent-{}", user.id)),
                                    "AGENT",
                                    p,
                                ))
                            }),
                    )
                    .child(div().text_xs().text_color(p.muted_foreground).truncate().child(handle)),
            )
            .when_some(tag, |el, tag| el.child(tag));
        Some(
            motion::rise(
                row,
                SharedString::from(format!("mv-person-in-{}", user.id)),
                Duration::from_millis(25 * n.min(14) as u64),
                8.0,
            )
            .into_any_element(),
        )
    }
}
