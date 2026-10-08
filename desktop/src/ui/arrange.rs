//! Dragging a server's channels and categories into order in the sidebar,
//! for whoever can manage its channels, the way the web app's
//! `hooks/use-arrange.ts` does. A channel goes between any two channels,
//! into a category through its header (or above every category, out of
//! any); a category goes between the others, taking its channels along.
//!
//! The rows have fixed heights, so the sidebar notes where each one sits as
//! it draws them ([`Slot`]) and nothing is measured during a drag. The copy
//! under the pointer is GPUI's drag view ([`ChannelDrag`]); the drop line
//! glides between places on a spring, and a category it would go into lights
//! up with a ring. Letting go puts the new order on screen at once and sends
//! it to the instance ([`crate::core::arrange`]); the row that moved flashes
//! where it landed.

use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, Context, DragMoveEvent, FontWeight, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Window, div, px,
};

use crate::core::arrange::{Drop, layout_of, moved};
use crate::ui::app::FuwaApp;
use crate::ui::motion;
use crate::ui::text::{WIDE, tracked};
use crate::ui::theme::{alpha, corner};
use crate::ui::widgets::{icon, pal};

/// What's being dragged, and the copy of it that follows the pointer.
#[derive(Clone)]
pub struct ChannelDrag {
    pub key: String,
    pub server: String,
    pub id: String,
    pub category: bool,
    pub name: SharedString,
    pub glyph: &'static str,
    /// A category's channels, which move with it.
    pub count: usize,
    pub width: f32,
    /// Where the pointer took hold of the row, from its top.
    pub grab: f32,
}

impl Render for ChannelDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let card = div()
            .w(px(self.width))
            .h(px(34.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(corner(10.0))
            .bg(p.background)
            .border_1()
            .border_color(alpha(p.primary, 0.5))
            .text_color(p.foreground)
            .shadow(vec![gpui_kit::BoxShadow {
                color: alpha(p.primary, 0.3),
                offset: gpui_kit::point(px(0.0), px(10.0)),
                blur_radius: px(24.0),
                spread_radius: px(-6.0),
                inset: false,
            }])
            .when(!self.category, |el| {
                el.child(icon(self.glyph).size(px(17.0)).text_color(p.primary))
                    .child(div().flex_1().font_weight(FontWeight::BOLD).child(self.name.clone()))
            })
            .when(self.category, |el| {
                el.child(icon("folder").size(px(15.0)).text_color(p.primary))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(tracked(self.name.to_uppercase(), WIDE)),
                    )
                    .when(self.count > 0, |el| {
                        el.child(
                            div()
                                .px(px(7.0))
                                .rounded_full()
                                .bg(p.primary)
                                .text_color(p.primary_foreground)
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .child(self.count.to_string()),
                        )
                    })
            });
        // A category is a little stack: its channels ride along under it.
        let stacked = self.category && self.count > 0;
        let lifted = div()
            .relative()
            .when(stacked, |el| {
                el.child(
                    div()
                        .absolute()
                        .left(px(6.0))
                        .top(px(6.0))
                        .w(px(self.width - 12.0))
                        .h(px(34.0))
                        .rounded(corner(10.0))
                        .bg(alpha(p.primary, 0.14))
                        .border_1()
                        .border_color(alpha(p.primary, 0.25)),
                )
            })
            .child(card);
        // Picked up: it slides off below and to the side of the pointer, so
        // the drop line under the pointer stays in sight.
        // (The drag view's box sits where the row was taken, so the gap is padding.)
        div().pt(px(self.grab + 10.0)).pl(px(14.0)).child(lifted.with_animation(
            SharedString::from(format!("lift|{}", self.id)),
            Animation::new(Duration::from_millis(200)).with_easing(gpui_kit::ease_out_quint()),
            |el, t| el.opacity(0.6 + 0.4 * t).top(px(-12.0 * (1.0 - t))).left(px(-8.0 * (1.0 - t))),
        ))
    }
}

/// Where a row sits in the channel list, as drawn.
#[derive(Debug, Clone)]
pub struct Slot {
    pub category: bool,
    pub id: String,
    /// The category a channel is in, or "".
    pub parent: String,
    pub top: f32,
    pub bottom: f32,
}

/// What letting go here would do, and how the list shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    key: String,
    pub drop: Drop,
    /// The drop line's place, between rows.
    pub line: Option<f32>,
    /// The category header it would go into.
    pub ring: Option<(f32, f32)>,
}

/// Where `id` would land with the pointer at `y` in the list (its own coordinates).
pub fn mark(slots: &[Slot], layout: &crate::core::arrange::Layout, id: &str, category: bool, y: f32) -> Option<Mark> {
    let rest: Vec<&Slot> = slots.iter().filter(|s| s.id != id && !(category && s.parent == id)).collect();
    let last = rest.last()?;
    let gap_above = |n: usize| if n > 0 { (rest[n - 1].bottom + rest[n].top) / 2.0 } else { rest[n].top - 3.0 };
    let line = |key: String, drop: Drop, at: f32| Some(Mark { key, drop, line: Some(at), ring: None });

    if category {
        let headers: Vec<(usize, &&Slot)> = rest.iter().enumerate().filter(|(_, s)| s.category).collect();
        for (n, (at, h)) in headers.iter().enumerate() {
            let end = headers.get(n + 1).map(|(_, s)| s.top).unwrap_or(last.bottom);
            if y < (h.top + end) / 2.0 {
                let drop = Drop::Category { id: id.into(), before: Some(h.id.clone()) };
                return line(format!("c:{}", h.id), drop, gap_above(*at));
            }
        }
        return line("c:end".into(), Drop::Category { id: id.into(), before: None }, last.bottom + 2.0);
    }

    let channel = |parent: &str, before: Option<&str>| Drop::Channel {
        id: id.into(),
        parent: parent.into(),
        before: before.map(str::to_owned),
    };
    let first_of = |cat: &str| {
        layout
            .categories
            .iter()
            .find(|(c, _)| c == cat)
            .and_then(|(_, children)| children.iter().find(|c| *c != id).cloned())
    };
    let group_after = |n: usize| if rest[n].category { rest[n].id.clone() } else { rest[n].parent.clone() };
    let into = |s: &Slot| {
        Some(Mark {
            key: format!("into:{}", s.id),
            drop: channel(&s.id, first_of(&s.id).as_deref()),
            line: None,
            ring: Some((s.top, s.bottom)),
        })
    };
    let end_of = |n: usize| {
        let s = rest[n];
        if s.category {
            return line(format!("start:{}", s.id), channel(&s.id, first_of(&s.id).as_deref()), s.bottom + 1.0);
        }
        line(format!("after:{}", s.id), channel(&group_after(n), None), s.bottom + 1.0)
    };

    let first = rest[0];
    if y < first.top {
        return if first.category {
            line("loose:end".into(), channel("", None), gap_above(0))
        } else {
            line(format!("before:{}", first.id), channel(&first.parent, Some(&first.id)), first.top - 1.0)
        };
    }
    for (n, s) in rest.iter().enumerate() {
        let next = rest.get(n + 1);
        // A row's own height plus half the gap below it.
        let bottom = next.map_or(f32::INFINITY, |next| (s.bottom + next.top) / 2.0);
        if y >= bottom {
            continue;
        }
        let h = s.bottom - s.top;
        if !s.category {
            if y < s.top + h / 2.0 {
                return line(format!("before:{}", s.id), channel(&s.parent, Some(&s.id)), s.top - 1.0);
            }
            return match next {
                Some(next) if !next.category && next.parent == s.parent => {
                    line(format!("before:{}", next.id), channel(&s.parent, Some(&next.id)), s.bottom + 1.0)
                }
                _ => end_of(n),
            };
        }
        // A category's header: its top part is the end of what's above it.
        if y < s.top + h * 0.4 {
            if n == 0 {
                return line("loose:end".into(), channel("", None), gap_above(0));
            }
            let group = group_after(n - 1);
            return line(format!("end:{group}"), channel(&group, None), gap_above(n));
        }
        if y < s.top + h * 0.85 {
            return into(s);
        }
        return end_of(n);
    }
    end_of(rest.len() - 1)
}

impl FuwaApp {
    /// The pointer moved during a drag: works out where it would land, and
    /// redraws only when that changes.
    pub(crate) fn drag_moved(&mut self, event: &DragMoveEvent<ChannelDrag>, cx: &mut Context<Self>) {
        let drag = event.drag(cx).clone();
        let y = f32::from(event.event.position.y - event.bounds.top());
        let x = f32::from(event.event.position.x - event.bounds.left());
        let width = f32::from(event.bounds.size.width);
        // Off to the side of the list, it goes back where it was.
        let inside = (-40.0..width + 40.0).contains(&x);
        let layout = self.arrange_layout(&drag.key, &drag.server);
        let found = if inside { mark(&self.arrange_slots, &layout, &drag.id, drag.category, y) } else { None };
        let same = self.arrange.as_ref().map(|m| &m.key) == found.as_ref().map(|m| &m.key);
        if self.dragging.as_deref() != Some(drag.id.as_str()) || !same {
            self.dragging = Some(drag.id.clone());
            self.arrange = found;
            cx.notify();
        }
    }

    /// Let go over the list: puts the new order on screen and sends it.
    pub(crate) fn drag_dropped(&mut self, drag: &ChannelDrag, cx: &mut Context<Self>) {
        let mark = self.arrange.take();
        self.dragging = None;
        cx.notify();
        let Some(mark) = mark else { return };
        let layout = self.arrange_layout(&drag.key, &drag.server);
        let next = moved(&layout, &mark.drop);
        if next == layout {
            return;
        }
        self.landed = Some((drag.id.clone(), Instant::now()));
        let (core, key, server) = (self.core.clone(), drag.key.clone(), drag.server.clone());
        self.run(cx, async move { core.reorder_channels(&key, &server, next).await }, |this, result, cx| {
            if let Err(err) = result {
                this.toast("triangle-alert", "Couldn't move that".into(), err.message, None, None, cx);
            }
        });
    }

    fn arrange_layout(&self, key: &str, server: &str) -> crate::core::arrange::Layout {
        self.core
            .shared
            .read(|s| s.instance(key).and_then(|i| i.channels.get(server)).map(|c| layout_of(c)))
            .unwrap_or_default()
    }

    /// The drop line (gliding between places) and the ring on a category it
    /// would go into, drawn over the list.
    pub(crate) fn arrange_marks(
        &self,
        key: &str,
        server: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let p = pal(cx);
        let mark = self.arrange.as_ref()?;
        let line = mark.line.map(|at| {
            let at = motion::follow(SharedString::from(format!("arrange-line|{key}|{server}")), at, window, cx);
            div()
                .absolute()
                .left(px(6.0))
                .right(px(6.0))
                .top(px(at - 1.0))
                .h(px(2.0))
                .rounded_full()
                .bg(p.primary)
                .child(
                    div()
                        .absolute()
                        .left(px(-3.0))
                        .top(px(-3.0))
                        .size(px(8.0))
                        .rounded_full()
                        .border_2()
                        .border_color(p.primary)
                        .bg(p.sidebar),
                )
        });
        let ring = mark.ring.map(|(top, bottom)| {
            div()
                .absolute()
                .left(px(2.0))
                .right(px(2.0))
                .top(px(top + 8.0))
                .h(px(bottom - top - 6.0))
                .rounded(corner(10.0))
                .border_2()
                .border_color(p.primary)
                .bg(alpha(p.primary, 0.08))
                .with_animation(
                    SharedString::from(format!("arrange-ring|{}", mark.key)),
                    Animation::new(Duration::from_millis(220)).with_easing(gpui_kit::ease_out_quint()),
                    |el, t| el.opacity(t),
                )
        });
        Some(
            div()
                .absolute()
                .inset_0()
                .when_some(line, |el, l| el.child(l))
                .when_some(ring, |el, r| el.child(r))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::arrange::Layout;

    fn slot(id: &str, parent: &str, category: bool, top: f32, h: f32) -> Slot {
        Slot { category, id: id.into(), parent: parent.into(), top, bottom: top + h }
    }

    /// general, then Games (minecraft, chess), then Art (doodles).
    fn list() -> (Vec<Slot>, Layout) {
        let slots = vec![
            slot("general", "", false, 8.0, 34.0),
            slot("games", "", true, 44.0, 34.0),
            slot("minecraft", "games", false, 78.0, 34.0),
            slot("chess", "games", false, 114.0, 34.0),
            slot("art", "", true, 150.0, 34.0),
            slot("doodles", "art", false, 184.0, 34.0),
        ];
        let layout = Layout {
            loose: vec!["general".into()],
            categories: vec![
                ("games".into(), vec!["minecraft".into(), "chess".into()]),
                ("art".into(), vec!["doodles".into()]),
            ],
        };
        (slots, layout)
    }

    #[test]
    fn a_channel_lands_between_channels_or_into_a_category() {
        let (slots, layout) = list();
        let at = |y| mark(&slots, &layout, "chess", false, y).unwrap().drop;
        let to = |parent: &str, before: Option<&str>| Drop::Channel {
            id: "chess".into(),
            parent: parent.into(),
            before: before.map(str::to_owned),
        };
        assert_eq!(at(10.0), to("", Some("general")), "the top half of a row goes before it");
        assert_eq!(at(35.0), to("", None), "the bottom half of the last loose channel: the end of the loose ones");
        assert_eq!(at(165.0), to("art", Some("doodles")), "the middle of a header: into it, first");
        assert!(mark(&slots, &layout, "chess", false, 165.0).unwrap().ring.is_some());
        assert_eq!(at(200.0), to("art", Some("doodles")), "the top half of doodles");
        assert_eq!(at(210.0), to("art", None), "past the last channel");
    }

    #[test]
    fn a_category_lands_between_categories() {
        let (slots, layout) = list();
        let at = |y| mark(&slots, &layout, "art", true, y).unwrap().drop;
        assert_eq!(at(50.0), Drop::Category { id: "art".into(), before: Some("games".into()) });
        assert_eq!(at(140.0), Drop::Category { id: "art".into(), before: None });
    }
}
