//! Writing, beyond typing: the @ list, editing a message in place, and the
//! keys both take before the text fields see them.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, Focusable as _, FontWeight, Hsla, InteractiveElement as _, IntoElement, Keystroke,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled, Window, div, px, rgb,
};

use crate::core::dms::Content;
use crate::core::i18n::t;
use crate::pb;
use crate::ui::app::{FuwaApp, Picker, Target};
use crate::ui::chat::Row;
use crate::ui::chat::emoji_glyph;
use crate::ui::mentions::Pick;
use crate::ui::motion;
use crate::ui::text::{WIDE, tracked};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_xl};
use crate::ui::widgets::{avatar, icon};
use crate::ui::{emoji, mentions};

/// A row of the lists over the composer.
pub(crate) const ABOVE_ROW: f32 = 36.0;

impl FuwaApp {
    /// Takes the keys the @ list and editing use. True when it took the key.
    pub(crate) fn intercept(&mut self, key: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let m = &key.modifiers;
        let bare = !(m.shift || m.control || m.alt || m.platform || m.function);
        if self.context_menu_keys(key, window, cx) {
            return true;
        }
        // Shift+F10 and the Menu key open the menu of what the pointer is over, or of where you are.
        let shift_only = m.shift && !(m.control || m.alt || m.platform);
        if (key.key == "f10" && shift_only) || (bare && matches!(key.key.as_str(), "menu" | "contextmenu")) {
            self.open_context_menu_here(window, cx);
            return true;
        }
        if self.search_keys(key, window, cx) {
            return true;
        }
        if key.key == "escape" && self.close_answer_picker(cx) {
            return true;
        }
        if key.key == "escape" && self.discard_recording(cx) {
            return true;
        }
        // Enter sends a recording started with a tap.
        if key.key == "enter" && bare && self.tapped_recording() && self.recording_here() {
            self.send_recording(cx);
            return true;
        }
        if key.key == "escape" && self.close_time_picker(window, cx) {
            return true;
        }
        if self.gifs.open && key.key == "escape" {
            self.close_gifs(cx);
            self.composer.update(cx, |state, cx| state.focus(window, cx));
            return true;
        }
        if self.emoji_open && key.key == "escape" {
            self.close_emoji(window, cx);
            return true;
        }
        if self.emoji_open
            && bare
            && self.emoji_query.read(cx).focus_handle(cx).is_focused(window)
            && self.emoji_key(&key.key, window, cx)
        {
            return true;
        }
        // Escape in a thread's reply box closes the thread, back to the channel.
        if key.key == "escape" && self.threads.reply.read(cx).focus_handle(cx).is_focused(window) {
            self.close_thread(cx);
            self.composer.update(cx, |state, cx| state.focus(window, cx));
            return true;
        }
        if self.edit_box.read(cx).focus_handle(cx).is_focused(window) {
            if key.key == "escape" {
                self.cancel_edit(window, cx);
                return true;
            }
            return false;
        }
        if self.picker.is_none() && self.command_keys(key, window, cx) {
            return true;
        }
        if !self.composer.read(cx).focus_handle(cx).is_focused(window) {
            return false;
        }
        if let Some(picker) = &mut self.picker {
            let count = picker.options.len();
            match key.key.as_str() {
                "down" if bare => picker.active = (picker.active + 1) % count,
                "up" if bare => picker.active = (picker.active + count - 1) % count,
                "enter" | "tab" if bare => {
                    let pick = picker.options.get(picker.active).cloned();
                    if let Some(pick) = pick {
                        self.pick_mention(pick, window, cx);
                    }
                }
                "escape" => {
                    self.picker_dismissed = Some(picker.start);
                    self.picker = None;
                }
                _ => return false,
            }
            cx.notify();
            return true;
        }
        // With the Chat setting on Ctrl+Enter, that sends and Enter adds a line.
        let with = self.core.prefs().send_with;
        if with == crate::core::config::SendWith::ModEnter && crate::ui::composer::sends_message(key, with) {
            self.send_now(window, cx);
            return true;
        }
        if key.key == "up" && bare && self.composer.read(cx).value().is_empty() {
            return self.edit_last(window, cx);
        }
        false
    }

    /// Opens, moves or closes the @ list (or the : one) as the text changes.
    pub(crate) fn update_picker(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.target() else {
            self.picker = None;
            return;
        };
        let (text, caret) = {
            let state = self.composer.read(cx);
            (state.value().to_string(), state.cursor())
        };
        let mention = match &target {
            Target::Channel { .. } => mentions::token(&text, caret),
            Target::Dm { .. } | Target::Secure { .. } => None,
        };
        let Some((start, query, kind)) = mention
            .map(|(start, query)| (start, query, '@'))
            .or_else(|| emoji::typing(&text, caret).map(|(start, query)| (start, query, ':')))
        else {
            self.picker = None;
            self.picker_dismissed = None;
            return;
        };
        if self.picker_dismissed == Some(start) {
            self.picker = None;
            return;
        }
        let tone = self.core.prefs().skin_tone;
        let options = self.core.shared.read(|s| match &target {
            Target::Channel { key, server, channel } => s
                .instance(key)
                .map(|i| match kind {
                    '@' => {
                        let everyone = i.access(server).has_in(channel, pb::Permission::MentionEveryone);
                        mentions::options(i, server, &query, everyone)
                    }
                    _ => {
                        let catalog = emoji::Catalog::of(&i.servers, &i.emojis, server);
                        emoji::search(&query, &catalog, tone, 8).into_iter().map(mentions::Pick::Emoji).collect()
                    }
                })
                .unwrap_or_default(),
            Target::Dm { .. } | Target::Secure { .. } => emoji::search(&query, &emoji::Catalog::default(), tone, 8)
                .into_iter()
                .map(mentions::Pick::Emoji)
                .collect(),
        });
        if options.is_empty() {
            self.picker = None;
            return;
        }
        let active = match &self.picker {
            Some(old) if old.start == start => old.active.min(options.len() - 1),
            _ => 0,
        };
        self.picker = Some(Picker { start, options, active });
    }

    /// Puts a pick from the @ list in the box, in place of what was typed.
    pub(crate) fn pick_mention(&mut self, pick: mentions::Pick, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.picker.take() else { return };
        if let mentions::Pick::Role { id, name, .. } = &pick {
            self.picked_roles.retain(|(n, _)| n != name);
            self.picked_roles.push((name.clone(), id.clone()));
        }
        let insert = format!("{} ", pick.insert());
        self.composer.update(cx, |state, cx| {
            let caret = state.cursor();
            state.set_selected_range(picker.start..caret, cx);
            state.replace(insert, window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    /// The text to send, with picked roles and the server's emoji as their tokens.
    pub(crate) fn encode_mentions(&mut self, text: &str) -> String {
        let out = mentions::encode(text, &self.picked_roles);
        self.picked_roles.clear();
        self.encode_emoji(&out)
    }

    /// `:name:` of the emoji you can use in the open server (its own and
    /// your other servers') as their tokens.
    pub(crate) fn encode_emoji(&self, text: &str) -> String {
        let Some(Target::Channel { key, server, .. }) = self.target() else { return text.to_owned() };
        self.core.shared.read(|s| match s.instance(&key) {
            Some(i) => emoji::Catalog::of(&i.servers, &i.emojis, &server).encode(text),
            None => text.to_owned(),
        })
    }

    /// The @ list (the web's `MentionPicker`), over the composer: people,
    /// roles and @everyone, or emoji after a colon. The pointer lights a row
    /// as the arrows do.
    /// `going` once it has closed, as it leaves ([`motion::kept`]).
    pub(crate) fn picker_list(
        &self,
        picker: Picker,
        going: Option<f32>,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let emoji = matches!(picker.options.first(), Some(Pick::Emoji(_)));
        // The lit row's fill glides from row to row (the web's `layoutId="mention-active"`).
        let lit = Self::above_glide(format!("mention-lit|{}", picker.start), picker.active, p, cx);
        let mut list = div().relative().flex().flex_col().child(lit).child(Self::above_title(
            if emoji { "face-slightly-smiling" } else { "at-sign" },
            &t(if emoji { "chat.mentionPicker.emoji" } else { "chat.mentionPicker.mention" }),
            p,
        ));
        for (n, pick) in picker.options.iter().enumerate() {
            let side = |text: String, glyph: Option<&str>| {
                div()
                    .ml_auto()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_xs()
                    .text_color(p.muted_foreground)
                    .children(glyph.map(|g| icon(g).size(px(12.0))))
                    .child(text)
            };
            let slot = || div().size(px(24.0)).flex_none().flex().items_center().justify_center();
            let (lead, name, sub): (AnyElement, String, Option<gpui_kit::Div>) = match pick {
                Pick::Member { user, name } => (
                    avatar(Some(user), 24.0, p).into_any_element(),
                    name.clone(),
                    Some(side(format!("@{}", user.username), None)),
                ),
                Pick::Role { name, color, .. } => (
                    slot()
                        .child(
                            div()
                                .size(px(10.0))
                                .rounded_full()
                                .bg(color.map(|c| Hsla::from(rgb(c))).unwrap_or(p.muted_foreground.into())),
                        )
                        .into_any_element(),
                    format!("@{name}"),
                    Some(side(t("chat.mentionPicker.role"), Some("shield"))),
                ),
                Pick::Everyone(which) => (
                    slot()
                        .rounded_full()
                        .bg(alpha(p.primary, 0.15))
                        .text_color(p.primary)
                        .child(icon("at-sign").size(px(14.0)))
                        .into_any_element(),
                    format!("@{which}"),
                    Some(side(t("chat.mentionPicker.everyone"), None)),
                ),
                Pick::Emoji(choice) => (
                    emoji_glyph(choice, 22.0),
                    format!(":{}:", choice.name),
                    match (&choice.from, &choice.url) {
                        (Some(server), _) => Some(side(server.clone(), None)),
                        (None, Some(_)) => Some(side(t("chat.mentionPicker.thisServer"), None)),
                        (None, None) => None,
                    },
                ),
            };
            let pick = pick.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("pick|{}", pick.id())))
                    .h(px(36.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(radius_xl())
                    .text_sm()
                    .cursor_pointer()
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered
                            && let Some(picker) = &mut this.picker
                            && picker.active != n
                        {
                            picker.active = n;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| this.pick_mention(pick.clone(), window, cx)))
                    .child(lead)
                    .child(div().min_w_0().truncate().font_weight(FontWeight::BOLD).child(name))
                    .children(sub),
            );
        }
        Self::above_composer(list, format!("picker-{}", picker.start), going, p)
    }

    /// A list floating just above the composer, as wide as its box (the web's
    /// `absolute inset-x-0 bottom-full mb-2 rounded-2xl border bg-popover p-1.5 shadow-xl`).
    /// `going` once it has closed: it sinks 6px and shrinks to 98% as it fades.
    pub(crate) fn above_composer(
        list: gpui_kit::Div,
        id: impl Into<SharedString>,
        going: Option<f32>,
        p: &Palette,
    ) -> AnyElement {
        let card = div()
            .mb(px(8.0))
            .overflow_hidden()
            .rounded(radius_2xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .shadow(vec![
                gpui_kit::BoxShadow {
                    color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.1),
                    offset: gpui_kit::point(px(0.0), px(20.0)),
                    blur_radius: px(25.0),
                    spread_radius: px(-5.0),
                    inset: false,
                },
                gpui_kit::BoxShadow {
                    color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.1),
                    offset: gpui_kit::point(px(0.0), px(8.0)),
                    blur_radius: px(10.0),
                    spread_radius: px(-6.0),
                    inset: false,
                },
            ])
            .child(list.p(px(6.0)));
        div()
            .absolute()
            .left(px(16.0))
            .right(px(16.0))
            .bottom(gpui_kit::relative(1.0))
            // The web's `origin-bottom`: it grows up out of the box as it rises.
            .child(motion::pop_in(card, ElementId::Name(id.into()), (0.5, 1.0), 0.97, 8.0))
            // `exit={{ opacity: 0, y: 6, scale: 0.98 }}`.
            .when_some(going, |el, t| {
                crate::ui::chat::closing(
                    el,
                    t,
                    crate::ui::chat::Gone { scale: 0.98, x: 0.0, y: 6.0, origin: (0.5, 1.0) },
                )
            })
            .into_any_element()
    }

    /// The fill behind such a list's lit row (`bg-primary/12`), gliding to
    /// row `n` of its 36px rows under the title. `id` names the list while
    /// it's open, so a new one starts where its first row is lit.
    pub(crate) fn above_glide(id: String, n: usize, p: &Palette, cx: &mut gpui_kit::App) -> AnyElement {
        // The list's 6px, then the title's 20.
        let top = 6.0 + 20.0 + n as f32 * ABOVE_ROW;
        motion::glide(
            div()
                .absolute()
                .left(px(6.0))
                .right(px(6.0))
                .h(px(ABOVE_ROW))
                .rounded(radius_xl())
                .bg(alpha(p.primary, 0.12)),
            id,
            top,
            cx,
            |el, top| el.top(px(top)),
        )
    }

    /// Such a list's small title: an icon and a word in capitals.
    pub(crate) fn above_title(glyph: &str, text: &str, p: &Palette) -> gpui_kit::Div {
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .px(px(8.0))
            .pt(px(2.0))
            .pb(px(4.0))
            .text_size(px(10.4))
            .line_height(px(14.0))
            .font_weight(FontWeight::EXTRA_BOLD)
            .text_color(p.muted_foreground)
            .when(!glyph.is_empty(), |el| el.child(icon(glyph).size(px(12.0))))
            .child(tracked(text.to_uppercase(), WIDE))
    }

    /// The message box's right-click menu (the web's `composerMenu`): cut and
    /// copy what's selected, paste, select all, then an emoji at the caret.
    pub(crate) fn composer_items(&self, cx: &gpui_kit::App) -> crate::ui::context_menu::Built {
        use crate::ui::context_menu::{Built, Item, run};
        use gpui_kit::component::input::{Copy, Cut, Paste, SelectAll};
        let (selected, has_text) = {
            let state = self.composer.read(cx);
            (!state.selected_value().is_empty(), !state.value().is_empty())
        };
        let mod_key = |k: &str| format!("{}+{k}", if cfg!(target_os = "macos") { "⌘" } else { "Ctrl" });
        // Each acts on the box, so it gets the focus back first.
        let on_box = |action: fn() -> Box<dyn gpui_kit::Action>| {
            run(move |this, window, cx| {
                this.composer.update(cx, |state, cx| state.focus(window, cx));
                window.dispatch_action(action(), cx);
            })
        };
        let mut edit = Vec::new();
        if selected {
            edit.push(
                Item::act(t("workspace.menu.composer.cut"), "scissors", on_box(|| Box::new(Cut))).hint(mod_key("X")),
            );
            edit.push(
                Item::act(t("workspace.menu.composer.copy"), "copy", on_box(|| Box::new(Copy))).hint(mod_key("C")),
            );
        }
        edit.push(
            Item::act(t("workspace.menu.composer.paste"), "clipboard-paste", on_box(|| Box::new(Paste)))
                .hint(mod_key("V")),
        );
        if has_text {
            edit.push(
                Item::act(t("workspace.menu.composer.selectAll"), "text-cursor-input", on_box(|| Box::new(SelectAll)))
                    .hint(mod_key("A")),
            );
        }
        let emoji = Item::act(
            t("workspace.menu.composer.emoji"),
            "face-slightly-smiling",
            run(|this, window, cx| this.open_emoji(window, cx)),
        );
        Built::of(vec![edit, vec![emoji]])
    }

    // ───────────────────────── Editing ─────────────────────────

    /// Starts editing your last message in the open list.
    fn edit_last(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let last = self.rows.iter().rev().find_map(|r| match r {
            Row::Msg(m) if m.mine && !m.pending && !m.unreadable => Some(m.id.clone()),
            _ => None,
        });
        match last {
            Some(id) => {
                self.start_edit(id, window, cx);
                true
            }
            None => false,
        }
    }

    pub(crate) fn start_edit(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(content) = self.rows.iter().chain(self.threads.rows.iter()).find_map(|r| match r {
            Row::Msg(m) if m.id == id => Some(m.content.clone()),
            _ => None,
        }) else {
            return;
        };
        self.editing = Some(id);
        self.edit_in_thread = false;
        self.edit_box.update(cx, |state, cx| {
            state.set_value(content, window, cx);
            state.focus(window, cx);
        });
        self.sync_list(cx);
        self.sync_thread(cx);
        cx.notify();
    }

    pub(crate) fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        if self.edit_in_thread && self.threads.open.is_some() {
            self.threads.reply.update(cx, |state, cx| state.focus(window, cx));
        } else {
            self.composer.update(cx, |state, cx| state.focus(window, cx));
        }
        self.sync_list(cx);
        self.sync_thread(cx);
        cx.notify();
    }

    /// Saves an edit. Leaving it unchanged (or empty) just stops editing.
    pub(crate) fn save_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.editing.clone() else { return };
        let text = self.edit_box.read(cx).value().trim().to_owned();
        let before = self.rows.iter().chain(self.threads.rows.iter()).find_map(|r| match r {
            Row::Msg(m) if m.id == id => Some(m.content.clone()),
            _ => None,
        });
        self.cancel_edit(window, cx);
        if text.is_empty() || before.as_deref() == Some(text.as_str()) {
            return;
        }
        let core = self.core.clone();
        match self.target() {
            Some(Target::Channel { key, server, channel }) => {
                let text = self.encode_emoji(&text);
                let edit = async move { core.edit_message(&key, &server, &channel, &id, &text).await };
                self.run(cx, edit, |this, result, cx| {
                    if let Err(err) = result {
                        this.toast("circle-alert", "Couldn't edit that".into(), err.message, None, None, cx);
                    }
                })
            }
            Some(Target::Dm { key, conversation } | Target::Secure { key, channel: conversation, .. }) => {
                let Ok(sequence) = id.parse::<i64>() else { return };
                self.run(
                    cx,
                    async move { core.send_dm(&key, &conversation, Content::Edit { sequence, text }).await },
                    |this, result, cx| {
                        if let Err(err) = result {
                            this.toast("circle-alert", "Couldn't edit that".into(), err.0, None, None, cx);
                        }
                    },
                )
            }
            None => {}
        }
    }
}
