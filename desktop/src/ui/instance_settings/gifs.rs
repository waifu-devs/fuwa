//! GIFs: which provider the instance asks and with what key, how strict,
//! and the caps (the web's `settings/instance/Gifs.tsx`). The provider never
//! sees who searches: the instance asks for everyone and hands every
//! picture back itself.

use crate::ui::instance_home::{focus_ring, has_focus};
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, IntoElement as _, ObjectFit, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Subscription, Window, div, px,
};

use super::InstanceSettingsView;
use super::controls::{Opt, amber_soft, input_box};
use crate::core::i18n::{Arg, t, t_with};
use crate::core::instance_admin::format_bytes;
use crate::pb;
use crate::ui::motion;
use crate::ui::settings_controls::{Look, button};
use crate::ui::theme::{Palette, alpha, radius_lg, radius_xl};
use crate::ui::widgets::icon;

/// "Try it": asking the provider for a few trending GIFs.
pub(super) enum Tried {
    Trying,
    Done(pb::TestGifProviderResponse),
}

pub(super) struct Gifs {
    key: Entity<InputState>,
    placeholder: String,
    tried: Option<Tried>,
}

impl Gifs {
    pub(super) fn new(window: &mut Window, cx: &mut Context<InstanceSettingsView>) -> (Self, Vec<Subscription>) {
        let key = cx.new(|cx| InputState::new(window, cx).masked(true));
        let sub = cx.subscribe(&key, |this: &mut InstanceSettingsView, state, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                let value = state.read(cx).value().to_string();
                this.edit_gifs(cx, move |g| g.api_key = value);
            }
        });
        (Self { key, placeholder: String::new(), tried: None }, vec![sub])
    }
}

/// The ratings a provider is held to: (value, label, hint key).
const RATINGS: [(&str, &str, &str); 4] = [
    ("g", "G", "instancesettings.gifs.ratingG"),
    ("pg", "PG", "instancesettings.gifs.ratingPg"),
    ("pg-13", "PG-13", "instancesettings.gifs.ratingPg13"),
    ("r", "R", "instancesettings.gifs.ratingR"),
];

fn provider_name(provider: i32) -> &'static str {
    if provider == pb::GifProvider::Giphy as i32 {
        "GIPHY"
    } else if provider == pb::GifProvider::Klipy as i32 {
        "Klipy"
    } else {
        ""
    }
}

fn gifs_of(s: &pb::InstanceSettings) -> pb::GifSettings {
    s.gifs.clone().unwrap_or_default()
}

impl InstanceSettingsView {
    fn edit_gifs(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut pb::GifSettings)) {
        let Some(draft) = self.draft.as_ref() else { return };
        let mut next = gifs_of(draft);
        let before = next.clone();
        f(&mut next);
        if next != before {
            self.patch(cx, |d| d.gifs = Some(next));
        }
    }

    /// Puts the draft's key into its box (after loading, a save or a discard).
    pub(super) fn sync_gifs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.as_ref() else { return };
        let key = gifs_of(draft).api_key;
        if self.gifs.key.read(cx).value().as_ref() != key {
            self.gifs.key.update(cx, |s, cx| s.set_value(key, window, cx));
        }
        self.gif_placeholder(window, cx);
    }

    /// The key box says whether a key is kept for the provider picked.
    fn gif_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(draft), Some(saved)) = (self.draft.as_ref(), self.saved()) else { return };
        let (g, was) = (gifs_of(draft), gifs_of(saved));
        let kept = was.api_key_set && was.provider == g.provider;
        let placeholder = match (kept, was.api_key_hint.as_str()) {
            (false, _) => t_with("instancesettings.gifs.paste", &[("name", Arg::Str(provider_name(g.provider)))]),
            (true, "") => t("instancesettings.shared.saved"),
            (true, hint) => t_with("instancesettings.shared.savedEnding", &[("hint", Arg::Str(hint))]),
        };
        if self.gifs.placeholder != placeholder {
            self.gifs.key.update(cx, |s, cx| s.set_placeholder(placeholder.clone(), window, cx));
            self.gifs.placeholder = placeholder;
        }
    }

    fn try_gifs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(settings) = self.draft.as_ref().map(gifs_of) else { return };
        self.gifs.tried = Some(Tried::Trying);
        let (core, key) = (self.core.clone(), self.key.clone());
        let rx = self.core.spawn(async move { core.test_gif_provider(&key, settings).await });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(result) = rx.await {
                let _ = this.update(cx, |this, cx| {
                    let answer = result.unwrap_or_else(|problem| pb::TestGifProviderResponse {
                        ok: false,
                        error: problem.message,
                        ..Default::default()
                    });
                    this.gifs.tried = Some(Tried::Done(answer));
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }

    pub(super) fn gifs_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (Some(draft), Some(config)) = (self.draft.clone(), self.config.clone()) else {
            return div().into_any_element();
        };
        self.gif_placeholder(window, cx);
        let saved = self.saved().cloned().unwrap_or_default();
        let def = gifs_of(&config.defaults.clone().unwrap_or_default());
        let (g, was) = (gifs_of(&draft), gifs_of(&saved));
        let off_value = pb::GifProvider::Unspecified as i32;
        let default_provider = match provider_name(def.provider) {
            "" => t("instancesettings.shared.off"),
            name => name.to_owned(),
        };
        let choice = self.choice(
            "gif-provider",
            g.provider,
            vec![
                Opt::new(off_value, t("serversettings.shared.off"), t("instancesettings.gifs.offHint"), "power-off"),
                Opt::new(pb::GifProvider::Giphy as i32, "GIPHY", t("instancesettings.gifs.giphyHint"), "sparkles"),
                Opt::new(pb::GifProvider::Klipy as i32, "Klipy", t("instancesettings.gifs.klipyHint"), "film"),
            ],
            p,
            window,
            cx,
            |this, v, _, cx| this.edit_gifs(cx, |g| g.provider = v),
        );
        let mut page = div().flex().flex_col().child(self.setting(
            "gif-provider",
            &t("instancesettings.nav.gifProvider"),
            Some(&t("instancesettings.gifs.providerHint")),
            &["gifs"],
            &default_provider,
            0,
            choice,
            p,
            cx,
        ));
        if g.provider == off_value {
            return page.into_any_element();
        }

        // The key: saved ones are never shown, only how they end.
        let name = provider_name(g.provider);
        let kept = was.api_key_set && was.provider == g.provider;
        let missing = g.api_key.trim().is_empty() && !kept;
        let field = focus_ring(
            input_box(Input::new(&self.gifs.key).appearance(false), Some("key-round"), p),
            has_focus(&self.gifs.key, window, cx),
            p,
        )
        .font_family("monospace")
        .when(missing, |el| el.border_2().border_color(gpui_kit::hsla(38.0 / 360.0, 0.92, 0.5, 0.5)));
        let trying = matches!(self.gifs.tried, Some(Tried::Trying));
        let result = match &self.gifs.tried {
            Some(Tried::Done(answer)) => Some(answer.clone()),
            _ => None,
        };
        let try_button = button(
            "igif-try",
            t("instancesettings.gifs.tryIt"),
            Some(if trying { "loader-circle" } else { "sparkles" }),
            Look::Outline,
            true,
            p,
        )
        .h(px(36.0))
        .rounded(radius_xl())
        .px(px(16.0))
        .font_weight(FontWeight::BOLD)
        .when(trying, |el| el.opacity(0.5))
        .when(!trying, |el| el.on_click(cx.listener(|this, _, window, cx| this.try_gifs(window, cx))));
        let mut tried = div().flex().flex_col().gap(px(8.0)).child(
            div().flex().items_center().gap(px(12.0)).child(try_button).when_some(result.clone(), |el, r| {
                let green: gpui_kit::Hsla =
                    if p.dark { gpui_kit::rgb(0x34d399).into() } else { gpui_kit::rgb(0x059669).into() };
                el.child(motion::slide_in(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(if r.ok { green } else { p.destructive.into() })
                        .child(icon(if r.ok { "circle-check" } else { "circle-x" }).size(px(16.0)))
                        .child(if r.ok {
                            t_with("instancesettings.gifs.works", &[("ms", Arg::Str(&r.elapsed_ms.to_string()))])
                        } else {
                            r.error.clone()
                        }),
                    SharedString::from(format!("igif-tried-{}", r.ok)),
                    -6.0,
                ))
            }),
        );
        if let Some(r) = result.filter(|r| r.ok && !r.results.is_empty()) {
            let gap = 6.0;
            let size = (self.column - gap * 5.0) / 6.0;
            let mut grid = div().flex().flex_wrap().gap(px(gap));
            for (n, gif) in r.results.iter().enumerate() {
                grid = grid.child(motion::once(
                    div().size(px(size)).rounded(radius_lg()).overflow_hidden().bg(p.muted).child(
                        crate::ui::widgets::picture(gif.preview_url.clone())
                            .size_full()
                            .rounded(radius_lg())
                            .object_fit(ObjectFit::Cover),
                    ),
                    SharedString::from(format!("igif-result-{}", gif.id)),
                    Duration::from_millis(300 + 40 * n as u64),
                    move |el, t| {
                        let start = (40.0 * n as f32) / (300.0 + 40.0 * n as f32);
                        let k = ((t - start) / (1.0 - start)).clamp(0.0, 1.0);
                        el.opacity(k)
                    },
                ));
            }
            tried = tried.child(grid);
        }
        let key_body = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(field)
            .when(missing, |el| {
                el.child(
                    div()
                        .text_xs()
                        .line_height(px(16.0))
                        .text_color(amber_soft(p))
                        .child(t("instancesettings.gifs.keyMissing")),
                )
            })
            .child(tried);
        // As on the web, the settings that come with a provider sit in a block of their own: its
        // first has no space above the rule.
        page = page.child(div().border_t_1().border_color(alpha(p.border, 0.7)).child(self.setting(
            "gif-key",
            &t_with("instancesettings.gifs.keyTitle", &[("name", Arg::Str(name))]),
            Some(&t_with("instancesettings.gifs.keyHint", &[("name", Arg::Str(name))])),
            &["gifs"],
            &t(if def.api_key_set { "instancesettings.shared.set" } else { "instancesettings.shared.none" }),
            0,
            key_body,
            p,
            cx,
        )));
        let rating = if g.rating.is_empty() { "pg-13".to_owned() } else { g.rating.clone() };
        let picked = RATINGS.iter().position(|(v, _, _)| *v == rating).unwrap_or(2) as i32;
        let ratings = self.choice(
            "gif-rating",
            picked,
            RATINGS
                .iter()
                .enumerate()
                .map(|(i, (_, label, hint))| Opt::new(i as i32, *label, t(hint), "shield"))
                .collect(),
            p,
            window,
            cx,
            |this, v, _, cx| {
                let value = RATINGS[v as usize].0.to_owned();
                this.edit_gifs(cx, move |g| g.rating = value)
            },
        );
        let def_rating = if def.rating.is_empty() { "pg-13" } else { def.rating.as_str() };
        let def_rating = RATINGS.iter().find(|(v, _, _)| *v == def_rating).map_or("PG-13", |r| r.1);
        page = page.child(self.setting(
            "gif-rating",
            &t("instancesettings.nav.gifRating"),
            Some(&t("instancesettings.gifs.ratingHint")),
            &["gifs"],
            def_rating,
            1,
            ratings,
            p,
            cx,
        ));
        let third = (self.column - 24.0) / 3.0;
        let caps = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .gap(px(12.0))
                    .child(div().w(px(third)).child(self.cap(
                        "gifs.gif_bytes",
                        &t("instancesettings.gifs.largest"),
                        true,
                        p,
                        window,
                        cx,
                    )))
                    .child(div().w(px(third)).child(self.cap(
                        "gifs.searches_per_minute",
                        &t("instancesettings.gifs.searches"),
                        false,
                        p,
                        window,
                        cx,
                    )))
                    .child(div().w(px(third)).child(self.cap(
                        "gifs.provider_calls_per_day",
                        &t("instancesettings.gifs.calls"),
                        false,
                        p,
                        window,
                        cx,
                    ))),
            )
            .when_some(g.gif_bytes, |el, bytes| {
                el.child(
                    div()
                        .text_xs()
                        .line_height(px(16.0))
                        .text_color(p.muted_foreground)
                        .child(t_with("instancesettings.gifs.bigger", &[("size", Arg::Str(&format_bytes(bytes)))])),
                )
            });
        page = page.child(self.setting(
            "gif-caps",
            &t("instancesettings.gifs.caps"),
            Some(&t("instancesettings.gifs.capsHint")),
            &["gifs"],
            &t("instancesettings.gifs.noCaps"),
            2,
            caps,
            p,
            cx,
        ));
        page.into_any_element()
    }
}
