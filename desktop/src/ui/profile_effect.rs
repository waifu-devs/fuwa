//! Profile effects on screen (the web's `ProfileEffect.tsx` and
//! `EffectPicker.tsx`): the particles `core::profile_effects` plans, drawn as
//! tinted shapes in a small view of their own that redraws only itself while
//! it plays. With reduced motion, or while a picker tile rests, it shows a
//! still. The picker is None, then a tile per effect, each a tiny card.

use std::time::Instant;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Transformation, Window,
    div, px, radians, rgb, size, svg,
};

use crate::core::i18n::t;
use crate::core::profile_effects::{self as fx, Paint, Particle};
use crate::ui::settings::SettingsView;
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_xl};
use crate::ui::widgets::{icon, pal};

/// One playing effect.
pub struct EffectView {
    effect: String,
    seed: String,
    accent: i32,
    width: f32,
    height: f32,
    playing: bool,
    particles: Vec<Particle>,
    fade_at: f32,
    started: Instant,
}

impl EffectView {
    pub fn new() -> Self {
        Self {
            effect: String::new(),
            seed: String::new(),
            accent: -1,
            width: 0.0,
            height: 0.0,
            playing: false,
            particles: Vec::new(),
            fade_at: 0.0,
            started: Instant::now(),
        }
    }

    /// Plays `effect` for `seed` on a card of this size; a new effect (or starting to play) starts the intro again.
    pub fn set(
        &mut self,
        effect: &str,
        seed: &str,
        accent: i32,
        width: f32,
        height: f32,
        playing: bool,
        cx: &mut Context<Self>,
    ) {
        let changed = effect != self.effect || seed != self.seed || width != self.width || height != self.height;
        if changed {
            self.effect = effect.to_owned();
            self.seed = seed.to_owned();
            self.width = width;
            self.height = height;
            self.particles = fx::spec(effect).map(|s| fx::plan(&s, width, height, seed)).unwrap_or_default();
            self.fade_at = fx::idle_fade_at(&self.particles);
        }
        if changed || (playing && !self.playing) {
            self.started = Instant::now();
        }
        if changed || playing != self.playing || accent != self.accent {
            self.playing = playing;
            self.accent = accent;
            cx.notify();
        }
    }
}

fn paint(p: &Palette, paint: Paint, profile: Hsla) -> Hsla {
    match paint {
        Paint::Primary | Paint::Ring => p.primary.into(),
        Paint::Accent => p.accent.into(),
        Paint::Foreground => p.foreground.into(),
        Paint::Card => p.card.into(),
        Paint::Profile => profile,
        Paint::Hex(c) => rgb(c).into(),
    }
}

/// The card's own color: the profile color, or the one picked from the id.
pub fn profile_color(seed: &str, accent: i32) -> Hsla {
    if accent >= 0 {
        rgb(accent as u32).into()
    } else {
        gpui_kit::hsla(crate::ui::widgets::hue_of(seed) / 360.0, 0.85, 0.72, 1.0)
    }
}

impl Render for EffectView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let moving = self.playing && !cx.reduce_motion();
        let elapsed = self.started.elapsed().as_secs_f32() * 1000.0;
        let profile = profile_color(&self.seed, self.accent);
        let mut layer = div().absolute().inset_0().overflow_hidden();
        for particle in &self.particles {
            let at = if moving { particle.at(elapsed, self.fade_at) } else { particle.still };
            let Some(f) = at else { continue };
            if f.opacity <= 0.01 {
                continue;
            }
            let squash = f.flip.map_or(1.0, |d| d.to_radians().cos().abs().max(0.08));
            let color = paint(&p, particle.paint, profile);
            layer = layer.child(
                div()
                    .absolute()
                    .left(px(f.x))
                    .top(px(f.y))
                    .size(px(particle.size))
                    .opacity(f.opacity.clamp(0.0, 1.0))
                    .child(
                        svg()
                            .path(SharedString::from(format!("fx/{}.svg", particle.shape.name())))
                            .size_full()
                            .text_color(color)
                            .with_transformation(
                                Transformation::rotate(radians(f.rotate.to_radians()))
                                    .with_scaling(size(f.scale * f.scale_x, f.scale * squash)),
                            ),
                    ),
            );
        }
        if moving {
            window.request_animation_frame();
        }
        layer
    }
}

/// A built-in effect's line, in the app's language.
pub fn about(id: &str) -> Option<String> {
    fx::IDS.contains(&id).then(|| t(&format!("accountsettings.effects.about.{id}")))
}

fn name(id: &str) -> String {
    t(&format!("accountsettings.effects.name.{id}"))
}

impl SettingsView {
    /// An effect over a card, playing in its own view: `place` keeps one view per spot.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn effect_layer(
        &mut self,
        place: &'static str,
        effect: &str,
        seed: &str,
        accent: i32,
        (width, height): (f32, f32),
        playing: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view: Entity<EffectView> =
            self.state.effects.entry(place).or_insert_with(|| cx.new(|_| EffectView::new())).clone();
        let (effect, seed) = (effect.to_owned(), seed.to_owned());
        view.update(cx, |v, cx| v.set(&effect, &seed, accent, width, height, playing, cx));
        gpui_kit::AnyView::from(view)
            .cached(gpui_kit::StyleRefinement::default().absolute().top_0().left_0().size_full())
            .into_any_element()
    }

    /// Picks a profile effect: None, then a tile per effect. Tiles sit still until pointed at
    /// or picked, so the grid never plays eight effects at once.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn effect_picker(
        &mut self,
        value: &str,
        user_id: &str,
        accent: i32,
        ready: bool,
        width: f32,
        p: &Palette,
        cx: &mut Context<Self>,
        pick: fn(&mut SettingsView, String, &mut Context<SettingsView>),
    ) -> AnyElement {
        let cols = 5usize;
        let gap = 8.0;
        let tile_w = (width - gap * (cols as f32 - 1.0)) / cols as f32;
        let card_w = tile_w - 8.0;
        let card_h = card_w * 5.0 / 4.0;
        let hovered = self.state.effect_hover.clone();
        let mut ids: Vec<&'static str> = vec![""];
        ids.extend(fx::IDS);
        let mut rows = div().flex().flex_col().gap(px(gap)).when(!ready, |el| el.opacity(0.5));
        for chunk in ids.chunks(cols) {
            let mut row = div().flex().gap(px(gap));
            for id in chunk {
                let id: &'static str = id;
                let active = value == id;
                let lively = active || hovered.as_deref() == Some(id);
                let label = if id.is_empty() { t("accountsettings.effects.none") } else { name(id) };
                let banner = |el: gpui_kit::Div| {
                    if accent < 0 {
                        crate::ui::widgets::hue_gradient(user_id, 0.0, el)
                    } else {
                        el.bg(rgb(accent as u32))
                    }
                };
                let mut card = div()
                    .relative()
                    .w(px(card_w))
                    .h(px(card_h))
                    .overflow_hidden()
                    .rounded(radius_xl())
                    .border_1()
                    .border_color(p.border)
                    .bg(p.card)
                    .shadow(crate::ui::settings_controls::shadow_sm())
                    .child(banner(div().absolute().top_0().left_0().right_0().h(px(card_h * 0.28))))
                    .child(
                        banner(div())
                            .absolute()
                            .top(px(card_h * 0.17))
                            .left(px(card_w * 0.11))
                            .size(px(card_w * 0.3))
                            .rounded_full()
                            .border_2()
                            .border_color(p.card),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(card_h * 0.56))
                            .left(px(card_w * 0.11))
                            .h(px(card_h * 0.06))
                            .w(px(card_w * 0.55))
                            .rounded_full()
                            .bg(alpha(p.foreground, 0.15)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(card_h * 0.67))
                            .left(px(card_w * 0.11))
                            .h(px(card_h * 0.05))
                            .w(px(card_w * 0.38))
                            .rounded_full()
                            .bg(alpha(p.foreground, 0.1)),
                    );
                if id.is_empty() {
                    card = card.child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .top(px(card_h * 0.28))
                            .bg(alpha(p.card, 0.7))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(p.muted_foreground)
                            .child(icon("ban").size(px(24.0))),
                    );
                } else {
                    let place: &'static str = tile_place(id);
                    card = card.child(self.effect_layer(place, id, user_id, accent, (card_w, card_h), lively, cx));
                }
                if active {
                    card =
                        card.child(div().absolute().inset_0().rounded(radius_xl()).border_2().border_color(p.primary));
                }
                row = row.child(
                    div()
                        .id(SharedString::from(format!("effect-{id}")))
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
                        .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                            if *on {
                                this.state.effect_hover = Some(id.to_owned());
                            } else if this.state.effect_hover.as_deref() == Some(id) {
                                this.state.effect_hover = None;
                            }
                            cx.notify();
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| pick(this, id.to_owned(), cx)))
                        .child(card)
                        .child(
                            div()
                                .truncate()
                                .text_color(if active { p.foreground } else { p.muted_foreground })
                                .child(label),
                        ),
                );
            }
            rows = rows.child(row);
        }
        rows.into_any_element()
    }
}

/// A picker tile's spot, kept for the app's life.
fn tile_place(id: &str) -> &'static str {
    fx::IDS
        .iter()
        .position(|x| *x == id)
        .map(|n| ["tile-0", "tile-1", "tile-2", "tile-3", "tile-4", "tile-5", "tile-6", "tile-7"][n])
        .unwrap_or("tile-x")
}
