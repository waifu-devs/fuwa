//! The Appearance and Background pages: fuwa's themes (the five built-in
//! ones and those made or imported here), light and dark picks that follow
//! the system, theme files in and out, and the picture and texture behind
//! the app. The same themes and files as the web app (`docs/themes.md`).

use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Subscription,
    Window, div, img, px,
};

use crate::core::config::Prefs;
use crate::core::themes::{self, Backdrop, Effect, Fit, Picture, Theme};
use crate::ui::settings::{SettingsView, radio, section, segmented, theme_preview, toggle_row};
use crate::ui::theme::{Palette, alpha, corner, mix, system_dark};
use crate::ui::widgets::{icon, icon_button, soft_button};

/// What the two pages keep between frames.
pub struct Look {
    dim: Entity<SliderState>,
    intensity: Entity<SliderState>,
    panels: Entity<SliderState>,
    /// The instance whose backgrounds are listed, and them.
    backgrounds: Option<(String, Vec<String>)>,
    loading: Option<String>,
    busy: bool,
    /// The last thing worth saying (an error when the flag is set).
    note: Option<(bool, String)>,
    _subscriptions: Vec<Subscription>,
}

impl Look {
    pub fn new(prefs: &Prefs, window: &mut Window, cx: &mut Context<SettingsView>) -> Self {
        let b = &prefs.backdrop;
        let slider = |min: u8, max: u8, value: u8, cx: &mut Context<SettingsView>| {
            cx.new(|_| {
                SliderState::new().min(f32::from(min)).max(f32::from(max)).step(1.0).default_value(f32::from(value))
            })
        };
        let dim = slider(themes::DIM.0, themes::DIM.1, b.dim, cx);
        let intensity = slider(themes::INTENSITY.0, themes::INTENSITY.1, b.intensity, cx);
        let panels = slider(themes::PANELS.0, themes::PANELS.1, b.panels, cx);
        let watch = |state: &Entity<SliderState>, set: fn(&mut Backdrop, u8), cx: &mut Context<SettingsView>| {
            cx.subscribe_in(state, window, move |this, _, event: &SliderEvent, _, cx| {
                let SliderEvent::Change(value) = event else { return };
                let v = value.start().round().clamp(0.0, 255.0) as u8;
                this.set(cx, |pr| set(&mut pr.backdrop, v));
            })
        };
        let subscriptions = vec![
            watch(&dim, |b, v| b.dim = v, cx),
            watch(&intensity, |b, v| b.intensity = v, cx),
            watch(&panels, |b, v| b.panels = v, cx),
        ];
        Self {
            dim,
            intensity,
            panels,
            backgrounds: None,
            loading: None,
            busy: false,
            note: None,
            _subscriptions: subscriptions,
        }
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
        self.say(false, "Theme deleted.", cx);
    }

    /// Asks for a theme file, reads it, uploads its picture (if it has one) and puts the theme on.
    fn import_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.look.busy {
            return;
        }
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import a theme".into()),
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
                    let meta = tokio::fs::metadata(&path).await.map_err(|e| format!("Couldn't read that file: {e}"))?;
                    if meta.len() > (themes::MAX_FILE_PICTURE_BYTES as u64) * 2 {
                        return Err("That file is too big to be a theme.".to_owned());
                    }
                    let text =
                        tokio::fs::read_to_string(&path).await.map_err(|_| "That isn't a theme file.".to_owned())?;
                    let mut imported = themes::parse_file(&text)?;
                    if let Some(picture) = imported.picture.take() {
                        match &key {
                            Some(key) => match core.upload_background(key, picture).await {
                                Ok(url) => {
                                    if let Some(b) = imported.theme.backdrop.as_mut() {
                                        b.image = url;
                                    }
                                }
                                Err(err) => imported.notes.push(format!("Its picture didn't upload: {}", err.message)),
                            },
                            None => imported
                                .notes
                                .push("Its picture needs an instance to live on; sign in and import it again.".into()),
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
                            this.say(true, "There are 50 themes here already; delete one to make room.", cx);
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
                        let mut note = format!("{name} is on.");
                        if this.core.prefs().follow_system && theme_dark != dark {
                            note = format!("{name} is your {} theme now.", if theme_dark { "dark" } else { "light" });
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
    fn export_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prefs = self.core.prefs();
        let theme = prefs.active_theme(system_dark(window.appearance()));
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
                    tokio::fs::write(&path, file).await.map_err(|e| format!("Couldn't save the theme: {e}"))?;
                    Ok::<_, String>(theme.name)
                }
            });
            let result = rx.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(Ok(name)) => this.say(false, format!("Saved {name}. Open it in any fuwa app to use it."), cx),
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
        let all = prefs.all_themes();
        let on_screen = prefs.active_theme(system_dark(window.appearance()));
        let pickers: AnyElement = if prefs.follow_system {
            let (light, dark): (Vec<Theme>, Vec<Theme>) = all.iter().cloned().partition(|t| !t.dark());
            div()
                .flex()
                .flex_col()
                .gap(px(18.0))
                .child(self.theme_grid("light", "sun", "Light", light, &prefs.light_theme, Slot::Light, p, cx))
                .child(self.theme_grid("dark", "moon", "Dark", dark, &prefs.dark_theme, Slot::Dark, p, cx))
                .into_any_element()
        } else {
            self.theme_grid("all", "palette", "", all, &prefs.theme, Slot::Only, p, cx)
        };
        let tools = div()
            .flex()
            .flex_wrap()
            .gap(px(10.0))
            .child(
                soft_button("theme-import", if self.look.busy { "Importing…" } else { "Import a theme file" }, p)
                    .child(icon("import").size(px(16.0)))
                    .on_click(cx.listener(|this, _, window, cx| this.import_theme(window, cx))),
            )
            .child(
                soft_button("theme-export", format!("Export {}", on_screen.name), p)
                    .child(icon("download").size(px(16.0)))
                    .on_click(cx.listener(|this, _, window, cx| this.export_theme(window, cx))),
            );
        let note = self.look.note.clone().map(|(error, text)| {
            div()
                .flex()
                .items_start()
                .gap(px(8.0))
                .text_sm()
                .text_color(if error { p.destructive } else { p.muted_foreground })
                .child(icon(if error { "circle-alert" } else { "sparkles" }).size(px(16.0)).mt(px(2.0)))
                .child(div().flex_1().child(text))
        });
        let sizes = [(0.9, "Small"), (1.0, "Normal"), (1.15, "Large"), (1.3, "Larger")];
        div()
            .flex()
            .flex_col()
            .gap(px(28.0))
            .child(toggle_row(
                "follow-system",
                "Light and dark like my computer",
                "Switches between a light theme and a dark one when your computer does.",
                prefs.follow_system,
                p,
                cx,
                |this, on, cx| this.set(cx, |pr| pr.follow_system = on),
            ))
            .child(section(
                "Theme",
                div().flex().flex_col().gap(px(14.0)).child(pickers).child(tools).children(note),
                p,
            ))
            .child(section(
                "Text size",
                segmented(
                    "size",
                    sizes.iter().map(|(v, l)| (*l, (prefs.text_scale - *v).abs() < 0.01)).collect(),
                    p,
                    window,
                    cx,
                    move |this, n, cx| {
                        let v = sizes[n].0;
                        this.set(cx, |pr| pr.text_scale = v);
                    },
                ),
                p,
            ))
            .child(section(
                "Message density",
                segmented(
                    "density",
                    vec![
                        ("Cozy", prefs.density == crate::core::config::Density::Cozy),
                        ("Compact", prefs.density == crate::core::config::Density::Compact),
                    ],
                    p,
                    window,
                    cx,
                    |this, n, cx| {
                        use crate::core::config::Density;
                        this.set(cx, |pr| pr.density = if n == 0 { Density::Cozy } else { Density::Compact })
                    },
                ),
                p,
            ))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn theme_grid(
        &mut self,
        id: &'static str,
        glyph: &str,
        label: &str,
        list: Vec<Theme>,
        chosen: &str,
        slot: Slot,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut grid = div().flex().flex_wrap().gap(px(12.0));
        for theme in list {
            let on = theme.id == chosen;
            let pv = Palette::of(&theme);
            let t = theme.clone();
            let delete = (!theme.builtin).then(|| theme.id.clone());
            grid = grid.child(
                div()
                    .id(SharedString::from(format!("theme-{id}-{}", theme.id)))
                    .relative()
                    .w(px(196.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .p(px(8.0))
                    .rounded(corner(16.0))
                    .border_2()
                    .border_color(if on { p.primary } else { p.border })
                    .bg(if on { alpha(p.primary, 0.06) } else { alpha(p.card, 0.5) })
                    .cursor_pointer()
                    .hover({
                        let c = mix(p.border, p.primary, 0.5);
                        move |s| s.border_color(c)
                    })
                    .active(|s| s.top(px(1.0)))
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_theme(&t, slot, cx)))
                    .child(theme_preview(&pv))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .font_weight(FontWeight::BOLD)
                            .text_sm()
                            .child(radio(on, p))
                            .child(div().flex_1().min_w_0().truncate().child(theme.name.clone()))
                            .when_some(delete, |el, theme_id| {
                                el.child(
                                    icon_button(SharedString::from(format!("theme-del-{id}-{theme_id}")), "trash", p)
                                        .size(px(24.0))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.delete_theme(theme_id.clone(), cx)
                                        })),
                                )
                            }),
                    ),
            );
        }
        if label.is_empty() {
            return grid.into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .child(icon(glyph).size(px(15.0)))
                    .child(label.to_owned()),
            )
            .child(grid)
            .into_any_element()
    }

    // ───────────────────────── Background ─────────────────────────

    fn load_backgrounds(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.look.loading.as_deref() == Some(key) {
            return;
        }
        self.look.loading = Some(key.to_owned());
        let (core, k) = (self.core.clone(), key.to_owned());
        let rx = self.core.spawn(async move { core.backgrounds(&k).await });
        let key = key.to_owned();
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.look.backgrounds = Some((key, result.unwrap_or_default()));
                cx.notify();
            });
        })
        .detach();
    }

    fn upload_background(&mut self, key: String, cx: &mut Context<Self>) {
        if self.look.busy {
            return;
        }
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose a picture".into()),
        });
        let core = self.core.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let Some(kind) = crate::core::account::picture_type(&name) else {
                let _ = this.update(cx, |this, cx| {
                    this.say(true, "That isn't a picture fuwa can use (PNG, JPEG, GIF or WebP).", cx)
                });
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.look.busy = true;
                this.look.note = None;
                cx.notify();
            });
            let rx = core.spawn({
                let (core, key) = (core.clone(), key.clone());
                async move {
                    let bytes = tokio::fs::read(&path).await.map_err(|err| {
                        crate::core::api::Problem::new(tonic::Code::NotFound, format!("Couldn't read that file: {err}"))
                    })?;
                    core.upload_background(&key, Picture { content_type: kind.into(), bytes }).await
                }
            });
            let result = rx.await;
            let _ = this.update(cx, |this, cx| {
                this.look.busy = false;
                match result {
                    Ok(Ok(url)) => {
                        if let Some((k, list)) = this.look.backgrounds.as_mut()
                            && *k == key
                        {
                            list.insert(0, url.clone());
                        }
                        this.set(cx, |pr| pr.backdrop.image = url);
                    }
                    Ok(Err(err)) => this.say(true, err.message, cx),
                    Err(_) => {}
                }
            });
        })
        .detach();
    }

    fn delete_background(&mut self, key: String, url: String, cx: &mut Context<Self>) {
        let (core, k, u) = (self.core.clone(), key.clone(), url.clone());
        let rx = self.core.spawn(async move { core.delete_background(&k, &u).await });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => {
                    if let Some((_, list)) = this.look.backgrounds.as_mut() {
                        list.retain(|l| *l != url);
                    }
                    if this.core.prefs().backdrop.image == url {
                        this.set(cx, |pr| pr.backdrop.image.clear());
                    }
                    cx.notify();
                }
                Err(err) => this.say(true, err.message, cx),
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
        let b = prefs.backdrop.clone();
        let theme = prefs.active_theme(system_dark(window.appearance()));
        let key = instance_in_use(self);
        if let Some(key) = &key
            && self.look.backgrounds.as_ref().is_none_or(|(k, _)| k != key)
        {
            self.load_backgrounds(key, cx);
        }

        let tile = |id: SharedString, on: bool| {
            div()
                .id(id)
                .relative()
                .w(px(136.0))
                .h(px(86.0))
                .rounded(corner(12.0))
                .overflow_hidden()
                .border_2()
                .border_color(if on { p.primary } else { p.border })
                .cursor_pointer()
                .hover({
                    let c = mix(p.border, p.primary, 0.5);
                    move |s| s.border_color(c)
                })
                .active(|s| s.top(px(1.0)))
        };
        let mut pictures = div().flex().flex_wrap().gap(px(10.0)).child(
            tile("bg-none".into(), b.image.is_empty())
                .bg(p.secondary)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(4.0))
                .text_sm()
                .text_color(p.muted_foreground)
                .on_click(cx.listener(|this, _, _, cx| this.set(cx, |pr| pr.backdrop.image.clear())))
                .child(icon("image-off").size(px(20.0)))
                .child("No picture"),
        );
        let list = match (&key, &self.look.backgrounds) {
            (Some(key), Some((k, list))) if k == key => list.clone(),
            _ => Vec::new(),
        };
        for url in list {
            let on = b.image == url;
            let (pick, gone, k) = (url.clone(), url.clone(), key.clone().unwrap_or_default());
            pictures = pictures.child(
                tile(SharedString::from(format!("bg-{url}")), on)
                    .group("bg")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let pick = pick.clone();
                        this.set(cx, |pr| pr.backdrop.image = pick)
                    }))
                    .child(img(SharedString::from(url.clone())).size_full().object_fit(ObjectFit::Cover))
                    .child(
                        div().absolute().top(px(4.0)).right(px(4.0)).child(
                            icon_button(SharedString::from(format!("bg-del-{url}")), "trash", p)
                                .size(px(26.0))
                                .bg(alpha(p.card, 0.85))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.delete_background(k.clone(), gone.clone(), cx)
                                })),
                        ),
                    ),
            );
        }
        let pictures: AnyElement = match key {
            Some(key) => pictures
                .child(
                    tile("bg-upload".into(), false)
                        .border_dashed()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(4.0))
                        .text_sm()
                        .text_color(p.primary)
                        .on_click(cx.listener(move |this, _, _, cx| this.upload_background(key.clone(), cx)))
                        .child(icon(if self.look.busy { "loader" } else { "image-plus" }).size(px(20.0)))
                        .child(if self.look.busy { "Uploading…" } else { "Upload a picture" }),
                )
                .into_any_element(),
            None => div()
                .text_sm()
                .text_color(p.muted_foreground)
                .child("Pictures live on an instance; sign in to one to add them.")
                .into_any_element(),
        };

        let mut effects = div().flex().flex_wrap().gap(px(8.0));
        for effect in [Effect::None, Effect::Grain, Effect::Paper, Effect::Dots, Effect::Grid] {
            let on = b.effect == effect;
            effects = effects.child(
                div()
                    .id(SharedString::from(format!("fx-{effect:?}")))
                    .w(px(120.0))
                    .p(px(10.0))
                    .rounded(corner(12.0))
                    .border_2()
                    .border_color(if on { p.primary } else { p.border })
                    .bg(if on { alpha(p.primary, 0.06) } else { alpha(p.card, 0.5) })
                    .cursor_pointer()
                    .hover({
                        let c = mix(p.border, p.primary, 0.5);
                        move |s| s.border_color(c)
                    })
                    .active(|s| s.top(px(1.0)))
                    .on_click(cx.listener(move |this, _, _, cx| this.set(cx, |pr| pr.backdrop.effect = effect)))
                    .child(div().text_sm().font_weight(FontWeight::BOLD).child(effect.name()))
                    .child(div().text_xs().text_color(p.muted_foreground).child(effect.hint())),
            );
        }
        let animated = !b.effect.texture() && b.effect != Effect::None;

        let slider_row = |label: &str, state: &Entity<SliderState>, value: String| {
            div()
                .flex()
                .items_center()
                .gap(px(16.0))
                .child(div().w(px(150.0)).text_sm().font_weight(FontWeight::BOLD).child(label.to_owned()))
                .child(div().flex_1().child(Slider::new(state)))
                .child(div().w(px(48.0)).text_sm().text_color(p.muted_foreground).child(value))
        };

        div()
            .flex()
            .flex_col()
            .gap(px(28.0))
            .when(theme.backdrop.is_some(), |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .p(px(14.0))
                        .rounded(corner(14.0))
                        .bg(p.secondary)
                        .text_sm()
                        .child(icon("palette").size(px(18.0)).text_color(p.primary))
                        .child(format!(
                            "{} brings its own background, so this one shows under your other themes.",
                            theme.name
                        )),
                )
            })
            .child(section("Picture", pictures, p))
            .when(!b.image.is_empty(), |el| {
                el.child(section(
                    "Fit",
                    segmented(
                        "fit",
                        vec![
                            ("Fill", b.fit == Fit::Cover),
                            ("Whole picture", b.fit == Fit::Contain),
                            ("Repeat", b.fit == Fit::Tile),
                        ],
                        p,
                        window,
                        cx,
                        |this, n, cx| {
                            let fit = [Fit::Cover, Fit::Contain, Fit::Tile][n];
                            this.set(cx, |pr| pr.backdrop.fit = fit)
                        },
                    ),
                    p,
                ))
                .child(slider_row("Dim the picture", &self.look.dim, format!("{}%", b.dim)))
            })
            .child(section(
                "Texture",
                div().flex().flex_col().gap(px(10.0)).child(effects).when(animated, |el| {
                    el.child(
                        div().text_sm().text_color(p.muted_foreground).child(format!(
                            "{} moves in the web app; here it shows as just the picture.",
                            b.effect.name()
                        )),
                    )
                }),
                p,
            ))
            .when(b.effect.texture(), |el| {
                el.child(slider_row("Strength", &self.look.intensity, format!("{}%", b.intensity)))
            })
            .when(b.any(), |el| el.child(slider_row("Solid panels", &self.look.panels, format!("{}%", b.panels))))
            .into_any_element()
    }
}

/// Which pick a theme card sets.
#[derive(Clone, Copy)]
enum Slot {
    Only,
    Light,
    Dark,
}

/// Keeps a slider where the setting is when something else changed it.
pub fn sync_sliders(look: &Look, b: &Backdrop, window: &mut Window, cx: &mut Context<SettingsView>) {
    for (state, v) in [(&look.dim, b.dim), (&look.intensity, b.intensity), (&look.panels, b.panels)] {
        if (state.read(cx).value().start() - f32::from(v)).abs() > 0.5 {
            state.update(cx, |s, cx| s.set_value(f32::from(v), window, cx));
        }
    }
}
