//! Profile effects on screen (the web's `ProfileEffect.tsx`, `EffectShader.tsx`
//! and `EffectPicker.tsx`): the particles `core::profile_effects` plans, drawn
//! as tinted shapes or the effect's own pictures in a small view of their own
//! that redraws only itself while it plays, over the effect's shader if it
//! has one. With reduced motion, or while a picker tile rests, it shows a
//! still. The picker is None, then a tile per effect, each a tiny card.
//!
//! The shader is drawn by the backdrop's painter (`ui/effects.rs` over
//! `core::effects::gpu`): checked, compiled and timed before it draws, at
//! most 30 frames a second, and only while the window is in front. A shader
//! that can't run here (no GPU, broken, too slow, it stopped the GPU) draws
//! nothing, and the particles play as ever. Its inputs are a card's:
//! `fuwa.primary` the theme's primary, `fuwa.accent` the card's own color,
//! `fuwa.background` the card and `fuwa.foreground` its text.

use std::path::PathBuf;
use std::time::Instant;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, Render, Rgba, SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _,
    Transformation, Window, div, img, px, radians, rgb, size, svg,
};

use crate::core::effects::gpu::Source;
use crate::core::i18n::t;
use crate::core::profile_effects::{self as fx, Paint, Particle, Picture};
use crate::ui::effects::{Wake, painted, shader_runs};
use crate::ui::settings::SettingsView;
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_xl};
use crate::ui::widgets::{icon, pal, picture};

/// One playing effect.
pub struct EffectView {
    effect: Option<fx::Spec>,
    seed: String,
    accent: i32,
    width: f32,
    height: f32,
    playing: bool,
    particles: Vec<Particle>,
    fade_at: f32,
    started: Instant,
    /// The shader's clock, in seconds, moving at its speed while it runs.
    shader_time: f32,
    shader_last: Option<Instant>,
}

impl EffectView {
    pub fn new() -> Self {
        Self {
            effect: None,
            seed: String::new(),
            accent: -1,
            width: 0.0,
            height: 0.0,
            playing: false,
            particles: Vec::new(),
            fade_at: 0.0,
            started: Instant::now(),
            shader_time: 0.0,
            shader_last: None,
        }
    }

    /// Plays `effect` (a built-in, or a spec an instance or server offers) for `seed` on a card
    /// of this size; a new effect (or starting to play) starts the intro again.
    #[allow(clippy::too_many_arguments)]
    pub fn set(
        &mut self,
        effect: Option<&fx::Spec>,
        seed: &str,
        accent: i32,
        width: f32,
        height: f32,
        playing: bool,
        cx: &mut Context<Self>,
    ) {
        let changed =
            effect != self.effect.as_ref() || seed != self.seed || width != self.width || height != self.height;
        if changed {
            self.effect = effect.cloned();
            self.seed = seed.to_owned();
            self.width = width;
            self.height = height;
            self.particles = effect.map(|s| fx::plan(s, width, height, seed)).unwrap_or_default();
            self.fade_at = fx::idle_fade_at(&self.particles);
            self.shader_time = 0.0;
            self.shader_last = None;
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

fn gpu_fps() -> f32 {
    crate::core::effects::gpu::FPS as f32
}

/// One of an effect's pictures, filling its particle's box: an upload on the
/// instance (loaded like every picture, from fuwa instances only), or a file
/// being added, shown from this computer. A moving one holds its first frame
/// when nothing may move (an image without an id stays on it).
fn effect_picture(shown: &Picture, still: bool) -> AnyElement {
    let fill = |el: gpui_kit::Img| el.size_full().object_fit(ObjectFit::Contain);
    let holds = still && shown.animated;
    match (shown.local, holds) {
        (true, true) => fill(img(PathBuf::from(&shown.url))).into_any_element(),
        (true, false) => fill(img(PathBuf::from(&shown.url)))
            .id(SharedString::from(format!("picture|{}", shown.url)))
            .into_any_element(),
        (false, true) => fill(img(shown.url.clone())).into_any_element(),
        (false, false) => picture(shown.url.clone()).size_full().object_fit(ObjectFit::Contain).into_any_element(),
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
        // A cached view's root is laid out on its own, in the box the view is given: placed
        // absolutely there it would have no size, and clip every particle away.
        let mut layer = div().relative().size_full().overflow_hidden();
        let shader = self.effect.as_ref().and_then(|s| s.shader.clone()).filter(|s| shader_runs(&s.code));
        if let Some(shader) = shader {
            // Still with the particles, and while the window is behind others.
            let running = moving && window.is_window_active() && shader.speed > 0.0;
            let now = Instant::now();
            if running {
                if let Some(last) = self.shader_last {
                    // A long gap (a busy moment) moves it on a little, not by the whole gap.
                    self.shader_time += (now - last).as_secs_f32().min(0.25) * shader.speed;
                }
                self.shader_last = Some(now);
            } else {
                self.shader_last = None;
            }
            // 30 frames a second: the clock changes no more often than that, so neither do the frames asked for.
            let time = (self.shader_time * gpu_fps()).floor() / gpu_fps();
            let rgba = |c: Rgba| [c.r, c.g, c.b, 1.0];
            let colors = [rgba(p.primary), rgba(Rgba::from(profile)), rgba(p.card), rgba(p.card_foreground)];
            layer = layer.child(painted(
                Source::Custom(shader.code),
                colors,
                shader.intensity,
                !running,
                time,
                false,
                Wake::NextFrame,
            ));
        }
        let pictures = self.effect.as_ref().map(|s| s.pictures.as_slice()).unwrap_or_default();
        let at = |particle: &Particle| if moving { particle.at(elapsed, self.fade_at) } else { particle.still };
        layer = draw_particles(layer, &self.particles, pictures, at, !moving, &p, profile);
        if moving {
            window.request_animation_frame();
        }
        layer
    }
}

/// Particles over `layer`, each where `at` puts it (`still` holds moving pictures on their first frame).
fn draw_particles(
    mut layer: gpui_kit::Div,
    particles: &[Particle],
    pictures: &[Picture],
    at: impl Fn(&Particle) -> Option<fx::Frame>,
    still: bool,
    p: &Palette,
    profile: Hsla,
) -> gpui_kit::Div {
    for particle in particles {
        let Some(f) = at(particle) else { continue };
        if f.opacity <= 0.01 {
            continue;
        }
        let squash = f.flip.map_or(1.0, |d| d.to_radians().cos().abs().max(0.08));
        let place =
            div().absolute().left(px(f.x)).top(px(f.y)).size(px(particle.size)).opacity(f.opacity.clamp(0.0, 1.0));
        let drawn = match (particle.shape, particle.image.and_then(|n| pictures.get(n))) {
            (Some(shape), _) => place.child(
                svg()
                    .path(SharedString::from(format!("fx/{}.svg", shape.name())))
                    .size_full()
                    .text_color(paint(p, particle.paint, profile))
                    .with_transformation(
                        Transformation::rotate(radians(f.rotate.to_radians()))
                            .with_scaling(size(f.scale * f.scale_x, f.scale * squash)),
                    ),
            ),
            // A picture turns and scales with its box.
            (None, Some(shown)) => place
                .rotate(radians(f.rotate.to_radians()))
                .scale_x(f.scale * f.scale_x)
                .scale_y(f.scale * squash)
                .child(effect_picture(shown, still)),
            (None, None) => continue,
        };
        layer = layer.child(drawn);
    }
    layer
}

/// An effect's still (what it shows with reduced motion), particles `plan`ned beforehand, filling
/// its card: for places that draw many and keep no view for each (a pull shared in chat).
/// Its shader, if any, isn't drawn.
pub fn still_effect(spec: &fx::Spec, particles: &[Particle], seed: &str, accent: i32, p: &Palette) -> AnyElement {
    let layer = div().absolute().top_0().left_0().size_full().overflow_hidden();
    draw_particles(layer, particles, &spec.pictures, |x| x.still, true, p, profile_color(seed, accent))
        .into_any_element()
}

/// A built-in effect's line, in the app's language.
pub fn about(id: &str) -> Option<String> {
    fx::IDS.contains(&id).then(|| t(&format!("accountsettings.effects.about.{id}")))
}

/// A built-in effect's name, in the app's language.
pub fn effect_name(id: &str) -> String {
    t(&format!("accountsettings.effects.name.{id}"))
}

/// An effect's name and line as the picker says them: a built-in's in the app's language, any
/// other its own (the web's `effectText`).
pub fn effect_text(spec: &fx::Spec) -> (String, String) {
    if fx::IDS.contains(&spec.id.as_str()) {
        (effect_name(&spec.id), about(&spec.id).unwrap_or_default())
    } else {
        (spec.name.clone(), spec.description.clone())
    }
}

/// A tiny profile card (the web's `MiniCard`): banner, avatar and two lines, so an effect reads
/// as it will on a card. `width` is the card's; it's 4 by 5.
pub fn mini_card(user_id: &str, accent: i32, width: f32, p: &Palette) -> gpui_kit::Div {
    let (card_w, card_h) = (width, width * 5.0 / 4.0);
    let banner = |el: gpui_kit::Div| {
        if accent < 0 { crate::ui::widgets::hue_gradient(user_id, 0.0, el) } else { el.bg(rgb(accent as u32)) }
    };
    div()
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
        )
}

/// An effect over a card, playing in a view of its own kept in `views` by `place`.
#[allow(clippy::too_many_arguments)]
pub fn effect_in<V: 'static>(
    views: &mut std::collections::HashMap<String, Entity<EffectView>>,
    place: &str,
    effect: &fx::Spec,
    seed: &str,
    accent: i32,
    (width, height): (f32, f32),
    playing: bool,
    cx: &mut Context<V>,
) -> AnyElement {
    let view: Entity<EffectView> =
        views.entry(place.to_owned()).or_insert_with(|| cx.new(|_| EffectView::new())).clone();
    let (effect, seed) = (effect.clone(), seed.to_owned());
    view.update(cx, |v, cx| v.set(Some(&effect), &seed, accent, width, height, playing, cx));
    over_card(view)
}

/// An effect's view laid over its card, cached so only it redraws while it plays.
pub fn over_card(view: Entity<EffectView>) -> AnyElement {
    gpui_kit::AnyView::from(view)
        .cached(gpui_kit::StyleRefinement::default().absolute().top_0().left_0().size_full())
        .into_any_element()
}

/// What a picker offers besides None: the effects fuwa ships with, then those `offered`.
fn choices(offered: &[(String, fx::Spec)]) -> Vec<(String, fx::Spec)> {
    let mut all: Vec<(String, fx::Spec)> =
        fx::IDS.iter().filter_map(|id| fx::spec(id).map(|s| ((*id).to_owned(), s))).collect();
    all.extend(offered.iter().cloned());
    all
}

impl SettingsView {
    /// An effect over a card, playing in its own view: `place` keeps one view per spot.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn effect_layer(
        &mut self,
        place: &str,
        effect: &fx::Spec,
        seed: &str,
        accent: i32,
        size: (f32, f32),
        playing: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut views = std::mem::take(&mut self.state.effects);
        let el = effect_in(&mut views, place, effect, seed, accent, size, playing, cx);
        self.state.effects = views;
        el
    }

    /// Picks a profile effect: None (or `none_label`, like "Use my profile's"), then a tile for
    /// each effect fuwa ships with and each one `offered` by the instance or the server. Tiles
    /// sit still until pointed at or picked, so the grid never plays them all at once. `place`
    /// keeps each picker's tiles apart.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn effect_picker(
        &mut self,
        place: &'static str,
        value: &str,
        user_id: &str,
        accent: i32,
        offered: &[(String, fx::Spec)],
        none_label: Option<String>,
        locked: &std::collections::HashMap<String, String>,
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
        let mut tiles: Vec<(String, Option<fx::Spec>)> = vec![(String::new(), None)];
        tiles.extend(choices(offered).into_iter().map(|(id, s)| (id, Some(s))));
        let picked = tiles.iter().position(|(id, _)| id == value);
        // A tile: 4px around the card, 6px, then a 16px label.
        let ring = crate::ui::decoration_picker::Ring {
            id: format!("effect-ring|{place}"),
            cols,
            tile: (tile_w, card_h + 30.0),
            gap,
        };
        let (gliding, ring) =
            crate::ui::decoration_picker::picked_ring(ring, picked, (card_w, card_h), p.primary.into(), cx);
        let mut rows = div().relative().flex().flex_col().gap(px(gap)).when(!ready, |el| el.opacity(0.5));
        for chunk in tiles.chunks(cols) {
            let mut row = div().flex().gap(px(gap));
            for (id, spec) in chunk {
                let id = id.clone();
                let hover_key = format!("{place}|{id}");
                let active = value == id;
                let lively = active || hovered.as_deref() == Some(hover_key.as_str());
                let label = match spec {
                    Some(spec) => effect_text(spec).0,
                    None => none_label.clone().unwrap_or_else(|| t("accountsettings.effects.none")),
                };
                let group = SharedString::from(format!("effect-{place}-{id}"));
                let mut card = mini_card(user_id, accent, card_w, p);
                match spec {
                    None => {
                        card = card.child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .bottom_0()
                                .top(px(card_h * 0.28))
                                .rounded_b(radius_xl())
                                .bg(alpha(p.card, 0.7))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(p.muted_foreground)
                                // The ban sign turns a quarter while its tile is pointed at.
                                .child(
                                    div()
                                        .id("effect-none-ban")
                                        .group_hover(group.clone(), |s| {
                                            s.rotate(gpui_kit::radians(std::f32::consts::FRAC_PI_2))
                                        })
                                        .child(icon("ban").size(px(24.0))),
                                ),
                        );
                    }
                    Some(spec) => {
                        let spot = format!("tile-{place}-{id}");
                        card =
                            card.child(self.effect_layer(&spot, spec, user_id, accent, (card_w, card_h), lively, cx));
                    }
                }
                if active && !gliding {
                    card =
                        card.child(div().absolute().inset_0().rounded(radius_xl()).border_2().border_color(p.primary));
                }
                // Sold in the server's shop and not bought: a padlock and its price, and no picking.
                let lock = locked.get(&id).filter(|_| !active);
                if let Some(price) = lock {
                    card = card.child(crate::ui::decoration_picker::lock_badge(price, radius_xl(), p));
                }
                let open = lock.is_none();
                let (pick_id, hover_on) = (id.clone(), hover_key.clone());
                let fg = p.foreground;
                // Its shadow deepens while pointed at.
                let card = card
                    .id("effect-card")
                    .group_hover(group.clone(), |s| s.shadow(crate::ui::profile_card::shadow_md()));
                row = row.child(
                    div()
                        .id(group.clone())
                        .group(group.clone())
                        .w(px(tile_w))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(6.0))
                        .p(px(4.0))
                        .rounded(radius_2xl())
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .when(open, |el| {
                            el.cursor_pointer().hover(|s| s.translate_y(px(-3.0))).active(|s| s.scale(0.95))
                        })
                        // A locked one is sold in the server's shop: it opens there (the web's `onLocked`).
                        .when(!open, |el| el.cursor_pointer().hover(|s| s.translate_y(px(-3.0))))
                        .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                            if *on {
                                this.state.effect_hover = Some(hover_on.clone());
                            } else if this.state.effect_hover.as_deref() == Some(hover_on.as_str()) {
                                this.state.effect_hover = None;
                            }
                            cx.notify();
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if open { pick(this, pick_id.clone(), cx) } else { this.locked_to_shop(cx) }
                        }))
                        .child(card)
                        .child(
                            // As on the web, a long name runs past its tile rather than being cut.
                            div()
                                .id("effect-label")
                                .whitespace_nowrap()
                                .line_height(px(16.0))
                                .text_color(if active { p.foreground } else { p.muted_foreground })
                                .group_hover(group, move |s| s.text_color(fg))
                                .child(label),
                        ),
                );
            }
            rows = rows.child(row);
        }
        rows.child(ring).into_any_element()
    }
}
