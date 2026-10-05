//! Writing, beyond typing: the @ list, editing a message in place, and the
//! keys both take before the text fields see them.

use gpui_kit::{Context, Focusable as _, Keystroke, Window};

use crate::core::dms::Content;
use crate::pb;
use crate::ui::app::{FuwaApp, Picker, Target};
use crate::ui::chat::Row;
use crate::ui::{emoji, mentions};

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
