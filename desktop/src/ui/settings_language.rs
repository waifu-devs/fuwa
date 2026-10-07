//! The Language page, as the web's `settings/app/Language.tsx`: "Like my
//! computer", then every language this build ships (`core::i18n`, the same
//! catalogs as the web app), as option cards with how much of each is there.

use gpui_kit::{AnyElement, Context, IntoElement, ParentElement as _, Styled as _, Window, div};

use crate::core::config::Prefs;
use crate::core::i18n::{self, Arg, t, t_with};
use crate::ui::settings::SettingsView;
use crate::ui::settings_controls::{At, Badge, Opt, choice};
use crate::ui::theme::Palette;

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
        let mut codes: Vec<Option<String>> = vec![None];
        let mut options = vec![Opt::new(
            t("settings.language.matchSystem"),
            t_with("settings.language.matchBrowserNote", &[("language", Arg::Str(&name_of(&system)))]),
            "monitor",
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
            if !l.meta.reviewed {
                notes.push(t("settings.language.draft"));
            }
            options.push(Opt::new(l.meta.name.clone(), notes.join(" · "), "languages"));
            codes.push(Some(l.code.clone()));
        }
        let at = codes.iter().position(|c| *c == chosen);
        let picker = choice("language", at, options, self.column, p, window, cx, move |this, n, cx| {
            let pick = codes[n].clone();
            this.set(cx, |pr| pr.language = pick);
            // Every window's text comes from the new catalog.
            cx.refresh_windows();
        });
        let row = self.setting(
            "language",
            &t("settings.language.title"),
            None,
            Badge::pref(prefs.language.is_some(), |pr| pr.language = None),
            At::of(0, 1),
            picker,
            p,
            cx,
        );
        div().flex().flex_col().child(row).into_any_element()
    }
}
