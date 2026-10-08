//! General: the instance's name and address, the web app, which sites may
//! connect, and how it was started.

use crate::ui::instance_home::{focus_ring, has_focus};
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{AnyElement, Context, FontWeight, IntoElement, ParentElement as _, Styled as _, Window, div, px};

use super::controls::{Opt, area_box, input_box};
use super::{HIDDEN_ADDRESS, InstanceSettingsView};
use crate::core::i18n::t;
use crate::ui::motion;
use crate::ui::theme::{Palette, corner};
use crate::ui::widgets::icon;

/// "Any site" and "Only these", as the origins choice's values.
const ANY: i32 = 0;
const LIST: i32 = 1;

impl InstanceSettingsView {
    pub(super) fn general_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (Some(draft), Some(config)) = (self.draft.clone(), self.config.clone()) else {
            return div().into_any_element();
        };
        let defaults = config.defaults.clone().unwrap_or_default();
        let startup = config.startup.clone().unwrap_or_default();
        let hide = self.core.prefs().streamer_mode;
        let mut page = div().flex().flex_col();
        let node = self.core.shared.read(|s| s.instance(&self.key).and_then(|i| i.node.clone()));
        if let Some(card) = super::release::newer_release(node.as_ref(), p) {
            page = page.child(card);
        }

        if let Some(name) = self.texts.get("name") {
            page = page.child(self.setting(
                "name",
                &t("instancesettings.nav.name"),
                Some(&t("instancesettings.general.nameHint")),
                &["name"],
                &defaults.name,
                0,
                focus_ring(input_box(Input::new(name).appearance(false), None, p), has_focus(name, window, cx), p),
                p,
                cx,
            ));
        }
        if let Some(url) = self.texts.get("public_url") {
            let default = if hide { HIDDEN_ADDRESS.to_owned() } else { defaults.public_url.clone() };
            page = page.child(self.setting(
                "public-url",
                &t("instancesettings.nav.publicUrl"),
                Some(&t("instancesettings.general.publicUrlHint")),
                &["public_url"],
                &default,
                1,
                focus_ring(
                    input_box(Input::new(url).appearance(false), Some("link"), p),
                    has_focus(url, window, cx),
                    p,
                ),
                p,
                cx,
            ));
        }
        let built_in = startup.web_built_in;
        page = page.child(self.setting(
            "web",
            &t("instancesettings.nav.web"),
            None,
            &["web"],
            &t(if defaults.web { "instancesettings.shared.on" } else { "instancesettings.shared.off" }),
            2,
            self.toggle(
                "web",
                draft.web,
                !built_in,
                &t("instancesettings.general.webLabel"),
                &t(if built_in { "instancesettings.general.webHint" } else { "instancesettings.general.webNotBuilt" }),
                p,
                window,
                cx,
                |d, on| d.web = on,
            ),
            p,
            cx,
        ));

        let any = draft.allowed_origins.iter().any(|o| o == "*");
        let default = match defaults.allowed_origins.join(", ") {
            _ if hide => HIDDEN_ADDRESS.to_owned(),
            all if all == "*" => t("instancesettings.origins.anySiteDefault"),
            all => all,
        };
        let choice = self.choice(
            "origins",
            if any { ANY } else { LIST },
            vec![
                Opt::new(ANY, t("instancesettings.origins.any"), t("instancesettings.origins.anyHint"), "globe"),
                Opt::new(LIST, t("instancesettings.origins.list"), t("instancesettings.origins.listHint"), "lock"),
            ],
            p,
            window,
            cx,
            |this, v, _, cx| this.pick_origins(v == ANY, cx),
        );
        let list = self.areas.get("allowed_origins").cloned();
        let origins = div().flex().flex_col().gap(px(12.0)).child(choice).when(!any, |el| {
            el.child(motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .map(|el| match (&list, hide) {
                        (_, true) => el.child(hidden(p)),
                        (Some(list), false) => el.child(
                            focus_ring(
                                area_box(Textarea::new(list).appearance(false), None, p),
                                has_focus(list, window, cx),
                                p,
                            )
                            .font_family("monospace")
                            .text_xs(),
                        ),
                        (None, false) => el,
                    })
                    .child(div().text_xs().text_color(p.muted_foreground).child(
                        "One per line, like https://chat.example.com. The instance's own address and this desktop \
                         app always work.",
                    )),
                "iorigins-list",
                std::time::Duration::ZERO,
                -6.0,
            ))
        });
        page = page.child(self.setting(
            "origins",
            &t("instancesettings.nav.origins"),
            Some(&t("instancesettings.origins.hint")),
            &["allowed_origins"],
            &default,
            3,
            origins,
            p,
            cx,
        ));

        let facts = [
            ("version", t("instancesettings.startup.version"), startup.version.clone(), false),
            ("port", t("instancesettings.startup.port"), startup.port.to_string(), false),
            (
                "encryption",
                t("instancesettings.startup.encryption"),
                t(if startup.encryption { "instancesettings.shared.on" } else { "instancesettings.shared.off" }),
                startup.encryption,
            ),
            (
                "admin-token",
                t("instancesettings.startup.adminToken"),
                t(if startup.admin_token { "instancesettings.shared.set" } else { "instancesettings.startup.notSet" }),
                false,
            ),
            (
                "hosting",
                t("instancesettings.startup.hosting"),
                if startup.hosted { "Waifu Devs".to_owned() } else { t("instancesettings.startup.selfHosted") },
                false,
            ),
        ];
        let mut pills = div().flex().flex_wrap().gap(px(8.0)).mt(px(12.0));
        for (n, (id, label, value, good)) in facts.into_iter().enumerate() {
            pills = pills.child(Self::fact(format!("ifact-{id}"), &label, &value, good, n, p));
        }
        page = page.child(motion::rise(
            div()
                .mt(px(10.0))
                .p(px(16.0))
                .rounded(corner(16.0))
                .border_1()
                .border_dashed()
                .border_color(p.border)
                .child(div().text_sm().font_weight(FontWeight::EXTRA_BOLD).child(t("instancesettings.startup.title")))
                .child(
                    div()
                        .mt(px(2.0))
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .child(t("instancesettings.startup.hint")),
                )
                .child(pills),
            "istartup",
            std::time::Duration::from_millis(160),
            12.0,
        ));
        page.into_any_element()
    }

    /// Any site, or the list typed (what was there before "Any site" comes back).
    fn pick_origins(&mut self, any: bool, cx: &mut Context<Self>) {
        let typed: Vec<String> = self
            .areas
            .get("allowed_origins")
            .map(|a| a.read(cx).value().to_string())
            .unwrap_or_default()
            .lines()
            .map(|o| o.trim().to_owned())
            .filter(|o| !o.is_empty())
            .collect();
        self.patch(cx, |d| d.allowed_origins = if any { vec!["*".to_owned()] } else { typed });
    }
}

/// What stands in for an address while streamer mode is on.
pub(super) fn hidden(p: &Palette) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(12.0))
        .h(px(40.0))
        .rounded(corner(10.0))
        .border_1()
        .border_color(p.border)
        .text_sm()
        .text_color(p.muted_foreground)
        .child(icon("eye-off").size(px(15.0)))
        .child("Hidden while streamer mode is on")
}
