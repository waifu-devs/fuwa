//! What floats over the window: dialogs and toasts.

use std::time::Duration;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Div, ElementId, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Rgba,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::i18n::t;
use crate::ui::app::{Dialog, FuwaApp};
use crate::ui::motion;
use crate::ui::text::{TIGHT, WIDE, tracked};
use crate::ui::theme::{Palette, alpha, corner, radius_3xl, radius_xl};
use crate::ui::widgets::{card, error_line, icon, labeled, pal};

/// What a server or instance calls its identity provider, for "Continue with …".
pub fn provider_name(name: &str) -> &str {
    if name.trim().is_empty() { "your organization" } else { name }
}

/// Names the site a sign-in sends people to before they go: it sees their
/// IP address, which fuwa itself never hands out.
pub fn host_notice(host: &str, p: &Palette) -> AnyElement {
    if host.is_empty() {
        return div().into_any_element();
    }
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(12.0))
        .py(px(10.0))
        .rounded(corner(12.0))
        .bg(alpha(p.primary, 0.08))
        .text_sm()
        .text_color(p.muted_foreground)
        .child(icon("globe").size(px(16.0)).flex_none().text_color(p.primary))
        .child(div().flex_1().min_w_0().child(host_sentence(host, p)))
        .into_any_element()
}

/// "Signs you in at <host>, which sees your IP address.", the host in bold.
pub fn host_sentence(host: &str, p: &Palette) -> gpui_kit::StyledText {
    let lead = "Signs you in at ";
    let text = format!("{lead}{host}, which sees your IP address.");
    let bold = gpui_kit::HighlightStyle {
        font_weight: Some(FontWeight::BOLD),
        color: Some(p.foreground.into()),
        ..Default::default()
    };
    gpui_kit::StyledText::new(text).with_highlights([(lead.len()..lead.len() + host.len(), bold)])
}

/// How much the web's dialog overlays blur what's behind them (`backdrop-blur-[2px]`).
pub const SCRIM_BLUR: f32 = 2.0;

/// The web's dialog overlay (`bg-black/50 backdrop-blur-[2px]`): the whole
/// window behind, rail and sidebar included, darkened by half and blurred a
/// little; it swallows clicks.
pub fn scrim(id: impl Into<ElementId>, _p: &Palette) -> Stateful<Div> {
    shade(id, 0.5, SCRIM_BLUR)
}

/// A layer over the whole window darkened by `dim` and blurred by `blur`.
pub fn shade(id: impl Into<ElementId>, dim: f32, blur: f32) -> Stateful<Div> {
    div()
        .id(id.into())
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .backdrop_blur(px(blur))
        .bg(gpui_kit::hsla(0.0, 0.0, 0.0, dim))
        .occlude()
}

/// The web's `shadow-2xl`.
pub fn shadow_2xl() -> Vec<gpui_kit::BoxShadow> {
    vec![gpui_kit::BoxShadow {
        color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.25),
        offset: gpui_kit::point(px(0.0), px(25.0)),
        blur_radius: px(50.0),
        spread_radius: px(-12.0),
        inset: false,
    }]
}

/// The web's `DialogContent`: a card `max-w-md` (`max-w-2xl` when wide),
/// `p-6 rounded-3xl border bg-card shadow-2xl`. Put [`dialog_close`] in it.
pub fn dialog_card(wide: bool, p: &Palette) -> Div {
    div()
        .relative()
        .w(px(if wide { 672.0 } else { 448.0 }))
        .p(px(24.0))
        .flex()
        .flex_col()
        .rounded(radius_3xl())
        .border_1()
        .border_color(p.border)
        .bg(p.card)
        .text_color(p.foreground)
        .shadow(shadow_2xl())
}

/// The web's `DialogHeader`: the title (`text-xl font-extrabold`) and a line
/// under it (`mt-1 text-sm text-muted-foreground`), `mb-5`, clear of the close button.
pub fn dialog_header(title: impl IntoElement, description: Option<AnyElement>, p: &Palette) -> Div {
    div()
        .mb(px(20.0))
        .pr(px(32.0))
        .flex()
        .flex_col()
        .child(div().text_xl().line_height(px(28.0)).font_weight(FontWeight::EXTRA_BOLD).child(title))
        .when_some(description, |el, d| {
            el.child(div().mt(px(4.0)).text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(d))
        })
}

/// The dialog's close button (`absolute top-4 right-4 size-8 rounded-full`),
/// which turns a quarter on hover (`hover:rotate-90`).
pub fn dialog_close(id: impl Into<SharedString>, p: &Palette) -> DialogClose {
    DialogClose { id: id.into(), muted: p.muted, foreground: p.foreground, quiet: p.muted_foreground, on_click: None }
}

/// See [`dialog_close`].
#[derive(gpui_kit::IntoElement)]
pub struct DialogClose {
    id: SharedString,
    muted: Rgba,
    foreground: Rgba,
    quiet: Rgba,
    on_click: Option<ClickHandler>,
}

type ClickHandler = Box<dyn Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App)>;

impl DialogClose {
    pub fn on_click(
        mut self,
        handler: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl gpui_kit::RenderOnce for DialogClose {
    fn render(self, window: &mut Window, cx: &mut gpui_kit::App) -> impl IntoElement {
        let (hover, fg) = (self.muted, self.foreground);
        let button = div()
            .id(self.id.clone())
            .absolute()
            .top(px(16.0))
            .right(px(16.0))
            .size(px(32.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .text_color(self.quiet)
            .hover(move |s| s.bg(hover).text_color(fg))
            .when_some(self.on_click, |el, handler| el.on_click(handler))
            .child(icon("x").size(px(16.0)));
        let turn = motion::Pose::turn(90.0);
        motion::answer(button, self.id, turn, turn, window, cx)
    }
}

/// A dialog over the window: the scrim fades in (200ms) and the card springs
/// up from 40px below, growing from 96%, as the web's dialogs do. `close`
/// runs on a click outside the card.
pub fn dialog_layer(
    tag: &str,
    panel: impl IntoElement,
    p: &Palette,
    close: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    motion::fade_in(
        scrim("dialog-scrim", p).on_click(close).child(motion::dialog_in(
            div().id("dialog-panel").on_click(|_, _, cx| cx.stop_propagation()).child(panel),
            SharedString::from(format!("dialog-{tag}")),
        )),
        SharedString::from(format!("dialog-fade-{tag}")),
        Duration::from_millis(200),
    )
    .into_any_element()
}

/// A dialog's buttons (`h-9 rounded-xl`, the action in bold): `Ghost` for
/// Cancel, `Primary` or `Destructive` for the action.
pub fn dialog_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    look: crate::ui::settings_controls::Look,
    p: &Palette,
) -> Stateful<Div> {
    use crate::ui::settings_controls::Look;
    crate::ui::settings_controls::button(id, label, None, look, false, p)
        .rounded(radius_xl())
        .when(look != Look::Ghost, |el| el.font_weight(FontWeight::BOLD))
}

impl FuwaApp {
    pub(crate) fn render_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.dialog.clone()?;
        let p = pal(cx);
        if let Some(el) = self.render_join_dialog(&dialog, window, cx) {
            return Some(el);
        }
        if let Some(el) = self.render_web_dialog(&dialog, window, cx) {
            return Some(el);
        }
        if let Dialog::Profile { key, user_id, server } = &dialog {
            return Some(self.render_profile_card(key, user_id, server.as_deref(), window, cx));
        }
        if let Dialog::Moderate { key, server, user_id, action } = &dialog {
            return Some(self.render_moderate(key, server, user_id, *action, window, cx));
        }
        if let Dialog::Welcome { key, server } = &dialog {
            return Some(self.render_welcome_web(key, server, window, cx));
        }
        if let Dialog::Onboarding { .. } = &dialog {
            return self.render_onboarding(window, cx);
        }
        if let Dialog::Secure { key, server, channel } = &dialog {
            return Some(self.render_secure(key, server, channel, window, cx));
        }
        if let Dialog::Safety { key, conversation } = &dialog {
            return Some(self.render_encryption(key, conversation, window, cx));
        }
        if let Dialog::Poll { .. } = &dialog {
            return Some(self.render_poll_editor(window, cx));
        }
        if let Dialog::Picture { .. } = &dialog {
            return Some(self.render_picture(&dialog, window, cx));
        }
        if let Dialog::PollVoters { key, server, channel, message } = &dialog {
            return Some(self.render_voters(key, server, channel, message, cx));
        }
        let busy = self.dialog_busy;
        let field = || Input::new(&self.dialog_input).large();
        let sso_label = match &dialog {
            Dialog::SsoJoin { .. } if busy => "Waiting for your browser…".to_owned(),
            Dialog::SsoJoin { provider, .. } => format!("Continue with {}", provider_name(provider)),
            _ => String::new(),
        };
        let (glyph, title, body, content, action): (&str, String, String, AnyElement, Option<&str>) = match &dialog {
            Dialog::JoinInvite { .. } => (
                "user-plus",
                "Join a server".into(),
                "Paste the invite link someone sent you, or just its code.".into(),
                labeled("Invite", field(), &p).into_any_element(),
                Some(if busy { "Joining…" } else { "Join" }),
            ),
            Dialog::SsoJoin { name, provider, host, .. } => (
                "lock-keyhole",
                format!("Sign in to join {name}"),
                format!(
                    "{name} asks its members to sign in through {}. Your browser opens to do it.",
                    provider_name(provider)
                ),
                host_notice(host, &p),
                Some(sso_label.as_str()),
            ),
            Dialog::LeaveServer { key, server } => {
                let name = self
                    .core
                    .shared
                    .read(|s| s.instance(key).and_then(|i| i.server(server)).map(|s| s.name.clone()))
                    .unwrap_or_default();
                (
                    "log-out",
                    format!("Leave {name}?"),
                    "You'll need a new invite to come back.".into(),
                    div().into_any_element(),
                    Some(if busy { "Leaving…" } else { "Leave" }),
                )
            }
            Dialog::AllowGame { name, .. } => (
                "gamepad-2",
                format!("{name} wants to show what you're playing"),
                "It reports to Discord's apps on this computer, and fuwa can show it to people you share a server with, if you turned that on. You can change your mind in Privacy settings.".into(),
                div().into_any_element(),
                Some("Allow"),
            ),
            Dialog::Profile { .. }
            | Dialog::Moderate { .. }
            | Dialog::Invite { .. }
            | Dialog::CreateChannel { .. }
            | Dialog::Rules { .. }
            | Dialog::CreateServer { .. }
            | Dialog::Apply { .. }
            | Dialog::Application { .. }
            | Dialog::Welcome { .. }
            | Dialog::Onboarding { .. }
            | Dialog::Secure { .. }
            | Dialog::Safety { .. }
            | Dialog::Poll { .. }
            | Dialog::PollVoters { .. }
            | Dialog::Picture { .. }
            | Dialog::ShareScreen => unreachable!("drawn on its own"),
        };
        let danger = matches!(
            dialog,
            Dialog::LeaveServer { .. }
                | Dialog::Moderate {
                    action: crate::core::moderation::Action::Kick | crate::core::moderation::Action::Ban(_),
                    ..
                }
        );
        use crate::ui::settings_controls::Look;
        let tint = if danger { p.destructive } else { p.primary };
        // The web's confirm dialogs: a small tinted tile beside the title (`size-9 rounded-xl`).
        let title_row = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .size(px(36.0))
                    .flex_none()
                    .rounded(radius_xl())
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(alpha(tint, 0.15))
                    .text_color(tint)
                    .child(icon(glyph).size(px(20.0))),
            )
            .child(div().flex_1().min_w_0().child(tracked(title, TIGHT).wraps()));
        let panel = dialog_card(false, &p)
            .child(dialog_header(title_row, Some(body.into_any_element()), &p))
            .child(content)
            .when_some(error_line(self.dialog_error.as_deref(), &p), |el, e| el.child(div().mt(px(12.0)).child(e)))
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .child(if matches!(dialog, Dialog::AllowGame { .. }) {
                        dialog_button("dialog-cancel", "Don't allow", Look::Ghost, &p)
                            .on_click(cx.listener(|this, _, _, cx| this.refuse_game(cx)))
                    } else {
                        dialog_button("dialog-cancel", t("common.cancel"), Look::Ghost, &p)
                            .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx)))
                    })
                    .when_some(action, |el, label| {
                        let look = if danger { Look::Destructive } else { Look::Primary };
                        let button = dialog_button("dialog-ok", label.to_owned(), look, &p)
                            .when(busy, |el| el.opacity(0.5))
                            .on_click(cx.listener(|this, _, window, cx| this.confirm_dialog(window, cx)));
                        el.child(button)
                    }),
            )
            .child(dialog_close("dialog-close", &p).on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))));
        let tag = match &dialog {
            Dialog::CreateServer { .. } => "create",
            Dialog::JoinInvite { .. } => "join",
            Dialog::Invite { .. } => "invite",
            Dialog::Safety { .. } => "safety",
            Dialog::LeaveServer { .. } => "leave",
            Dialog::CreateChannel { .. } => "channel",
            Dialog::Rules { .. } => "rules",
            Dialog::Profile { .. } => "profile",
            Dialog::Welcome { .. } => "welcome",
            Dialog::Onboarding { .. } => "onboarding",
            Dialog::Secure { .. } => "secure",
            Dialog::Moderate { .. } => "moderate",
            Dialog::SsoJoin { .. } => "sso",
            Dialog::AllowGame { .. } => "game",
            Dialog::Poll { .. } => "poll",
            Dialog::PollVoters { .. } => "voters",
            Dialog::Picture { .. } => "picture",
            Dialog::Apply { .. } => "apply",
            Dialog::Application { .. } => "application",
            Dialog::ShareScreen => "share",
        };
        Some(dialog_layer(tag, panel, &p, cx.listener(|this, _, _, cx| this.close_dialog(cx))))
    }

    pub(crate) fn render_toasts(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let mut stack = div().absolute().right(px(20.0)).bottom(px(100.0)).flex().flex_col().gap(px(10.0)).w(px(340.0));
        // The web's toasts: a note in a pill, over the middle of the window's foot
        // (`bottom-28`), the last three; they spring up and drop away.
        let mut notes = div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(112.0))
            .px(px(16.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.0));
        let plain: Vec<_> = self.toasts.iter().filter(|t| t.open.is_none()).collect();
        for toast in plain.iter().skip(plain.len().saturating_sub(3)) {
            let text =
                if toast.body.is_empty() { toast.title.clone() } else { format!("{} {}", toast.title, toast.body) };
            let pill = div()
                .rounded_full()
                .bg(p.foreground)
                .px(px(16.0))
                .py(px(8.0))
                .text_sm()
                .line_height(px(20.0))
                .font_weight(FontWeight::BOLD)
                .text_color(p.background)
                .shadow(crate::ui::settings_controls::shadow_xl())
                .child(text);
            let id = toast.id;
            let el: AnyElement = if toast.leaving {
                use gpui_kit::{Animation, AnimationExt as _};
                div()
                    .child(pill)
                    .with_animation(
                        SharedString::from(format!("note-out-{id}")),
                        Animation::new(Duration::from_millis(150)),
                        |el, t| el.opacity(1.0 - t).relative().top(px(8.0 * t)),
                    )
                    .into_any_element()
            } else {
                motion::rise(div().child(pill), SharedString::from(format!("note-in-{id}")), Duration::ZERO, 24.0)
                    .into_any_element()
            };
            notes = notes.child(el);
        }
        for toast in self.toasts.iter().filter(|t| t.open.is_some()) {
            let id = toast.id;
            let open = toast.open.clone();
            let channel = toast.channel.clone();
            let thread = toast.thread.clone();
            let item = card(&p)
                .id(SharedString::from(format!("toast-{id}")))
                .p(px(14.0))
                .flex()
                .gap(px(12.0))
                .cursor_pointer()
                .hover({
                    let border = p.primary;
                    move |s| s.border_color(border)
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(nav) = open.clone() {
                        if let (crate::ui::app::Nav::Server { key, server }, Some(channel)) = (&nav, &channel) {
                            this.open_channel(&key.clone(), &server.clone(), channel, window, cx);
                            if let Some(thread) = thread.clone() {
                                this.open_thread(thread, window, cx);
                            }
                        } else {
                            this.navigate(nav, window, cx);
                        }
                    }
                    this.dismiss_toast(id, cx);
                }))
                .child(
                    div()
                        .size(px(36.0))
                        .flex_none()
                        .rounded(corner(12.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(alpha(p.primary, 0.14))
                        .text_color(p.primary)
                        .child(icon(toast.icon).size(px(18.0))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(div().font_weight(FontWeight::EXTRA_BOLD).text_sm().child(toast.title.clone()))
                        .child(div().text_sm().text_color(p.muted_foreground).line_clamp(2).child(toast.body.clone())),
                );
            let el: AnyElement = if toast.leaving {
                div().child(item).with_animation_out(SharedString::from(format!("toast-out-{id}"))).into_any_element()
            } else {
                motion::slide_in(div().child(item), SharedString::from(format!("toast-in-{id}")), 60.0)
                    .into_any_element()
            };
            stack = stack.child(el);
        }
        div().absolute().inset_0().child(notes).child(stack)
    }
}

pub(crate) fn section_title(text: &str, p: &Palette) -> Div {
    div()
        .text_size(px(11.0))
        .font_weight(FontWeight::EXTRA_BOLD)
        .text_color(p.muted_foreground)
        .child(tracked(text.to_uppercase(), WIDE))
}

trait LeaveExt: Sized {
    fn with_animation_out(self, id: SharedString) -> AnyElement;
}

impl LeaveExt for Div {
    /// Slides a toast away to the right as it fades.
    fn with_animation_out(self, id: SharedString) -> AnyElement {
        use gpui_kit::{Animation, AnimationExt as _};
        self.with_animation(
            id,
            Animation::new(Duration::from_millis(240)).with_easing(gpui_kit::ease_in_out),
            |el, t| el.opacity(1.0 - t).relative().left(px(80.0 * t)),
        )
        .into_any_element()
    }
}

/// A suggested channel's emoji: a Unicode one, one of the server's own, or a #.
pub(crate) fn welcome_emoji(emoji: &str, look: &crate::ui::mentions::Look, p: &Palette) -> AnyElement {
    emoji_tile(emoji, look, p.primary.into(), "hash")
}

/// An emoji (Unicode or the server's own) on a tile tinted `tint`, or `fallback`'s icon without one.
pub(crate) fn emoji_tile(
    emoji: &str,
    look: &crate::ui::mentions::Look,
    tint: gpui_kit::Hsla,
    fallback: &str,
) -> AnyElement {
    let base = div()
        .size(px(36.0))
        .flex_none()
        .rounded(corner(12.0))
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui_kit::Hsla { a: 0.14, ..tint })
        .text_color(tint);
    let own = emoji
        .strip_prefix('<')
        .and_then(|e| e.strip_suffix('>'))
        .and_then(|e| e.rsplit_once(':'))
        .and_then(|(_, id)| look.emojis.get(&id.to_uppercase()));
    match own {
        Some(url) => {
            use gpui_kit::StyledImage as _;
            base.child(
                gpui_kit::img(SharedString::from(url.clone())).size(px(24.0)).object_fit(gpui_kit::ObjectFit::Contain),
            )
            .into_any_element()
        }
        None if !emoji.is_empty() && !emoji.starts_with('<') => {
            base.text_size(px(20.0)).child(emoji.to_owned()).into_any_element()
        }
        None => base.child(icon(fallback).size(px(18.0))).into_any_element(),
    }
}

/// The web's tooltip (`TooltipContent`): `bg-primary text-primary-foreground
/// rounded-md px-3 py-1.5 text-xs`, no border or shadow. Use it in place of
/// the kit's `Tooltip::new(...)`, which draws a popover card.
pub struct Tip(SharedString);

impl Tip {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self(text.into())
    }

    pub fn build(self, window: &mut Window, cx: &mut gpui_kit::App) -> gpui_kit::AnyView {
        let p = pal(cx);
        gpui_kit::component::tooltip::Tooltip::new(self.0)
            .bg(p.primary)
            .text_color(p.primary_foreground)
            .border_color(gpui_kit::transparent_black())
            .shadow(Vec::new())
            .rounded(crate::ui::theme::radius_md())
            .px(px(12.0))
            .py(px(6.0))
            .text_xs()
            .line_height(px(16.0))
            .build(window, cx)
    }
}
