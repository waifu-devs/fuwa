//! Themes and the Background page, as the web's `settings/app/Themes.tsx`,
//! `ThemePreview.tsx`, `Backgrounds.tsx` and `BackdropForm.tsx`: your own
//! themes (made from any theme, tuned with a live preview, passed around as
//! files), the app in miniature, and the picture and effect behind it.
//! Custom WGSL shaders are written in `settings_shader.rs`.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, img,
    px, rgb,
};

use crate::core::config::Prefs;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::themes::{self, Backdrop, Effect, Fit, Picture, SEEDS, TOKENS, Theme, Tokens};
use crate::ui::motion;
use crate::ui::settings::{Page, SettingsView};
use crate::ui::settings_controls::{At, Badge, Look, Opt, Setter, button, choice, field, toggle, with_preview};
use crate::ui::settings_menu::Item;
use crate::ui::text::{WIDE, tracked};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_3xl, radius_lg, radius_xl, system_dark};
use crate::ui::widgets::icon;

/// Which backdrop a form edits.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    /// The app's, in the settings.
    App,
    /// The theme being edited.
    Draft,
}

#[derive(Default)]
pub(crate) struct ThemesForm {
    /// The theme being edited, and whether it's new.
    pub draft: Option<(Theme, bool)>,
    /// The theme as it was when the editor opened, to tell what changed.
    start: Option<Theme>,
    all_colors: bool,
    name: Option<Entity<InputState>>,
    description: Option<Entity<InputState>>,
    hex: HashMap<&'static str, Entity<InputState>>,
    /// What an import said, as (file, lines).
    notes: Option<(String, Vec<String>)>,
    importing: bool,
    /// Your backgrounds on the instance, and which one asks to be deleted.
    backgrounds: Option<(String, Vec<String>)>,
    loading: Option<String>,
    uploading: bool,
    confirming: Option<String>,
    picture_error: Option<String>,
    /// The custom shader editor.
    pub shader: crate::ui::settings_shader::ShaderForm,
}

fn rgba(c: u32) -> gpui_kit::Rgba {
    rgb(c)
}

/// The catalog's name for a token's label ("card-foreground" is `cardForeground`).
fn token_label(token: &str) -> String {
    let mut out = String::new();
    let mut up = false;
    for c in token.chars() {
        if c == '-' {
            up = true;
        } else if up {
            out.extend(c.to_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    t(&format!("appsettings.themes.token.{out}"))
}

/// WCAG contrast between two colors.
fn contrast(a: u32, b: u32) -> f32 {
    let lum = |c: u32| {
        let ch = |v: u32| {
            let c = v as f32 / 255.0;
            if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch((c >> 16) & 255) + 0.7152 * ch((c >> 8) & 255) + 0.0722 * ch(c & 255)
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// fuwa in miniature, in a theme and over a backdrop: the rail, the sidebar and a few messages
/// (the web's `ThemePreview`).
pub(crate) fn theme_preview(
    theme: &Theme,
    backdrop: &Backdrop,
    p: &Palette,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) -> AnyElement {
    let tk = &theme.tokens;
    let c = |name: &str| rgba(tk.get(name));
    let on = backdrop.any();
    let pv = Palette::of(theme);
    let panel = |color: u32, more: u8| -> Hsla {
        if on {
            alpha(rgba(color), (f32::from(backdrop.panels.saturating_add(more)) / 100.0).min(1.0))
        } else {
            rgba(color).into()
        }
    };
    let rail = themes::mix(tk.get("primary"), tk.get("muted"), 0.09);
    let side = themes::mix(tk.get("card"), tk.get("background"), 0.55);
    let layers = on.then(|| crate::ui::backdrop::layers(backdrop, &pv, window, cx)).flatten();
    let _ = p;
    let mut rail_col =
        div().w(px(48.0)).flex_none().flex().flex_col().items_center().gap(px(8.0)).py(px(12.0)).bg(panel(rail, 20));
    for (n, color) in ["primary", "muted-foreground", "border"].iter().enumerate() {
        rail_col = rail_col.child(div().size(px(28.0)).bg(c(color)).rounded(px(if n == 0 { 9.0 } else { 14.0 })));
    }
    let mut channels = div()
        .w(px(112.0))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .border_r_1()
        .border_color(c("border"))
        .p(px(12.0))
        .bg(panel(side, 20))
        .child(div().mb(px(4.0)).h(px(10.0)).w(px(64.0)).rounded_full().bg(alpha(c("foreground"), 0.8)));
    for (n, key) in ["general", "art", "music", "games"].iter().enumerate() {
        channels = channels.child(
            div()
                .rounded(px(6.0))
                .px(px(6.0))
                .py(px(4.0))
                .text_size(px(9.6))
                .font_weight(FontWeight::BOLD)
                .map(|el| {
                    if n == 0 {
                        el.bg(c("accent")).text_color(c("accent-foreground"))
                    } else {
                        el.text_color(c("muted-foreground"))
                    }
                })
                .child(format!("# {}", t(&format!("appsettings.preview.{key}")))),
        );
    }
    let lines = [("Hana", 0.78, Some(0.52)), ("Ren", 0.64, None), ("", 0.70, Some(0.38))];
    let mut chat =
        div().flex_1().min_w_0().flex().flex_col().gap(px(12.0)).p(px(12.0)).bg(panel(tk.get("background"), 0));
    for (n, (name, w1, w2)) in lines.into_iter().enumerate() {
        let you = n == 2;
        chat = chat.child(
            div()
                .flex()
                .gap(px(8.0))
                .child(
                    div().size(px(24.0)).flex_none().rounded_full().border_1().border_color(c("border")).bg(if you {
                        c("primary")
                    } else {
                        c("secondary")
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .text_size(px(10.4))
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .text_color(if you { c("primary") } else { c("foreground") })
                                .child(if you { t("appsettings.preview.you") } else { name.to_owned() }),
                        )
                        .child(
                            div().h(px(8.0)).w(gpui_kit::relative(w1)).rounded_full().bg(alpha(c("foreground"), 0.55)),
                        )
                        .when_some(w2, |el, w| {
                            el.child(
                                div()
                                    .h(px(8.0))
                                    .w(gpui_kit::relative(w))
                                    .rounded_full()
                                    .bg(alpha(c("foreground"), 0.35)),
                            )
                        }),
                ),
        );
    }
    let r = 16.0 * theme.radius;
    chat = chat.child(div().flex_1()).child(
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(px(r))
            .border_1()
            .border_color(c("border"))
            .bg(panel(tk.get("card"), 40))
            .px(px(8.0))
            .py(px(6.0))
            .child(div().flex_1().h(px(8.0)).rounded_full().bg(alpha(c("muted-foreground"), 0.35)))
            .child(
                div()
                    .rounded(px((r - 4.0).max(0.0)))
                    .bg(c("primary"))
                    .px(px(8.0))
                    .py(px(2.0))
                    .text_size(px(9.6))
                    .font_weight(FontWeight::BOLD)
                    .text_color(c("primary-foreground"))
                    .child(t("appsettings.preview.send")),
            ),
    );
    div()
        .relative()
        .h(px(256.0))
        .flex()
        .overflow_hidden()
        .rounded(radius_3xl())
        .border_1()
        .border_color(c("border"))
        .bg(c("background"))
        .text_color(c("foreground"))
        .shadow(crate::ui::settings_controls::shadow_lg())
        .when_some(layers, |el, l| el.child(l))
        .child(rail_col.rounded_l(radius_3xl()))
        .child(channels)
        .child(chat.rounded_r(radius_3xl()))
        .into_any_element()
}

impl SettingsView {
    fn backdrop_of(&self, target: Target) -> Backdrop {
        match target {
            Target::App => self.core.prefs().backdrop,
            Target::Draft => self.themes.draft.as_ref().and_then(|(t, _)| t.backdrop.clone()).unwrap_or_default(),
        }
    }

    pub(crate) fn patch_backdrop(&mut self, target: Target, cx: &mut Context<Self>, f: impl FnOnce(&mut Backdrop)) {
        match target {
            Target::App => self.set(cx, |pr| f(&mut pr.backdrop)),
            Target::Draft => {
                if let Some((theme, _)) = self.themes.draft.as_mut() {
                    f(theme.backdrop.get_or_insert_with(Backdrop::default));
                }
                cx.notify();
            }
        }
    }

    fn backdrop_setter(target: Target, f: fn(&mut Backdrop, u8)) -> Setter {
        Rc::new(move |this: &mut SettingsView, v: f32, cx: &mut Context<SettingsView>| {
            let v = v.round().clamp(0.0, 255.0) as u8;
            this.patch_backdrop(target, cx, move |b| f(b, v))
        })
    }

    /// A slider with its name and value beside it (the web's `Labeled`).
    #[allow(clippy::too_many_arguments)]
    fn labeled_slider(
        &mut self,
        id: &'static str,
        label: &str,
        shown: String,
        range: (f32, f32, f32),
        value: f32,
        set: Setter,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let slider = self.slider_with(
            id,
            range,
            value,
            Vec::new(),
            |v| format!("{}", v.round() as i64),
            false,
            p,
            window,
            cx,
            set,
        );
        div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .child(
                div()
                    .w(px(80.0))
                    .flex_none()
                    .pt(px(20.0))
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child(label.to_owned()),
            )
            .child(div().flex_1().min_w_0().child(slider))
            .child(
                div()
                    .w(px(48.0))
                    .whitespace_nowrap()
                    .flex_none()
                    .pt(px(20.0))
                    .text_right()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .child(shown),
            )
            .into_any_element()
    }

    /// Everything about a backdrop: the picture, how it sits, and the effect over it (the web's `BackdropForm`).
    pub(crate) fn backdrop_form(
        &mut self,
        target: Target,
        width: f32,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let b = self.backdrop_of(target);
        let ids = |name: &str| {
            crate::ui::settings_servers::leak_id(&format!(
                "bd-{}-{name}",
                if target == Target::App { "app" } else { "draft" }
            ))
        };
        let block = |title: String, hint: String, body: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    div()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.muted_foreground)
                                .child(tracked(title.to_uppercase(), WIDE)),
                        )
                        .child(div().mt(px(2.0)).text_xs().text_color(p.muted_foreground).child(hint)),
                )
                .child(body)
        };
        let library = self.picture_library(target, &b, width, p, cx);
        let mut picture = div().flex().flex_col().gap(px(8.0)).child(library);
        if !b.image.is_empty() {
            let at = match b.fit {
                Fit::Cover => 0,
                Fit::Contain => 1,
                Fit::Tile => 2,
            };
            picture = picture.child(motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(choice(
                        ids("fit"),
                        Some(at),
                        vec![
                            Opt::new(t("appsettings.backdrop.fill"), t("appsettings.backdrop.fillHint"), "maximize-2"),
                            Opt::new(t("appsettings.backdrop.fit"), t("appsettings.backdrop.fitHint"), "minimize-2"),
                            Opt::new(t("appsettings.backdrop.tile"), t("appsettings.backdrop.tileHint"), "grid-2x2"),
                        ],
                        width,
                        p,
                        window,
                        cx,
                        move |this, n, cx| {
                            let fit = [Fit::Cover, Fit::Contain, Fit::Tile][n];
                            this.patch_backdrop(target, cx, |b| b.fit = fit)
                        },
                    ))
                    .child(self.labeled_slider(
                        ids("dim"),
                        &t("appsettings.backdrop.dim"),
                        format!("{}%", b.dim),
                        (f32::from(themes::DIM.0), f32::from(themes::DIM.1), 1.0),
                        f32::from(b.dim),
                        Self::backdrop_setter(target, |b, v| b.dim = v),
                        p,
                        window,
                        cx,
                    ))
                    .child(self.labeled_slider(
                        ids("blur"),
                        &t("appsettings.backdrop.blur"),
                        format!("{}px", b.blur),
                        (f32::from(themes::BLUR.0), f32::from(themes::BLUR.1), 1.0),
                        f32::from(b.blur),
                        Self::backdrop_setter(target, |b, v| b.blur = v),
                        p,
                        window,
                        cx,
                    )),
                SharedString::from(format!("{}-in", ids("pic-opts"))),
                Duration::ZERO,
                -6.0,
            ));
        }
        // Effects, each shown in miniature.
        let cols = 5;
        let gap = 8.0;
        let tile_w = (width - gap * (cols as f32 - 1.0)) / cols as f32;
        let pv = *p;
        let mut grid = div().flex().flex_col().gap(px(gap));
        for (r, chunk) in Effect::ALL.chunks(cols).enumerate() {
            let mut row = div().flex().gap(px(gap));
            for (c, effect) in chunk.iter().copied().enumerate() {
                let n = r * cols + c;
                let active = b.effect == effect;
                let key = format!("{effect:?}").to_lowercase();
                let name_key = format!("appsettings.backdrop.effect.{key}");
                let thumb_backdrop = Backdrop {
                    effect,
                    intensity: 60,
                    speed: if active { b.speed.max(40) } else { 0 },
                    ..Backdrop::default()
                };
                let thumb: Option<AnyElement> = if effect == Effect::None {
                    None
                } else if effect == Effect::Custom {
                    Some(
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon("code-xml").size(px(20.0)).text_color(p.primary))
                            .into_any_element(),
                    )
                } else if effect.texture() {
                    crate::ui::backdrop::layers(&thumb_backdrop, &pv, window, cx)
                } else {
                    crate::ui::effects::drawn(effect, 90, thumb_backdrop.speed, &pv, window, cx)
                };
                let hover = alpha(p.primary, 0.4);
                row = row.child(motion::rise(
                    div()
                        .id(SharedString::from(format!("{}-{n}", ids("fx"))))
                        .w(px(tile_w))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .rounded(radius_xl())
                        .border_1()
                        .border_color(if active { p.primary } else { p.border })
                        .cursor_pointer()
                        .hover(|s| s.translate_y(px(-2.0)))
                        .when(!active, |el| el.hover(move |s| s.border_color(hover).translate_y(px(-2.0))))
                        .active(|s| s.scale(0.95))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.patch_backdrop(target, cx, |b| {
                                b.effect = effect;
                                if effect == Effect::Custom && b.shader.is_none() {
                                    b.shader = Some(crate::core::effects::custom::default_shader());
                                }
                            })
                        }))
                        .child(
                            div()
                                .relative()
                                .h(px(56.0))
                                .overflow_hidden()
                                .bg(p.background)
                                .rounded_t(radius_xl())
                                .children(thumb),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(4.0))
                                .px(px(8.0))
                                .py(px(6.0))
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .child(div().truncate().child(match (effect, &b.shader) {
                                    (Effect::Custom, Some(shader)) => shader.name.clone(),
                                    _ => t(&name_key),
                                }))
                                .when(active, |el| {
                                    el.child(
                                        div()
                                            .size(px(16.0))
                                            .flex_none()
                                            .rounded_full()
                                            .bg(p.primary)
                                            .text_color(p.primary_foreground)
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(icon("check").size(px(12.0))),
                                    )
                                }),
                        ),
                    SharedString::from(format!("{}-in-{n}", ids("fx"))),
                    Duration::from_millis(25 * n as u64),
                    8.0,
                ));
            }
            grid = grid.child(row);
        }
        let mut effect = div().flex().flex_col().gap(px(8.0)).child(grid);
        if let Some(shader) = b.shader.as_ref().filter(|_| b.effect == Effect::Custom) {
            effect = effect.child(self.shader_editor(target, shader, p, window, cx));
        }
        if b.effect != Effect::None {
            let moves = b.effect.moves();
            let speed = if b.speed == 0 { t("appsettings.backdrop.still") } else { format!("{}%", b.speed) };
            effect = effect.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(self.labeled_slider(
                        ids("strength"),
                        &t("appsettings.backdrop.strength"),
                        format!("{}%", b.intensity),
                        (f32::from(themes::INTENSITY.0), f32::from(themes::INTENSITY.1), 1.0),
                        f32::from(b.intensity),
                        Self::backdrop_setter(target, |b, v| b.intensity = v),
                        p,
                        window,
                        cx,
                    ))
                    .when(moves, |el| {
                        el.child(self.labeled_slider(
                            ids("speed"),
                            &t("appsettings.backdrop.speed"),
                            speed,
                            (f32::from(themes::SPEED.0), f32::from(themes::SPEED.1), 10.0),
                            f32::from(b.speed),
                            Self::backdrop_setter(target, |b, v| b.speed = v),
                            p,
                            window,
                            cx,
                        ))
                    }),
            );
        }
        let panels = self.labeled_slider(
            ids("panels"),
            &t("appsettings.backdrop.panels"),
            format!("{}%", b.panels),
            (f32::from(themes::PANELS.0), f32::from(themes::PANELS.1), 1.0),
            f32::from(b.panels),
            Self::backdrop_setter(target, |b, v| b.panels = v),
            p,
            window,
            cx,
        );
        div()
            .flex()
            .flex_col()
            .gap(px(24.0))
            .child(block(
                t("appsettings.backdrop.picture"),
                t("appsettings.backdrop.pictureHint"),
                picture.into_any_element(),
            ))
            .child(block(
                t("appsettings.backdrop.effect"),
                t("desktop.background.effectNote"),
                effect.into_any_element(),
            ))
            .child(block(t("appsettings.backdrop.panels"), t("appsettings.backdrop.panelsHint"), panels))
            .into_any_element()
    }

    /// The instance pictures go to: the one on screen, or the first signed in.
    fn picture_instance(&mut self) -> Option<String> {
        self.account_me().map(|(k, _)| k)
    }

    /// Your backgrounds on the instance, a tile to add one, and None.
    fn picture_library(
        &mut self,
        target: Target,
        b: &Backdrop,
        width: f32,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = self.picture_instance();
        if let Some(key) = &key
            && self.themes.backgrounds.as_ref().is_none_or(|(k, _)| k != key)
            && self.themes.loading.as_deref() != Some(key.as_str())
        {
            self.themes.loading = Some(key.clone());
            let (core, k) = (self.core.clone(), key.clone());
            let rx = self.core.spawn(async move { core.backgrounds(&k).await });
            let key = key.clone();
            cx.spawn(async move |this, cx| {
                let Ok(result) = rx.await else { return };
                let _ = this.update(cx, |this, cx| {
                    this.themes.backgrounds = Some((key, result.unwrap_or_default()));
                    this.themes.loading = None;
                    cx.notify();
                });
            })
            .detach();
        }
        let cols = 4;
        let gap = 8.0;
        let tile_w = (width - gap * (cols as f32 - 1.0)) / cols as f32;
        let tile_h = tile_w * 9.0 / 16.0;
        let kept: Vec<String> = self.themes.backgrounds.as_ref().map(|(_, l)| l.clone()).unwrap_or_default();
        let mut shown: Vec<String> = Vec::new();
        if !b.image.is_empty() && !kept.contains(&b.image) {
            shown.push(b.image.clone());
        }
        shown.extend(kept.iter().cloned());
        let mut tiles: Vec<AnyElement> = Vec::new();
        let check = |p: &Palette| {
            div()
                .absolute()
                .bottom(px(4.0))
                .left(px(4.0))
                .size(px(20.0))
                .rounded_full()
                .bg(p.primary)
                .text_color(p.primary_foreground)
                .flex()
                .items_center()
                .justify_center()
                .shadow(crate::ui::settings_controls::shadow_sm())
                .child(icon("check").size(px(12.0)))
        };
        let tile = |id: String, active: bool, p: &Palette| {
            let hover = alpha(p.primary, 0.4);
            div()
                .id(SharedString::from(id))
                .relative()
                .w(px(tile_w))
                .h(px(tile_h))
                .overflow_hidden()
                .rounded(radius_xl())
                .border_2()
                .border_color(if active { p.primary.into() } else { alpha(p.primary, 0.0) })
                .cursor_pointer()
                .hover(|s| s.translate_y(px(-2.0)))
                .when(!active, |el| el.hover(move |s| s.border_color(hover).translate_y(px(-2.0))))
                .active(|s| s.scale(0.95))
        };
        tiles.push(
            tile(format!("bg-none-{}", target == Target::App), b.image.is_empty(), p)
                .on_click(cx.listener(move |this, _, _, cx| this.patch_backdrop(target, cx, |b| b.image.clear())))
                .child(
                    div()
                        .size_full()
                        .bg(p.muted)
                        .text_color(p.muted_foreground)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon("image-off").size(px(20.0))),
                )
                .when(b.image.is_empty(), |el| el.child(check(p)))
                .into_any_element(),
        );
        if let Some(key) = key.clone() {
            let uploading = self.themes.uploading;
            let hover_fg = p.primary;
            tiles.push(
                div()
                    .id(SharedString::from(format!("bg-add-{}", target == Target::App)))
                    .w(px(tile_w))
                    .h(px(tile_h))
                    .rounded(radius_xl())
                    .border_2()
                    .border_dashed()
                    .border_color(p.border)
                    .text_color(p.muted_foreground)
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(4.0))
                    .text_size(px(10.4))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .cursor_pointer()
                    .hover(move |s| s.border_color(alpha(hover_fg, 0.6)).text_color(hover_fg).translate_y(px(-2.0)))
                    .active(|s| s.scale(0.95))
                    .on_click(cx.listener(move |this, _, _, cx| this.upload_background_to(key.clone(), target, cx)))
                    .child(icon(if uploading { "loader-circle" } else { "image-plus" }).size(px(20.0)))
                    .child(tracked(t("appsettings.backdrop.add").to_uppercase(), WIDE))
                    .into_any_element(),
            );
        }
        for url in shown {
            let active = b.image == url;
            let is_kept = kept.contains(&url);
            let confirming = self.themes.confirming.as_deref() == Some(url.as_str());
            let (pick, gone) = (url.clone(), url.clone());
            let key2 = key.clone();
            tiles.push(
                tile(format!("bg-{}-{url}", target == Target::App), active, p)
                    .group("bgtile")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let pick = pick.clone();
                        this.patch_backdrop(target, cx, move |b| b.image = pick)
                    }))
                    .child(img(SharedString::from(url.clone())).size_full().object_fit(ObjectFit::Cover))
                    .when(active, |el| el.child(check(p)))
                    .when(is_kept, |el| {
                        el.child(
                            div()
                                .id(SharedString::from(format!("bg-del-{url}")))
                                .absolute()
                                .top(px(4.0))
                                .right(px(4.0))
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .rounded_full()
                                .bg(alpha(p.background, 0.9))
                                .px(px(6.0))
                                .py(px(4.0))
                                .text_size(px(10.4))
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.destructive)
                                .opacity(if confirming { 1.0 } else { 0.0 })
                                .group_hover("bgtile", |s| s.opacity(1.0))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    if this.themes.confirming.as_deref() != Some(gone.as_str()) {
                                        this.themes.confirming = Some(gone.clone());
                                        cx.notify();
                                        return;
                                    }
                                    this.themes.confirming = None;
                                    if let Some(k) = key2.clone() {
                                        this.delete_background_at(k, gone.clone(), target, cx);
                                    }
                                }))
                                .child(icon("trash").size(px(12.0)))
                                .when(confirming, |el| el.child(t("appsettings.backdrop.confirmDelete"))),
                        )
                    })
                    .into_any_element(),
            );
        }
        let mut grid = div().flex().flex_col().gap(px(gap));
        let mut it = tiles.into_iter().peekable();
        while it.peek().is_some() {
            let mut row = div().flex().gap(px(gap));
            for _ in 0..cols {
                if let Some(t) = it.next() {
                    row = row.child(t);
                }
            }
            grid = grid.child(row);
        }
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(grid)
            .when(key.is_none(), |el| {
                el.child(div().text_xs().text_color(p.muted_foreground).child(t("appsettings.backdrop.needInstance")))
            })
            .when_some(self.themes.picture_error.clone(), |el, e| {
                el.child(div().text_xs().font_weight(FontWeight::BOLD).text_color(p.destructive).child(e))
            })
            .into_any_element()
    }

    fn upload_background_to(&mut self, key: String, target: Target, cx: &mut Context<Self>) {
        if self.themes.uploading {
            return;
        }
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("desktop.account.choosePicture").into()),
        });
        let core = self.core.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let Some(kind) = crate::core::account::picture_type(&name) else {
                let _ = this.update(cx, |this, cx| {
                    this.themes.picture_error = Some(t("appsettings.backdrop.pickType"));
                    cx.notify();
                });
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.themes.uploading = true;
                this.themes.picture_error = None;
                cx.notify();
            });
            let rx = core.spawn({
                let (core, key) = (core.clone(), key.clone());
                async move {
                    let bytes = crate::core::account::read_picture(&path).await?;
                    core.upload_background(&key, Picture { content_type: kind.into(), bytes }).await
                }
            });
            let result = rx.await;
            let _ = this.update(cx, |this, cx| {
                this.themes.uploading = false;
                match result {
                    Ok(Ok(url)) => {
                        if let Some((k, list)) = this.themes.backgrounds.as_mut()
                            && *k == key
                        {
                            list.retain(|u| *u != url);
                            list.insert(0, url.clone());
                        }
                        this.patch_backdrop(target, cx, move |b| b.image = url);
                    }
                    Ok(Err(err)) => this.themes.picture_error = Some(err.message),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn delete_background_at(&mut self, key: String, url: String, target: Target, cx: &mut Context<Self>) {
        let (core, k, u) = (self.core.clone(), key, url.clone());
        let rx = self.core.spawn(async move { core.delete_background(&k, &u).await });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => {
                    if let Some((_, list)) = this.themes.backgrounds.as_mut() {
                        list.retain(|l| *l != url);
                    }
                    if this.backdrop_of(target).image == url {
                        this.patch_backdrop(target, cx, |b| b.image.clear());
                    }
                    cx.notify();
                }
                Err(err) => {
                    this.themes.picture_error = Some(err.message);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn background_page(
        &mut self,
        prefs: &Prefs,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = prefs.active_theme(system_dark(window.appearance()));
        let shown = prefs.active_backdrop(&theme);
        let form_w = if self.wide { self.column - 40.0 - 288.0 } else { self.column };
        let form = self.backdrop_form(Target::App, form_w, p, window, cx);
        let changed = prefs.backdrop != Backdrop::default();
        let setting = self.setting(
            "backdrop",
            &t("appsettings.backgrounds.title"),
            Some(crate::ui::settings_controls::hint(t("appsettings.backgrounds.hint"), p)),
            Badge::pref(changed, |pr| pr.backdrop = Backdrop::default()),
            At::of(0, 1),
            form,
            p,
            cx,
        );
        let own = theme.backdrop.is_some();
        let banner = own.then(|| {
            motion::rise(
                div()
                    .mb(px(24.0))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .rounded(radius_2xl())
                    .border_1()
                    .border_color(alpha(p.primary, 0.3))
                    .bg(alpha(p.primary, 0.1))
                    .p(px(12.0))
                    .text_sm()
                    .child(icon("palette").size(px(16.0)).text_color(p.primary))
                    .child(
                        div()
                            .flex_1()
                            .child(t_with("appsettings.backgrounds.themeHasOwn", &[("theme", Arg::Str(&theme.name))])),
                    )
                    .child(
                        div()
                            .id("bg-edit-theme")
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.primary)
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .on_click(cx.listener(|this, _, _, cx| this.choose(Page::Themes, None, cx)))
                            .child(t("appsettings.backgrounds.editTheme")),
                    ),
                "bg-own",
                Duration::ZERO,
                -6.0,
            )
        });
        let preview = theme_preview(&theme, &shown, p, window, cx);
        with_preview(div().flex().flex_col().children(banner).child(setting), preview, self.wide, p)
    }

    // ───────────────────────── Themes ─────────────────────────

    /// Opens the editor on a theme (a copy of `base` when `new`).
    fn edit_theme(&mut self, base: &Theme, new: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mut theme = base.clone();
        if new {
            theme.id = themes::new_theme_id();
            theme.builtin = false;
            theme.name = if base.builtin {
                t_with("appsettings.themes.mine", &[("theme", Arg::Str(&base.name))])
            } else {
                t_with("appsettings.themes.copy", &[("theme", Arg::Str(&base.name))])
            };
            theme.name = theme.name.chars().take(themes::NAME_MAX).collect();
            if base.builtin {
                theme.description = None;
            }
        }
        let name = cx.new(|cx| InputState::new(window, cx));
        name.update(cx, |s, cx| s.set_value(theme.name.clone(), window, cx));
        cx.subscribe(&name, |this: &mut SettingsView, s, _: &InputEvent, cx| {
            let v: String = s.read(cx).value().chars().take(themes::NAME_MAX).collect();
            if let Some((th, _)) = this.themes.draft.as_mut() {
                th.name = v;
            }
            cx.notify();
        })
        .detach();
        let description = cx.new(|cx| InputState::new(window, cx).placeholder(t("appsettings.themes.descriptionHint")));
        description.update(cx, |s, cx| s.set_value(theme.description.clone().unwrap_or_default(), window, cx));
        cx.subscribe(&description, |this: &mut SettingsView, s, _: &InputEvent, cx| {
            let v: String = s.read(cx).value().chars().take(themes::DESCRIPTION_MAX).collect();
            if let Some((th, _)) = this.themes.draft.as_mut() {
                th.description = (!v.is_empty()).then_some(v);
            }
            cx.notify();
        })
        .detach();
        self.themes.hex.clear();
        for token in TOKENS {
            let state = cx.new(|cx| InputState::new(window, cx));
            state.update(cx, |s, cx| s.set_value(themes::hex(theme.tokens.get(token)), window, cx));
            cx.subscribe(&state, move |this: &mut SettingsView, s, e: &InputEvent, cx| {
                if !matches!(e, InputEvent::Change) {
                    return;
                }
                let v = s.read(cx).value().trim().to_owned();
                let v = if v.starts_with('#') { v } else { format!("#{v}") };
                if let Some(c) = themes::parse_hex(&v) {
                    this.set_token(token, c, cx);
                }
            })
            .detach();
            self.themes.hex.insert(token, state);
        }
        self.themes.name = Some(name);
        self.themes.description = Some(description);
        self.themes.start = Some(theme.clone());
        self.themes.draft = Some((theme, new));
        self.themes.all_colors = false;
        cx.notify();
    }

    /// A color changed: a seed brings the colors made from it along, unless they were tuned by hand.
    fn set_token(&mut self, token: &'static str, color: u32, cx: &mut Context<Self>) {
        let Some((theme, _)) = self.themes.draft.as_mut() else { return };
        if theme.tokens.get(token) == color {
            return;
        }
        let tokens = theme.tokens;
        if let Some(i) = SEEDS.iter().position(|s| *s == token) {
            let seeds = SEEDS.map(|s| tokens.get(s));
            let before = Tokens::derive(seeds);
            let mut next_seeds = seeds;
            next_seeds[i] = color;
            let after = Tokens::derive(next_seeds);
            let mut next = tokens;
            next.set(token, color);
            for k in TOKENS {
                if !SEEDS.contains(&k) && tokens.get(k) == before.get(k) {
                    next.set(k, after.get(k));
                }
            }
            theme.tokens = next;
        } else {
            theme.tokens.set(token, color);
        }
        cx.notify();
    }

    fn save_theme(&mut self, use_it: bool, cx: &mut Context<Self>) {
        let Some((mut theme, new)) = self.themes.draft.take() else { return };
        if theme.name.trim().is_empty() {
            theme.name = t("appsettings.themes.untitled");
        }
        theme.updated_at = crate::core::dms::now_ms();
        let (id, name) = (theme.id.clone(), theme.name.clone());
        let in_use = self.core.prefs().theme == id;
        self.set(cx, |pr| {
            match pr.custom_themes.iter_mut().find(|t| t.id == theme.id) {
                Some(slot) => *slot = theme,
                None => pr.custom_themes.push(theme),
            }
            if use_it || in_use {
                pr.theme = id;
                pr.follow_system = false;
            }
        });
        self.toast(
            "sparkles",
            t_with(
                if new { "appsettings.themes.made" } else { "appsettings.themes.saved" },
                &[("theme", Arg::Str(&name))],
            ),
            cx,
        );
    }

    pub(crate) fn themes_page(
        &mut self,
        prefs: &Prefs,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.themes.draft.is_some() {
            return motion::slide_in(div().child(self.theme_editor(prefs, p, window, cx)), "theme-editor", 24.0)
                .into_any_element();
        }
        motion::slide_in(div().child(self.theme_library(prefs, p, window, cx)), "theme-library", -24.0)
            .into_any_element()
    }

    fn theme_library(&mut self, prefs: &Prefs, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let custom = prefs.custom_themes.clone();
        let current = prefs.active_theme(system_dark(window.appearance()));
        let full = custom.len() >= themes::MAX_CUSTOM_THEMES;
        let importing = self.themes.importing;
        let head = div()
            .flex()
            .flex_wrap()
            .items_end()
            .justify_between()
            .gap(px(12.0))
            .child(
                div().child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("appsettings.themes.yourThemes"))).child(
                    div()
                        .mt(px(2.0))
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child(t("appsettings.themes.yourThemesHint")),
                ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(
                        button(
                            "themes-import",
                            if importing { t("appsettings.themes.importing") } else { t("appsettings.themes.import") },
                            Some("file-up"),
                            Look::Outline,
                            true,
                            p,
                        )
                        .when(full || importing, |el| el.opacity(0.5))
                        .when(!full && !importing, |el| {
                            el.on_click(cx.listener(|this, _, window, cx| this.import_theme(window, cx)))
                        }),
                    )
                    .child({
                        let base = current.clone();
                        button("themes-new", t("appsettings.themes.new"), Some("plus"), Look::Primary, true, p)
                            .when(full, |el| el.opacity(0.5))
                            .when(!full, |el| {
                                el.on_click(
                                    cx.listener(move |this, _, window, cx| this.edit_theme(&base, true, window, cx)),
                                )
                            })
                    }),
            );
        let notes = self.themes.notes.clone().or_else(|| self.look_note());
        let notes = notes.map(|(name, lines)| {
            motion::rise(
                div()
                    .flex()
                    .gap(px(12.0))
                    .rounded(radius_2xl())
                    .border_1()
                    .border_color(alpha(rgb(0xf59e0b), 0.4))
                    .bg(alpha(rgb(0xf59e0b), 0.1))
                    .p(px(12.0))
                    .text_sm()
                    .child(icon("triangle-alert").size(px(16.0)).mt(px(2.0)).text_color(rgb(0xd97706)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().font_weight(FontWeight::BOLD).child(name))
                            .children(lines.into_iter().map(|l| div().text_color(p.muted_foreground).child(l))),
                    )
                    .child(
                        div()
                            .id("themes-got-it")
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.muted_foreground)
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.themes.notes = None;
                                this.clear_look_note();
                                cx.notify();
                            }))
                            .child(t("appsettings.themes.gotIt")),
                    ),
                "themes-notes",
                Duration::ZERO,
                -6.0,
            )
        });
        let body: AnyElement = if custom.is_empty() {
            let base = current.clone();
            let hover = alpha(p.primary, 0.5);
            div()
                .id("themes-empty")
                .group("themes-empty")
                .flex()
                .flex_col()
                .items_center()
                .gap(px(12.0))
                .rounded(radius_3xl())
                .border_2()
                .border_dashed()
                .border_color(p.border)
                .px(px(24.0))
                .py(px(40.0))
                .text_center()
                .cursor_pointer()
                .hover(move |s| s.border_color(hover))
                .active(|s| s.scale(0.98))
                .on_click(cx.listener(move |this, _, window, cx| this.edit_theme(&base, true, window, cx)))
                .child(
                    div()
                        .id("themes-empty-wand")
                        .group_hover("themes-empty", |s| s.scale(1.1))
                        .size(px(56.0))
                        .rounded(radius_2xl())
                        .bg(alpha(p.primary, 0.15))
                        .text_color(p.primary)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon("wand-sparkles").size(px(28.0))),
                )
                .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("appsettings.themes.empty")))
                .child(
                    div()
                        .max_w(px(384.0))
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child(t("appsettings.themes.emptyHint")),
                )
                .into_any_element()
        } else {
            let w = (self.column - 12.0) / 2.0;
            let mut grid = div().flex().flex_col().gap(px(12.0));
            let mut row = div().flex().gap(px(12.0));
            for (n, theme) in custom.iter().enumerate() {
                row = row.child(self.theme_card(theme, theme.id == current.id, w, n, full, p, cx));
                if n % 2 == 1 {
                    grid = grid.child(row);
                    row = div().flex().gap(px(12.0));
                }
            }
            if custom.len() % 2 == 1 {
                grid = grid.child(row);
            }
            grid.into_any_element()
        };
        let mut builtins = div().flex().flex_wrap().gap(px(8.0));
        for theme in themes::builtins() {
            let tk = theme.tokens;
            let base = theme.clone();
            builtins = builtins.child(
                div()
                    .id(SharedString::from(format!("from-{}", theme.id)))
                    .group(SharedString::from(format!("from-{}", theme.id)))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded_full()
                    .border_1()
                    .border_color(rgb(tk.get("border")))
                    .bg(rgb(tk.get("background")))
                    .text_color(rgb(tk.get("foreground")))
                    .py(px(6.0))
                    .pl(px(6.0))
                    .pr(px(12.0))
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .hover(|s| s.translate_y(px(-2.0)))
                    .active(|s| s.scale(0.95))
                    .when(full, |el| el.opacity(0.5))
                    .when(!full, |el| {
                        el.on_click(cx.listener(move |this, _, window, cx| this.edit_theme(&base, true, window, cx)))
                    })
                    .child(
                        div()
                            .size(px(24.0))
                            .rounded_full()
                            .bg(rgb(tk.get("primary")))
                            .text_color(rgb(tk.get("primary-foreground")))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .id(SharedString::from(format!("from-{}-brush", theme.id)))
                                    .group_hover(SharedString::from(format!("from-{}", theme.id)), |s| {
                                        s.rotate(gpui_kit::radians(-12f32.to_radians()))
                                    })
                                    .child(icon("paintbrush").size(px(14.0))),
                            ),
                    )
                    .child(theme.name.clone()),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(32.0))
            .child(div().flex().flex_col().gap(px(16.0)).child(head).children(notes).child(body).when(full, |el| {
                el.child(
                    div().text_xs().text_color(p.muted_foreground).child(t_with(
                        "appsettings.themes.full",
                        &[("count", Arg::Num(themes::MAX_CUSTOM_THEMES as i64))],
                    )),
                )
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(
                        div()
                            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("appsettings.themes.fromBuiltin")))
                            .child(
                                div()
                                    .mt(px(2.0))
                                    .text_sm()
                                    .text_color(p.muted_foreground)
                                    .child(t("appsettings.themes.fromBuiltinHint")),
                            ),
                    )
                    .child(builtins),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn theme_card(
        &mut self,
        theme: &Theme,
        active: bool,
        width: f32,
        n: usize,
        full: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tk = theme.tokens;
        let c = |name: &str| rgb(tk.get(name));
        let picture = theme.backdrop.as_ref().map(|b| b.image.clone()).filter(|s| !s.is_empty());
        let effect = theme.backdrop.as_ref().is_some_and(|b| b.effect != Effect::None);
        let id = theme.id.clone();
        let (edit, dup, export, gone) = (theme.clone(), theme.clone(), theme.clone(), theme.id.clone());
        let mut items = vec![Item::action(t("appsettings.themes.edit"), Some("pencil"), move |this, cx| {
            this.state.pending_edit = Some((edit.clone(), false));
            cx.notify();
        })];
        if !full {
            items.push(Item::action(t("appsettings.themes.duplicate"), Some("copy"), move |this, cx| {
                this.state.pending_edit = Some((dup.clone(), true));
                cx.notify();
            }));
        }
        items.push(Item::action(t("appsettings.themes.export"), Some("download"), move |this, cx| {
            this.export_theme_file(export.clone(), cx)
        }));
        items.push(Item::Separator);
        items.push(Item::danger(t("appsettings.themes.delete"), Some("trash"), move |this, cx| {
            this.delete_theme_by_id(gone.clone(), cx)
        }));
        let more = self.dropdown(
            format!("theme-more-{}", theme.id),
            div()
                .id(SharedString::from(format!("theme-more-btn-{}", theme.id)))
                .size(px(28.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.1)))
                .child(icon("ellipsis").size(px(16.0))),
            items,
            true,
            192.0,
            p,
            cx,
        );
        motion::rise(
            div()
                .id(SharedString::from(format!("theme-card-{id}")))
                .relative()
                .w(px(width))
                .hover(|s| s.translate_y(px(-3.0)))
                .flex_none()
                .overflow_hidden()
                .rounded(radius_2xl())
                .border_2()
                .border_color(if active { c("primary") } else { c("border") })
                .bg(c("background"))
                .text_color(c("foreground"))
                .when(active, |el| el.shadow(crate::ui::settings_controls::shadow_lg()))
                .when_some(picture, |el, url| {
                    el.child(
                        img(SharedString::from(url))
                            .absolute()
                            .inset_0()
                            .size_full()
                            .object_fit(ObjectFit::Cover)
                            .opacity(0.3),
                    )
                })
                .child(
                    div()
                        .id(SharedString::from(format!("theme-use-{id}")))
                        .relative()
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .p(px(16.0))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let id = id.clone();
                            this.set(cx, move |pr| {
                                pr.theme = id;
                                pr.follow_system = false;
                            })
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .children(["primary", "card", "muted-foreground", "border"].map(|n| {
                                    div().size(px(20.0)).rounded_full().border_1().border_color(c("border")).bg(c(n))
                                }))
                                .when(effect, |el| el.child(icon("sparkles").size(px(16.0)).text_color(c("primary")))),
                        )
                        .child(
                            div()
                                .pr(px(32.0))
                                .child(div().truncate().font_weight(FontWeight::EXTRA_BOLD).child(theme.name.clone()))
                                .child(div().truncate().text_xs().text_color(c("muted-foreground")).child(
                                    theme.description.clone().unwrap_or_else(|| {
                                        if active {
                                            t("appsettings.themes.inUse")
                                        } else {
                                            t("appsettings.themes.clickToUse")
                                        }
                                    }),
                                )),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(12.0))
                        .right(px(12.0))
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .when(active, |el| {
                            el.child(
                                div()
                                    .size(px(24.0))
                                    .rounded_full()
                                    .bg(c("primary"))
                                    .text_color(c("primary-foreground"))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(icon("check").size(px(16.0))),
                            )
                        })
                        .child(more),
                ),
            SharedString::from(format!("theme-card-{}", theme.id)),
            Duration::from_millis(30 * n as u64),
            10.0,
        )
        .into_any_element()
    }

    fn theme_editor(&mut self, prefs: &Prefs, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some((theme, new)) = self.themes.draft.clone() else { return div().into_any_element() };
        let changed = new || self.themes.start.as_ref() != Some(&theme);
        let wide = self.wide;
        let form_w = if wide { self.column - 40.0 - 288.0 } else { self.column };
        let tk = theme.tokens;
        // Keep the hex fields in step with colors changed another way (a seed moving the rest).
        for token in TOKENS {
            if let Some(state) = self.themes.hex.get(token).cloned() {
                let typed = state.read(cx).value().to_string();
                let focused = gpui_kit::Focusable::focus_handle(state.read(cx), cx).is_focused(window);
                let typed_color = themes::parse_hex(if typed.starts_with('#') { &typed } else { "" });
                if !focused && typed_color != Some(tk.get(token)) {
                    state.update(cx, |s, cx| s.set_value(themes::hex(tk.get(token)), window, cx));
                }
            }
        }
        let group = |title: String, hint: String, body: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .border_t_1()
                .border_color(p.border)
                .pt(px(24.0))
                .child(
                    div()
                        .child(div().font_weight(FontWeight::EXTRA_BOLD).child(title))
                        .child(div().mt(px(2.0)).text_sm().text_color(p.muted_foreground).child(hint)),
                )
                .child(body)
        };
        let color_field = |token: &'static str, w: f32, this: &Self| {
            let label = token_label(token);
            div()
                .w(px(w))
                .flex()
                .items_center()
                .gap(px(8.0))
                .rounded(radius_xl())
                .border_1()
                .border_color(p.border)
                .p(px(6.0))
                .pr(px(8.0))
                .child(
                    div()
                        .size(px(32.0))
                        .flex_none()
                        .rounded(radius_lg())
                        .border_1()
                        .border_color(p.border)
                        .bg(rgb(tk.get(token))),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(10.4))
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.muted_foreground)
                                .child(label),
                        )
                        .when_some(this.themes.hex.get(token).cloned(), |el, s| {
                            el.child(
                                div().h(px(18.0)).text_xs().font_family("monospace").child(
                                    gpui_kit::component::Sizable::xsmall(Input::new(&s).appearance(false))
                                        .px(px(0.0))
                                        .py(px(0.0))
                                        .h(px(18.0)),
                                ),
                            )
                        }),
                )
        };
        let seeds_w = (form_w - 16.0) / 3.0;
        let mut seeds = div().flex().flex_col().gap(px(8.0));
        for chunk in SEEDS.chunks(3) {
            let mut row = div().flex().gap(px(8.0));
            for token in chunk {
                row = row.child(color_field(token, seeds_w, self));
            }
            seeds = seeds.child(row);
        }
        let rest: Vec<&'static str> = TOKENS.iter().copied().filter(|k| !SEEDS.contains(k)).collect();
        let rest_w = (form_w - 24.0) / 4.0;
        let mut others = div().flex().flex_col().gap(px(8.0)).pt(px(4.0));
        for chunk in rest.chunks(4) {
            let mut row = div().flex().gap(px(8.0));
            for token in chunk {
                row = row.child(color_field(token, rest_w, self));
            }
            others = others.child(row);
        }
        let all = self.themes.all_colors;
        let colors = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(seeds)
            .child(
                div()
                    .id("theme-all-colors")
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.primary)
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.themes.all_colors = !this.themes.all_colors;
                        cx.notify();
                    }))
                    .child(if all { t("appsettings.themes.hideColors") } else { t("appsettings.themes.allColors") }),
            )
            .when(all, |el| el.child(motion::rise(others, "theme-others", Duration::ZERO, -6.0)));
        let corners = self.labeled_slider(
            "theme-corners",
            &t("appsettings.themes.corners"),
            format!("{:.2}rem", theme.radius),
            (themes::RADIUS_MIN * 100.0, themes::RADIUS_MAX * 100.0, 5.0),
            theme.radius * 100.0,
            Rc::new(|this: &mut SettingsView, v: f32, cx: &mut Context<SettingsView>| {
                if let Some((th, _)) = this.themes.draft.as_mut() {
                    th.radius = (v / 100.0 * 100.0).round() / 100.0;
                }
                cx.notify();
            }),
            p,
            window,
            cx,
        );
        let app_backdrop = prefs.backdrop.clone();
        let own = theme.backdrop.is_some();
        let mut backdrop = div().flex().flex_col().gap(px(8.0)).child(toggle(
            "theme-own-backdrop",
            &t("appsettings.themes.ownBackdrop"),
            Some(&t("appsettings.themes.ownBackdropHint")),
            own,
            false,
            p,
            window,
            cx,
            move |this, on, cx| {
                if let Some((th, _)) = this.themes.draft.as_mut() {
                    th.backdrop = on.then(|| app_backdrop.clone());
                }
                cx.notify();
            },
        ));
        if own {
            backdrop = backdrop.child(motion::rise(
                div().pt(px(8.0)).child(self.backdrop_form(Target::Draft, form_w, p, window, cx)),
                "theme-backdrop-form",
                Duration::ZERO,
                -6.0,
            ));
        }
        let label = |text: String| {
            div()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(p.muted_foreground)
                .child(tracked(text.to_uppercase(), WIDE))
        };
        let form = div()
            .flex()
            .flex_col()
            .gap(px(28.0))
            .pb(px(24.0))
            .child(
                div().flex().child(
                    button("theme-back", t("appsettings.themes.yourThemes"), Some("arrow-left"), Look::Ghost, true, p)
                        .ml(px(-8.0))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.themes.draft = None;
                            cx.notify();
                        })),
                ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(12.0))
                    .when_some(self.themes.name.clone(), |el, s| {
                        el.child(
                            div()
                                .w(px((form_w - 12.0) * 0.4))
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .child(label(t("appsettings.themes.name")))
                                .child(
                                    field(Input::new(&s).appearance(false), p)
                                        .h(px(36.0))
                                        .rounded(crate::ui::theme::radius_md()),
                                ),
                        )
                    })
                    .when_some(self.themes.description.clone(), |el, s| {
                        el.child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .child(label(t("appsettings.themes.description")))
                                .child(
                                    field(Input::new(&s).appearance(false), p)
                                        .h(px(36.0))
                                        .rounded(crate::ui::theme::radius_md()),
                                ),
                        )
                    }),
            )
            .child(group(t("appsettings.themes.colors"), t("appsettings.themes.colorsHint"), colors.into_any_element()))
            .child(group(t("appsettings.themes.corners"), t("appsettings.themes.cornersHint"), corners))
            .child(group(
                t("appsettings.themes.backdrop"),
                t("appsettings.themes.backdropHint"),
                backdrop.into_any_element(),
            ));
        let bar = div()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(8.0))
            .rounded(radius_2xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .p(px(12.0))
            .shadow(crate::ui::settings_controls::shadow_xl())
            .child(div().mr_auto().text_sm().text_color(p.muted_foreground).child(if changed {
                if new { t("appsettings.themes.newTheme") } else { t("settings.controls.unsaved") }
            } else {
                t("appsettings.themes.noChanges")
            }))
            .child(button("theme-cancel", t("common.cancel"), None, Look::Ghost, false, p).on_click(cx.listener(
                |this, _, _, cx| {
                    this.themes.draft = None;
                    cx.notify();
                },
            )))
            .child(
                button("theme-save", t("appsettings.themes.save"), None, Look::Outline, false, p)
                    .when(!changed, |el| el.opacity(0.5))
                    .when(changed, |el| el.on_click(cx.listener(|this, _, _, cx| this.save_theme(false, cx)))),
            )
            .child(
                button("theme-save-use", t("appsettings.themes.saveAndUse"), Some("sparkles"), Look::Primary, false, p)
                    .on_click(cx.listener(|this, _, _, cx| this.save_theme(true, cx))),
            );
        let shown = theme.backdrop.clone().unwrap_or_else(|| prefs.backdrop.clone());
        let readable = contrast(tk.get("foreground"), tk.get("background"));
        let on_primary = contrast(tk.get("primary-foreground"), tk.get("primary"));
        let rating = |label: String, ratio: f32| {
            let (key, color) = if ratio >= 4.5 {
                ("appsettings.themes.easy", rgb(0x059669))
            } else if ratio >= 3.0 {
                ("appsettings.themes.largeOnly", rgb(0xd97706))
            } else {
                ("appsettings.themes.hard", p.destructive)
            };
            div()
                .flex()
                .items_center()
                .justify_between()
                .rounded(radius_xl())
                .border_1()
                .border_color(p.border)
                .px(px(12.0))
                .py(px(8.0))
                .text_xs()
                .child(div().font_weight(FontWeight::BOLD).child(label))
                .child(
                    div()
                        .rounded_full()
                        .bg(alpha(color, 0.15))
                        .px(px(8.0))
                        .py(px(2.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(color)
                        .child(t_with(key, &[("ratio", Arg::Str(&format!("{ratio:.1}")))])),
                )
        };
        let preview = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(theme_preview(&theme, &shown, p, window, cx))
            .child(rating(t("appsettings.themes.token.foreground"), readable))
            .child(rating(t("appsettings.themes.token.primaryForeground"), on_primary));
        div()
            .flex()
            .flex_col()
            .child(with_preview(form, preview, wide, p))
            .child(motion::rise(div().pb(px(16.0)).child(bar), "theme-bar", Duration::ZERO, 24.0))
            .into_any_element()
    }

    /// An edit or duplicate picked from a card's menu, opened once the menu's gone.
    pub(crate) fn open_pending_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((theme, new)) = self.state.pending_edit.take() {
            self.edit_theme(&theme, new, window, cx);
        }
    }
}
