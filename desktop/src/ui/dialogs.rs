//! Dialogs drawn as the web draws them, in its `DialogContent` frame
//! (`overlay::dialog_card`): inviting people (`dialogs/InviteDialog.tsx`),
//! making a channel (`dialogs/CreateChannelDialog.tsx`), a server's rules
//! (`join/Rules.tsx`), and what a right-click menu asks before something that
//! can't be undone (`menus/MenuDialogs.tsx`).

use std::time::{Duration, Instant};

use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::core::invites::{self, EXPIRE_AFTER, MAX_USES, Options};
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp};
use crate::ui::motion;
use crate::ui::overlay::{dialog_button, dialog_card, dialog_close, dialog_header, dialog_layer};
use crate::ui::settings_controls::{Look, button, field};
use crate::ui::text::{TIGHT, WIDE, tracked};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_lg, radius_md, radius_xl};
use crate::ui::widgets::{icon, pal, server_icon};

/// Tailwind's `emerald-500`, the web's "copied" color.
const EMERALD: u32 = 0x10b981;

/// The name field's example for a kind of channel.
fn channel_placeholder(kind: pb::ChannelType) -> String {
    t(match kind {
        pb::ChannelType::Category => "workspace.createChannel.placeholder.category",
        pb::ChannelType::Voice => "workspace.createChannel.placeholder.voice",
        _ => "workspace.createChannel.placeholder.text",
    })
}

/// The name and category a new channel is made with, as the web's form sends
/// them: channels you type in are lowercase with dashes, voice channels and
/// categories keep their name as typed, and categories sit at the top.
pub(crate) fn channel_request(kind: pb::ChannelType, typed: &str, parent: &str) -> (String, String) {
    let category = kind == pb::ChannelType::Category;
    let free = category || kind == pb::ChannelType::Voice;
    let name = if free { typed.trim().to_owned() } else { slug(typed) };
    (name, if category { String::new() } else { parent.to_owned() })
}

/// The web's `slug`: lowercase, spaces to dashes, only letters, numbers, `_` and `-`, 100 at most.
fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in name.to_lowercase().chars() {
        if c.is_whitespace() {
            if !space {
                out.push('-');
            }
            space = true;
            continue;
        }
        space = false;
        if c.is_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
        }
    }
    out.chars().take(100).collect()
}

/// The web's input focus: `border-ring ring-[3px] ring-ring/50`.
pub(crate) fn focus_ring(field: gpui_kit::Div, focused: bool, p: &Palette) -> gpui_kit::Div {
    field.when(focused, |el| {
        el.border_color(p.primary).shadow(vec![gpui_kit::BoxShadow {
            color: alpha(p.primary, 0.5),
            offset: gpui_kit::point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: px(3.0),
            inset: false,
        }])
    })
}

/// The web's `Chips`: small pill choices, the chosen one filled in with a check.
fn chips(
    id: &'static str,
    options: Vec<String>,
    chosen: usize,
    p: &Palette,
    cx: &mut Context<FuwaApp>,
    pick: impl Fn(&mut FuwaApp, usize, &mut Context<FuwaApp>) + 'static,
) -> AnyElement {
    let pick = std::rc::Rc::new(pick);
    div()
        .flex()
        .flex_wrap()
        .gap(px(6.0))
        .children(options.into_iter().enumerate().map(|(n, label)| {
            let on = n == chosen;
            let pick = pick.clone();
            let (hover_border, fg) = (alpha(p.primary, 0.4), p.foreground);
            div()
                .id(SharedString::from(format!("chip-{id}-{n}")))
                .flex()
                .items_center()
                .rounded_full()
                .border_1()
                .px(px(12.0))
                .py(px(4.0))
                .text_xs()
                .line_height(px(16.0))
                .font_weight(FontWeight::BOLD)
                .cursor_pointer()
                .map(|el| {
                    if on {
                        el.border_color(p.primary).bg(p.primary).text_color(p.primary_foreground)
                    } else {
                        el.border_color(p.border)
                            .text_color(p.muted_foreground)
                            .hover(move |s| s.border_color(hover_border).text_color(fg))
                    }
                })
                // `whileTap={{ scale: 0.92 }}`.
                .active(|s| s.scale(0.92))
                .on_click(cx.listener(move |this, _, _, cx| pick(this, n, cx)))
                .when(on, |el| {
                    // The tick grows in from nothing.
                    el.child(motion::pop_in(
                        div().mr(px(4.0)).child(icon("check").size(px(12.0))),
                        SharedString::from(format!("chip-{id}-{n}-on")),
                        (0.5, 0.5),
                        0.0,
                        0.0,
                    ))
                })
                .child(label)
        }))
        .into_any_element()
}

/// A server's rules, numbered (`RulesList`), scrolling past `max_h`.
pub(crate) fn rules_list(
    rules: &[String],
    max_h: f32,
    p: &Palette,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let muted = alpha(p.muted, 0.5);
    crate::ui::widgets::inner_scroll("rules-list", window, cx)
        .max_h(px(max_h))
        .pr(px(4.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .children(rules.iter().enumerate().map(|(n, rule)| {
            motion::rise(
                div()
                    .flex()
                    .items_start()
                    .gap(px(12.0))
                    .rounded(radius_2xl())
                    .bg(muted)
                    .p(px(12.0))
                    .text_sm()
                    .line_height(px(20.0))
                    .child(
                        div()
                            .size(px(24.0))
                            .flex_none()
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(alpha(p.primary, 0.15))
                            .text_color(p.primary)
                            .text_xs()
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child((n + 1).to_string()),
                    )
                    // `InlineMarkdown`: a rule is a line of Markdown.
                    .child(
                        div().flex_1().min_w_0().pt(px(2.0)).child(
                            gpui_kit::base::TextView::markdown(
                                SharedString::from(format!("rule-md|{n}")),
                                crate::ui::text::images_as_links(rule),
                            )
                            .markdown_extensions(crate::ui::emoji::markdown_extensions())
                            .style(crate::ui::chat::chat_markdown(p))
                            .on_link_click(|url, _, _, cx| crate::ui::text::open_link(url, cx))
                            .w_full(),
                        ),
                    ),
                SharedString::from(format!("rule-{n}")),
                Duration::from_millis(45 * n.min(10) as u64),
                0.0,
            )
        }))
}

/// "I've read the rules and agree to them" (`AgreeCheck`): a card that fills
/// in and draws its tick when pressed. The caller says what a press does.
pub(crate) fn agree_check(id: &str, checked: bool, p: &Palette) -> gpui_kit::Stateful<gpui_kit::Div> {
    let hover = alpha(p.primary, 0.4);
    div()
        .id(SharedString::from(id.to_owned()))
        .flex()
        .items_center()
        .gap(px(12.0))
        .w_full()
        .rounded(radius_2xl())
        .border_1()
        .p(px(12.0))
        .text_sm()
        .font_weight(FontWeight::BOLD)
        .cursor_pointer()
        .map(|el| {
            if checked {
                el.border_color(alpha(p.primary, 0.6)).bg(alpha(p.primary, 0.1))
            } else {
                el.border_color(p.border).hover(move |s| s.border_color(hover))
            }
        })
        // `whileTap={{ scale: 0.98 }}`.
        .active(|s| s.scale(0.98))
        .child(motion::once(
            div()
                .size(px(24.0))
                .flex_none()
                .rounded(radius_lg())
                .border_2()
                .flex()
                .items_center()
                .justify_center()
                .map(|el| {
                    if checked {
                        el.border_color(p.primary).bg(p.primary).text_color(p.primary_foreground)
                    } else {
                        el.border_color(alpha(p.muted_foreground, 0.4))
                    }
                })
                .when(checked, |el| {
                    el.child(motion::rise(icon("check").size(px(16.0)), "rules-tick", Duration::ZERO, 0.0))
                }),
            // The box bumps up a quarter and back as it's checked.
            SharedString::from(format!("{id}|{checked}")),
            Duration::from_millis(300),
            move |el, t| el.scale(if checked { 1.0 + 0.25 * (t * std::f32::consts::PI).sin() } else { 1.0 }),
        ))
        .child(div().flex_1().min_w_0().child(t("join.rules.agree")))
}

/// A suggested channel's mark: its emoji (Unicode or the server's own), or a # (a megaphone) in the accent.
fn emoji_glyph(emoji: &str, look: &crate::ui::mentions::Look, tint: Hsla, fallback: &str) -> AnyElement {
    let own = emoji
        .strip_prefix('<')
        .and_then(|e| e.strip_suffix('>'))
        .and_then(|e| e.rsplit_once(':'))
        .and_then(|(_, id)| look.emojis.get(&id.to_uppercase()));
    match own {
        Some(url) => {
            use gpui_kit::StyledImage as _;
            crate::ui::widgets::picture(url.clone())
                .size(px(24.0))
                .object_fit(gpui_kit::ObjectFit::Contain)
                .into_any_element()
        }
        None if !emoji.is_empty() && !emoji.starts_with('<') => {
            div().text_size(px(20.0)).line_height(px(28.0)).child(emoji.to_owned()).into_any_element()
        }
        None => icon(fallback).size(px(20.0)).text_color(tint).into_any_element(),
    }
}

/// What the web keeps in these dialogs' React state.
#[derive(Default)]
pub struct WebDialogs {
    /// The instance and channel of the next invite, set just before the dialog opens.
    next_invite: Option<(String, String)>,
    pub(crate) invite: InviteState,
    /// "I've read the rules" ticked, and when the button last shook for it.
    rules_checked: bool,
    rules_nudge: Option<Instant>,
}

#[derive(Default)]
pub(crate) struct InviteState {
    key: String,
    channel: String,
    invite: Option<pb::Invite>,
    error: Option<String>,
    editing: bool,
    options: Options,
    making: bool,
    copied: Option<Instant>,
    /// Counts openings, so an answer for an earlier one is dropped.
    generation: u64,
}

impl FuwaApp {
    /// Opens the invite dialog for a server, or one of its channels.
    pub(crate) fn open_invite(
        &mut self,
        key: &str,
        server: &str,
        channel: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.web_dialogs.next_invite = Some((key.to_owned(), channel.to_owned()));
        self.open_dialog(Dialog::Invite { link: None, server: server.to_owned() }, window, cx);
    }

    /// Called as a dialog opens: starts what it needs.
    pub(crate) fn web_dialog_opened(&mut self, dialog: &Dialog, window: &mut Window, cx: &mut Context<Self>) {
        match dialog {
            Dialog::CreateChannel { kind, .. } => {
                let hint = channel_placeholder(*kind);
                self.dialog_input.update(cx, |s, cx| s.set_placeholder(hint, window, cx));
            }
            Dialog::Invite { server, .. } => {
                let (key, channel) = match self.web_dialogs.next_invite.take() {
                    Some(next) => next,
                    None => (self.nav_key().unwrap_or_default(), String::new()),
                };
                let generation = self.web_dialogs.invite.generation + 1;
                self.web_dialogs.invite =
                    InviteState { key: key.clone(), channel: channel.clone(), generation, ..Default::default() };
                let (core, server) = (self.core.clone(), server.clone());
                self.run(cx, async move { core.invite_for(&key, &server, &channel).await }, move |this, result, cx| {
                    let state = &mut this.web_dialogs.invite;
                    if state.generation != generation {
                        return;
                    }
                    match result {
                        Ok(invite) => state.invite = Some(invite),
                        Err(err) => state.error = Some(err.message),
                    }
                    cx.notify();
                });
            }
            Dialog::Rules { .. } => {
                self.web_dialogs.rules_checked = false;
                self.web_dialogs.rules_nudge = None;
            }
            // A newcomer who hasn't agreed reads the rules at the end of the welcome screen.
            Dialog::Welcome { key, server } => self.welcome_opened(key, server, cx),
            _ => {}
        }
    }

    /// The welcome screen opened, asked for or greeting a newcomer: the rules
    /// to agree to at its end, for someone who hasn't yet.
    pub(crate) fn welcome_opened(&mut self, key: &str, server: &str, cx: &mut Context<Self>) {
        self.web_dialogs.rules_checked = false;
        self.web_dialogs.rules_nudge = None;
        let pending = self.core.shared.read(|s| {
            s.instance(key).is_some_and(|i| i.access(server).pending && i.server(server).is_some_and(|sv| sv.has_rules))
        });
        if pending {
            self.rules = None;
            let (core, key, server) = (self.core.clone(), key.to_owned(), server.to_owned());
            self.run(cx, async move { core.server_rules(&key, &server).await }, |this, result, cx| {
                this.rules = Some(result.unwrap_or_default());
                cx.notify();
            });
        }
    }

    fn nav_key(&self) -> Option<String> {
        match &self.nav {
            crate::ui::app::Nav::Server { key, .. } => Some(key.clone()),
            _ => None,
        }
    }

    /// The dialogs this file draws, or None for the others.
    pub(crate) fn render_web_dialog(
        &mut self,
        dialog: &Dialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let p = pal(cx);
        let (tag, panel) = match dialog {
            Dialog::Invite { server, .. } => ("invite", self.invite_panel(server, &p, window, cx)),
            Dialog::CreateChannel { key, server, parent, kind } => {
                ("channel", self.create_channel_panel(key, server, parent, *kind, &p, window, cx))
            }
            Dialog::Rules { key, server } => ("rules", self.rules_panel(key, server, &p, window, cx)),
            Dialog::ShareScreen => ("share", self.share_panel(&p, window, cx)),
            _ => return None,
        };
        let panel =
            panel.child(dialog_close("dialog-close", &p).on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))));
        Some(dialog_layer(tag, panel, &p, cx.listener(|this, _, _, cx| this.close_dialog(cx)), cx))
    }

    // ───────────────────────── Inviting people ─────────────────────────

    fn invite_panel(
        &mut self,
        server_id: &str,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let state = &self.web_dialogs.invite;
        let key = state.key.clone();
        let (server, channel) = self.core.shared.read(|s| {
            let i = s.instance(&key);
            (
                i.and_then(|i| i.server(server_id)).cloned(),
                (!state.channel.is_empty())
                    .then(|| i.and_then(|i| i.channel(server_id, &state.channel)).map(|c| c.name.clone()))
                    .flatten(),
            )
        });
        let base = self.core.public_base(&key);
        let link = state.invite.as_ref().map(|i| invites::link(&base, &i.code));
        let copied = state.copied.is_some_and(|at| at.elapsed() < Duration::from_millis(1600));
        let name = server.as_ref().map(|s| s.name.clone()).unwrap_or_else(|| t("workspace.invite.thisServer"));

        let title = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .when_some(server.as_ref(), |el, server| {
                // It pops in, turning upright from a little tilt.
                el.child(motion::pop(server_icon(server, 40.0, 12.8, p), "invite-icon", 0.6, -12.0, Duration::ZERO))
            })
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(tracked(t_with("workspace.invite.title", &[("server", Arg::Str(&name))]), TIGHT).wraps())
                    .when_some(channel, |el, channel| {
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .text_sm()
                                .line_height(px(20.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.muted_foreground)
                                .child(icon("hash").size(px(14.0)))
                                .child(channel),
                        )
                    }),
            );

        let streamer = self.prefs.streamer_mode;
        let shown: AnyElement = match &link {
            Some(link) => motion::rise(
                div().truncate().font_family("monospace").text_sm().line_height(px(20.0)).child(if streamer {
                    "•".repeat(link.chars().count().min(40))
                } else {
                    link.clone()
                }),
                SharedString::from(format!("invite-link-{}", state.invite.as_ref().map_or("", |i| i.code.as_str()))),
                Duration::ZERO,
                12.0,
            )
            .into_any_element(),
            None => div().h(px(20.0)).w(px(240.0)).rounded(radius_md()).bg(p.muted).into_any_element(),
        };
        let green: Hsla = gpui_kit::rgb(EMERALD).into();
        let copy_fg = if copied { gpui_kit::white() } else { p.primary_foreground.into() };
        let copy = div()
            .id("invite-copy")
            .flex_none()
            .h(px(36.0))
            .w(px(96.0))
            .rounded(radius_lg())
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .text_sm()
            .font_weight(FontWeight::BOLD)
            .bg(if copied { green } else { p.primary.into() })
            .text_color(copy_fg)
            .when(link.is_none(), |el| el.opacity(0.5))
            .when(link.is_some(), |el| {
                // The web's `.btn`, lifting with a glow, in a `whileTap={{ scale: 0.92 }}`.
                let glow = if copied { green } else { p.primary.into() };
                el.cursor_pointer()
                    .hover(move |s| {
                        s.translate_y(px(-2.0)).shadow(vec![gpui_kit::BoxShadow {
                            color: glow,
                            offset: gpui_kit::point(px(0.0), px(8.0)),
                            blur_radius: px(22.0),
                            spread_radius: px(-8.0),
                            inset: false,
                        }])
                    })
                    .active(|s| s.translate_y(px(0.0)).scale(0.92))
            })
            .on_click(cx.listener(|this, _, _, cx| this.copy_invite(cx)))
            .child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(icon(if copied { "check" } else { "copy" }).size(px(16.0)))
                    .child(if copied { t("workspace.invite.copied") } else { t("workspace.invite.copy") }),
                SharedString::from(format!("invite-copy-{copied}")),
                Duration::ZERO,
                if copied { 16.0 } else { -16.0 },
            ));
        let link_field = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(radius_xl())
            .border_1()
            .border_color(if copied { Hsla { a: 0.6, ..green } } else { p.border.into() })
            .bg(if copied { Hsla { a: 0.05, ..green } } else { alpha(p.background, 0.6) })
            .p(px(6.0))
            .pl(px(12.0))
            .child(div().flex_1().min_w_0().overflow_hidden().child(shown))
            .child(copy);

        let editing = state.editing;
        // The chevron turns over while the options are open (`rotate-180`, 300ms).
        let turn = motion::follow("invite-edit-turn", if editing { 1.0 } else { 0.0 }, window, cx);
        let terms = state.invite.as_ref().map(|i| invites::terms(i, crate::core::dms::now_ms()));
        let fg = p.primary;
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .text_xs()
                    .line_height(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .child(tracked(t("workspace.invite.link").to_uppercase(), WIDE)),
            )
            .child(link_field);
        if let Some(error) = state.error.clone() {
            col = col.child(motion::rise(
                div().text_sm().text_color(p.destructive).child(error),
                "invite-error",
                Duration::ZERO,
                6.0,
            ));
        }
        col = col.child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(4.0))
                .text_xs()
                .line_height(px(16.0))
                .text_color(p.muted_foreground)
                .when_some(terms, |el, terms| el.child(terms))
                .child(
                    div()
                        .id("invite-edit")
                        .flex()
                        .items_center()
                        .gap(px(2.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(fg)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.web_dialogs.invite.editing = !this.web_dialogs.invite.editing;
                            cx.notify();
                        }))
                        .child(t("workspace.invite.edit"))
                        .child(
                            icon("chevron-down").size(px(14.0)).rotate(gpui_kit::radians(turn * std::f32::consts::PI)),
                        ),
                ),
        );
        let mut panel = dialog_card(false, p)
            .child(dialog_header(title, Some(t("workspace.invite.about").into_any_element()), p))
            .child(col);
        if editing {
            panel = panel.child(motion::rise(self.invite_options(p, cx), "invite-options", Duration::ZERO, -8.0));
        }
        panel
    }

    fn invite_options(&self, p: &Palette, cx: &mut Context<Self>) -> gpui_kit::Div {
        let state = &self.web_dialogs.invite;
        let age = EXPIRE_AFTER.iter().position(|(v, _)| *v == state.options.max_age_seconds).unwrap_or(0);
        let uses = MAX_USES.iter().position(|v| *v == state.options.max_uses).unwrap_or(0);
        let label = |text: String| div().text_sm().font_weight(FontWeight::BOLD).child(text);
        div()
            .mt(px(16.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .rounded(radius_2xl())
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.muted, 0.3))
            .p(px(16.0))
            .child(div().flex().flex_col().gap(px(8.0)).child(label(t("workspace.invite.expireAfter"))).child(chips(
                "invite-age",
                EXPIRE_AFTER.iter().map(|(_, k)| t(k)).collect(),
                age,
                p,
                cx,
                |this: &mut FuwaApp, n, cx| {
                    this.web_dialogs.invite.options.max_age_seconds = EXPIRE_AFTER[n].0;
                    cx.notify();
                },
            )))
            .child(
                div().flex().flex_col().gap(px(8.0)).child(label(t("workspace.invite.howMany"))).child(chips(
                    "invite-uses",
                    MAX_USES
                        .iter()
                        .map(|n| {
                            if *n == 0 {
                                t("workspace.invite.uses.none")
                            } else {
                                t_with("workspace.invite.uses.count", &[("count", Arg::Num(i64::from(*n)))])
                            }
                        })
                        .collect(),
                    uses,
                    p,
                    cx,
                    |this: &mut FuwaApp, n, cx| {
                        this.web_dialogs.invite.options.max_uses = MAX_USES[n];
                        cx.notify();
                    },
                )),
            )
            .child(
                div().flex().justify_end().child(
                    button(
                        "invite-generate",
                        t("workspace.invite.generate"),
                        Some(if state.making { "loader-circle" } else { "refresh-cw" }),
                        Look::Primary,
                        false,
                        p,
                    )
                    .rounded(radius_xl())
                    .font_weight(FontWeight::BOLD)
                    .when(state.making, |el| el.opacity(0.5))
                    .on_click(cx.listener(|this, _, _, cx| this.generate_invite(cx))),
                ),
            )
    }

    pub(crate) fn copy_invite(&mut self, cx: &mut Context<Self>) {
        let key = self.web_dialogs.invite.key.clone();
        let Some(code) = self.web_dialogs.invite.invite.as_ref().map(|i| i.code.clone()) else { return };
        let link = invites::link(&self.core.public_base(&key), &code);
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(link));
        self.web_dialogs.invite.copied = Some(Instant::now());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1650)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        cx.notify();
    }

    fn generate_invite(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::Invite { server, .. }) = self.dialog.clone() else { return };
        let state = &mut self.web_dialogs.invite;
        let key = state.key.clone();
        if state.making {
            return;
        }
        state.making = true;
        state.error = None;
        let (core, channel, options, generation) =
            (self.core.clone(), state.channel.clone(), state.options, state.generation);
        self.run(
            cx,
            async move { core.make_invite(&key, &server, &channel, options).await },
            move |this, result, cx| {
                let state = &mut this.web_dialogs.invite;
                if state.generation != generation {
                    return;
                }
                state.making = false;
                match result {
                    Ok(invite) => {
                        state.invite = Some(invite);
                        state.editing = false;
                        state.copied = None;
                    }
                    Err(err) => state.error = Some(err.message),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    // ───────────────────────── Making a channel ─────────────────────────

    #[allow(clippy::too_many_arguments)]
    fn create_channel_panel(
        &mut self,
        _key: &str,
        _server: &str,
        _parent: &str,
        kind: pb::ChannelType,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        use pb::ChannelType as K;
        let focused = gpui_kit::Focusable::focus_handle(self.dialog_input.read(cx), cx).is_focused(window);
        const KINDS: [(K, &str, &str, &str); 5] = [
            (K::Text, "hash", "workspace.createChannel.type.text", "workspace.createChannel.type.textHint"),
            (
                K::Announcement,
                "megaphone",
                "workspace.createChannel.type.announcement",
                "workspace.createChannel.type.announcementHint",
            ),
            (
                K::Secure,
                "shield-check",
                "workspace.createChannel.type.secure",
                "workspace.createChannel.type.secureHint",
            ),
            (K::Voice, "volume-2", "workspace.createChannel.type.voice", "workspace.createChannel.type.voiceHint"),
            (
                K::Category,
                "folder",
                "workspace.createChannel.type.category",
                "workspace.createChannel.type.categoryHint",
            ),
        ];
        let category = kind == K::Category;
        let busy = self.dialog_busy;
        let named = !self.dialog_input.read(cx).value().trim().is_empty();
        // The choice's highlight glides to the one picked (`layoutId="channel-type"`): where
        // each sits is measured as it's drawn, and until then the picked one lights itself.
        let spots = window.use_keyed_state("channel-kinds", cx, |_, _| Vec::<(f32, f32)>::new());
        let spot = KINDS.iter().position(|(k, ..)| *k == kind).and_then(|n| spots.read(cx).get(n).copied());
        let glider = spot.map(|(top, h)| {
            let top = motion::follow("channel-kind-glide", top, window, cx);
            // `bg-primary/10 ring-2 ring-primary/40`. Opaque: GPUI fills under a
            // shadow, which would show through a see-through tint.
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px(top))
                .h(px(h))
                .rounded(radius_2xl())
                .bg(crate::ui::theme::mix(p.card, p.primary, 0.1))
                .shadow(vec![gpui_kit::BoxShadow {
                    color: alpha(p.primary, 0.4),
                    offset: gpui_kit::point(px(0.0), px(0.0)),
                    blur_radius: px(0.0),
                    spread_radius: px(2.0),
                    inset: false,
                }])
        });
        let glides = glider.is_some();
        let choices = div().flex().flex_col().gap(px(8.0)).on_children_prepainted(move |bounds, window, cx| {
            let measured = crate::ui::menus::measured(&bounds);
            if *spots.read(cx) != measured {
                spots.update(cx, |s, _| *s = measured);
                window.request_animation_frame();
            }
        });
        let choices = choices.children(KINDS.into_iter().enumerate().map(|(n, (k, glyph, label, hint))| {
            let on = k == kind;
            let hover = alpha(p.primary, 0.4);
            div()
                .id(SharedString::from(format!("kind-{n}")))
                .relative()
                .flex()
                .items_center()
                .gap(px(12.0))
                .p(px(12.0))
                .rounded(radius_2xl())
                .border_1()
                .cursor_pointer()
                .map(|el| {
                    if on && glides {
                        el.border_color(alpha(p.primary, 0.6))
                    } else if on {
                        // `border-primary/60`, and the glider's look on the choice itself.
                        el.border_color(alpha(p.primary, 0.6)).bg(crate::ui::theme::mix(p.card, p.primary, 0.1)).shadow(
                            vec![gpui_kit::BoxShadow {
                                color: alpha(p.primary, 0.4),
                                offset: gpui_kit::point(px(0.0), px(0.0)),
                                blur_radius: px(0.0),
                                spread_radius: px(2.0),
                                inset: false,
                            }],
                        )
                    } else {
                        el.border_color(p.border)
                    }
                })
                // `whileHover={{ x: 2 }} whileTap={{ scale: 0.98 }}`, and `hover:border-primary/40`.
                .hover(move |s| {
                    let s = s.translate_x(px(2.0));
                    if on { s } else { s.border_color(hover) }
                })
                .active(|s| s.scale(0.98))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(Dialog::CreateChannel { kind, .. }) = &mut this.dialog {
                        *kind = k;
                    }
                    let hint = channel_placeholder(k);
                    this.dialog_input.update(cx, |s, cx| s.set_placeholder(hint, window, cx));
                    cx.notify();
                }))
                .child(
                    div()
                        .size(px(36.0))
                        .flex_none()
                        .rounded(radius_xl())
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(if on { p.primary } else { p.muted })
                        .text_color(if on { p.primary_foreground } else { p.muted_foreground })
                        // Picked, the icon springs up from small and tilted.
                        .child(motion::pop(
                            icon(glyph).size(px(18.0)),
                            SharedString::from(format!("kind-glyph-{n}-{on}")),
                            if on { 0.4 } else { 1.0 },
                            if on { -30.0 } else { 0.0 },
                            Duration::ZERO,
                        )),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().text_sm().line_height(px(20.0)).font_weight(FontWeight::BOLD).child(t(label)))
                        .child(div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(t(hint))),
                )
        }));
        let mark = match kind {
            K::Category => None,
            K::Voice => Some("volume-2"),
            K::Secure => Some("shield-check"),
            _ => Some("hash"),
        };
        let input = Input::new(&self.dialog_input).appearance(false).when_some(mark, |el, mark| {
            el.prefix(
                motion::rise(
                    icon(mark).size(px(16.0)).text_color(p.muted_foreground),
                    SharedString::from(format!("name-mark-{mark}")),
                    Duration::ZERO,
                    4.0,
                )
                .into_any_element(),
            )
        });
        let submit = button(
            "dialog-ok",
            t(if category {
                "workspace.createChannel.createCategory"
            } else {
                "workspace.createChannel.createChannel"
            }),
            busy.then_some("loader-circle"),
            Look::Primary,
            false,
            p,
        )
        .h(px(44.0))
        .w_full()
        .rounded(radius_xl())
        .font_weight(FontWeight::BOLD)
        .when(busy || !named, |el| el.opacity(0.5))
        .on_click(cx.listener(|this, _, window, cx| this.confirm_dialog(window, cx)));
        dialog_card(false, p).child(dialog_header(tracked(t("workspace.createChannel.title"), TIGHT), None, p)).child(
            div()
                .flex()
                .flex_col()
                .gap(px(16.0))
                .child(div().relative().children(glider).child(choices))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_sm()
                                .line_height(px(14.0))
                                .font_weight(FontWeight::BOLD)
                                .child(t("workspace.createChannel.name")),
                        )
                        // The web's input is see-through on the card.
                        .child(focus_ring(field(input, p).bg(p.card), focused, p)),
                )
                .when_some(self.dialog_error.clone(), |el, e| {
                    el.child(div().text_sm().text_color(p.destructive).child(e))
                })
                .child(submit),
        )
    }

    // ───────────────────────── A server's rules ─────────────────────────

    fn rules_panel(
        &mut self,
        key: &str,
        server_id: &str,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let (server, agreeing) = self.core.shared.read(|s| {
            let i = s.instance(key);
            (i.and_then(|i| i.server(server_id)).cloned(), i.is_some_and(|i| i.access(server_id).pending))
        });
        let name = server.as_ref().map(|s| s.name.clone()).unwrap_or_default();
        let title = t_with(
            if agreeing { "join.rules.beforeYouTalkIn" } else { "join.rules.title" },
            &[("server", Arg::Str(&name))],
        );
        let note = t(if agreeing { "join.rules.agreeNote" } else { "join.rules.readNote" });
        // The band at the top: the server's icon with a scroll on it, over the primary fading down.
        let band = div()
            .mx(px(-24.0))
            .mt(px(-24.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .rounded_t(crate::ui::theme::radius_3xl())
            .bg(gpui_kit::linear_gradient(
                180.0,
                gpui_kit::linear_color_stop(alpha(p.primary, 0.15), 0.0),
                gpui_kit::linear_color_stop(alpha(p.primary, 0.0), 1.0),
            ))
            .px(px(24.0))
            .pt(px(24.0))
            .pb(px(4.0))
            .when_some(server.as_ref(), |el, server| {
                el.child(motion::rise(
                    div().relative().flex_none().child(server_icon(server, 48.0, 15.4, p)).child(
                        div()
                            .absolute()
                            .right(px(-6.0))
                            .bottom(px(-6.0))
                            .size(px(24.0))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(p.primary)
                            .text_color(p.primary_foreground)
                            .border_2()
                            .border_color(p.card)
                            .child(icon("scroll-text").size(px(14.0))),
                    ),
                    "rules-icon",
                    Duration::ZERO,
                    -8.0,
                ))
            })
            .child(div().flex_1().min_w_0().pt(px(4.0)).child(dialog_header(
                tracked(title, TIGHT).wraps(),
                Some(note.into_any_element()),
                p,
            )));
        let muted = alpha(p.muted, 0.5);
        let content: AnyElement = match &self.rules {
            None if self.dialog_error.is_some() => div()
                .text_sm()
                .text_color(p.muted_foreground)
                .child(self.dialog_error.clone().unwrap_or_default())
                .into_any_element(),
            None => div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .children(
                    [1.0, 0.8, 0.9]
                        .into_iter()
                        .map(|w| div().h(px(48.0)).w(gpui_kit::relative(w)).rounded(radius_2xl()).bg(p.muted)),
                )
                .into_any_element(),
            Some(rules) if rules.is_empty() => div()
                .rounded(radius_2xl())
                .bg(muted)
                .p(px(12.0))
                .text_sm()
                .text_color(p.muted_foreground)
                .child(t(if agreeing { "join.rules.noneTalk" } else { "join.rules.none" }))
                .into_any_element(),
            Some(rules) => rules_list(rules, 360.0, p, window, cx).into_any_element(),
        };
        let has_rules = self.rules.as_ref().is_some_and(|r| !r.is_empty());
        let mut col = div().flex().flex_col().gap(px(16.0)).child(band).child(content);
        if agreeing && has_rules {
            col = col.child(self.agree_and_talk(p, window, cx));
        }
        if !agreeing || self.rules.as_ref().is_some_and(|r| r.is_empty()) {
            col = col.child(
                button(
                    "rules-close",
                    t(if agreeing { "join.rules.startTalking" } else { "common.close" }),
                    None,
                    Look::Outline,
                    false,
                    p,
                )
                .h(px(40.0))
                .w_full()
                .rounded(radius_xl())
                .font_weight(FontWeight::BOLD)
                .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
            );
        }
        dialog_card(false, p).child(col)
    }

    /// "I agree" and the button that agrees and lets you talk; pressed before
    /// the box is ticked, it all shakes.
    fn agree_and_talk(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let checked = self.web_dialogs.rules_checked;
        let busy = self.dialog_busy;
        let check = agree_check("rules-agree", checked, p).on_click(cx.listener(|this, _, _, cx| {
            this.web_dialogs.rules_checked = !this.web_dialogs.rules_checked;
            cx.notify();
        }));
        let go = button(
            "dialog-ok",
            t("join.rules.agreeAndTalk"),
            Some(if busy { "loader-circle" } else { "party-popper" }),
            Look::Primary,
            false,
            p,
        )
        .h(px(44.0))
        .w_full()
        .rounded(radius_xl())
        .font_weight(FontWeight::BOLD)
        .when(!checked || busy, |el| el.opacity(0.6))
        .on_click(cx.listener(|this, _, _, cx| {
            if this.web_dialogs.rules_checked {
                this.agree_now(cx);
            } else {
                this.web_dialogs.rules_nudge = Some(Instant::now());
                cx.notify();
            }
        }));
        let body = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(check)
            .when_some(self.dialog_error.clone(), |el, e| el.child(div().text_sm().text_color(p.destructive).child(e)))
            .child(go);
        let _ = window;
        match self.web_dialogs.rules_nudge {
            // x: 0, -8, 8, -5, 5, 0 over 0.4s.
            Some(at) => motion::once(
                body,
                SharedString::from(format!("rules-nudge-{at:?}")),
                Duration::from_millis(400),
                |el, t| {
                    let keys = [0.0, -8.0, 8.0, -5.0, 5.0, 0.0];
                    let at = t * 5.0;
                    let i = (at.floor() as usize).min(4);
                    let x = keys[i] + (keys[i + 1] - keys[i]) * (at - i as f32);
                    el.relative().left(px(x))
                },
            ),
            None => body.into_any_element(),
        }
    }

    // ───────────────────────── The welcome screen ─────────────────────────

    /// A server's welcome screen (`join/Welcome.tsx`): its banner with the icon
    /// and name over it, a few words, the channels it suggests two to a row,
    /// and either "I'll look around myself" or, for a newcomer who hasn't
    /// agreed yet, the rules and "I agree".
    pub(crate) fn render_welcome_web(
        &mut self,
        key: &str,
        server_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let (server, channels, look, pending) = self.core.shared.read(|s| {
            let i = s.instance(key);
            (
                i.and_then(|i| i.server(server_id)).cloned(),
                i.and_then(|i| i.channels.get(server_id)).cloned().unwrap_or_default(),
                i.map(|i| crate::ui::mentions::Look::of(i, server_id)).unwrap_or_default(),
                i.is_some_and(|i| i.access(server_id).pending),
            )
        });
        let Some(server) = server else { return div().into_any_element() };
        let agreeing = pending && server.has_rules;
        let tint = crate::ui::banner::accent(&server);
        const WIDTH: f32 = 672.0;
        let hero = crate::ui::join::banner_hero_wide(
            &server,
            &t("join.welcome.eyebrow"),
            Some(div().text_sm().child("👋").into_any_element()),
            WIDTH - 2.0,
            &p,
            window,
            cx,
        );
        let mut body = div().flex().flex_col().px(px(24.0)).pb(px(20.0));
        match &self.welcome {
            None => {
                body = body.child(
                    div()
                        .mt(px(16.0))
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .when_some(self.dialog_error.clone(), |el, e| el.child(e)),
                )
            }
            Some(screen) => {
                if !screen.description.trim().is_empty() {
                    let shown = crate::ui::mentions::mention_links(
                        &crate::ui::text::images_as_links(&screen.description),
                        &look,
                    );
                    body = body.child(motion::rise(
                        div().mt(px(4.0)).w_full().min_w_0().text_sm().text_color(p.muted_foreground).child(
                            crate::ui::text::markdown("welcome-description", shown)
                                .markdown_extensions(crate::ui::emoji::markdown_extensions()),
                        ),
                        "welcome-description",
                        Duration::from_millis(180),
                        6.0,
                    ));
                }
                let suggested: Vec<_> = screen
                    .channels
                    .iter()
                    .filter_map(|w| channels.iter().find(|c| c.id == w.channel_id).map(|c| (w.clone(), c.clone())))
                    .collect();
                if !suggested.is_empty() {
                    let cell = (WIDTH - 2.0 - 48.0 - 8.0) / 2.0;
                    let mut grid = div().flex().flex_wrap().gap(px(8.0));
                    for (n, (w, channel)) in suggested.into_iter().enumerate() {
                        let (hover_bg, hover_border) = (Hsla { a: 0.08, ..tint }, Hsla { a: 0.55, ..tint });
                        let (k, sid, cid) = (key.to_owned(), server_id.to_owned(), channel.id.clone());
                        let fallback =
                            if channel.r#type == pb::ChannelType::Announcement as i32 { "megaphone" } else { "hash" };
                        let group = SharedString::from(format!("welcome-{}", channel.id));
                        // The tile grows and tips as its card's hovered (`group-hover:scale-110 -rotate-6`).
                        let lead = div()
                            .id(SharedString::from(format!("{group}|tile")))
                            .when(!agreeing, |el| {
                                el.group_hover(group.clone(), |s| {
                                    s.scale(1.1).rotate(gpui_kit::radians(-6f32.to_radians()))
                                })
                            })
                            .size(px(40.0))
                            .flex_none()
                            .rounded(radius_xl())
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(Hsla { a: 0.16, ..tint })
                            .child(emoji_glyph(&w.emoji, &look, tint, fallback));
                        let pick = !agreeing;
                        grid = grid.child(motion::rise(
                            div()
                                .id(group.clone())
                                .group(group.clone())
                                .w(px(cell))
                                .flex()
                                .items_center()
                                .gap(px(12.0))
                                .p(px(12.0))
                                .rounded(radius_2xl())
                                .border_1()
                                .border_color(p.border)
                                .bg(alpha(p.background, 0.6))
                                .when(pick, |el| {
                                    el.cursor_pointer()
                                        .hover(move |s| s.bg(hover_bg).border_color(hover_border).translate_y(px(-3.0)))
                                        .active(|s| s.scale(0.97))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.dialog = None;
                                            this.open_channel(&k, &sid, &cid, window, cx);
                                        }))
                                })
                                .child(lead)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .truncate()
                                                .text_sm()
                                                .line_height(px(20.0))
                                                .font_weight(FontWeight::BOLD)
                                                .child(format!("#{}", channel.name)),
                                        )
                                        .when(!w.description.is_empty(), |el| {
                                            el.child(
                                                div()
                                                    .truncate()
                                                    .text_xs()
                                                    .line_height(px(16.0))
                                                    .text_color(p.muted_foreground)
                                                    .child(w.description.clone()),
                                            )
                                        }),
                                )
                                .when(pick, |el| {
                                    // The arrow leans on and takes the server's color.
                                    el.child(
                                        div()
                                            .id(SharedString::from(format!("{group}|arrow")))
                                            .flex_none()
                                            .text_color(p.muted_foreground)
                                            .group_hover(group.clone(), move |s| {
                                                s.translate_x(px(4.0)).text_color(tint)
                                            })
                                            .child(icon("arrow-right").size(px(16.0))),
                                    )
                                }),
                            SharedString::from(format!("welcome-in-{n}")),
                            Duration::from_millis(220 + 60 * n as u64),
                            16.0,
                        ));
                    }
                    body = body.child(
                        div()
                            .mt(px(16.0))
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(11.2))
                                    .line_height(px(16.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(p.muted_foreground)
                                    .child(tracked(t("join.startHere").to_uppercase(), WIDE)),
                            )
                            .child(grid),
                    );
                }
            }
        }
        if agreeing {
            let rules = self.rules.clone();
            let mut section = div().mt(px(20.0)).flex().flex_col().gap(px(12.0)).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_size(px(11.2))
                    .line_height(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .child(icon("scroll-text").size(px(14.0)))
                    .child(tracked(t("join.welcome.beforeYouTalk").to_uppercase(), WIDE)),
            );
            section = section.child(match rules {
                None => div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .children(
                        [1.0, 0.8]
                            .into_iter()
                            .map(|w| div().h(px(48.0)).w(gpui_kit::relative(w)).rounded(radius_2xl()).bg(p.muted)),
                    )
                    .into_any_element(),
                Some(rules) => rules_list(&rules, 224.0, &p, window, cx).into_any_element(),
            });
            section = section.child(self.agree_and_talk(&p, window, cx));
            body = body.child(motion::rise(section, "welcome-rules", Duration::from_millis(300), 12.0));
        } else {
            let fg = p.foreground;
            body = body.child(
                div().mt(px(20.0)).flex().justify_center().child(
                    div()
                        .id("welcome-skip")
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .cursor_pointer()
                        .hover(move |s| s.text_color(fg))
                        .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx)))
                        .child(t("join.welcome.lookAround")),
                ),
            );
        }
        let panel = div()
            .relative()
            .w(px(WIDTH))
            .rounded(crate::ui::theme::radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .text_color(p.foreground)
            .shadow(crate::ui::overlay::shadow_2xl())
            .child(
                div()
                    .id("welcome-body")
                    .max_h(px(736.0))
                    .overflow_y_scroll()
                    .child(hero)
                    .child(div().mt(px(0.0)).child(body)),
            )
            .child(dialog_close("dialog-close", &p).on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))));
        dialog_layer("welcome", panel, &p, cx.listener(|this, _, _, cx| this.close_dialog(cx)), cx)
    }

    /// Agrees to the open server's rules (from the rules or the welcome
    /// screen), then says welcome and closes.
    fn agree_now(&mut self, cx: &mut Context<Self>) {
        let (key, server) = match &self.dialog {
            Some(Dialog::Rules { key, server } | Dialog::Welcome { key, server }) => (key.clone(), server.clone()),
            _ => return,
        };
        if self.dialog_busy {
            return;
        }
        self.dialog_busy = true;
        self.dialog_error = None;
        let name = self
            .core
            .shared
            .read(|s| s.instance(&key).and_then(|i| i.server(&server)).map(|s| s.name.clone()))
            .unwrap_or_default();
        let core = self.core.clone();
        self.run(cx, async move { core.agree_to_rules(&key, &server).await }, move |this, result, cx| {
            this.dialog_busy = false;
            match result {
                Ok(()) => {
                    this.dialog = None;
                    let text = t_with("join.rules.welcomeToast", &[("server", Arg::Str(&name))]);
                    this.toast("party-popper", text, String::new(), None, None, cx);
                }
                Err(err) => this.dialog_error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    // ───────────────────────── Asking before ─────────────────────────

    /// What a menu asks before something that can't be undone, as the web's
    /// confirm dialog: a warning tile beside the title, the line under it,
    /// Cancel and the red action.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn confirm_panel(
        &self,
        title: String,
        body: String,
        action: String,
        p: &Palette,
        cancel: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
        cancel_x: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
        go: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
    ) -> gpui_kit::Div {
        let title_row = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(motion::rise(
                div()
                    .size(px(36.0))
                    .flex_none()
                    .rounded(radius_xl())
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(alpha(p.destructive, 0.15))
                    .text_color(p.destructive)
                    .child(icon("triangle-alert").size(px(20.0))),
                "confirm-tile",
                Duration::ZERO,
                -6.0,
            ))
            .child(div().flex_1().min_w_0().child(tracked(title, TIGHT).wraps()));
        dialog_card(false, p)
            .child(dialog_header(title_row, Some(body.into_any_element()), p))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .child(dialog_button("confirm-cancel", t("common.cancel"), Look::Ghost, p).on_click(cancel))
                    .child(dialog_button("confirm-go", action, Look::Destructive, p).on_click(go)),
            )
            .child(dialog_close("confirm-close", p).on_click(cancel_x))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_names_are_made_like_the_web_makes_them() {
        assert_eq!(slug("Cozy  Corner!"), "cozy-corner");
        assert_eq!(channel_request(pb::ChannelType::Text, "My Room", "cat"), ("my-room".into(), "cat".into()));
        assert_eq!(channel_request(pb::ChannelType::Voice, " The Lounge ", "cat"), ("The Lounge".into(), "cat".into()));
        assert_eq!(channel_request(pb::ChannelType::Category, "Fun", "cat"), ("Fun".into(), String::new()));
    }
}
