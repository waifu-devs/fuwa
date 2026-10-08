//! What the settings pages share between frames, beside each page's own form.

/// Every page's own state.
#[derive(Default)]
pub(crate) struct PageState {
    /// Profile effects playing, by where they are.
    pub effects: std::collections::HashMap<String, gpui_kit::Entity<crate::ui::profile_effect::EffectView>>,
    /// The effect tile under the pointer.
    pub effect_hover: Option<String>,
    /// The menu open on the page, by id.
    pub menu: Option<String>,
    /// A dialog a page put up, drawn over the whole screen.
    pub overlay: Option<gpui_kit::AnyElement>,
    /// A theme to open in the editor (from a card's menu), and whether it's a copy.
    pub pending_edit: Option<(crate::core::themes::Theme, bool)>,
}
