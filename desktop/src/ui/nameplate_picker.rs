//! Picking a nameplate (docs/profile-items.md), as the web's
//! `settings/account/NameplatePicker.tsx`: None (or "Use my profile's"),
//! then each nameplate offered, drawn as your own row in the member list.

use std::collections::HashMap;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px,
};

use crate::core::i18n::t;
use crate::pb;
use crate::ui::decoration_picker::{Ring, lock_badge, picked_ring};
use crate::ui::settings::SettingsView;
use crate::ui::theme::{Palette, radius_2xl, radius_xl};
use crate::ui::widgets::{avatar, icon, nameplate};

/// A tile's row: as tall as a row in the member list.
const FACE: f32 = 48.0;

impl SettingsView {
    /// The grid of nameplates: two to a row across `width`, the picked one
    /// ringed. `place` keeps each picker's tiles apart.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn nameplate_picker(
        &mut self,
        place: &'static str,
        value: &str,
        me: &pb::User,
        items: &[pb::ProfileItem],
        none_label: Option<String>,
        locked: &HashMap<String, String>,
        ready: bool,
        width: f32,
        p: &Palette,
        cx: &mut Context<Self>,
        pick: fn(&mut SettingsView, String, &mut Context<SettingsView>),
    ) -> AnyElement {
        let cols = if width >= 360.0 { 2usize } else { 1 };
        let gap = 8.0;
        let tile_w = (width - gap * (cols as f32 - 1.0)) / cols as f32;
        let face_w = tile_w - 8.0;
        let mut tiles: Vec<(String, String, Option<&pb::ProfileItem>)> =
            vec![(String::new(), none_label.unwrap_or_else(|| t("accountsettings.effects.none")), None)];
        tiles.extend(items.iter().map(|i| (i.id.clone(), i.name.clone(), Some(i))));
        let picked = tiles.iter().position(|(id, ..)| id == value);
        // A tile: 4px around the row, 6px, then a 16px label.
        let ring = Ring { id: format!("nameplate-ring|{place}"), cols, tile: (tile_w, FACE + 30.0), gap };
        let (gliding, ring) = picked_ring(ring, picked, (face_w, FACE), p.primary.into(), cx);
        let name = crate::core::store::user_name(me);
        let mut rows = div().relative().flex().flex_col().gap(px(gap)).when(!ready, |el| el.opacity(0.5));
        for chunk in tiles.chunks(cols) {
            let mut row = div().flex().gap(px(gap));
            for (id, label, item) in chunk {
                let active = value == id;
                let group = SharedString::from(format!("nameplate-{place}-{id}"));
                let lock = locked.get(id).filter(|_| !active);
                let dim = if lock.is_some() { 0.6 } else { 1.0 };
                let mut face = div()
                    .id("nameplate-face")
                    .relative()
                    .w(px(face_w))
                    .h(px(FACE))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(radius_xl())
                    .border_1()
                    .border_color(p.border)
                    .bg(p.card)
                    .shadow(crate::ui::settings_controls::shadow_sm())
                    .group_hover(group.clone(), |s| s.shadow(crate::ui::profile_card::shadow_md()))
                    .when_some(item.filter(|i| !i.picture_url.is_empty()), |el, item| {
                        el.child(nameplate(&item.picture_url, FACE, radius_xl(), p.card, p.card, group.clone()))
                    })
                    .child(div().flex_none().opacity(dim).child(avatar(Some(me), 32.0, p)))
                    .child(div().flex_1().min_w_0().truncate().text_sm().opacity(dim).child(name.clone()))
                    .when(item.is_none(), |el| {
                        el.child(
                            div()
                                .id("nameplate-none-ban")
                                .flex_none()
                                .text_color(p.muted_foreground)
                                // The ban sign turns a quarter while its tile is pointed at.
                                .group_hover(group.clone(), |s| {
                                    s.rotate(gpui_kit::radians(std::f32::consts::FRAC_PI_2))
                                })
                                .child(icon("ban").size(px(14.0))),
                        )
                    });
                if active && !gliding {
                    face =
                        face.child(div().absolute().inset_0().rounded(radius_xl()).border_2().border_color(p.primary));
                }
                // Sold in the server's shop and not bought: a padlock and its price, and no picking.
                if let Some(price) = lock {
                    face = face.child(lock_badge(price, radius_xl(), p));
                }
                let open = lock.is_none();
                let pick_id = id.clone();
                let fg = p.foreground;
                row = row.child(
                    div()
                        .id(group.clone())
                        .group(group.clone())
                        .w(px(tile_w))
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .p(px(4.0))
                        .rounded(radius_2xl())
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .hover(|s| s.translate_y(px(-2.0)))
                        .when(open, |el| {
                            el.active(|s| s.scale(0.97))
                                .on_click(cx.listener(move |this, _, _, cx| pick(this, pick_id.clone(), cx)))
                        })
                        // A locked one is sold in the server's shop: it opens there (the web's `onLocked`).
                        .when(!open, |el| el.on_click(cx.listener(|this, _, _, cx| this.locked_to_shop(cx))))
                        .child(face)
                        .child(
                            div()
                                .id("nameplate-label")
                                .px(px(4.0))
                                .max_w_full()
                                .truncate()
                                .line_height(px(16.0))
                                .text_color(if active { p.foreground } else { p.muted_foreground })
                                .group_hover(group, move |s| s.text_color(fg))
                                .child(label.clone()),
                        ),
                );
            }
            rows = rows.child(row);
        }
        rows.child(ring).into_any_element()
    }
}
