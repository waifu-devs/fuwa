//! Writing, beyond typing: the @ list, editing a message in place, and the
//! keys both take before the text fields see them.

use gpui_kit::{Context, Focusable as _, Keystroke, Window};

use crate::core::dms::Content;
use crate::pb;
use crate::ui::app::{FuwaApp, Picker, Target};
use crate::ui::chat::Row;
use crate::ui::mentions;

impl FuwaApp {
    /// Takes the keys the @ list and editing use. True when it took the key.
    pub(crate) fn intercept(&mut self, key: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let m = &key.modifiers;
        let bare = !(m.shift || m.control || m.alt || m.platform || m.function);
        if self.edit_box.read(cx).focus_handle(cx).is_focused(window) {
            if key.key == "escape" {
                self.cancel_edit(window, cx);
                return true;
            }
            return false;
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

    /// Opens, moves or closes the @ list as the text changes.
    pub(crate) fn update_picker(&mut self, cx: &mut Context<Self>) {
        let Some(Target::Channel { key, server, channel }) = self.target() else {
            self.picker = None;
            return;
        };
        let (text, caret) = {
            let state = self.composer.read(cx);
            (state.value().to_string(), state.cursor())
        };
        let Some((start, query)) = mentions::token(&text, caret) else {
            self.picker = None;
            self.picker_dismissed = None;
            return;
        };
        if self.picker_dismissed == Some(start) {
            self.picker = None;
            return;
        }
        let options = self.core.shared.read(|s| {
            s.instance(&key)
                .map(|i| {
                    let everyone = i.access(&server).has_in(&channel, pb::Permission::MentionEveryone);
                    mentions::options(i, &server, &query, everyone)
                })
                .unwrap_or_default()
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

    /// The text to send, with picked roles as their tokens.
    pub(crate) fn encode_mentions(&mut self, text: &str) -> String {
        let out = mentions::encode(text, &self.picked_roles);
        self.picked_roles.clear();
        out
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
        let Some(content) = self.rows.iter().find_map(|r| match r {
            Row::Msg(m) if m.id == id => Some(m.content.clone()),
            _ => None,
        }) else {
            return;
        };
        self.editing = Some(id);
        self.edit_box.update(cx, |state, cx| {
            state.set_value(content, window, cx);
            state.focus(window, cx);
        });
        self.sync_list(cx);
        cx.notify();
    }

    pub(crate) fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        self.composer.update(cx, |state, cx| state.focus(window, cx));
        self.sync_list(cx);
        cx.notify();
    }

    /// Saves an edit. Leaving it unchanged (or empty) just stops editing.
    pub(crate) fn save_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.editing.clone() else { return };
        let text = self.edit_box.read(cx).value().trim().to_owned();
        let before = self.rows.iter().find_map(|r| match r {
            Row::Msg(m) if m.id == id => Some(m.content.clone()),
            _ => None,
        });
        self.cancel_edit(window, cx);
        if text.is_empty() || before.as_deref() == Some(text.as_str()) {
            return;
        }
        let core = self.core.clone();
        match self.target() {
            Some(Target::Channel { key, server, .. }) => {
                self.run(cx, async move { core.edit_message(&key, &server, &id, &text).await }, |this, result, cx| {
                    if let Err(err) = result {
                        this.toast("circle-alert", "Couldn't edit that".into(), err.message, None, None, cx);
                    }
                })
            }
            Some(Target::Dm { key, conversation }) => {
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
