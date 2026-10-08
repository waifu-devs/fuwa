//! Picking an avatar decoration (docs/profile-items.md), as the web's
//! `settings/account/DecorationPicker.tsx`: None (or "Use my profile's"),
//! then each decoration offered, drawn around your own avatar.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px,
};

use crate::core::i18n::t;
use crate::pb;
use crate::ui::settings::SettingsView;
use crate::ui::theme::{Palette, radius_2xl, radius_xl};
use crate::ui::widgets::{avatar, decorated, icon};

impl SettingsView {
    /// The grid of decorations: five to a row across `width`, the picked one ringed.
    /// `place` keeps each picker's tiles apart.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn decoration_picker(
        &mut self,
        place: &'static str,
        value: &str,
        me: &pb::User,
        items: &[pb::ProfileItem],
        none_label: Option<String>,
        ready: bool,
        width: f32,
        p: &Palette,
        cx: &mut Context<Self>,
        pick: fn(&mut SettingsView, String, &mut Context<SettingsView>),
    ) -> AnyElement {
        let cols = 5usize;
        let gap = 8.0;
        let tile_w = (width - gap * (cols as f32 - 1.0)) / cols as f32;
        let face_w = tile_w - 8.0;
        let size = (face_w * 0.55).round();
        let mut tiles: Vec<(String, String, Option<&pb::ProfileItem>)> =
            vec![(String::new(), none_label.unwrap_or_else(|| t("accountsettings.effects.none")), None)];
        tiles.extend(items.iter().map(|i| (i.id.clone(), i.name.clone(), Some(i))));
        let mut rows = div().flex().flex_col().gap(px(gap)).when(!ready, |el| el.opacity(0.5));
        for chunk in tiles.chunks(cols) {
            let mut row = div().flex().gap(px(gap));
            for (id, label, item) in chunk {
                let active = value == id;
                let face = decorated(avatar(Some(me), size, p), size, item.map(|i| i.picture_url.as_str()));
                let face = match item {
                    Some(_) => face,
                    None => face.child(
                        div()
                            .absolute()
                            .right(px(-4.0))
                            .bottom(px(-4.0))
                            .size(px(24.0))
                            .rounded_full()
                            .bg(p.card)
                            .shadow(crate::ui::settings_controls::shadow_sm())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(p.muted_foreground)
                            .child(icon("ban").size(px(14.0))),
                    ),
                };
                let mut square = div()
                    .relative()
                    .w(px(face_w))
                    .h(px(face_w))
                    .rounded(radius_xl())
                    .border_1()
                    .border_color(p.border)
                    .bg(p.card)
                    .shadow(crate::ui::settings_controls::shadow_sm())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(face);
                if active {
                    square = square
                        .child(div().absolute().inset_0().rounded(radius_xl()).border_2().border_color(p.primary));
                }
                let pick_id = id.clone();
                row = row.child(
                    div()
                        .id(SharedString::from(format!("decoration-{place}-{id}")))
                        .w(px(tile_w))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(6.0))
                        .p(px(4.0))
                        .rounded(radius_2xl())
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .hover(|s| s.top(px(-3.0)))
                        .active(|s| s.top(px(1.0)))
                        .on_click(cx.listener(move |this, _, _, cx| pick(this, pick_id.clone(), cx)))
                        .child(square)
                        .child(
                            div()
                                .max_w_full()
                                .truncate()
                                .text_color(if active { p.foreground } else { p.muted_foreground })
                                .child(label.clone()),
                        ),
                );
            }
            rows = rows.child(row);
        }
        rows.into_any_element()
    }
}
