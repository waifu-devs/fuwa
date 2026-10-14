//! Typing indicators on screen (docs/typing.md): the line over a box saying
//! who's writing ("Ana is typing", with three dots bobbing before it), and
//! saying so for you as you write. Who's typing is `core::typing`'s, kept
//! apart from the store; the line floats just over the box, so it never
//! pushes the messages around. The dots move only while it shows (and the
//! window is in front), and hold still with reduced motion.

use std::time::Duration;

use gpui_kit::{AnyElement, Context, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div, px};

use crate::core::i18n::t;
use crate::core::typing::{self, Place};
use crate::ui::app::{FuwaApp, Target};
use crate::ui::motion;
use crate::ui::theme::Palette;

/// How long the dots take to bob once.
const BOB: Duration = Duration::from_millis(1200);

/// What the line says: a translation and the names that go in it, bold.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Words {
    key: &'static str,
    names: Vec<(&'static str, String)>,
}

impl FuwaApp {
    /// The main box was typed in: says so where it writes (a server's channel,
    /// a secure channel or a conversation), when you may write there.
    pub(crate) fn typed_in_composer(&mut self, cx: &mut Context<Self>) {
        if self.composer.read(cx).value().is_empty() || self.commands.form.is_some() {
            return;
        }
        match self.target() {
            Some(Target::Channel { key, server, channel } | Target::Secure { key, server, channel }) => {
                if self.channel_blocked(&key, &server, &channel).is_none() {
                    self.core.typed(Some(&server), Place::channel(&key, &channel, ""));
                }
            }
            Some(Target::Dm { key, conversation }) => {
                let blocked = self.core.shared.read(|s| {
                    s.instance(&key).is_none_or(|i| i.dms.blocked.get(&conversation).is_some_and(|b| !b.is_empty()))
                });
                if !blocked {
                    self.core.typed(None, Place::dm(&key, &conversation));
                }
            }
            None => {}
        }
    }

    /// The open thread's box was typed in. A secure channel's threads have
    /// no server message under them: they go by the parent record's
    /// sequence, written out (as the web does).
    pub(crate) fn typed_in_thread(&mut self, cx: &mut Context<Self>) {
        if self.threads.reply.read(cx).value().is_empty() {
            return;
        }
        if let Some(Target::Secure { .. }) = self.target() {
            if let Some((key, server, channel, parent)) = crate::ui::secure_threads::open_thread(self)
                && self.secure_thread_locked().is_none()
                && self.channel_blocked(&key, &server, &channel).is_none()
            {
                self.core.typed(Some(&server), Place::channel(&key, &channel, &parent.to_string()));
            }
            return;
        }
        let Some(open) = self.threads.open.clone() else { return };
        if self.thread_blocked().is_none() {
            self.core.typed(Some(&open.server), Place::channel(&open.key, &open.channel, &open.id));
        }
    }

    /// What the line over a box says now, if anyone's typing there: names
    /// as the member list shows them in a server, a conversation's people by
    /// their names.
    fn typing_words(&self, place: &Place, server: Option<&str>) -> Option<Words> {
        let who = self.core.shared.typing_in(place);
        if who.is_empty() {
            return None;
        }
        // Four or more are "several people": no names to look up.
        if who.len() > 3 {
            let (key, names) = typing::label(&who)?;
            return Some(Words { key, names });
        }
        let names: Vec<String> = self.core.shared.read(|s| {
            let Some(i) = s.instance(&place.instance) else { return Vec::new() };
            who.iter()
                .map(|id| match server {
                    Some(server) => typing::name_in(i, server, id),
                    None => i
                        .dms
                        .conversations
                        .iter()
                        .find(|c| c.id == place.room)
                        .and_then(|c| c.users.iter().find(|u| &u.id == id))
                        .or_else(|| i.users.get(id))
                        .map(crate::core::store::user_name)
                        .unwrap_or_else(|| t("chat.typing.someone")),
                })
                .collect()
        });
        let (key, names) = typing::label(&names)?;
        Some(Words { key, names })
    }

    /// The typing line for a box (`tag` tells boxes apart): a row of its
    /// own just above the box, kept wherever someone could type, so it never
    /// covers the last message nor moves anything when it shows. Its words
    /// spring in and fade out.
    pub(crate) fn typing_line(
        &self,
        tag: &str,
        place: Option<(Place, Option<String>)>,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (place, server) = place?;
        // The row is there whether or not anyone types.
        let row = div().relative().flex_none().h(px(20.0));
        let words = self.typing_words(&place, server.as_deref());
        let id = format!("typing-line|{tag}");
        let (words, leaving) = match words {
            Some(words) => {
                motion::kept(&id, Some(&words), window, cx);
                (words, None)
            }
            None => match motion::kept::<Words>(&id, None, window, cx) {
                Some((words, t)) => (words, Some(t)),
                None => return Some(row.into_any_element()),
            },
        };
        let template = t(words.key);
        let values: Vec<(&str, &str)> = words.names.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let text = crate::ui::text::hint_line(&template, &values, p);
        let still = cx.reduce_motion();
        let dots = (0..3).fold(div().flex().items_center().gap(px(3.0)).h(px(16.0)), |row, n| {
            row.child(dot(n, still, p, &id, window))
        });
        let line = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .max_w_full()
            .px(px(8.0))
            .py(px(1.0))
            .rounded_full()
            .text_size(px(11.2))
            .line_height(px(16.0))
            .text_color(p.muted_foreground)
            .child(dots)
            .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(text));
        let shown: AnyElement = match leaving {
            Some(t) => div().opacity(1.0 - gpui_kit::ease_out_quint()(t)).child(line).into_any_element(),
            None => motion::rise(line, SharedString::from(format!("{id}|in")), Duration::ZERO, 4.0).into_any_element(),
        };
        Some(row.child(div().absolute().inset_0().left(px(4.0)).flex().items_center().child(shown)).into_any_element())
    }
}

/// One of the three dots: each bobs a third of a beat after the one before,
/// rising and brightening; with reduced motion they sit still.
fn dot(n: usize, still: bool, p: &Palette, id: &str, window: &Window) -> AnyElement {
    let el = div().relative().size(px(4.0)).rounded_full().bg(p.muted_foreground);
    if still {
        return el.opacity(0.7).into_any_element();
    }
    let phase = n as f32 / 3.0;
    motion::ambient(el, SharedString::from(format!("{id}|dot{n}")), BOB, window, move |el, t| {
        // Up and back down in the first half of its own beat, resting for the rest.
        let x = (t - phase).rem_euclid(1.0) * 2.0;
        let lift = if x < 1.0 { (x * std::f32::consts::PI).sin() } else { 0.0 };
        el.opacity(0.4 + 0.6 * lift).top(px(-2.5 * lift))
    })
}
