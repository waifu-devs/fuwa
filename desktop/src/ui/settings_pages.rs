//! What the account pages keep between frames, so each page's file holds
//! only its drawing and its calls.

use gpui_kit::{AnyElement, Context, IntoElement, ParentElement as _, Styled as _, Window, div};

use crate::core::config::Prefs;
use crate::ui::settings::SettingsView;
use crate::ui::theme::Palette;

/// Every page's own state.
#[derive(Default)]
pub(crate) struct PageState {
    /// Profile effects playing, by where they are.
    pub effects: std::collections::HashMap<&'static str, gpui_kit::Entity<crate::ui::profile_effect::EffectView>>,
    /// The effect tile under the pointer.
    pub effect_hover: Option<String>,
    /// The menu open on the page, by id.
    pub menu: Option<String>,
}

impl SettingsView {
    pub(crate) fn themes_page(
        &mut self,
        _prefs: &Prefs,
        p: &Palette,
        _w: &mut Window,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        todo_page(p)
    }
    pub(crate) fn sign_in_page(&mut self, p: &Palette, _w: &mut Window, _cx: &mut Context<Self>) -> AnyElement {
        todo_page(p)
    }
    pub(crate) fn message_backup(
        &mut self,
        _key: &str,
        p: &Palette,
        _w: &mut Window,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        todo_page(p)
    }
    pub(crate) fn linked_page(&mut self, p: &Palette, _cx: &mut Context<Self>) -> AnyElement {
        todo_page(p)
    }
    pub(crate) fn agents_page(&mut self, p: &Palette, _w: &mut Window, _cx: &mut Context<Self>) -> AnyElement {
        todo_page(p)
    }
    pub(crate) fn privacy_page(&mut self, p: &Palette, _w: &mut Window, _cx: &mut Context<Self>) -> AnyElement {
        todo_page(p)
    }
    pub(crate) fn security_page(&mut self, p: &Palette, _w: &mut Window, _cx: &mut Context<Self>) -> AnyElement {
        todo_page(p)
    }
}

fn todo_page(p: &Palette) -> AnyElement {
    div().text_color(p.muted_foreground).child("…").into_any_element()
}
