//! The Language page: "Like my computer", then every language this build
//! ships (`core::i18n`, the same catalogs as the web app), each by its own name.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::config::Prefs;
use crate::core::i18n::{self, Arg, t, t_with};
use crate::ui::motion;
use crate::ui::settings::SettingsView;
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::icon;

impl SettingsView {
    pub(crate) fn language_page(
        &mut self,
        prefs: &Prefs,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let system = i18n::system_language();
        let name_of = |code: &str| {
            i18n::LANGUAGES
                .iter()
                .find(|l| l.code == code)
                .map(|l| l.meta.name.clone())
                .unwrap_or_else(|| "English".into())
        };
        let chosen = prefs.language.clone().filter(|c| i18n::LANGUAGES.iter().any(|l| &l.code == c));
        let mut rows: Vec<(Option<String>, String, Vec<String>, bool)> = vec![(
            None,
            t("settings.language.matchSystem"),
            vec![t_with("settings.language.matchBrowserNote", &[("language", Arg::Str(&name_of(&system)))])],
            false,
        )];
        for l in i18n::LANGUAGES.iter() {
            let mut notes = Vec::new();
            if l.meta.english != l.meta.name {
                notes.push(l.meta.english.clone());
            }
            let share = i18n::coverage(&l.code);
            if share < 1.0 {
                let percent = (share * 100.0).floor() as i64;
                notes.push(t_with("settings.language.coverage", &[("percent", Arg::Str(&format!("{percent}%")))]));
            }
            rows.push((Some(l.code.clone()), l.meta.name.clone(), notes, !l.meta.reviewed));
        }

        let mut list = div().flex().flex_col().gap(px(6.0)).max_w(px(560.0));
        for (n, (code, name, notes, draft)) in rows.into_iter().enumerate() {
            let on = code == chosen;
            // The highlight eases between rows instead of jumping.
            let lit =
                motion::follow(SharedString::from(format!("lang-lit-{n}")), if on { 1.0 } else { 0.0 }, window, cx);
            let pick = code.clone();
            let check = div().size(px(20.0)).flex_none().flex().items_center().justify_center().when(on, |el| {
                el.child(motion::once(
                    icon("check").size(px(16.0)).text_color(p.primary),
                    SharedString::from(format!("lang-check-{}", code.as_deref().unwrap_or("auto"))),
                    Duration::from_millis(360),
                    |el, t| {
                        let s = 1.0 - (1.0 - t).powi(3);
                        el.opacity(s).relative().top(px(4.0 * (1.0 - s)))
                    },
                ))
            });
            let mut text = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(div().text_sm().font_weight(FontWeight::BOLD).text_color(p.foreground).child(name));
            for note in notes {
                text = text.child(div().text_xs().text_color(p.muted_foreground).child(note));
            }
            if draft {
                text = text.child(div().text_xs().text_color(p.primary).child(t("settings.language.draft")));
            }
            list = list.child(
                div()
                    .id(SharedString::from(format!("lang-{n}")))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .px(px(14.0))
                    .py(px(12.0))
                    .rounded(corner(14.0))
                    .border_1()
                    .border_color(alpha(p.primary, 0.35 * lit))
                    .bg(alpha(p.primary, 0.10 * lit))
                    .cursor_pointer()
                    .hover(|el| el.bg(alpha(p.primary, 0.06)))
                    .child(
                        icon(if code.is_none() { "monitor" } else { "languages" })
                            .size(px(16.0))
                            .text_color(p.muted_foreground),
                    )
                    .child(text)
                    .child(check)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let pick = pick.clone();
                        this.set(cx, |pr| pr.language = pick);
                        // Every window's text comes from the new catalog.
                        window.refresh();
                        cx.refresh_windows();
                    })),
            );
        }
        list.into_any_element()
    }
}
