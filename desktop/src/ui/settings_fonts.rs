//! The Font and Code font settings on the Appearance page, as the web's: the
//! app's own font, the system's, any family on this computer (found with a
//! search, each name drawn in its own font) or a font file of yours
//! (`core::fonts`), with a line in the font picked. Kept on this computer.

use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::config::{FontPick, Prefs};
use crate::core::fonts;
use crate::core::i18n::{Arg, t, t_with};
use crate::ui::motion;
use crate::ui::settings::SettingsView;
use crate::ui::settings_controls::{Look, Opt, button, choice, field, hint, sample, search_clear, warn};
use crate::ui::theme::{self, Palette, alpha, radius_lg, radius_xl};
use crate::ui::widgets::{icon, inner_scroll};

/// What kind of font a pick is, as its cards offer them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Default,
    System,
    Installed,
    File,
}

fn kind_of(pick: &FontPick) -> Kind {
    match pick {
        FontPick::Default => Kind::Default,
        FontPick::System => Kind::System,
        FontPick::Installed { .. } => Kind::Installed,
        FontPick::File { .. } => Kind::File,
    }
}

/// What one font setting keeps between frames.
#[derive(Default)]
pub(crate) struct FontForm {
    /// A card picked that needs a family or a file before it's saved, with
    /// the pick it was over (another change to the pick forgets it).
    kind: Option<(Kind, FontPick)>,
    search: Option<Entity<InputState>>,
    busy: bool,
    error: Option<String>,
}

/// The font setting `code` is about.
fn pick_of(prefs: &Prefs, code: bool) -> &FontPick {
    if code { &prefs.code_font } else { &prefs.font }
}

fn pick_mut(prefs: &mut Prefs, code: bool) -> &mut FontPick {
    if code { &mut prefs.code_font } else { &mut prefs.font }
}

/// How tall a family's line is in the list, and how much of the list shows.
const ROW: f32 = 34.0;
const LIST_H: f32 = 238.0;

impl SettingsView {
    /// The Font (or, with `code`, Code font) setting: its cards, what the kind picked asks
    /// for, and a line in the font on screen.
    pub(crate) fn font_setting(
        &mut self,
        code: bool,
        prefs: &Prefs,
        p: &Palette,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let slot = usize::from(code);
        let pick = pick_of(prefs, code).clone();
        if self.look.fonts[slot].kind.as_ref().is_some_and(|(_, over)| *over != pick) {
            self.look.fonts[slot].kind = None;
        }
        let kinds: &'static [Kind] = if code {
            &[Kind::Default, Kind::Installed, Kind::File]
        } else {
            &[Kind::Default, Kind::System, Kind::Installed, Kind::File]
        };
        let shown = self.look.fonts[slot].kind.as_ref().map_or(kind_of(&pick), |(k, _)| *k);
        let options = kinds
            .iter()
            .map(|k| match k {
                Kind::Default if code => Opt::new(
                    t("appsettings.appearance.codeFontDefault"),
                    t("appsettings.appearance.codeFontDefaultHint"),
                    "code",
                ),
                Kind::Default => Opt::new(
                    t("appsettings.appearance.fontDefault"),
                    t("appsettings.appearance.fontDefaultHint"),
                    "cloud",
                ),
                Kind::System => Opt::new(
                    t("appsettings.appearance.fontSystem"),
                    t("appsettings.appearance.fontSystemHint"),
                    "monitor",
                ),
                Kind::Installed => Opt::new(
                    t("appsettings.appearance.fontInstalled"),
                    t("appsettings.appearance.fontInstalledHint"),
                    "type",
                ),
                Kind::File => {
                    Opt::new(t("appsettings.appearance.fontFile"), t("appsettings.appearance.fontFileHint"), "file-up")
                }
            })
            .collect();
        let cards = choice(
            if code { "code-font" } else { "font" },
            kinds.iter().position(|k| *k == shown),
            options,
            width,
            p,
            window,
            cx,
            move |this, n, cx| {
                let kind = kinds[n];
                let form = &mut this.look.fonts[slot];
                form.error = None;
                match kind {
                    Kind::Default | Kind::System => {
                        form.kind = None;
                        let pick = if kind == Kind::Default { FontPick::Default } else { FontPick::System };
                        this.set(cx, |pr| *pick_mut(pr, code) = pick);
                    }
                    Kind::Installed | Kind::File => {
                        let now = pick_of(&this.core.prefs_now(), code).clone();
                        form.kind = (kind_of(&now) != kind).then_some((kind, now));
                        cx.notify();
                    }
                }
            },
        );
        let below: Option<AnyElement> = match shown {
            Kind::Installed => Some(self.font_list(code, &pick, p, window, cx)),
            Kind::File => Some(self.font_file(code, &pick, p, cx)),
            Kind::Default | Kind::System => None,
        };
        let id = if code { "code-font" } else { "font" };
        let missing = match &pick {
            FontPick::Installed { family } | FontPick::File { family, .. } if theme::font_missing(code) => {
                Some(t_with("appsettings.appearance.fontMissing", &[("name", Arg::Str(family))]))
            }
            _ => None,
        };
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(cards)
            .when_some(below, |el, below| {
                el.child(motion::rise(
                    div().child(below),
                    SharedString::from(format!("{id}-below-{shown:?}")),
                    Duration::ZERO,
                    10.0,
                ))
            })
            .when_some(missing, |el, m| el.child(warn(m, p)))
            .child(
                sample(p)
                    .font_family(if code { theme::mono() } else { theme::font() })
                    .child(t("appsettings.appearance.fontSample")),
            )
            .into_any_element()
    }

    /// The families on this computer, found with a search, each drawn in its own font. Only
    /// the lines in sight are built: there can be thousands.
    fn font_list(
        &mut self,
        code: bool,
        pick: &FontPick,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let slot = usize::from(code);
        let id = if code { "code-font" } else { "font" };
        let search = self.look.fonts[slot]
            .search
            .get_or_insert_with(|| {
                let s = cx.new(|cx| InputState::new(window, cx).placeholder(t("appsettings.appearance.fontSearch")));
                cx.subscribe(&s, |_, _, e: &InputEvent, cx| {
                    if matches!(e, InputEvent::Change) {
                        cx.notify();
                    }
                })
                .detach();
                s
            })
            .clone();
        let query = search.read(cx).value().trim().to_lowercase();
        let all = theme::font_names(cx);
        let found: Vec<&String> =
            all.iter().filter(|n| !n.starts_with('.') && n.to_lowercase().contains(&query)).collect();
        let chosen = match pick {
            FontPick::Installed { family } => Some(family.as_str()),
            _ => None,
        };
        let input = div()
            .relative()
            .child(field(div().pl(px(28.0)).pr(px(28.0)).child(Input::new(&search).appearance(false)), p))
            .child(
                div()
                    .absolute()
                    .left(px(12.0))
                    .top(px(14.0))
                    .text_color(p.muted_foreground)
                    .child(icon("search").size(px(16.0))),
            )
            .children(search_clear(
                if code { "code-font-search-clear" } else { "font-search-clear" },
                !query.is_empty(),
                p,
                window,
                cx,
                move |this, window, cx| {
                    if let Some(s) = &this.look.fonts[slot].search {
                        s.update(cx, |s, cx| s.set_value("", window, cx));
                    }
                    cx.notify();
                },
            ));
        if found.is_empty() {
            return div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(input)
                .child(hint(t("appsettings.appearance.fontNone"), p))
                .into_any_element();
        }
        // The list's own scroll handle (`inner_scroll` keeps it under the same key), to know
        // which lines are in sight.
        let list_id = SharedString::from(format!("{id}-list"));
        let handle = window
            .use_keyed_state(SharedString::from(format!("{list_id}|inner-scroll")), cx, |_, _| ScrollHandle::new())
            .read(cx)
            .clone();
        let top = (-f32::from(handle.offset().y)).max(0.0);
        let first = ((top / ROW).floor() as usize).min(found.len().saturating_sub(1));
        let last = (((top + LIST_H) / ROW).ceil() as usize + 1).min(found.len());
        let (hover, on_bg, on_fg) = (alpha(p.primary, 0.08), alpha(p.primary, 0.14), p.primary);
        let mut lines = div().relative().w_full().h(px(found.len() as f32 * ROW));
        for (n, name) in found.iter().enumerate().take(last).skip(first) {
            let on = chosen == Some(name.as_str());
            let family = (*name).clone();
            lines = lines.child(
                div()
                    .id(SharedString::from(format!("{id}-family-{n}")))
                    .absolute()
                    .top(px(n as f32 * ROW))
                    .left_0()
                    .right_0()
                    .h(px(ROW))
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(radius_lg())
                    .cursor_pointer()
                    .when(on, |el| el.bg(on_bg).text_color(on_fg))
                    .when(!on, |el| el.hover(move |s| s.bg(hover)))
                    .active(|s| s.scale(0.98))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.look.fonts[slot].kind = None;
                        let family = family.clone();
                        this.set(cx, |pr| *pick_mut(pr, code) = FontPick::Installed { family });
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_family(SharedString::from((*name).clone()))
                            .child((*name).clone()),
                    )
                    .when(on, |el| el.child(icon("check").size(px(16.0)))),
            );
        }
        let list = inner_scroll(list_id, window, cx)
            .h(px(LIST_H.min(found.len() as f32 * ROW + 8.0)))
            .p(px(4.0))
            .rounded(radius_xl())
            .border_1()
            .border_color(p.border)
            .bg(p.background)
            .child(lines);
        div().flex().flex_col().gap(px(8.0)).child(input).child(list).into_any_element()
    }

    /// A font file of yours: the button to choose one, its name, and what went wrong.
    fn font_file(&mut self, code: bool, pick: &FontPick, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let slot = usize::from(code);
        let id = if code { "code-font" } else { "font" };
        let form = &self.look.fonts[slot];
        let name = match pick {
            FontPick::File { name, family, .. } => Some(if name.is_empty() { family.clone() } else { name.clone() }),
            _ => None,
        };
        let label = if name.is_some() {
            t("appsettings.appearance.fontOtherFile")
        } else {
            t("appsettings.appearance.fontPickFile")
        };
        let types = t_with(
            "appsettings.appearance.fontFileTypes",
            &[("types", Arg::Str(fonts::FILE_TYPES)), ("size", Arg::Str(&fonts::most_size()))],
        );
        let busy = form.busy;
        let error = form.error.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        button(
                            SharedString::from(format!("{id}-choose-file")),
                            label,
                            Some(if busy { "loader-circle" } else { "folder-open" }),
                            Look::Outline,
                            true,
                            p,
                        )
                        .when(busy, |el| el.opacity(0.6))
                        .on_click(cx.listener(move |this, _, _, cx| this.choose_font_file(code, cx))),
                    )
                    .when_some(name, |el, name| {
                        el.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child(icon("file-type").size(px(14.0)).text_color(p.muted_foreground))
                                .child(div().min_w_0().truncate().child(name)),
                        )
                    }),
            )
            .child(div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(types))
            .when_some(error, |el, e| el.child(warn(e, p)))
            .into_any_element()
    }

    /// Asks for a font file, copies it into the app's folder once it's known to be a font,
    /// and makes it the font (or the code font).
    fn choose_font_file(&mut self, code: bool, cx: &mut Context<Self>) {
        let slot = usize::from(code);
        if self.look.fonts[slot].busy {
            return;
        }
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("appsettings.appearance.fontPickFile").into()),
        });
        let (core, dir) = (self.core.clone(), self.core.paths.fonts());
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| {
                let form = &mut this.look.fonts[slot];
                form.busy = true;
                form.error = None;
                cx.notify();
            });
            let rx = core.spawn(async move {
                tokio::task::spawn_blocking(move || fonts::import(&dir, &path))
                    .await
                    .unwrap_or_else(|_| Err(t("appsettings.appearance.fontCantSave")))
            });
            let result = rx.await;
            let _ = this.update(cx, |this, cx| {
                let form = &mut this.look.fonts[slot];
                form.busy = false;
                match result {
                    Ok(Ok(pick)) => {
                        form.kind = None;
                        crate::core::reports::used("fonts.file");
                        this.set(cx, |pr| *pick_mut(pr, code) = pick);
                    }
                    Ok(Err(error)) => {
                        form.error = Some(error);
                        cx.notify();
                    }
                    Err(_) => cx.notify(),
                }
            });
        })
        .detach();
    }
}
