//! Before a masked link opens (its words aren't where it goes,
//! `core::links`): the real address, its site picked out, and a way to trust
//! that site so its links open straight away from then on, on this computer.
//! The web's `components/LeavingDialog.tsx`.

use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::core::links;
use crate::ui::app::{Dialog, FuwaApp};
use crate::ui::motion;
use crate::ui::overlay::{dialog_button, dialog_card, dialog_close, dialog_header, dialog_layer};
use crate::ui::settings_controls::Look;
use crate::ui::theme::{alpha, radius_2xl, radius_xl};
use crate::ui::widgets::{icon, pal};

impl FuwaApp {
    /// A click on a masked link: asks first, unless its site is trusted or
    /// it's one of the instances you added.
    pub fn ask_to_leave(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        let host = links::host_of(&url);
        let ours = self.core.instance_urls().iter().any(|i| links::host_of(i) == host);
        if ours || links::trusted(&self.prefs.trusted_domains, &host) {
            crate::ui::text::open_link(&url, cx);
            return;
        }
        self.open_dialog(Dialog::Leaving { url, trust: false }, window, cx);
    }

    /// "Visit site": trusts the site when asked to, then opens the link.
    fn leave(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::Leaving { url, trust }) = self.dialog.clone() else { return };
        let host = links::host_of(&url);
        if trust && !host.is_empty() {
            self.core.set_prefs(|p| {
                p.trusted_domains.push(host);
                p.trusted_domains = links::tidy_domains(std::mem::take(&mut p.trusted_domains));
            });
            self.prefs = self.core.prefs();
        }
        crate::ui::text::open_link(&url, cx);
        self.close_dialog(cx);
    }

    pub(crate) fn render_leaving_link(
        &mut self,
        url: &str,
        trust: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let host = links::host_of(url);
        // The address, its site in bold and the rest muted.
        let address = match url.find(&host).filter(|_| !host.is_empty()) {
            Some(at) => div()
                .child(div().text_color(p.muted_foreground).child(url[..at].to_owned()))
                .child(div().font_weight(FontWeight::BOLD).text_color(p.foreground).child(host.clone()))
                .child(div().text_color(p.muted_foreground).child(url[at + host.len()..].to_owned()))
                .flex()
                .flex_wrap(),
            None => div().child(url.to_owned()),
        };
        let shown = motion::rise(
            div()
                .flex()
                .items_start()
                .gap(px(12.0))
                .p(px(12.0))
                .rounded(radius_2xl())
                .border_1()
                .border_color(p.border)
                .bg(alpha(p.muted, 0.4))
                .child(div().mt(px(2.0)).flex_none().text_color(p.primary).child(icon("shield-alert").size(px(20.0))))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(crate::ui::theme::mono())
                        .text_sm()
                        .line_height(px(20.0))
                        .child(address),
                ),
            "leaving-address",
            std::time::Duration::from_millis(50),
            6.0,
        );
        let trust_row = (!host.is_empty()).then(|| {
            let set = |this: &mut FuwaApp, on: bool, cx: &mut Context<FuwaApp>| {
                if let Some(Dialog::Leaving { trust, .. }) = this.dialog.as_mut() {
                    *trust = on;
                }
                cx.notify();
            };
            div()
                .id("leaving-trust")
                .mt(px(12.0))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(16.0))
                .p(px(12.0))
                .rounded(radius_2xl())
                .border_1()
                .border_color(p.border)
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| set(this, !trust, cx)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child(t_with("chat.leaving.trust", &[("site", Arg::Str(&host))])),
                        )
                        .child(div().text_xs().text_color(p.muted_foreground).child(t("chat.leaving.trustHint"))),
                )
                .child(crate::ui::settings_controls::switch("leaving-trust-switch", trust, false, &p, window, cx, set))
        });
        let panel = dialog_card(false, &p)
            .child(dialog_header(
                SharedString::from(t("chat.leaving.title")),
                Some(div().child(t("chat.leaving.about")).into_any_element()),
                &p,
            ))
            .child(shown)
            .children(trust_row)
            .child(
                div()
                    .mt(px(20.0))
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        dialog_button("leaving-back", t("chat.leaving.back"), Look::Ghost, &p)
                            .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
                    )
                    .child(
                        crate::ui::settings_controls::button(
                            "leaving-visit",
                            t("chat.leaving.visit"),
                            Some("external-link"),
                            Look::Primary,
                            false,
                            &p,
                        )
                        .rounded(radius_xl())
                        .font_weight(FontWeight::BOLD)
                        .on_click(cx.listener(|this, _, _, cx| this.leave(cx))),
                    ),
            )
            .child(dialog_close("leaving-close", &p).on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))));
        dialog_layer("leaving", panel, &p, cx.listener(|this, _, _, cx| this.close_dialog(cx)), cx)
    }
}
