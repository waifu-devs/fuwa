//! Moderation: the services servers' AutoMod smart filters can ask, set up
//! once for the whole instance (a key and a switch each), and how many checks
//! each server gets a day, as the web's `instance/Moderation.tsx` draws them.

use crate::ui::instance_home::{focus_ring, has_focus};
use std::time::Duration;

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, linear_color_stop, linear_gradient, px,
};

use super::controls::{self, amber_text, input_box};
use super::{InstanceSettingsView, Tried};
use crate::core::i18n::{Arg, t, t_with};
use crate::core::instance_admin::{self as admin, MAX_CUSTOM};
use crate::pb;
use crate::ui::motion;
use crate::ui::server_settings::spinner;
use crate::ui::settings_controls::{Look, button, shadow_sm};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_3xl, radius_xl};
use crate::ui::widgets::icon;

/// A Tailwind color, light and dark (`text-x-600 dark:text-x-300`).
fn tw(light: u32, dark: u32, p: &Palette) -> Hsla {
    gpui_kit::rgb(if p.dark { dark } else { light }).into()
}

fn rgb(hex: u32) -> gpui_kit::Rgba {
    gpui_kit::rgb(hex)
}

/// `violet-500`, the moderation page's own color.
const VIOLET: u32 = 0x8e51ff;

/// A provider as its card shows it (the web's `Shown`).
struct Shown {
    name: String,
    host: String,
    blurb: String,
    key_help: String,
    /// The badge's gradient (from, to) and its letters' color.
    tint: (Hsla, Hsla, Hsla),
}

fn shown(provider: &pb::AutoModProviderSettings, host: Option<&str>, p: &Palette) -> Shown {
    if admin::is_custom(provider) {
        return Shown {
            name: match provider.name.trim() {
                "" => t("instancesettings.moderation.yourProvider"),
                name => name.to_owned(),
            },
            host: host.map_or_else(|| t("instancesettings.moderation.anAddress"), str::to_owned),
            blurb: t("instancesettings.moderation.customBlurb"),
            key_help: t("instancesettings.moderation.customKeyHelp"),
            tint: (alpha(rgb(VIOLET), 0.25), alpha(rgb(0xe12afb), 0.1), tw(0x7f22fe, 0xc4b4ff, p)),
        };
    }
    match admin::known(&provider.id) {
        Some(k) => Shown {
            name: k.name.to_owned(),
            host: k.host.to_owned(),
            blurb: t(k.blurb),
            key_help: t(k.key_help),
            tint: if provider.id == "cloudflare-clef" {
                (alpha(rgb(0xff6900), 0.25), alpha(rgb(0xfe9a00), 0.1), tw(0xf54900, 0xffb86a, p))
            } else {
                (alpha(rgb(0x00a6f4), 0.25), alpha(rgb(0x625fff), 0.1), tw(0x0084d1, 0x74d4ff, p))
            },
        },
        None => Shown {
            name: provider.id.clone(),
            host: String::new(),
            blurb: String::new(),
            key_help: String::new(),
            tint: (p.muted.into(), p.muted.into(), p.muted_foreground.into()),
        },
    }
}

/// A translated sentence with one placeholder in bold and the text color.
fn with_bold(key: &str, name: &str, value: &str, p: &Palette) -> gpui_kit::StyledText {
    let template = t(key);
    let marker = format!("{{{name}}}");
    let (before, after) = template.split_once(&marker).unwrap_or((template.as_str(), ""));
    let text = format!("{before}{value}{after}");
    let bold = gpui_kit::HighlightStyle {
        font_weight: Some(FontWeight::BOLD),
        color: Some(p.foreground.into()),
        ..Default::default()
    };
    gpui_kit::StyledText::new(text).with_highlights([(before.len()..before.len() + value.len(), bold)])
}

/// A column taking `share` parts of a row (`grid-cols-[minmax(0,2fr)_minmax(0,3fr)]`).
fn grow(share: f32) -> gpui_kit::Div {
    let mut el = div().flex_basis(px(0.0)).min_w_0();
    el.style().flex_grow = Some(share);
    el
}

/// A labelled field in a card (`flex flex-col gap-1.5`, the label `text-sm font-extrabold`).
fn labelled(label: String, control: impl IntoElement) -> gpui_kit::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(div().text_sm().line_height(px(20.0)).font_weight(FontWeight::EXTRA_BOLD).child(label))
        .child(control)
}

impl InstanceSettingsView {
    /// Sets a box's placeholder when it should say something else now.
    fn placeholder(&mut self, state: &Entity<InputState>, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let id = state.entity_id();
        if self.placeholders.get(&id) != Some(&text) {
            self.placeholders.insert(id, text.clone());
            state.update(cx, |s, cx| s.set_placeholder(text, window, cx));
        }
    }
}

impl InstanceSettingsView {
    pub(super) fn moderation_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(draft) = self.draft.clone() else { return div().into_any_element() };
        let violet = rgb(VIOLET);
        let violet_fg = tw(0x7f22fe, 0xc4b4ff, p);
        // `mb-4 flex items-start gap-3 rounded-2xl border border-violet-500/30 bg-violet-500/5 p-3 text-sm`.
        let intro = motion::rise(
            div()
                .mb(px(16.0))
                .flex()
                .items_start()
                .gap(px(12.0))
                .p(px(12.0))
                .rounded(radius_2xl())
                .border_1()
                .border_color(alpha(violet, 0.3))
                .bg(alpha(violet, 0.05))
                .text_sm()
                .line_height(px(20.0))
                .child(
                    div()
                        .flex_none()
                        .size(px(32.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(radius_xl())
                        .bg(alpha(violet, 0.15))
                        .text_color(violet_fg)
                        .child(motion::ambient(
                            icon("sparkles").size(px(16.0)),
                            "instance-moderation-sparkle",
                            Duration::from_millis(6400),
                            window,
                            |el, t| {
                                // A wiggle every few seconds, as on the web (1.4s, then 5s still).
                                let k = (t * 6400.0 / 1400.0).min(1.0);
                                let bump = if k < 1.0 { (k * std::f32::consts::PI).sin() } else { 0.0 };
                                el.rotate(gpui_kit::radians(0.2 * bump * (1.0 - 2.0 * k)))
                            },
                        )),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(p.muted_foreground)
                        .child(t("instancesettings.moderation.intro")),
                ),
            "instance-moderation-intro",
            Duration::ZERO,
            8.0,
        );

        let checks_default =
            self.config.as_ref().and_then(|c| c.defaults.as_ref()).and_then(|d| d.automod_checks_per_day).map_or_else(
                || t("instancesettings.shared.noLimit"),
                |n| t_with("instancesettings.shared.perDay", &[("count", Arg::Num(n))]),
            );
        let checks = self.setting_ruled(
            "automod-checks-per-day",
            &t("instancesettings.nav.automodChecks"),
            Some(div().child(t("instancesettings.moderation.checksHint")).into_any_element()),
            &["automod_checks_per_day"],
            &checks_default,
            1,
            false,
            true,
            self.cap_with(
                "automod_checks_per_day",
                &t("instancesettings.shared.upTo"),
                false,
                Some("1000"),
                p,
                window,
                cx,
            ),
            p,
            cx,
        );

        let mut page = div().flex().flex_col().child(intro).child(checks);
        for (n, provider) in draft.automod_providers.iter().enumerate() {
            let Some(slot) = self.fields.get(n).map(|f| f.slot) else { continue };
            let saved = self.saved().and_then(|s| s.automod_providers.iter().find(|x| x.id == provider.id)).cloned();
            let card = self.provider_card(n, slot, provider, saved.as_ref(), p, window, cx);
            let name = shown(provider, None, p).name;
            let id: &'static str = match provider.id.as_str() {
                "typesafe-jev" => "automod-typesafe-jev",
                "cloudflare-clef" => "automod-cloudflare-clef",
                _ => "automod-custom-provider",
            };
            // The first card carries the providers' Default or Reset (they're saved as one list).
            let paths: &'static [&'static str] = if n == 0 { &["automod_providers"] } else { &[] };
            page = page.child(self.setting_ruled(
                id,
                &name,
                None,
                paths,
                &t("instancesettings.shared.off"),
                2 + n,
                // Each card is the web's `first:pt-0 last:border-b-0` Setting in a wrapper of its
                // own: only the rule under the checks above the first, and no room under it.
                n == 0,
                false,
                card,
                p,
                cx,
            ));
        }

        let customs = draft.automod_providers.iter().filter(|x| admin::is_custom(x)).count();
        let full = customs >= MAX_CUSTOM;
        // `rounded-3xl border-2 border-dashed p-4`, lifting on hover with its plus turning.
        let add = div()
            .id("instance-add-provider")
            .flex()
            .items_center()
            .gap(px(12.0))
            .p(px(16.0))
            .rounded(radius_3xl())
            .border_2()
            .border_dashed()
            .border_color(p.border)
            .when(full, |el| el.opacity(0.5))
            .when(!full, |el| {
                let (hover_border, hover_bg) = (alpha(p.primary, 0.5), alpha(p.primary, 0.03));
                el.cursor_pointer()
                    .hover(move |s| s.border_color(hover_border).bg(hover_bg).top(px(-2.0)))
                    .active(|s| s.top(px(0.0)))
                    .on_click(cx.listener(|this, _, window, cx| this.add_custom(window, cx)))
            })
            .child(
                div()
                    .flex_none()
                    .size(px(40.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius_2xl())
                    .bg(alpha(violet, 0.15))
                    .text_color(violet_fg)
                    .child(icon("plus").size(px(20.0))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .line_height(px(24.0))
                            .child(t("instancesettings.moderation.addOwn")),
                    )
                    .child(div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(if full {
                        t_with("instancesettings.moderation.max", &[("count", Arg::Num(MAX_CUSTOM as i64))])
                    } else {
                        t("instancesettings.moderation.addOwnHint")
                    })),
            );
        page.child(div().relative().pt(px(8.0)).child(add)).into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn provider_card(
        &mut self,
        n: usize,
        slot: u64,
        provider: &pb::AutoModProviderSettings,
        saved: Option<&pb::AutoModProviderSettings>,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let custom = admin::is_custom(provider);
        let host = if custom { admin::host_of(&provider.url) } else { None };
        let known = shown(provider, host.as_deref(), p);
        let missing = admin::missing_for(provider, saved);
        let enabled = provider.enabled;
        let live = saved.is_some_and(|s| s.enabled);
        let moved = admin::moved(provider, saved);
        let clef = provider.id == "cloudflare-clef";
        let Some(fields) = self.fields.get(n) else { return div().into_any_element() };
        let (key, account, name_box, url_box, header_box, model_box) = (
            fields.key.clone(),
            fields.account.clone(),
            fields.name.clone(),
            fields.url.clone(),
            fields.header.clone(),
            fields.model.clone(),
        );

        // The head: the provider, where it is, whether it's live, and its switch.
        let (from, to, letters) = known.tint;
        let badge = motion::once(
            div()
                .flex_none()
                .size(px(40.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius_2xl())
                .bg(linear_gradient(135.0, linear_color_stop(from, 0.0), linear_color_stop(to, 1.0)))
                .text_color(letters)
                .text_sm()
                .font_weight(FontWeight::EXTRA_BOLD)
                .map(|el| {
                    if custom {
                        el.child(icon("webhook").size(px(20.0)))
                    } else {
                        el.child(known.name.split(' ').filter_map(|w| w.chars().next()).collect::<String>())
                    }
                }),
            SharedString::from(format!("instance-badge-{slot}-{enabled}")),
            Duration::from_millis(400),
            move |el, t| {
                // A hop when it's turned on (the web's scale and tilt).
                if enabled { el.relative().top(px(-4.0 * (t * std::f32::consts::PI).sin())) } else { el }
            },
        );
        let green = tw(0x009966, 0x00d492, p);
        let head = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .p(px(12.0))
            .pl(px(16.0))
            .child(badge)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_xs()
                                    .line_height(px(16.0))
                                    .text_color(p.muted_foreground)
                                    .map(|el| {
                                        if custom && host.is_none() {
                                            el.italic()
                                        } else {
                                            el.font_family("monospace").font_weight(FontWeight::BOLD)
                                        }
                                    })
                                    .child(known.host.clone()),
                            )
                            .when(live, |el| {
                                el.child(motion::once(
                                    div()
                                        .rounded_full()
                                        .bg(alpha(rgb(0x00bc7d), 0.15))
                                        .px(px(8.0))
                                        .py(px(2.0))
                                        .text_size(px(10.4))
                                        .line_height(px(14.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(green)
                                        .child(t("instancesettings.moderation.live").to_uppercase()),
                                    SharedString::from(format!("instance-live-{slot}")),
                                    Duration::from_millis(300),
                                    |el, t| el.opacity(t),
                                ))
                            }),
                    )
                    .child(
                        div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(known.blurb.clone()),
                    ),
            )
            .child(controls::switch(
                SharedString::from(format!("instance-provider-on-{slot}")),
                enabled,
                !enabled && missing.is_some(),
                p,
                window,
                cx,
                move |this, on, _, cx| {
                    let Some(n) = this.fields.iter().position(|f| f.slot == slot) else { return };
                    this.patch(cx, |d| {
                        if let Some(p) = d.automod_providers.get_mut(n) {
                            p.enabled = on
                        }
                    })
                },
            ));

        // Where messages go.
        let mut body = div().flex().flex_col().gap(px(12.0)).p(px(16.0)).border_t_1().border_color(p.border).child(
            div()
                .flex()
                .items_start()
                .gap(px(10.0))
                .p(px(12.0))
                .rounded(radius_2xl())
                .bg(alpha(p.muted, 0.6))
                .text_xs()
                .line_height(px(16.0))
                .text_color(p.muted_foreground)
                .child(icon("globe-lock").size(px(16.0)).mt(px(2.0)).text_color(p.primary))
                .child(div().flex_1().min_w_0().child(with_bold(
                    "instancesettings.moderation.goesTo",
                    "host",
                    &known.host,
                    p,
                ))),
        );
        if custom {
            let bad_url = !provider.url.trim().is_empty() && host.is_none();
            self.placeholder(&name_box, t("instancesettings.moderation.namePlaceholder"), window, cx);
            self.placeholder(&model_box, t("instancesettings.moderation.modelOptional"), window, cx);
            // `grid sm:grid-cols-[2fr_3fr]`, then two even columns.
            body = body
                .child(
                    div()
                        .flex()
                        .gap(px(12.0))
                        .child(grow(2.0).child(labelled(
                            t("instancesettings.nav.name"),
                            focus_ring(
                                input_box(Input::new(&name_box).appearance(false), None, p),
                                has_focus(&name_box, window, cx),
                                p,
                            ),
                        )))
                        .child(
                            grow(3.0).child(labelled(
                                t("instancesettings.moderation.address"),
                                focus_ring(
                                    input_box(Input::new(&url_box).appearance(false), None, p),
                                    has_focus(&url_box, window, cx),
                                    p,
                                )
                                .font_family("monospace"),
                            )),
                        ),
                )
                .when(bad_url, |el| {
                    el.child(motion::rise(
                        div()
                            .mt(px(-4.0))
                            .text_xs()
                            .text_color(amber_text(p))
                            .child(t("instancesettings.moderation.httpsOnly")),
                        SharedString::from(format!("instance-bad-url-{slot}")),
                        Duration::ZERO,
                        -4.0,
                    ))
                })
                .child(
                    div()
                        .flex()
                        .gap(px(12.0))
                        .child(
                            div().flex_1().min_w_0().child(labelled(
                                t("instancesettings.moderation.keyHeader"),
                                focus_ring(
                                    input_box(Input::new(&header_box).appearance(false), None, p),
                                    has_focus(&header_box, window, cx),
                                    p,
                                )
                                .font_family("monospace"),
                            )),
                        )
                        .child(
                            div().flex_1().min_w_0().child(labelled(
                                t("instancesettings.moderation.model"),
                                focus_ring(
                                    input_box(Input::new(&model_box).appearance(false), None, p),
                                    has_focus(&model_box, window, cx),
                                    p,
                                )
                                .font_family("monospace"),
                            )),
                        ),
                );
        }

        // The key (a token for Cloudflare), never shown again once saved.
        let key_placeholder = if moved && provider.api_key_set {
            t("instancesettings.moderation.retypeKey")
        } else if provider.api_key_set {
            let hint = if provider.api_key_hint.is_empty() { "••••" } else { provider.api_key_hint.as_str() };
            t_with("instancesettings.moderation.savedEnds", &[("hint", Arg::Str(hint))])
        } else if custom {
            t("instancesettings.moderation.noneOrPaste")
        } else {
            t("instancesettings.moderation.paste")
        };
        self.placeholder(&key, key_placeholder, window, cx);
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(div().text_sm().line_height(px(20.0)).font_weight(FontWeight::EXTRA_BOLD).child(t(if clef {
                    "instancesettings.moderation.apiToken"
                } else {
                    "instancesettings.moderation.apiKey"
                })))
                .child(
                    div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(known.key_help.clone()),
                )
                .child(focus_ring(
                    input_box(Input::new(&key).appearance(false), Some("key-round"), p),
                    has_focus(&key, window, cx),
                    p,
                ))
                .child(
                    div()
                        .text_size(px(11.2))
                        .line_height(px(16.0))
                        .text_color(p.muted_foreground)
                        .child(t("instancesettings.moderation.kept")),
                ),
        );
        if clef {
            self.placeholder(&account, t("instancesettings.moderation.accountIdPlaceholder"), window, cx);
            body = body.child(labelled(
                t("instancesettings.moderation.accountId"),
                focus_ring(
                    input_box(Input::new(&account).appearance(false), None, p),
                    has_focus(&account, window, cx),
                    p,
                )
                .font_family("monospace"),
            ));
        }

        // Which of a known provider's models to ask; the first is the default.
        if let Some(k) = admin::known(&provider.id).filter(|k| !custom && k.models.len() > 1) {
            let picked = if provider.model.is_empty() { k.models[0].0 } else { provider.model.as_str() };
            let at = k.models.iter().position(|(id, _, _)| *id == picked).unwrap_or(0);
            let count = k.models.len() as f32;
            let x = motion::follow(SharedString::from(format!("instance-model-x-{slot}")), at as f32, window, cx);
            let mut models = div()
                .relative()
                .flex()
                .gap(px(4.0))
                .p(px(4.0))
                .rounded(radius_2xl())
                .bg(alpha(p.muted, 0.6))
                // The picked one's background glides between them.
                .child(
                    div()
                        .absolute()
                        .top(px(4.0))
                        .bottom(px(4.0))
                        .left(gpui_kit::relative(x / count))
                        .w(gpui_kit::relative(1.0 / count))
                        .px(px(4.0))
                        .child(div().size_full().rounded(radius_xl()).bg(p.background).shadow(shadow_sm())),
                );
            for (i, (id, label, hint)) in k.models.iter().enumerate() {
                let on = *id == picked;
                let value = if i == 0 { String::new() } else { (*id).to_owned() };
                let (fg, hover) = (if on { p.foreground } else { p.muted_foreground }, p.foreground);
                models = models.child(
                    div()
                        .id(SharedString::from(format!("instance-model-{slot}-{i}")))
                        .relative()
                        .flex_1()
                        .rounded(radius_xl())
                        .px(px(12.0))
                        .py(px(6.0))
                        .text_color(fg)
                        .cursor_pointer()
                        .hover(move |s| s.text_color(hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let Some(n) = this.fields.iter().position(|f| f.slot == slot) else { return };
                            let value = value.clone();
                            this.patch(cx, |d| {
                                if let Some(p) = d.automod_providers.get_mut(n) {
                                    p.model = value
                                }
                            });
                            this.tried.remove(&slot);
                        }))
                        .child(div().text_sm().line_height(px(20.0)).font_weight(FontWeight::BOLD).child(*label))
                        .child(div().text_size(px(11.2)).line_height(px(16.0)).child(t(hint))),
                );
            }
            body = body.child(labelled(t("instancesettings.moderation.model"), models));
        }
        body = body.child(self.tester(slot, missing, p, window, cx));
        if custom {
            let name = provider.name.trim();
            let label = if name.is_empty() {
                t("instancesettings.moderation.removeThis")
            } else {
                t_with("instancesettings.moderation.remove", &[("name", Arg::Str(name))])
            };
            let red = alpha(p.destructive, 0.1);
            body = body.child(
                div().flex().child(
                    button(
                        SharedString::from(format!("instance-remove-{slot}")),
                        label,
                        Some("trash-2"),
                        Look::Ghost,
                        true,
                        p,
                    )
                    .rounded(radius_xl())
                    .text_color(p.destructive)
                    .hover(move |s| s.bg(red))
                    .on_click(cx.listener(move |this, _, _, cx| this.remove(slot, cx))),
                ),
            );
        }
        div()
            .rounded(radius_3xl())
            .border_1()
            .map(|el| {
                if enabled {
                    el.border_color(alpha(p.primary, 0.4)).bg(alpha(p.primary, 0.03))
                } else if custom {
                    el.border_dashed().border_color(p.border)
                } else {
                    el.border_color(p.border)
                }
            })
            .child(head)
            .child(body)
            .into_any_element()
    }

    /// "Test connection": a sample scam sent through the provider, and what it said; or what it
    /// still needs first.
    fn tester(
        &self,
        slot: u64,
        missing: Option<&'static str>,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let asking = matches!(self.tried.get(&slot), Some(Tried::Asking));
        let ready = missing.is_none();
        let label =
            t(if asking { "instancesettings.moderation.asking" } else { "instancesettings.moderation.trySample" });
        let glyph = if asking { None } else { Some("sparkles") };
        let mut try_it =
            button(SharedString::from(format!("instance-try-{slot}")), label, glyph, Look::Outline, true, p)
                .rounded(radius_xl())
                .font_weight(FontWeight::BOLD)
                .when(asking, |el| el.child(spinner(format!("instance-try-spin-{slot}"), 16.0, window)));
        if ready && !asking {
            try_it = try_it.on_click(cx.listener(move |this, _, window, cx| this.try_provider(slot, window, cx)));
        } else {
            try_it = try_it.opacity(0.5);
        }
        let mut out =
            div().flex().flex_col().gap(px(8.0)).p(px(12.0)).rounded(radius_2xl()).bg(alpha(p.muted, 0.5)).child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_sm()
                            .line_height(px(20.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .whitespace_nowrap()
                            .child(icon("flask-conical").size(px(16.0)).text_color(p.primary))
                            .child(t("instancesettings.moderation.test")),
                    )
                    .child(try_it),
            );
        match self.tried.get(&slot) {
            Some(Tried::Answered(answer)) if answer.ok => {
                let green = tw(0x009966, 0x00d492, p);
                let ms = answer.elapsed_ms.to_string();
                let mut answered = div().flex().flex_col().gap(px(8.0)).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(green)
                        .child(icon("check").size(px(14.0)))
                        .child(t_with("instancesettings.moderation.answered", &[("ms", Arg::Str(&ms))])),
                );
                for (n, score) in answer.scores.iter().take(4).enumerate() {
                    let probability = score.probability.clamp(0.0, 1.0);
                    let fill: Hsla = if probability >= 0.8 { p.destructive.into() } else { alpha(p.primary, 0.6) };
                    answered = answered.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .text_xs()
                            .child(
                                div()
                                    .w(px(96.0))
                                    .flex_none()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_weight(FontWeight::BOLD)
                                    .child(admin::label_name(&score.label)),
                            )
                            .child(div().flex_1().h(px(8.0)).rounded_full().bg(p.background).overflow_hidden().child(
                                motion::once(
                                    div().h_full().rounded_full().bg(fill),
                                    SharedString::from(format!("instance-score-{slot}-{n}-{}", score.label)),
                                    Duration::from_millis(420 + 60 * n as u64),
                                    move |el, t| {
                                        let eased = 1.0 - (1.0 - t).powi(3);
                                        el.w(gpui_kit::relative(probability * eased))
                                    },
                                ),
                            ))
                            .child(
                                div()
                                    .w(px(36.0))
                                    .flex_none()
                                    .text_right()
                                    .child(format!("{}%", (probability * 100.0).round() as i32)),
                            ),
                    );
                }
                out = out.child(motion::rise(
                    answered,
                    SharedString::from(format!("instance-tried-ok-{slot}")),
                    Duration::ZERO,
                    6.0,
                ));
            }
            Some(Tried::Answered(answer)) => out = out.child(failed(&answer.error, slot, p)),
            Some(Tried::Failed(error)) => out = out.child(failed(error, slot, p)),
            _ => {}
        }
        if let Some(missing) = missing {
            out = out.child(div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(t(missing)));
        }
        out.into_any_element()
    }
}

/// Why a provider didn't answer, as the instance put it.
fn failed(error: &str, slot: u64, p: &Palette) -> AnyElement {
    let mut text = error.to_owned();
    if let Some(first) = text.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    motion::rise(
        div()
            .flex()
            .items_start()
            .gap(px(6.0))
            .text_xs()
            .text_color(amber_text(p))
            .child(icon("triangle-alert").size(px(14.0)).mt(px(1.0)))
            .child(div().flex_1().min_w_0().child(text)),
        SharedString::from(format!("instance-tried-failed-{slot}")),
        Duration::ZERO,
        6.0,
    )
    .into_any_element()
}
