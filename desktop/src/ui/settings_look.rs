//! The Appearance page, as the web's `settings/app/Appearance.tsx`: the
//! theme cards (light and dark picks that follow the system), density,
//! message display, text size and zoom beside a live chat preview; and
//! theme files in and out, for the Themes page (`docs/themes.md`).

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb,
};
use std::time::Duration;

use crate::core::config::Prefs;
use crate::core::config::{Density, Spacing};
use crate::core::i18n::{Arg, t, t_with};
use crate::core::themes::{self, Backdrop, Picture, Theme};
use crate::ui::motion;
use crate::ui::settings::{Page, SettingsView};
use crate::ui::settings_app::pref;
use crate::ui::settings_controls::{Badge, Opt, choice, toggle, with_preview};
use crate::ui::text::{WIDE, tracked};
use crate::ui::theme::{Palette, alpha, system_dark};
use crate::ui::widgets::icon;

/// What the Appearance page keeps between frames: an import going, and what it said.
pub struct Look {
    busy: bool,
    /// The last thing worth saying (an error when the flag is set).
    note: Option<(bool, String)>,
}

impl Look {
    pub fn new() -> Self {
        Self { busy: false, note: None }
    }
}

/// The instance pictures go to: the account the settings show, or the first one signed in.
fn instance_in_use(view: &SettingsView) -> Option<String> {
    let signed_in: Vec<String> = view.core.shared.read(|s| {
        s.order
            .iter()
            .filter_map(|k| s.instance(k))
            .filter(|i| i.connection != crate::core::store::Connection::SignedOut && i.me.is_some())
            .map(|i| i.key.clone())
            .collect()
    });
    view.account.key.clone().filter(|k| signed_in.contains(k)).or_else(|| signed_in.into_iter().next())
}

impl SettingsView {
    fn say(&mut self, error: bool, text: impl Into<String>, cx: &mut Context<Self>) {
        self.look.note = Some((error, text.into()));
        cx.notify();
    }

    /// Picks a theme: the one on screen, or the light or dark one when following the system.
    fn pick_theme(&mut self, theme: &Theme, slot: Slot, cx: &mut Context<Self>) {
        let id = theme.id.clone();
        crate::core::reports::used("theme.change");
        self.set(cx, |pr| match slot {
            Slot::Only => pr.theme = id,
            Slot::Light => pr.light_theme = id,
            Slot::Dark => pr.dark_theme = id,
        });
    }

    fn delete_theme(&mut self, id: String, cx: &mut Context<Self>) {
        self.set(cx, |pr| {
            pr.custom_themes.retain(|t| t.id != id);
            pr.tidy();
        });
        self.say(false, t("desktop.look.themeDeleted"), cx);
    }

    /// Asks for a theme file, reads it, uploads its picture (if it has one) and puts the theme on.
    pub(crate) fn import_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.look.busy {
            return;
        }
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("desktop.look.importTheme").into()),
        });
        let (core, key, dark) = (self.core.clone(), instance_in_use(self), system_dark(window.appearance()));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| {
                this.look.busy = true;
                this.look.note = None;
                cx.notify();
            });
            let rx = core.spawn({
                let core = core.clone();
                async move {
                    let meta = tokio::fs::metadata(&path).await.map_err(|_| t("desktop.look.cantRead"))?;
                    if meta.len() > (themes::MAX_FILE_PICTURE_BYTES as u64) * 2 {
                        return Err(t("appsettings.themes.tooBig"));
                    }
                    let text = tokio::fs::read_to_string(&path).await.map_err(|_| t("system.themeFile.notTheme"))?;
                    let mut imported = themes::parse_file(&text)?;
                    if let Some(picture) = imported.picture.take() {
                        match &key {
                            Some(key) => match core.upload_background(key, picture).await {
                                Ok(url) => {
                                    if let Some(b) = imported.theme.backdrop.as_mut() {
                                        b.image = url;
                                    }
                                }
                                Err(err) => imported
                                    .notes
                                    .push(t_with("desktop.look.pictureFailed", &[("error", Arg::Str(&err.message))])),
                            },
                            None => imported.notes.push(t("desktop.look.pictureNeedsInstance")),
                        }
                    }
                    Ok(imported)
                }
            });
            let result = rx.await;
            let _ = this.update(cx, |this, cx| {
                this.look.busy = false;
                match result {
                    Ok(Ok(imported)) => {
                        let theme = imported.theme;
                        let (id, name, theme_dark) = (theme.id.clone(), theme.name.clone(), theme.dark());
                        let full = this.core.prefs().custom_themes.len() >= themes::MAX_CUSTOM_THEMES;
                        if full {
                            this.say(true, t("desktop.look.full"), cx);
                            return;
                        }
                        this.set(cx, |pr| {
                            pr.custom_themes.push(theme);
                            if !pr.follow_system {
                                pr.theme = id;
                            } else if theme_dark {
                                pr.dark_theme = id;
                            } else {
                                pr.light_theme = id;
                            }
                        });
                        let mut note = t_with("desktop.look.themeOn", &[("theme", Arg::Str(&name))]);
                        if this.core.prefs().follow_system && theme_dark != dark {
                            note = if theme_dark {
                                t_with("desktop.look.nowDark", &[("theme", Arg::Str(&name))])
                            } else {
                                t_with("desktop.look.nowLight", &[("theme", Arg::Str(&name))])
                            };
                        }
                        for n in imported.notes {
                            note.push(' ');
                            note.push_str(&n);
                        }
                        this.say(false, note, cx);
                    }
                    Ok(Err(message)) => this.say(true, message, cx),
                    Err(_) => {}
                }
            });
        })
        .detach();
    }

    /// Saves the theme on screen as a file, with its background picture when it has one.
    pub(crate) fn export_theme_file(&mut self, theme: Theme, cx: &mut Context<Self>) {
        let prefs = self.core.prefs();
        let backdrop = Some(prefs.active_backdrop(&theme)).filter(Backdrop::any);
        let dir = dirs::download_dir().or_else(dirs::home_dir).unwrap_or_default();
        let path = cx.prompt_for_new_path(&dir, Some(&themes::file_name(&theme.name)));
        let core = self.core.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = path.await else { return };
            let rx = core.spawn({
                let core = core.clone();
                async move {
                    let link = backdrop.as_ref().map(|b| b.image.clone()).filter(|l| !l.is_empty());
                    let picture: Option<Picture> = match link {
                        Some(link) => core.background_bytes(&link).await,
                        None => None,
                    };
                    let file = themes::to_file(&theme, backdrop.as_ref(), picture.as_ref());
                    tokio::fs::write(&path, file).await.map_err(|_| t("desktop.look.cantSave"))?;
                    Ok::<_, String>(theme.name)
                }
            });
            let result = rx.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(Ok(name)) => {
                    this.toast("download", t_with("appsettings.themes.exported", &[("theme", Arg::Str(&name))]), cx)
                }
                Ok(Err(message)) => this.say(true, message, cx),
                Err(_) => {}
            });
        })
        .detach();
    }

    pub(crate) fn appearance_page(
        &mut self,
        prefs: &Prefs,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wide = self.wide;
        let form_w = if wide { self.column - 40.0 - 288.0 } else { self.column };
        let all = prefs.all_themes();
        let follow = toggle(
            "follow-system",
            &t("appsettings.appearance.followSystem"),
            Some(&t("appsettings.appearance.followSystemHint")),
            prefs.follow_system,
            false,
            p,
            window,
            cx,
            |this, on, cx| this.set(cx, |pr| pr.follow_system = on),
        );
        let grids: AnyElement = if prefs.follow_system {
            let (light, dark): (Vec<Theme>, Vec<Theme>) = all.iter().cloned().partition(|t| !t.dark());
            motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .child(
                        div()
                            .child(
                                div()
                                    .mb(px(8.0))
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(p.muted_foreground)
                                    .child(tracked(t("appsettings.appearance.whenLight").to_uppercase(), WIDE)),
                            )
                            .child(self.theme_grid(
                                "light",
                                light,
                                &prefs.light_theme,
                                Slot::Light,
                                false,
                                form_w,
                                p,
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .child(
                                div()
                                    .mb(px(8.0))
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(p.muted_foreground)
                                    .child(tracked(t("appsettings.appearance.whenDark").to_uppercase(), WIDE)),
                            )
                            .child(self.theme_grid("dark", dark, &prefs.dark_theme, Slot::Dark, false, form_w, p, cx)),
                    ),
                "grids-system",
                Duration::ZERO,
                10.0,
            )
            .into_any_element()
        } else {
            motion::rise(
                div().child(self.theme_grid("fixed", all, &prefs.theme, Slot::Only, true, form_w, p, cx)),
                "grids-fixed",
                Duration::ZERO,
                10.0,
            )
            .into_any_element()
        };
        let theme = div().flex().flex_col().gap(px(12.0)).child(follow).child(grids);
        let density_at = match prefs.spacing {
            Spacing::Compact => 0,
            Spacing::Default => 1,
            Spacing::Spacious => 2,
        };
        let density = choice(
            "density",
            Some(density_at),
            vec![
                Opt::new(t("appsettings.appearance.compact"), t("appsettings.appearance.compactHint"), "rows-4"),
                Opt::new(t("appsettings.appearance.default"), t("appsettings.appearance.defaultHint"), "rows-3"),
                Opt::new(t("appsettings.appearance.spacious"), t("appsettings.appearance.spaciousHint"), "rows-2"),
            ],
            form_w,
            p,
            window,
            cx,
            |this, n, cx| {
                let s = [Spacing::Compact, Spacing::Default, Spacing::Spacious][n];
                this.set(cx, |pr| pr.spacing = s)
            },
        );
        let display = choice(
            "message-display",
            Some(if prefs.density == Density::Cozy { 0 } else { 1 }),
            vec![
                Opt::new(t("appsettings.appearance.cozy"), t("appsettings.appearance.cozyHint"), "message-square-text"),
                Opt::new(
                    t("appsettings.appearance.compact"),
                    t("appsettings.appearance.compactDisplayHint"),
                    "text-align-justify",
                ),
            ],
            form_w,
            p,
            window,
            cx,
            |this, n, cx| {
                let d = if n == 0 { Density::Cozy } else { Density::Compact };
                this.set(cx, |pr| pr.density = d)
            },
        );
        let px_of = |n: f32| format!("{}px", n.round() as i64);
        let font = self.slider(
            "chat-font-size",
            (12.0, 20.0, 1.0),
            f32::from(prefs.chat_font_size),
            vec![(12.0, px_of(12.0)), (15.0, px_of(15.0)), (20.0, px_of(20.0))],
            |n| format!("{}px", n.round() as i64),
            false,
            p,
            window,
            cx,
            |pr, v| pr.chat_font_size = v.round() as u8,
        );
        let percent = |n: f32| format!("{}%", n.round() as i64);
        let zoom = self.slider(
            "zoom",
            (80.0, 150.0, 10.0),
            f32::from(prefs.zoom),
            vec![(80.0, percent(80.0)), (100.0, percent(100.0)), (150.0, percent(150.0))],
            |n| format!("{}%", n.round() as i64),
            true,
            p,
            window,
            cx,
            |pr, v| {
                pr.zoom = v.round() as u16;
                pr.text_scale = f32::from(pr.zoom) / 100.0;
            },
        );
        let d = Prefs::default();
        let theme_changed = prefs.theme != d.theme
            || prefs.follow_system != d.follow_system
            || prefs.light_theme != d.light_theme
            || prefs.dark_theme != d.dark_theme;
        let form = self.stack(
            [
                (
                    "theme",
                    t("appsettings.appearance.theme"),
                    Some(t("appsettings.appearance.themeHint")),
                    Badge::pref(theme_changed, |pr| {
                        let d = Prefs::default();
                        pr.theme = d.theme;
                        pr.follow_system = d.follow_system;
                        pr.light_theme = d.light_theme;
                        pr.dark_theme = d.dark_theme;
                    }),
                    theme.into_any_element(),
                ),
                (
                    "density",
                    t("appsettings.appearance.density"),
                    Some(t("appsettings.appearance.densityHint")),
                    pref!(prefs, spacing),
                    density,
                ),
                ("message-display", t("appsettings.appearance.display"), None, pref!(prefs, density), display),
                ("chat-font-size", t("appsettings.appearance.textSize"), None, pref!(prefs, chat_font_size), font),
                (
                    "zoom",
                    t("appsettings.appearance.zoom"),
                    Some(t("appsettings.appearance.zoomHint")),
                    Badge::pref(prefs.zoom != 100, |pr| {
                        pr.zoom = 100;
                        pr.text_scale = 1.0;
                    }),
                    zoom,
                ),
            ],
            p,
            cx,
        );
        let preview = self.chat_preview(prefs, p);
        with_preview(form, preview, wide, p)
    }

    /// A few messages in the current look, so density, display and text size show before you
    /// leave (the web's `ChatPreview`).
    fn chat_preview(&mut self, prefs: &Prefs, p: &Palette) -> AnyElement {
        let you = self.account_me().map(|(_, me)| me);
        let now = crate::core::dms::now_ms();
        let hana = crate::pb::User {
            id: "01HANA".into(),
            username: "hana".into(),
            display_name: "Hana".into(),
            ..Default::default()
        };
        let you = you.unwrap_or_else(|| crate::pb::User {
            id: "01YOU".into(),
            username: "you".into(),
            display_name: t("appsettings.preview.you"),
            ..Default::default()
        });
        let lines = [
            (&hana, true, now - 6 * 60_000, t("appsettings.preview.newThemes")),
            (&hana, false, now - 5 * 60_000, t_with("appsettings.preview.summer", &[("theme", Arg::Str("Sora"))])),
            (&you, true, now - 60_000, t_with("appsettings.preview.favorite", &[("theme", Arg::Str("**Matcha**"))])),
        ];
        let compact = prefs.density == Density::Compact;
        let k = match prefs.spacing {
            Spacing::Compact => 0.5,
            Spacing::Default => 1.0,
            Spacing::Spacious => 1.6,
        };
        let font = f32::from(prefs.chat_font_size);
        let mut card = div()
            .rounded(crate::ui::theme::radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(p.background)
            .py(px(12.0))
            .shadow(crate::ui::settings_controls::shadow_lg());
        for (n, (user, first, at, text)) in lines.into_iter().enumerate() {
            let name = crate::core::store::user_name(user);
            let tint = crate::ui::widgets::name_tint(&user.id, p);
            let body = crate::ui::text::markdown(SharedString::from(format!("preview-md-{n}")), text)
                .w_full()
                .into_any_element();
            let row = if compact {
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(8.0))
                    .px(px(16.0))
                    .py(px(1.0 * k))
                    .when(first, |el| el.mt(px(6.0 * k)).pt(px(2.0 * k)))
                    .child(div().flex_none().text_xs().text_color(p.muted_foreground).child(crate::ui::text::clock(at)))
                    .child(div().flex_none().font_weight(FontWeight::BOLD).text_color(tint).child(name))
                    .child(div().flex_1().min_w_0().text_size(px(font)).line_height(px(font * 1.625)).child(body))
            } else {
                div()
                    .flex()
                    .gap(px(12.0))
                    .px(px(16.0))
                    .py(px(2.0 * k))
                    .when(first, |el| el.mt(px(12.0 * k)).pt(px(4.0 * k)))
                    .child(
                        div()
                            .w(px(40.0))
                            .flex_none()
                            .when(first, |el| el.child(crate::ui::widgets::avatar(Some(user), 40.0, p))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .when(first, |el| {
                                el.child(
                                    div()
                                        .flex()
                                        .items_baseline()
                                        .gap(px(8.0))
                                        .h(px(24.0))
                                        .child(
                                            div()
                                                .font_weight(FontWeight::BOLD)
                                                .text_size(px(16.0))
                                                .text_color(tint)
                                                .child(name),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(p.muted_foreground)
                                                .child(crate::ui::text::when(at)),
                                        ),
                                )
                            })
                            .child(div().text_size(px(font)).line_height(px(font * 1.625)).child(body)),
                    )
            };
            card = card.child(row);
        }
        card.into_any_element()
    }

    /// Theme cards in their own colors; the check sits on the picked one. With `make`, a card to
    /// make your own (the web's `ThemeGrid`).
    #[allow(clippy::too_many_arguments)]
    fn theme_grid(
        &mut self,
        id: &'static str,
        list: Vec<Theme>,
        chosen: &str,
        slot: Slot,
        make: bool,
        width: f32,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let gap = 12.0;
        let card_w = (width - gap) / 2.0;
        let mut cards: Vec<AnyElement> = Vec::new();
        for (n, theme) in list.into_iter().enumerate() {
            let on = theme.id == chosen;
            let tk = &theme.tokens;
            let c = |name: &str| rgb(tk.get(name));
            let about = if theme.builtin {
                let key = format!("appsettings.themes.builtin.{}", theme.id);
                let text = t(&key);
                if text == key { theme.description.clone().unwrap_or_default() } else { text }
            } else {
                theme.description.clone().unwrap_or_default()
            };
            let picked = theme.clone();
            let hover_border = if on { c("primary") } else { c("border") };
            let card = div()
                .id(SharedString::from(format!("theme-{id}-{}", theme.id)))
                .relative()
                .w(px(card_w))
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .p(px(16.0))
                .rounded(crate::ui::theme::radius_2xl())
                .border_2()
                .border_color(if on { c("primary") } else { c("border") })
                .bg(c("background"))
                .text_color(c("foreground"))
                .when(on, |el| el.shadow(crate::ui::settings_controls::shadow_lg()))
                .cursor_pointer()
                .hover(move |s| s.top(px(-3.0)).border_color(hover_border))
                .active(|s| s.top(px(1.0)))
                .on_click(cx.listener(move |this, _, _, cx| this.pick_theme(&picked, slot, cx)))
                .child(div().flex().items_center().gap(px(8.0)).children(
                    ["primary", "card", "muted-foreground", "border"].map(|name| {
                        div().size(px(20.0)).rounded_full().border_1().border_color(c("border")).bg(c(name))
                    }),
                ))
                .child(
                    div()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .child(theme.name.clone())
                                .when(!theme.builtin, |el| {
                                    el.child(icon("sparkles").size(px(14.0)).text_color(c("primary")))
                                }),
                        )
                        .child(div().text_xs().line_height(px(16.0)).text_color(c("muted-foreground")).child(about)),
                )
                .when(on, |el| {
                    el.child(motion::once(
                        div()
                            .absolute()
                            .top(px(12.0))
                            .right(px(12.0))
                            .size(px(24.0))
                            .rounded_full()
                            .bg(c("primary"))
                            .text_color(c("primary-foreground"))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon("check").size(px(16.0))),
                        SharedString::from(format!("theme-check-{id}-{}", theme.id)),
                        Duration::from_millis(280),
                        |el, t| el.opacity(t),
                    ))
                });
            cards.push(
                motion::rise(
                    div().child(card),
                    SharedString::from(format!("theme-in-{id}-{n}")),
                    Duration::from_millis(40 * n as u64),
                    10.0,
                )
                .into_any_element(),
            );
        }
        if make {
            let n = cards.len();
            let (hover_border, hover_fg) = (alpha(p.primary, 0.5), p.primary);
            cards.push(
                motion::rise(
                    div().child(
                        div()
                            .id(SharedString::from(format!("theme-make-{id}")))
                            .w(px(card_w))
                            .h_full()
                            .min_h(px(124.0))
                            .flex()
                            .flex_col()
                            .items_start()
                            .justify_between()
                            .gap(px(12.0))
                            .p(px(16.0))
                            .rounded(crate::ui::theme::radius_2xl())
                            .border_2()
                            .border_dashed()
                            .border_color(p.border)
                            .text_color(p.muted_foreground)
                            .cursor_pointer()
                            .hover(move |s| s.border_color(hover_border).text_color(hover_fg).top(px(-3.0)))
                            .active(|s| s.top(px(1.0)))
                            .on_click(cx.listener(|this, _, _, cx| this.choose(Page::Themes, None, cx)))
                            .child(
                                div()
                                    .size(px(28.0))
                                    .rounded_full()
                                    .bg(p.muted)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(icon("plus").size(px(16.0))),
                            )
                            .child(
                                div()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::EXTRA_BOLD)
                                            .child(t("appsettings.themes.makeYourOwn")),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .line_height(px(16.0))
                                            .child(t("appsettings.themes.makeYourOwnHint")),
                                    ),
                            ),
                    ),
                    SharedString::from(format!("theme-in-{id}-make")),
                    Duration::from_millis(40 * n as u64),
                    10.0,
                )
                .into_any_element(),
            );
        }
        let mut grid = div().flex().flex_col().gap(px(gap));
        let mut cards = cards.into_iter();
        loop {
            let a = cards.next();
            let Some(a) = a else { break };
            let b = cards.next();
            grid = grid.child(div().flex().gap(px(gap)).child(a).children(b));
        }
        grid.into_any_element()
    }

    /// What the last import or export said, for the Themes page.
    pub(crate) fn look_note(&self) -> Option<(String, Vec<String>)> {
        self.look.note.clone().map(|(_, text)| (String::new(), vec![text]))
    }

    pub(crate) fn clear_look_note(&mut self) {
        self.look.note = None;
    }

    pub(crate) fn delete_theme_by_id(&mut self, id: String, cx: &mut Context<Self>) {
        self.delete_theme(id, cx);
    }
}

/// Which pick a theme card sets.
#[derive(Clone, Copy)]
enum Slot {
    Only,
    Light,
    Dark,
}
