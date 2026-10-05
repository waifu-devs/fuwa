//! Polls in a channel, as the web app's `Poll.tsx` (the card under a
//! message: voting, results, ending it, who voted) and `PollEditor.tsx`
//! (making one, from the composer's chart button).
//!
//! Answers are buttons: pick one (or several) and the bars grow to
//! everyone's results, your picks checked. Whether votes are anonymous shows
//! before you vote; an anonymous poll keeps its counts hidden until it ends.
//! The creator and moderators can end it early; an ended poll keeps its
//! results, the winning answer crowned.

use std::collections::{HashMap, HashSet};
use std::hash::Hash as _;
use std::time::Duration;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    WeakEntity, Window, div, px,
};
use prost::Message as _;

use crate::core::polls::{self, ANSWER, DURATIONS, Draft, MAX_ANSWERS, MIN_ANSWERS, QUESTION};
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp, Target};
use crate::ui::emoji::{self, Catalog, Choice, InColor as _};
use crate::ui::mentions::Look;
use crate::ui::motion;
use crate::ui::overlay::scrim;
use crate::ui::server_settings::roles::switch;
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{avatar, card, error_line, icon, icon_button, pal, primary_button, soft_button};

/// What the window keeps about polls between frames.
#[derive(Default)]
pub struct PollState {
    /// Answers picked whose vote hasn't landed yet, by message: shown meanwhile.
    pub picking: HashMap<String, Vec<u32>>,
    /// The newest pick waiting for the one in flight, and where it goes.
    want: HashMap<String, (Place, Vec<u32>)>,
    sending: HashSet<String>,
    /// Polls whose results you asked to see before voting.
    pub peek: HashSet<String>,
    /// The poll asking "End it now?", and those being ended.
    pub confirm_end: Option<String>,
    pub ending: HashSet<String>,
    /// A timer is set to redraw running polls' time left.
    ticking: bool,
    pub editor: Option<Editor>,
    pub voters: Option<Voters>,
}

/// Where a poll is: an instance, a server and a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Place {
    key: String,
    server: String,
    channel: String,
}

/// A poll as its card draws it, worked out with the rest of its message.
#[derive(Debug, Clone, PartialEq)]
pub struct PollCard {
    pub poll: pb::Poll,
    /// Your answers, or those being sent.
    pub chosen: Vec<u32>,
    pub closed: bool,
    /// The counts show: you voted, it's over, or you peeked (never while an anonymous one runs).
    pub results: bool,
    /// Anonymous and still running: no counts yet.
    pub hidden: bool,
    pub can_vote: bool,
    pub can_end: bool,
    pub busy: bool,
    pub confirming: bool,
    pub ending: bool,
    /// How many voted, and how it runs: "Ends in…", "Ended…".
    pub status: String,
    /// Each answer's server emoji picture, if it has one.
    pub pictures: Vec<Option<String>>,
    /// Each answer's bar as it was last drawn, to grow from.
    pub from: Vec<f32>,
}

impl PollCard {
    #[allow(clippy::too_many_arguments)]
    pub fn of(
        poll: &pb::Poll,
        message_id: &str,
        mine: bool,
        can_vote: bool,
        moderator: bool,
        state: &PollState,
        look: &Look,
        now: i64,
    ) -> Self {
        let closed = polls::closed(poll, now);
        let chosen = state.picking.get(message_id).cloned().unwrap_or_else(|| poll.my_answer_ids.clone());
        let voted = !poll.my_answer_ids.is_empty();
        let hidden = poll.anonymous && !closed;
        let results = !hidden && (voted || closed || state.peek.contains(message_id));
        let total: i64 = poll.answers.iter().map(|a| a.votes).sum();
        let ends_at = polls::ends_at_ms(poll);
        let status = if closed && poll.anonymous && total == 0 && poll.voters > 0 {
            "Counting the votes…".to_owned()
        } else if let Some(at) = &poll.ended_at {
            format!("Ended {}", lower_day(&crate::ui::text::when(crate::ui::text::ms_of(Some(at)))))
        } else if closed {
            format!("Ended {}", lower_day(&crate::ui::text::when(ends_at)))
        } else {
            let left = if ends_at > 0 { polls::left(ends_at - now) } else { "Runs until it's ended".to_owned() };
            if hidden { format!("{left} · results show at the end") } else { left }
        };
        let pictures = poll
            .answers
            .iter()
            .map(|a| {
                let (_, id, _) = emoji::token_at(&a.emoji)?;
                look.emojis.get(id).or_else(|| look.emojis.get(&id.to_uppercase())).cloned()
            })
            .collect();
        Self {
            poll: poll.clone(),
            chosen,
            closed,
            results,
            hidden,
            can_vote: can_vote && !closed,
            can_end: !closed && (mine || moderator),
            busy: state.picking.contains_key(message_id),
            confirming: state.confirm_end.as_deref() == Some(message_id),
            ending: state.ending.contains(message_id),
            status,
            pictures,
            from: Vec::new(),
        }
    }

    /// Everything that changes what's drawn, except where the bars grow from.
    pub fn digest(&self, h: &mut impl std::hash::Hasher) {
        self.poll.encode_to_vec().hash(h);
        (&self.chosen, self.closed, self.results, self.hidden, self.can_vote, self.can_end).hash(h);
        (self.busy, self.confirming, self.ending, &self.status, &self.pictures).hash(h);
    }

    pub fn total(&self) -> i64 {
        self.poll.answers.iter().map(|a| a.votes).sum()
    }

    /// Each answer's share of the votes, as its bar shows it (0 while the counts don't show).
    pub fn shares(&self) -> Vec<f32> {
        let total = self.total();
        self.poll
            .answers
            .iter()
            .map(|a| if self.results && total > 0 { a.votes as f32 / total as f32 } else { 0.0 })
            .collect()
    }
}

/// "Today at 14:02" reads "today at 14:02" after "Ended".
fn lower_day(when: &str) -> String {
    match when.strip_prefix("Today").or_else(|| when.strip_prefix("Yesterday")) {
        Some(rest) => format!("{}{rest}", if when.starts_with('T') { "today" } else { "yesterday" }),
        None => format!("on {when}"),
    }
}

/// A poll answer's emoji: a server emoji's picture, the character, or nothing.
fn answer_emoji(emoji: &str, picture: Option<&String>, size: f32) -> Option<AnyElement> {
    let base = div().size(px(size + 2.0)).flex_none().flex().items_center().justify_center();
    if let Some(url) = picture {
        use gpui_kit::StyledImage as _;
        return Some(
            base.child(
                gpui_kit::img(SharedString::from(url.clone())).size(px(size)).object_fit(gpui_kit::ObjectFit::Contain),
            )
            .into_any_element(),
        );
    }
    (!emoji.is_empty() && !emoji.starts_with('<'))
        .then(|| base.in_color().text_size(px(size * 0.85)).child(emoji.to_owned()).into_any_element())
}

fn pill(
    glyph: Option<&'static str>,
    text: &'static str,
    fg: impl Into<gpui_kit::Hsla>,
    bg: impl Into<gpui_kit::Hsla>,
) -> gpui_kit::Div {
    let (fg, bg) = (fg.into(), bg.into());
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .px(px(8.0))
        .py(px(2.0))
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .when_some(glyph, |el, g| el.child(icon(g).size(px(12.0))))
        .child(text)
}

fn footer_button(
    id: SharedString,
    glyph: &'static str,
    label: Option<&'static str>,
    p: &Palette,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let hover = alpha(p.muted_foreground, 0.12);
    let fg = p.foreground;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(4.0))
        .px(px(8.0))
        .py(px(4.0))
        .rounded(corner(8.0))
        .font_weight(FontWeight::BOLD)
        .cursor_pointer()
        .hover(move |s| s.bg(hover).text_color(fg))
        .active(|s| s.top(px(1.0)))
        .child(icon(glyph).size(px(14.0)))
        .when_some(label, |el, l| el.child(l))
}

/// The card under a poll's message.
pub(crate) fn poll_card(mid: &str, c: &PollCard, p: &Palette, this: &WeakEntity<FuwaApp>) -> AnyElement {
    let poll = &c.poll;
    let muted = alpha(p.muted_foreground, 0.12);
    let mut pills = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(6.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::EXTRA_BOLD)
        .text_color(p.muted_foreground)
        .child(pill(Some("chart-column"), "Poll", p.primary, alpha(p.primary, 0.12)))
        .child(pill(None, if poll.multiple { "Pick any" } else { "Pick one" }, p.muted_foreground, muted))
        .child({
            let (glyph, text, tip) = if poll.anonymous {
                (
                    "eye-off",
                    "Anonymous",
                    "Nobody here, moderators and admins included, can see who voted for what. Results show when it ends.",
                )
            } else {
                ("eye", "Public votes", "Everyone here can see who voted for what")
            };
            div()
                .id(SharedString::from(format!("poll-kind|{mid}")))
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx))
                .child(pill(Some(glyph), text, p.muted_foreground, muted))
        });
    if c.closed {
        pills = pills.child(motion::rise(
            pill(Some("flag"), "Final results", p.background, p.foreground),
            SharedString::from(format!("poll-final|{mid}")),
            Duration::ZERO,
            4.0,
        ));
    }
    let header = div().flex().flex_col().gap(px(6.0)).child(pills).child(
        div()
            .text_size(px(16.0))
            .line_height(px(21.0))
            .font_weight(FontWeight::EXTRA_BOLD)
            .child(poll.question.clone()),
    );

    let total = c.total();
    let top = poll.answers.iter().map(|a| a.votes).max().unwrap_or(0);
    let shares = c.shares();
    let mut answers = div().flex().flex_col().gap(px(6.0));
    for (n, a) in poll.answers.iter().enumerate() {
        let chosen = c.chosen.contains(&a.id);
        let winner = c.closed && top > 0 && a.votes == top;
        let dim = c.closed && top > 0 && a.votes != top;
        let to = shares[n];
        let from = c.from.get(n).copied().unwrap_or(0.0);
        let bar_color = if winner {
            alpha(p.primary, 0.3)
        } else if chosen {
            alpha(p.primary, 0.22)
        } else {
            alpha(p.foreground, 0.08)
        };
        let bar = div().absolute().left_0().top_0().bottom_0().rounded(corner(11.0)).bg(bar_color).with_animation(
            SharedString::from(format!("poll-bar|{mid}|{}|{}|{}", a.id, from.to_bits(), to.to_bits())),
            Animation::new(Duration::from_millis(700)).with_easing(gpui_kit::ease_out_quint()),
            move |el, t| el.w(gpui_kit::relative(from + (to - from) * t)),
        );
        let mark = div()
            .size(px(18.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .border_2()
            .map(|el| if poll.multiple { el.rounded(corner(6.0)) } else { el.rounded_full() })
            .when(chosen, |el| el.border_color(p.primary).bg(p.primary).text_color(p.primary_foreground))
            .when(!chosen, |el| el.border_color(alpha(p.muted_foreground, 0.4)))
            .when(chosen, |el| {
                el.child(
                    div().flex().items_center().justify_center().child(icon("check").size(px(12.0))).with_animation(
                        SharedString::from(format!("poll-check|{mid}|{}", a.id)),
                        Animation::new(Duration::from_millis(260)).with_easing(gpui_kit::ease_out_quint()),
                        |el, t| el.opacity(t).size(px(6.0 + 6.0 * t)),
                    ),
                )
            });
        let percent = (to * 100.0).round() as i64;
        let id = a.id;
        let row = div()
            .id(SharedString::from(format!("poll-answer|{mid}|{id}")))
            .relative()
            .rounded(corner(12.0))
            .border_1()
            .border_color(if chosen { alpha(p.primary, 0.6) } else { p.border.into() })
            .when(dim, |el| el.opacity(0.62))
            .child(bar)
            .child(
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .text_sm()
                    .child(mark)
                    .when_some(answer_emoji(&a.emoji, c.pictures.get(n).and_then(Option::as_ref), 18.0), |el, e| {
                        el.child(e)
                    })
                    .child(div().flex_1().min_w_0().font_weight(FontWeight::SEMIBOLD).child(a.text.clone()))
                    .when(winner, |el| el.child(icon("trophy").size(px(15.0)).text_color(p.primary)))
                    .when(c.results, |el| {
                        el.child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(p.muted_foreground)
                                .child(format!("{} · {percent}%", a.votes)),
                        )
                    }),
            );
        let row = if c.can_vote {
            let hover = alpha(p.primary, 0.4);
            let (this, mid) = (this.clone(), mid.to_owned());
            row.cursor_pointer().hover(move |s| s.border_color(hover)).active(|s| s.top(px(1.0))).on_click(
                move |_, _, cx| {
                    let _ = this.update(cx, |this, cx| this.poll_pick(mid.clone(), id, cx));
                },
            )
        } else {
            row
        };
        answers = answers.child(row);
    }

    let voters = poll.voters;
    let mut buttons = div().ml_auto().flex().flex_wrap().items_center().justify_end().gap(px(4.0));
    if c.busy {
        buttons = buttons.child(icon("loader-circle").size(px(14.0)).with_animation(
            SharedString::from(format!("poll-busy|{mid}")),
            Animation::new(Duration::from_millis(900)).repeat(),
            |el, t| el.rotate(gpui_kit::percentage(t)),
        ));
    }
    if !c.results && !c.closed && !c.hidden {
        let (this, mid) = (this.clone(), mid.to_owned());
        buttons = buttons.child(
            footer_button(SharedString::from(format!("poll-peek|{mid}")), "chart-column", Some("Show results"), p)
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.polls.peek.insert(mid.clone());
                        this.sync_list(cx);
                        cx.notify();
                    });
                }),
        );
    }
    if !c.poll.my_answer_ids.is_empty() && c.can_vote {
        let (this, mid) = (this.clone(), mid.to_owned());
        buttons = buttons.child(
            footer_button(SharedString::from(format!("poll-unvote|{mid}")), "undo", Some("Take back vote"), p)
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |this, cx| this.poll_vote(mid.clone(), Vec::new(), cx));
                }),
        );
    }
    if !poll.anonymous && total > 0 {
        let (this, mid) = (this.clone(), mid.to_owned());
        buttons = buttons.child(
            footer_button(SharedString::from(format!("poll-who|{mid}")), "users", Some("Who voted"), p).on_click(
                move |_, _, cx| {
                    let _ = this.update(cx, |this, cx| this.open_voters(mid.clone(), cx));
                },
            ),
        );
    }
    if c.can_end && c.confirming {
        let (yes, no) = (this.clone(), this.clone());
        let (m1, m2) = (mid.to_owned(), mid.to_owned());
        buttons = buttons.child(motion::slide_in(
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(div().font_weight(FontWeight::BOLD).text_color(p.destructive).child("End it now?"))
                .child(
                    footer_button(
                        SharedString::from(format!("poll-end-yes|{mid}")),
                        if c.ending { "loader-circle" } else { "check" },
                        None,
                        p,
                    )
                    .text_color(p.destructive)
                    .on_click(move |_, _, cx| {
                        let _ = yes.update(cx, |this, cx| this.poll_end(m1.clone(), cx));
                    }),
                )
                .child(footer_button(SharedString::from(format!("poll-end-no|{mid}")), "x", None, p).on_click(
                    move |_, _, cx| {
                        let _ = no.update(cx, |this, cx| {
                            this.polls.confirm_end = None;
                            this.sync_list(cx);
                            cx.notify();
                        });
                    },
                )),
            SharedString::from(format!("poll-end-ask|{m2}")),
            8.0,
        ));
    } else if c.can_end {
        let (this, mid) = (this.clone(), mid.to_owned());
        buttons = buttons.child(
            footer_button(SharedString::from(format!("poll-end|{mid}")), "flag", Some("End poll"), p).on_click(
                move |_, _, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.polls.confirm_end = Some(mid.clone());
                        this.sync_list(cx);
                        cx.notify();
                    });
                },
            ),
        );
    }
    let footer = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .text_xs()
        .text_color(p.muted_foreground)
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .child(format!("{voters} {}", if voters == 1 { "vote" } else { "votes" })),
        )
        .child("·")
        .child(c.status.clone())
        .child(buttons);

    div()
        .mt(px(4.0))
        .max_w(px(512.0))
        .flex()
        .flex_col()
        .gap(px(12.0))
        .p(px(14.0))
        .rounded(corner(16.0))
        .border_1()
        .border_color(p.border)
        .bg(alpha(p.card, 0.7))
        .child(header)
        .child(answers)
        .child(footer)
        .into_any_element()
}

// ───────────────────────── Voting ─────────────────────────

impl FuwaApp {
    fn poll_place(&self) -> Option<Place> {
        match self.target()? {
            Target::Channel { key, server, channel } => Some(Place { key, server, channel }),
            _ => None,
        }
    }

    /// A click on an answer.
    pub fn poll_pick(&mut self, message_id: String, answer: u32, cx: &mut Context<Self>) {
        let Some(place) = self.poll_place() else { return };
        let poll = self.core.shared.read(|s| {
            s.instance(&place.key)?
                .messages
                .get(&place.channel)?
                .items
                .iter()
                .find(|m| m.id == message_id)?
                .poll
                .clone()
        });
        let Some(poll) = poll else { return };
        let chosen = self.polls.picking.get(&message_id).cloned().unwrap_or_else(|| poll.my_answer_ids.clone());
        self.poll_vote(message_id, polls::after_pick(&poll, &chosen, answer), cx);
    }

    /// Votes, showing the pick at once. Clicks queue up: only the newest goes
    /// out after the one in flight, so answers never land out of order.
    pub fn poll_vote(&mut self, message_id: String, ids: Vec<u32>, cx: &mut Context<Self>) {
        let Some(place) = self.poll_place() else { return };
        self.polls.picking.insert(message_id.clone(), ids.clone());
        self.polls.want.insert(message_id.clone(), (place, ids));
        self.poll_flush(message_id, cx);
        self.sync_list(cx);
        cx.notify();
    }

    fn poll_flush(&mut self, message_id: String, cx: &mut Context<Self>) {
        if self.polls.sending.contains(&message_id) {
            return;
        }
        let Some((place, ids)) = self.polls.want.remove(&message_id) else {
            self.polls.picking.remove(&message_id);
            self.sync_list(cx);
            cx.notify();
            return;
        };
        self.polls.sending.insert(message_id.clone());
        let core = self.core.clone();
        let id = message_id.clone();
        self.run(
            cx,
            async move { core.vote_poll(&place.key, &place.server, &place.channel, &id, ids).await },
            move |this, result, cx| {
                this.polls.sending.remove(&message_id);
                if let Err(err) = result {
                    this.toast("circle-alert", "Couldn't vote".into(), err.message, None, None, cx);
                }
                this.poll_flush(message_id, cx);
            },
        );
    }

    /// Ends a poll early, once asked again.
    pub fn poll_end(&mut self, message_id: String, cx: &mut Context<Self>) {
        let Some(place) = self.poll_place() else { return };
        if !self.polls.ending.insert(message_id.clone()) {
            return;
        }
        let core = self.core.clone();
        let id = message_id.clone();
        self.run(
            cx,
            async move { core.end_poll(&place.key, &place.server, &place.channel, &id).await },
            move |this, result, cx| {
                this.polls.ending.remove(&message_id);
                if this.polls.confirm_end.as_deref() == Some(message_id.as_str()) {
                    this.polls.confirm_end = None;
                }
                if let Err(err) = result {
                    this.toast("circle-alert", "Couldn't end the poll".into(), err.message, None, None, cx);
                }
                this.sync_list(cx);
                cx.notify();
            },
        );
        self.sync_list(cx);
        cx.notify();
    }

    /// Sets a timer to redraw the open list's running polls as their time
    /// runs down: every 15 seconds in their last hour, every minute before,
    /// and once more the moment one ends.
    pub(crate) fn poll_tick(&mut self, rows: &[crate::ui::chat::Row], cx: &mut Context<Self>) {
        if self.polls.ticking {
            return;
        }
        let now = crate::core::dms::now_ms();
        let wait = rows
            .iter()
            .filter_map(|r| match r {
                crate::ui::chat::Row::Msg(m) => m.poll.as_ref(),
                _ => None,
            })
            .filter(|c| !c.closed)
            .filter_map(|c| {
                let ends = polls::ends_at_ms(&c.poll);
                (ends > 0).then(|| {
                    let ms = ends - now;
                    (if ms < 3_600_000 { 15_000 } else { 60_000 }).min(ms.max(0) + 50)
                })
            })
            .min();
        let Some(wait) = wait else { return };
        self.polls.ticking = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(wait as u64)).await;
            let _ = this.update(cx, |this, cx| {
                this.polls.ticking = false;
                this.sync_list(cx);
                cx.notify();
            });
        })
        .detach();
    }

    // ───────────────────────── Who voted ─────────────────────────

    /// Opens who voted for what, on the first answer anyone picked.
    pub fn open_voters(&mut self, message_id: String, cx: &mut Context<Self>) {
        let Some(place) = self.poll_place() else { return };
        let poll = self.voters_poll(&place, &message_id);
        let first = poll.as_ref().and_then(|p| p.answers.iter().find(|a| a.votes > 0).or(p.answers.first()));
        let Some(answer) = first.map(|a| a.id) else { return };
        self.menu = None;
        self.polls.voters = Some(Voters { answer, pages: HashMap::new() });
        self.dialog = Some(Dialog::PollVoters {
            key: place.key,
            server: place.server,
            channel: place.channel,
            message: message_id,
        });
        self.dialog_error = None;
        self.load_voters(answer, false, cx);
        cx.notify();
    }

    fn voters_poll(&self, place: &Place, message_id: &str) -> Option<pb::Poll> {
        self.core.shared.read(|s| {
            s.instance(&place.key)?
                .messages
                .get(&place.channel)?
                .items
                .iter()
                .find(|m| m.id == message_id)?
                .poll
                .clone()
        })
    }

    /// Loads an answer's voters: the first page, or with `more` the next.
    fn load_voters(&mut self, answer: u32, more: bool, cx: &mut Context<Self>) {
        let Some(Dialog::PollVoters { key, server, message, .. }) = self.dialog.clone() else { return };
        let Some(voters) = self.polls.voters.as_mut() else { return };
        let page = voters.pages.entry(answer).or_default();
        if page.loading || (page.loaded && !more) {
            return;
        }
        page.loading = true;
        page.error = None;
        let after = if more { page.users.last().map(|u| u.id.clone()).unwrap_or_default() } else { String::new() };
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.poll_voters(&key, &server, &message, answer, &after).await },
            move |this, result, cx| {
                let Some(page) = this.polls.voters.as_mut().and_then(|v| v.pages.get_mut(&answer)) else { return };
                page.loading = false;
                match result {
                    Ok((users, has_more)) => {
                        page.users.extend(users);
                        page.more = has_more;
                        page.loaded = true;
                    }
                    Err(err) => page.error = Some(err.message),
                }
                cx.notify();
            },
        );
    }

    pub(crate) fn render_voters(
        &mut self,
        key: &str,
        server: &str,
        channel: &str,
        message: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let place = Place { key: key.to_owned(), server: server.to_owned(), channel: channel.to_owned() };
        let poll = self.voters_poll(&place, message);
        let (Some(poll), Some(voters)) = (poll, self.polls.voters.as_ref()) else { return div().into_any_element() };
        let look = self.core.shared.read(|s| s.instance(key).map(|i| Look::of(i, server)).unwrap_or_default());
        let current = voters.answer;
        let mut tabs = div().flex().flex_wrap().gap(px(6.0));
        for a in &poll.answers {
            let on = a.id == current;
            let id = a.id;
            let hover = alpha(p.primary, 0.08);
            let picture = answer_emoji(&a.emoji, look_picture(&look, &a.emoji).as_ref(), 14.0);
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("voters-tab|{id}")))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(10.0))
                    .h(px(30.0))
                    .rounded_full()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .when(on, |el| el.bg(alpha(p.primary, 0.14)).text_color(p.primary))
                    .when(!on, |el| el.text_color(p.muted_foreground).hover(move |s| s.bg(hover)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(v) = this.polls.voters.as_mut() {
                            v.answer = id;
                        }
                        this.load_voters(id, false, cx);
                        cx.notify();
                    }))
                    .when_some(picture, |el, e| el.child(e))
                    .child(
                        div()
                            .max_w(px(160.0))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(a.text.clone()),
                    )
                    .child(div().text_xs().opacity(0.8).child(a.votes.to_string())),
            );
        }
        let page = voters.pages.get(&current);
        let mut list = div().id("voters-list").max_h(px(320.0)).overflow_y_scroll().flex().flex_col().gap(px(2.0));
        match page {
            Some(page) if page.loaded && page.users.is_empty() => {
                list = list.child(
                    div()
                        .py(px(24.0))
                        .flex()
                        .justify_center()
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child("Nobody picked this one yet."),
                );
            }
            Some(page) => {
                for (n, user) in page.users.iter().enumerate() {
                    let (k, uid, sid) = (key.to_owned(), user.id.clone(), server.to_owned());
                    let hover = alpha(p.primary, 0.08);
                    let name = self.core.shared.read(|s| {
                        s.instance(key)
                            .map(|i| i.display_name(Some(server), &user.id))
                            .unwrap_or_else(|| user.username.clone())
                    });
                    list = list.child(motion::rise(
                        div()
                            .id(SharedString::from(format!("voter|{}", user.id)))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .px(px(8.0))
                            .py(px(6.0))
                            .rounded(corner(10.0))
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_dialog(
                                    Dialog::Profile { key: k.clone(), user_id: uid.clone(), server: Some(sid.clone()) },
                                    window,
                                    cx,
                                )
                            }))
                            .child(avatar(Some(user), 32.0, &p))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_sm().font_weight(FontWeight::BOLD).child(name))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(p.muted_foreground)
                                            .child(format!("@{}", user.username)),
                                    ),
                            ),
                        SharedString::from(format!("voter-in|{current}|{n}")),
                        Duration::from_millis(20 * n.min(12) as u64),
                        6.0,
                    ));
                }
                if page.loading {
                    list = list.child(
                        div()
                            .py(px(10.0))
                            .flex()
                            .justify_center()
                            .text_sm()
                            .text_color(p.muted_foreground)
                            .child("Loading…"),
                    );
                } else if page.more {
                    list = list.child(div().pt(px(6.0)).flex().justify_center().child(
                        soft_button("voters-more", "Show more", &p).on_click(cx.listener(move |this, _, _, cx| {
                            this.load_voters(current, true, cx);
                        })),
                    ));
                }
            }
            None => {}
        }
        let error = page.and_then(|p| p.error.clone());
        let panel = card(&p)
            .w(px(440.0))
            .p(px(24.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(dialog_head("users", "Who voted", poll.question.clone(), &p, cx))
            .child(tabs)
            .child(list)
            .when_some(error_line(error.as_deref(), &p), |el, e| el.child(e));
        dialog_frame(panel, "voters", &p, cx)
    }

    // ───────────────────────── Making one ─────────────────────────

    /// Opens the poll editor for the open channel.
    pub fn open_poll_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(place) = self.poll_place() else { return };
        self.emoji_open = false;
        self.picker = None;
        let channel_name = self.core.shared.read(|s| {
            s.instance(&place.key).and_then(|i| i.channel(&place.server, &place.channel)).map(|c| c.name.clone())
        });
        let question = cx.new(|cx| InputState::new(window, cx).placeholder("What should we play tonight?"));
        let emoji_query = cx.new(|cx| InputState::new(window, cx).placeholder("Find an emoji"));
        let mut subs = vec![
            cx.subscribe_in(&question, window, |this: &mut Self, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    clip(input, QUESTION, window, cx);
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    if let Some(first) = this.polls.editor.as_ref().and_then(|e| e.answers.first()) {
                        first.text.update(cx, |s, cx| s.focus(window, cx));
                    }
                }
                _ => {}
            }),
            cx.subscribe_in(&emoji_query, window, |_: &mut Self, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }),
        ];
        let answers = (0..MIN_ANSWERS).map(|n| self.new_answer(n as u64, &mut subs, window, cx)).collect();
        self.polls.editor = Some(Editor {
            place,
            channel_name: channel_name.unwrap_or_default(),
            question: question.clone(),
            answers,
            next: MIN_ANSWERS as u64,
            multiple: false,
            anonymous: false,
            hours: 24,
            busy: false,
            error: None,
            emoji_for: None,
            emoji_query,
            _subs: subs,
        });
        let Some(place) = self.poll_place() else { return };
        self.open_dialog(Dialog::Poll { key: place.key, server: place.server, channel: place.channel }, window, cx);
        question.update(cx, |s, cx| s.focus(window, cx));
    }

    /// Escape closes an answer's emoji picker before the editor.
    pub(crate) fn close_answer_picker(&mut self, cx: &mut Context<Self>) -> bool {
        let open = matches!(self.dialog, Some(Dialog::Poll { .. }));
        match self.polls.editor.as_mut().filter(|e| open && e.emoji_for.is_some()) {
            Some(e) => {
                e.emoji_for = None;
                cx.notify();
                true
            }
            None => false,
        }
    }

    fn new_answer(
        &mut self,
        id: u64,
        subs: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Answer {
        let text = cx.new(|cx| InputState::new(window, cx).placeholder("Add an answer"));
        subs.push(cx.subscribe_in(
            &text,
            window,
            |this: &mut Self, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    clip(input, ANSWER, window, cx);
                    cx.notify();
                }
                // Enter moves to the next answer, adding one at the end.
                InputEvent::PressEnter { .. } => {
                    let Some(editor) = this.polls.editor.as_ref() else { return };
                    let Some(n) = editor.answers.iter().position(|a| a.text == *input) else { return };
                    if let Some(next) = editor.answers.get(n + 1) {
                        next.text.update(cx, |s, cx| s.focus(window, cx));
                    } else if !input.read(cx).value().trim().is_empty() {
                        this.add_answer(window, cx);
                    }
                }
                _ => {}
            },
        ));
        Answer { id, text, emoji: String::new() }
    }

    fn add_answer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut editor) = self.polls.editor.take() else { return };
        if editor.answers.len() < MAX_ANSWERS {
            let mut subs = std::mem::take(&mut editor._subs);
            let answer = self.new_answer(editor.next, &mut subs, window, cx);
            editor._subs = subs;
            editor.next += 1;
            answer.text.update(cx, |s, cx| s.focus(window, cx));
            editor.answers.push(answer);
        }
        self.polls.editor = Some(editor);
        cx.notify();
    }

    fn poll_draft(&self, cx: &gpui_kit::App) -> Option<Draft> {
        let e = self.polls.editor.as_ref()?;
        Some(Draft {
            question: e.question.read(cx).value().to_string(),
            answers: e.answers.iter().map(|a| (a.text.read(cx).value().to_string(), a.emoji.clone())).collect(),
            multiple: e.multiple,
            anonymous: e.anonymous,
            hours: e.hours,
        })
    }

    pub(crate) fn send_poll(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.poll_draft(cx).filter(Draft::ready) else { return };
        let Some(editor) = self.polls.editor.as_mut() else { return };
        if editor.busy {
            return;
        }
        editor.busy = true;
        editor.error = None;
        let place = editor.place.clone();
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.send_poll(&place.key, &place.server, &place.channel, &draft).await },
            |this, result, cx| {
                match result {
                    Ok(()) => {
                        this.polls.editor = None;
                        if matches!(this.dialog, Some(Dialog::Poll { .. })) {
                            this.dialog = None;
                        }
                    }
                    Err(err) => {
                        if let Some(e) = this.polls.editor.as_mut() {
                            e.busy = false;
                            e.error = Some(err.message);
                        }
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    pub(crate) fn render_poll_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let Some(draft) = self.poll_draft(cx) else { return div().into_any_element() };
        let Some(e) = self.polls.editor.as_ref() else { return div().into_any_element() };
        let (look, catalog, tone) = self.core.shared.read(|s| {
            let i = s.instance(&e.place.key);
            let look = i.map(|i| Look::of(i, &e.place.server)).unwrap_or_default();
            let catalog = i
                .and_then(|i| Some(Catalog::own(i.server(&e.place.server)?, i.emojis.get(&e.place.server)?)))
                .unwrap_or_default();
            (look, catalog, self.prefs.skin_tone)
        });
        let count = draft.question.chars().count();
        let question = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(label("QUESTION", &p))
                    .child(div().text_xs().text_color(p.muted_foreground).child(format!("{count}/{QUESTION}"))),
            )
            .child(Input::new(&e.question).large());

        let mut answers = div().flex().flex_col().gap(px(8.0)).child(label("ANSWERS", &p));
        let removable = e.answers.len() > MIN_ANSWERS;
        for (n, a) in e.answers.iter().enumerate() {
            let picking = e.emoji_for == Some(n);
            let glyph = answer_emoji(&a.emoji, look_picture(&look, &a.emoji).as_ref(), 18.0);
            let id = a.id;
            let emoji_button = div()
                .id(SharedString::from(format!("poll-emoji|{id}")))
                .size(px(40.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(corner(12.0))
                .border_1()
                .border_color(if picking { p.primary } else { p.border })
                .text_color(p.muted_foreground)
                .cursor_pointer()
                .hover({
                    let border = p.primary;
                    move |s| s.border_color(border)
                })
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(format!("Add an emoji to answer {}", n + 1))
                        .build(window, cx)
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    let Some(e) = this.polls.editor.as_mut() else { return };
                    let Some(at) = e.answers.iter().position(|a| a.id == id) else { return };
                    e.emoji_for = if e.emoji_for == Some(at) { None } else { Some(at) };
                    if e.emoji_for.is_some() {
                        e.emoji_query.update(cx, |s, cx| {
                            s.set_value("", window, cx);
                            s.focus(window, cx);
                        });
                    }
                    cx.notify();
                }))
                .child(glyph.unwrap_or_else(|| icon("face-slightly-smiling-plus").size(px(18.0)).into_any_element()));
            let row = div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(emoji_button)
                .child(div().flex_1().min_w_0().child(Input::new(&a.text).large()))
                .when(removable, |el| {
                    el.child(icon_button(SharedString::from(format!("poll-remove|{id}")), "x", &p).on_click(
                        cx.listener(move |this, _, _, cx| {
                            if let Some(e) = this.polls.editor.as_mut() {
                                e.answers.retain(|a| a.id != id);
                                e.emoji_for = None;
                            }
                            cx.notify();
                        }),
                    ))
                });
            answers =
                answers.child(motion::rise(row, SharedString::from(format!("poll-row|{id}")), Duration::ZERO, 6.0));
            if picking {
                answers = answers.child(self.answer_picker(n, &catalog, tone, &p, cx));
            }
        }
        if e.answers.len() < MAX_ANSWERS {
            let hover = alpha(p.primary, 0.08);
            answers = answers.child(
                div()
                    .id("poll-add")
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .h(px(40.0))
                    .px(px(12.0))
                    .rounded(corner(12.0))
                    .border_1()
                    .border_dashed()
                    .border_color(p.border)
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(|this, _, window, cx| this.add_answer(window, cx)))
                    .child(icon("plus").size(px(16.0)))
                    .child("Add an answer"),
            );
        }

        let mut durations = div().flex().flex_wrap().gap(px(6.0));
        for (name, hours) in DURATIONS {
            let on = e.hours == hours;
            let hover = alpha(p.primary, 0.08);
            durations = durations.child(
                div()
                    .id(SharedString::from(format!("poll-hours|{hours}")))
                    .px(px(12.0))
                    .h(px(30.0))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .border_1()
                    .when(on, |el| el.border_color(p.primary).bg(alpha(p.primary, 0.14)).text_color(p.primary))
                    .when(!on, |el| {
                        el.border_color(p.border).text_color(p.muted_foreground).hover(move |s| s.bg(hover))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(e) = this.polls.editor.as_mut() {
                            e.hours = hours;
                        }
                        cx.notify();
                    }))
                    .child(name),
            );
        }
        let toggles = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(toggle_row(
                "Pick more than one",
                if e.multiple { "People can choose as many answers as they like." } else { "People choose one answer." },
                switch("poll-multiple".into(), e.multiple, false, cx, |this: &mut Self, on, cx| {
                    if let Some(e) = this.polls.editor.as_mut() {
                        e.multiple = on;
                    }
                    cx.notify();
                }),
                &p,
            ))
            .child(toggle_row(
                "Anonymous votes",
                if e.anonymous {
                    "Nobody here, moderators and admins included, sees who voted for what, and results show when the poll ends. Voters see this before they vote."
                } else {
                    "Everyone in the channel can see who voted for what. Voters see this before they vote."
                },
                switch("poll-anonymous".into(), e.anonymous, false, cx, |this: &mut Self, on, cx| {
                    if let Some(e) = this.polls.editor.as_mut() {
                        e.anonymous = on;
                    }
                    cx.notify();
                }),
                &p,
            ));

        let ready = draft.ready();
        let submit = if draft.filled() < MIN_ANSWERS {
            "Add at least two answers"
        } else if draft.question.trim().is_empty() {
            "Ask a question"
        } else if e.busy {
            "Posting…"
        } else {
            "Post poll"
        };
        let body = div()
            .id("poll-editor-body")
            .max_h((window.viewport_size().height - px(220.0)).max(px(240.0)))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(18.0))
            .pr(px(4.0))
            .child(question)
            .child(answers)
            .child(div().flex().flex_col().gap(px(8.0)).child(label("HOW LONG IT RUNS", &p)).child(durations))
            .child(toggles);
        let panel = card(&p)
            .w(px(520.0))
            .p(px(24.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(dialog_head(
                "chart-column",
                "Make a poll",
                format!("Everyone who can see #{} can vote.", e.channel_name),
                &p,
                cx,
            ))
            .child(body)
            .when_some(error_line(e.error.as_deref(), &p), |el, err| el.child(err))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(10.0))
                    .child(
                        soft_button("poll-cancel", "Cancel", &p)
                            .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
                    )
                    .child(
                        primary_button("poll-post", submit, &p)
                            .when(!ready || e.busy, |el| el.opacity(0.55))
                            .on_click(cx.listener(|this, _, _, cx| this.send_poll(cx))),
                    ),
            );
        dialog_frame(panel, "poll", &p, cx)
    }

    /// A small emoji picker under an answer: this server's emoji and the standard set, searchable.
    fn answer_picker(&self, n: usize, catalog: &Catalog, tone: u8, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(e) = self.polls.editor.as_ref() else { return div().into_any_element() };
        let query = e.emoji_query.read(cx).value().to_string();
        let choices: Vec<Choice> = if query.trim().is_empty() {
            let own = catalog.emojis.iter().map(|c| Choice::custom(catalog, c));
            let standard = emoji::standard().iter().flat_map(|g| g.emojis.iter()).map(|s| Choice::standard(s, tone));
            own.chain(standard).take(PICKS).collect()
        } else {
            emoji::search(&query, catalog, tone, PICKS)
        };
        let mut grid = div().flex().flex_wrap();
        for choice in choices {
            let value = match choice.key.strip_prefix("c:").and_then(|id| catalog.by_id(id)) {
                Some(custom) => emoji::token(&custom.emoji),
                None => choice.insert.clone(),
            };
            let hover = alpha(p.primary, 0.12);
            grid = grid.child(
                div()
                    .id(SharedString::from(format!("poll-pick|{n}|{}", choice.key)))
                    .size(px(36.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(corner(10.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(e) = this.polls.editor.as_mut() {
                            if let Some(a) = e.answers.get_mut(n) {
                                a.emoji = value.clone();
                            }
                            e.emoji_for = None;
                        }
                        cx.notify();
                    }))
                    .child(crate::ui::chat::emoji_glyph(&choice, 22.0)),
            );
        }
        let has = !e.answers.get(n).is_none_or(|a| a.emoji.is_empty());
        motion::rise(
            div()
                .p(px(8.0))
                .rounded(corner(14.0))
                .border_1()
                .border_color(p.border)
                .bg(p.secondary)
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(div().flex_1().child(Input::new(&e.emoji_query).prefix(icon("search").size(px(14.0)))))
                        .when(has, |el| {
                            el.child(soft_button("poll-emoji-clear", "No emoji", p).h(px(32.0)).text_sm().on_click(
                                cx.listener(move |this, _, _, cx| {
                                    if let Some(e) = this.polls.editor.as_mut() {
                                        if let Some(a) = e.answers.get_mut(n) {
                                            a.emoji.clear();
                                        }
                                        e.emoji_for = None;
                                    }
                                    cx.notify();
                                }),
                            ))
                        }),
                )
                .child(div().id("poll-pick-grid").max_h(px(152.0)).overflow_y_scroll().child(grid)),
            SharedString::from(format!("poll-picker|{n}")),
            Duration::ZERO,
            6.0,
        )
        .into_any_element()
    }
}

/// Emoji the answer picker shows at once.
const PICKS: usize = 48;

/// A server emoji's picture for an answer's `<:name:id>`.
fn look_picture(look: &Look, emoji: &str) -> Option<String> {
    let (_, id, _) = emoji::token_at(emoji)?;
    look.emojis.get(id).or_else(|| look.emojis.get(&id.to_uppercase())).cloned()
}

/// Keeps a field to `max` characters.
fn clip(input: &Entity<InputState>, max: usize, window: &mut Window, cx: &mut Context<FuwaApp>) {
    let value = input.read(cx).value().to_string();
    if value.chars().count() > max {
        let cut: String = value.chars().take(max).collect();
        input.update(cx, |s, cx| s.set_value(cut, window, cx));
    }
}

fn label(text: &'static str, p: &Palette) -> gpui_kit::Div {
    div().text_size(px(11.0)).font_weight(FontWeight::EXTRA_BOLD).text_color(p.muted_foreground).child(text)
}

fn toggle_row(title: &'static str, hint: &'static str, control: impl IntoElement, p: &Palette) -> gpui_kit::Div {
    div()
        .flex()
        .items_start()
        .gap(px(12.0))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(div().text_sm().font_weight(FontWeight::BOLD).child(title))
                .child(div().text_xs().text_color(p.muted_foreground).child(hint)),
        )
        .child(control)
}

fn dialog_head(
    glyph: &'static str,
    title: &'static str,
    body: String,
    p: &Palette,
    cx: &mut Context<FuwaApp>,
) -> gpui_kit::Div {
    div()
        .flex()
        .items_start()
        .gap(px(14.0))
        .child(
            div()
                .size(px(44.0))
                .flex_none()
                .rounded(corner(14.0))
                .flex()
                .items_center()
                .justify_center()
                .bg(alpha(p.primary, 0.14))
                .text_color(p.primary)
                .child(icon(glyph).size(px(22.0))),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(div().text_lg().font_weight(FontWeight::EXTRA_BOLD).child(title))
                .child(div().text_sm().text_color(p.muted_foreground).child(body)),
        )
        .child(icon_button("dialog-close", "x", p).on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))))
}

fn dialog_frame(panel: gpui_kit::Div, tag: &'static str, p: &Palette, cx: &mut Context<FuwaApp>) -> AnyElement {
    motion::fade_in(
        scrim("dialog-scrim", p).on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))).child(motion::rise(
            div().id("dialog-panel").on_click(|_, _, cx| cx.stop_propagation()).child(panel),
            SharedString::from(format!("dialog-{tag}")),
            Duration::ZERO,
            24.0,
        )),
        SharedString::from(format!("dialog-fade-{tag}")),
        Duration::from_millis(180),
    )
    .into_any_element()
}

/// The poll editor, while it's open.
pub struct Editor {
    place: Place,
    channel_name: String,
    question: Entity<InputState>,
    answers: Vec<Answer>,
    /// The next answer's id, so rows keep their place as others go.
    next: u64,
    multiple: bool,
    anonymous: bool,
    hours: i32,
    busy: bool,
    error: Option<String>,
    /// The answer whose emoji is being picked, and the search for it.
    emoji_for: Option<usize>,
    emoji_query: Entity<InputState>,
    _subs: Vec<Subscription>,
}

struct Answer {
    id: u64,
    text: Entity<InputState>,
    emoji: String,
}

/// Who voted for what, while it's open: the answer shown and each answer's pages.
pub struct Voters {
    pub answer: u32,
    pages: HashMap<u32, Page>,
}

#[derive(Default)]
struct Page {
    users: Vec<pb::User>,
    more: bool,
    loading: bool,
    loaded: bool,
    error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poll(anonymous: bool) -> pb::Poll {
        pb::Poll {
            question: "Snacks?".into(),
            answers: vec![
                pb::PollAnswer { id: 1, text: "Chips".into(), votes: 3, ..Default::default() },
                pb::PollAnswer { id: 2, text: "Fruit".into(), votes: 1, ..Default::default() },
            ],
            anonymous,
            voters: 4,
            ..Default::default()
        }
    }

    #[test]
    fn results_show_after_voting_and_never_while_an_anonymous_poll_runs() {
        let state = PollState::default();
        let look = Look::default();
        let fresh = PollCard::of(&poll(false), "m", false, true, false, &state, &look, 0);
        assert!(!fresh.results && fresh.can_vote && !fresh.can_end);
        assert_eq!(fresh.shares(), vec![0.0, 0.0]);
        let voted =
            PollCard::of(&pb::Poll { my_answer_ids: vec![1], ..poll(false) }, "m", true, true, false, &state, &look, 0);
        assert!(voted.results && voted.can_end);
        assert_eq!(voted.shares(), vec![0.75, 0.25]);
        let secret =
            PollCard::of(&pb::Poll { my_answer_ids: vec![1], ..poll(true) }, "m", false, true, true, &state, &look, 0);
        assert!(!secret.results && secret.hidden && secret.can_end);
        assert!(secret.status.ends_with("results show at the end"));
    }

    #[test]
    fn an_ended_poll_is_final() {
        let ended = pb::Poll { ended_at: Some(prost_types::Timestamp { seconds: 10, nanos: 0 }), ..poll(true) };
        let c = PollCard::of(&ended, "m", true, true, true, &PollState::default(), &Look::default(), 20_000);
        assert!(c.closed && c.results && !c.can_vote && !c.can_end);
        assert!(c.status.starts_with("Ended "));
    }

    #[test]
    fn a_pick_in_flight_shows_at_once() {
        let mut state = PollState::default();
        state.picking.insert("m".into(), vec![2]);
        let c = PollCard::of(&poll(false), "m", false, true, false, &state, &Look::default(), 0);
        assert_eq!(c.chosen, vec![2]);
        assert!(c.busy);
    }
}
