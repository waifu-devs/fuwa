//! Spoilers in messages (`||like this||`, found by `core::spoilers`), as the
//! web's `components/Spoiler.tsx` draws them: covered until the reader clicks
//! it, then shown for as long as the app runs. While covered, the words are
//! a node of their own whose text is shade, so nothing inside can be clicked,
//! selected or copied; revealed, the words go back into the message's
//! Markdown as they were written (bold, links, mentions and all). The cover
//! fades away, at once with reduced motion.
//!
//! Files marked as spoilers (`Attachment.spoiler`, `SealedFile.spoiler`) are
//! covered the same way until clicked: a picture's box shows a sheet saying
//! "Spoiler" and never its pixels, a file's name is shade. Revealed, they
//! stay so for as long as the app runs.

use std::cell::RefCell;
use std::collections::HashSet;
use std::time::Duration;

use gpui_kit::{
    InteractiveElement as _, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};

use crate::core::spoilers;
use crate::ui::motion;
use crate::ui::theme::alpha;

/// Where a covered spoiler's node points, in the Markdown handed to the text view.
pub const SCHEME: &str = "fuwa-spoiler:";

/// How long the cover takes to fade (the web's 0.25s).
const FADE: Duration = Duration::from_millis(250);

#[derive(Default)]
struct State {
    /// Spoilers revealed (`<scope>/<n>`): their words are drawn.
    revealed: HashSet<String>,
    /// Covers fading away.
    revealing: HashSet<String>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn key(scope: &str, n: usize) -> String {
    format!("{scope}/{n}")
}

/// Which of `scope`'s spoilers are revealed, for a message's signature.
pub fn revealed_in(scope: &str) -> Vec<String> {
    let prefix = format!("{}/", clean_scope(scope));
    STATE.with(|s| {
        let mut keys: Vec<String> = s.borrow().revealed.iter().filter(|k| k.starts_with(&prefix)).cloned().collect();
        keys.sort();
        keys
    })
}

/// The words a cover is as wide as: tokens and Markdown's marks stand in as
/// plain letters, so nothing in them is read as Markdown again.
fn cover_words(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '<' => {
                // A mention, emoji or timestamp token: about a name's width.
                for c in chars.by_ref() {
                    if c == '>' {
                        break;
                    }
                }
                out.push_str("@member");
            }
            '\n' | '\r' | '\t' => out.push(' '),
            c if c.is_alphanumeric() || c == ' ' || ".,!?'-".contains(c) => out.push(c),
            '*' | '_' | '~' | '`' | '[' | ']' | '(' | ')' | '\\' | '#' | '|' | '>' | '!' => {}
            _ => out.push('x'),
        }
    }
    let out = out.trim().to_owned();
    if out.is_empty() { "xxx".to_owned() } else { out }
}

/// A scope as it may go in a node's address.
fn clean_scope(scope: &str) -> String {
    scope.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '-' }).collect()
}

/// Turns `source`'s spoilers into nodes the [`Plugin`] covers, or (revealed
/// ones) into their own words. `scope` names the message, so each spoiler
/// stays revealed apart from the others. Run after `images_as_links`, before
/// the other node passes.
pub fn spoiler_nodes(source: &str, scope: &str) -> String {
    let found = spoilers::spans(source);
    if found.is_empty() {
        return source.to_owned();
    }
    let scope = clean_scope(scope);
    let mut out = String::with_capacity(source.len() + 32 * found.len());
    let mut at = 0;
    STATE.with(|s| {
        let state = s.borrow();
        for (n, span) in found.iter().enumerate() {
            out.push_str(&source[at..span.whole.start]);
            let inner = &source[span.inner.clone()];
            if state.revealed.contains(&key(&scope, n)) {
                out.push_str(inner);
            } else {
                out.push_str(&format!("![{}]({SCHEME}{scope}/{n})", cover_words(inner)));
            }
            at = span.whole.end;
        }
    });
    out.push_str(&source[at..]);
    out
}

/// Text with its spoilers blanked out, for a notification.
pub fn hide(text: &str) -> String {
    spoilers::hide(text)
}

struct Cover {
    key: String,
    words: String,
}

/// Draws covered spoilers: a block of the text's colour, its words see-through.
pub struct Plugin;

impl gpui_kit::component::text::MarkdownPlugin for Plugin {
    fn name(&self) -> &str {
        "fuwa-spoiler"
    }

    fn parse(
        &self,
        node: &gpui_kit::component::text::markdown_ast::Node,
        _: &gpui_kit::component::text::MarkdownParseContext<'_>,
    ) -> Option<gpui_kit::component::text::MarkdownNode> {
        let gpui_kit::component::text::markdown_ast::Node::Image(image) = node else { return None };
        let key = image.url.strip_prefix(SCHEME)?.to_owned();
        let shade: String = "▒".repeat(image.alt.trim().chars().count().clamp(3, 12));
        Some(
            gpui_kit::component::text::MarkdownNode::new("fuwa-spoiler", Cover { key, words: image.alt.clone() })
                .text(shade.clone())
                .markdown(shade)
                .accessibility_label(crate::core::i18n::t("chat.spoiler.reveal")),
        )
    }

    fn render_inline(
        &self,
        node: &gpui_kit::component::text::MarkdownNode,
        _: &gpui_kit::component::text::InlineRenderContext,
        _: &mut gpui_kit::Window,
        cx: &mut gpui_kit::App,
    ) -> Option<gpui_kit::component::text::InlineElement> {
        let cover = node.data::<Cover>()?;
        let p = crate::ui::widgets::pal(cx);
        let fading = STATE.with(|s| s.borrow().revealing.contains(&cover.key));
        let hover = alpha(p.foreground, 0.72);
        let key = cover.key.clone();
        let el = div()
            .id(SharedString::from(format!("spoiler|{}", cover.key)))
            .px(px(2.0))
            .rounded(px(4.0))
            .bg(alpha(p.foreground, 0.82))
            .text_color(gpui_kit::transparent_black())
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .tooltip(|window, cx| {
                crate::ui::overlay::Tip::new(crate::core::i18n::t("chat.spoiler.reveal")).build(window, cx)
            })
            // A spoiler inside a link or a clickable row reveals, and does nothing else.
            .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                reveal(key.clone(), cx);
            })
            .child(cover.words.clone());
        Some(gpui_kit::component::text::InlineElement::new(if fading {
            motion::once(el, SharedString::from(format!("spoiler-fade|{}", cover.key)), FADE, |el, t| {
                el.opacity(1.0 - t)
            })
        } else {
            gpui_kit::IntoElement::into_any_element(el)
        }))
    }
}

// ───────────────────────── Files marked as spoilers ─────────────────────────

/// Where a file marked as a spoiler stands: covered, its cover fading, or shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shown {
    Covered,
    Fading,
    Revealed,
}

/// A file's place among the revealed spoilers, by its media id (one file
/// belongs to one message, so revealing it reveals it only there).
fn file_key(id: &str) -> String {
    format!("file:{}", clean_scope(id))
}

/// Where the spoiler file `id` stands; a file that isn't one is always shown.
pub fn file_shown(id: &str, spoiler: bool) -> Shown {
    if !spoiler {
        return Shown::Revealed;
    }
    let key = file_key(id);
    STATE.with(|s| {
        let s = s.borrow();
        if s.revealed.contains(&key) {
            Shown::Revealed
        } else if s.revealing.contains(&key) {
            Shown::Fading
        } else {
            Shown::Covered
        }
    })
}

/// Lifts a spoiler file's cover.
pub fn reveal_file(id: &str, cx: &mut gpui_kit::App) {
    reveal(file_key(id), cx);
}

/// What covers a spoiler picture until it's clicked, filling the picture's
/// box: a tinted sheet with "Spoiler" on it, in place of the picture (whose
/// pixels are never drawn before then). The first click only reveals it.
/// While `fading`, it's drawn over the picture and fades away.
pub fn picture_cover(id: &str, file: &str, fading: bool, p: &crate::ui::theme::Palette) -> gpui_kit::AnyElement {
    let hover = crate::ui::theme::mix(p.muted, p.foreground, 0.12);
    let file = file.to_owned();
    let el = div()
        .id(SharedString::from(format!("file-spoiler|{id}")))
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .rounded(crate::ui::theme::radius_xl())
        .flex()
        .items_center()
        .justify_center()
        .bg(crate::ui::theme::mix(p.muted, p.foreground, 0.06))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .tooltip(|window, cx| {
            crate::ui::overlay::Tip::new(crate::core::i18n::t("chat.attachments.spoiler")).build(window, cx)
        })
        // Inside a clickable picture it reveals, and does nothing else.
        .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            reveal_file(&file, cx);
        })
        .child(spoiler_pill());
    fade(el, id, fading)
}

/// The "Spoiler" pill a covered file shows.
pub fn spoiler_pill() -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .px(px(12.0))
        .py(px(4.0))
        .rounded_full()
        .bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.6))
        .text_color(gpui_kit::white())
        .text_xs()
        .font_weight(gpui_kit::FontWeight::BOLD)
        .child(crate::ui::widgets::icon("eye-off").size(px(14.0)))
        .child(crate::core::i18n::t("chat.files.spoilerBadge"))
}

/// A cover fading away as it's revealed, at once with reduced motion.
fn fade(el: gpui_kit::Stateful<gpui_kit::Div>, id: &str, fading: bool) -> gpui_kit::AnyElement {
    if fading {
        motion::once(el, SharedString::from(format!("file-spoiler-fade|{id}")), FADE, |el, t| el.opacity(1.0 - t))
    } else {
        gpui_kit::IntoElement::into_any_element(el)
    }
}

/// A spoiler file's name while it's covered: shade as long as the name
/// (about), like a text spoiler's cover, so the name itself is never drawn.
pub fn covered_name(name: &str, p: &crate::ui::theme::Palette) -> impl gpui_kit::IntoElement {
    let shade: String = "▒".repeat(name.chars().count().clamp(3, 24));
    div()
        .px(px(2.0))
        .rounded(px(4.0))
        .bg(alpha(p.foreground, 0.82))
        .text_color(gpui_kit::transparent_black())
        .child(shade)
}

/// Lifts a spoiler's cover: it fades, then its words take its place.
fn reveal(key: String, cx: &mut gpui_kit::App) {
    let Some(app) = crate::ui::mentions::app() else { return };
    let show = move |key: String, cx: &mut gpui_kit::App| {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.revealing.remove(&key);
            s.revealed.insert(key);
        });
        let _ = app.update(cx, |this, cx| {
            this.sync_list(cx);
            cx.notify();
        });
    };
    if cx.reduce_motion() {
        show(key, cx);
        return;
    }
    if !STATE.with(|s| s.borrow_mut().revealing.insert(key.clone())) {
        return;
    }
    cx.refresh_windows();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(FADE).await;
        cx.update(|cx| show(key, cx));
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covered_spoilers_become_nodes() {
        assert_eq!(spoiler_nodes("a ||b|| c", "m1"), format!("a ![b]({SCHEME}m1/0) c"));
        assert_eq!(
            spoiler_nodes("||**hi** <@01J9>|| and ||x||", "m|2"),
            format!("![hi @member]({SCHEME}m-2/0) and ![x]({SCHEME}m-2/1)")
        );
        assert_eq!(spoiler_nodes("no spoilers || here", "m"), "no spoilers || here");
    }

    #[test]
    fn revealed_ones_are_their_words() {
        STATE.with(|s| s.borrow_mut().revealed.insert("r/1".into()));
        assert_eq!(spoiler_nodes("||a|| ||**b**||", "r"), format!("![a]({SCHEME}r/0) **b**"));
        assert_eq!(revealed_in("r"), ["r/1"]);
    }

    #[test]
    fn spoiler_files_stay_covered_until_revealed() {
        assert_eq!(file_shown("01JFILE", false), Shown::Revealed);
        assert_eq!(file_shown("01JFILE", true), Shown::Covered);
        STATE.with(|s| s.borrow_mut().revealing.insert(file_key("01JFILE")));
        assert_eq!(file_shown("01JFILE", true), Shown::Fading);
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.revealing.clear();
            s.revealed.insert(file_key("01JFILE"));
        });
        assert_eq!(file_shown("01JFILE", true), Shown::Revealed);
        assert_eq!(file_shown("01JOTHER", true), Shown::Covered);
        // A file's reveal is apart from any message's text spoilers.
        assert!(revealed_in("01JFILE").is_empty());
        assert_eq!(file_key("a/b"), "file:a-b");
    }
}
