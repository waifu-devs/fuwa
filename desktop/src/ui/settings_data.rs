//! Data and privacy for the instance on screen, as the web's
//! `settings/account/Privacy.tsx`: whether people see what you're doing
//! (and the desktop's own say over which games may show), downloading
//! everything the instance keeps about you, and deleting the account.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::pb;
use crate::ui::motion;
use crate::ui::settings::SettingsView;
use crate::ui::settings_controls::{Look, button, field, toggle, warn};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_3xl, radius_xl};
use crate::ui::widgets::{icon, server_icon};

#[derive(Default)]
pub(crate) enum Export {
    #[default]
    Idle,
    Working,
    Done {
        bytes: u64,
        path: PathBuf,
    },
}

#[derive(Default)]
pub(crate) struct DataForm {
    export: Export,
    deleting: bool,
    two_step: bool,
    pending: bool,
    error: Option<String>,
    shake: Option<std::time::Instant>,
    password: Option<Entity<InputState>>,
    code: Option<Entity<InputState>>,
    username: Option<Entity<InputState>>,
}

/// "12.3 MB", as the web's `formatBytes`.
pub(crate) fn bytes(n: u64) -> String {
    let n = n as f64;
    if n < 1024.0 {
        format!("{n} B")
    } else if n < 1024.0 * 1024.0 {
        format!("{:.1} KB", n / 1024.0)
    } else if n < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", n / 1024.0 / 1024.0)
    } else {
        format!("{:.1} GB", n / 1024.0 / 1024.0 / 1024.0)
    }
}

fn input(window: &mut Window, cx: &mut Context<SettingsView>, masked: bool, placeholder: &str) -> Entity<InputState> {
    let placeholder = placeholder.to_owned();
    let state = cx.new(|cx| InputState::new(window, cx).masked(masked).placeholder(placeholder));
    cx.subscribe(&state, |_, _, _: &InputEvent, cx| cx.notify()).detach();
    state
}

impl SettingsView {
    pub(crate) fn privacy_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some((key, me)) = self.account_me() else { return div().into_any_element() };
        let place = self.place(&key);
        let owned: Vec<pb::Server> = self.core.shared.read(|s| {
            s.instance(&key)
                .map(|i| i.servers.iter().filter(|s| s.owner_id == me.id).cloned().collect())
                .unwrap_or_default()
        });
        let mut page = div().flex().flex_col().gap(px(32.0));
        if let Some(activity) = self.activity_section(&key, p, window, cx) {
            page = page.child(motion::rise(div().child(activity), "privacy-activity", Duration::ZERO, 12.0));
        }
        page = page.child(self.games_card(p, window, cx));
        page = page.child(self.export_card(&key, &place, p, window, cx));
        // Deleting.
        let blocked = !owned.is_empty();
        let delete = div()
            .rounded(radius_3xl())
            .border_1()
            .border_color(alpha(p.destructive, 0.3))
            .bg(alpha(p.destructive, 0.05))
            .p(px(20.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.0))
                    .child(
                        div()
                            .size(px(48.0))
                            .flex_none()
                            .rounded(radius_2xl())
                            .bg(alpha(p.destructive, 0.15))
                            .text_color(p.destructive)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon("trash").size(px(24.0))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("settings.nav.deleteAccount")))
                            .child(div().text_sm().text_color(p.muted_foreground).child(t_with(
                                "accountsettings.privacy.deleteHint",
                                &[("instance", Arg::Str(&place))],
                            ))),
                    )
                    .child(
                        button(
                            "delete-account",
                            t("accountsettings.privacy.deleteButton"),
                            None,
                            Look::Destructive,
                            false,
                            p,
                        )
                        .rounded(radius_xl())
                        .font_weight(FontWeight::BOLD)
                        .when(blocked, |el| el.opacity(0.5))
                        .when(!blocked, |el| {
                            el.on_click(cx.listener(|this, _, window, cx| this.open_delete(window, cx)))
                        }),
                    ),
            )
            .when(blocked, |el| {
                el.child(
                    div()
                        .mt(px(16.0))
                        .rounded(radius_2xl())
                        .bg(alpha(p.background, 0.6))
                        .p(px(12.0))
                        .child(
                            div()
                                .mb(px(8.0))
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child(icon("crown").size(px(16.0)).text_color(rgb(0xfbbf24)))
                                .child(t_with(
                                    "accountsettings.privacy.owned",
                                    &[("count", Arg::Num(owned.len() as i64))],
                                )),
                        )
                        .child(div().flex().flex_wrap().gap(px(8.0)).children(owned.iter().map(|s| {
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .rounded(radius_xl())
                                .border_1()
                                .border_color(p.border)
                                .bg(p.card)
                                .py(px(4.0))
                                .pl(px(4.0))
                                .pr(px(12.0))
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child(server_icon(s, 24.0, 8.0, p))
                                .child(s.name.clone())
                        }))),
                )
            });
        let delete = motion::rise(div().child(delete), "privacy-delete", Duration::from_millis(60), 12.0);
        page = page.child(self.found_mark("delete-account", div().child(delete), p));
        if self.data.deleting {
            let el = self.delete_dialog(&key, &place, &me, p, cx);
            self.state.overlay = Some(el);
        }
        page.into_any_element()
    }

    /// The desktop's own part of "what I'm doing": whether games may show at all, and each one's answer.
    fn games_card(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let prefs = self.core.prefs();
        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .rounded(radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .p(px(20.0))
            .child(toggle(
                "game-activity",
                &t("desktop.privacy.gameActivity"),
                Some(&t("desktop.privacy.gameActivityHint")),
                prefs.game_activity,
                false,
                p,
                window,
                cx,
                |this, on, cx| this.set(cx, |pr| pr.game_activity = on),
            ));
        for (n, (key, allowed)) in prefs.game_answers.iter().enumerate() {
            let name = match key.split_once(':') {
                Some(("program", name)) => name.to_owned(),
                Some((_, id)) => t_with("desktop.privacy.game", &[("id", Arg::Str(id))]),
                None => key.clone(),
            };
            let key = key.clone();
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .rounded(radius_xl())
                    .px(px(8.0))
                    .py(px(6.0))
                    .child(icon("gamepad-2").size(px(16.0)).text_color(if *allowed {
                        p.primary
                    } else {
                        p.muted_foreground
                    }))
                    .child(div().flex_1().min_w_0().truncate().text_sm().font_weight(FontWeight::BOLD).child(name))
                    .child(div().text_xs().text_color(p.muted_foreground).child(if *allowed {
                        t("desktop.privacy.allowed")
                    } else {
                        t("desktop.privacy.notAllowed")
                    }))
                    .child(
                        button(
                            SharedString::from(format!("forget-game-{n}")),
                            t("desktop.privacy.askAgain"),
                            None,
                            Look::Ghost,
                            true,
                            p,
                        )
                        .rounded(radius_xl())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let key = key.clone();
                            this.set(cx, move |pr| {
                                pr.game_answers.remove(&key);
                            })
                        })),
                    ),
            );
        }
        motion::rise(card, "privacy-games", Duration::from_millis(30), 12.0).into_any_element()
    }

    fn export_card(
        &mut self,
        key: &str,
        place: &str,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let working = matches!(self.data.export, Export::Working);
        let done = match &self.data.export {
            Export::Done { bytes, path } => Some((*bytes, path.clone())),
            _ => None,
        };
        let k = key.to_owned();
        let place_name = place.to_owned();
        let action: AnyElement = match &done {
            Some((_, path)) => {
                let path = path.clone();
                button(
                    "export-again",
                    t("accountsettings.privacy.saveAgain"),
                    Some("folder-open"),
                    Look::Outline,
                    false,
                    p,
                )
                .rounded(radius_xl())
                .on_click(move |_, _, cx| {
                    if let Some(dir) = path.parent() {
                        cx.open_url(&format!("file://{}", dir.display()));
                    }
                })
                .into_any_element()
            }
            None => button(
                "export-start",
                if working { t("accountsettings.privacy.gathering") } else { t("accountsettings.shared.download") },
                Some("download"),
                Look::Primary,
                false,
                p,
            )
            .rounded(radius_xl())
            .px(px(20.0))
            .font_weight(FontWeight::BOLD)
            .when(working, |el| el.opacity(0.6))
            .when(!working, |el| el.on_click(cx.listener(move |this, _, _, cx| this.start_export(&k, &place_name, cx))))
            .into_any_element(),
        };
        let bar = (!matches!(self.data.export, Export::Idle)).then(|| {
            let track = div().relative().flex_1().h(px(8.0)).overflow_hidden().rounded_full().bg(p.muted);
            let track = if working {
                track
                    .child(motion::ambient(
                        div().absolute().top_0().bottom_0().w(gpui_kit::relative(0.33)).rounded_full().bg(p.primary),
                        "export-bar",
                        Duration::from_millis(1100),
                        window,
                        |el, t| el.left(gpui_kit::relative(-0.33 + 1.33 * t)),
                    ))
                    .into_any_element()
            } else {
                track.child(div().absolute().inset_0().rounded_full().bg(rgb(0x10b981))).into_any_element()
            };
            motion::rise(
                div().mt(px(16.0)).flex().items_center().gap(px(12.0)).child(track).child(
                    div()
                        .w(px(112.0))
                        .text_right()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child(match &done {
                            Some((n, _)) => t_with("accountsettings.privacy.ready", &[("size", Arg::Str(&bytes(*n)))]),
                            None => String::new(),
                        }),
                ),
                "export-bar-in",
                Duration::ZERO,
                -6.0,
            )
        });
        let card =
            div()
                .rounded(radius_3xl())
                .border_1()
                .border_color(p.border)
                .bg(p.card)
                .p(px(20.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(16.0))
                        .child(
                            div()
                                .size(px(48.0))
                                .flex_none()
                                .rounded(radius_2xl())
                                .bg(alpha(p.primary, 0.15))
                                .text_color(p.primary)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon(if done.is_some() { "check" } else { "file-braces" }).size(px(24.0))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("settings.nav.export")))
                                .child(div().text_sm().text_color(p.muted_foreground).child(t_with(
                                    "accountsettings.privacy.exportHint",
                                    &[("instance", Arg::Str(place))],
                                ))),
                        )
                        .child(action),
                )
                .children(bar);
        let card = motion::rise(div().child(card), "privacy-export", Duration::from_millis(40), 12.0);
        self.found_mark("export", div().child(card), p)
    }

    fn start_export(&mut self, key: &str, place: &str, cx: &mut Context<Self>) {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let slug: String = place
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c.to_ascii_lowercase() } else { '-' })
            .collect();
        let name = format!("fuwa-{slug}-{date}.json");
        let dir = dirs::download_dir().or_else(dirs::home_dir).unwrap_or_default();
        let path = cx.prompt_for_new_path(&dir, Some(&name));
        let (core, key) = (self.core.clone(), key.to_owned());
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = path.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.data.export = Export::Working;
                cx.notify();
            });
            let target = path.clone();
            let rx = core.spawn({
                let core = core.clone();
                async move { core.export_data(&key, &target, |_| {}).await }
            });
            let result = rx.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Ok(n)) => this.data.export = Export::Done { bytes: n, path },
                    Ok(Err(e)) => {
                        this.data.export = Export::Idle;
                        this.toast("circle-alert", e.message, cx);
                    }
                    Err(_) => this.data.export = Export::Idle,
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.data.deleting = true;
        self.data.error = None;
        self.data.password = Some(input(window, cx, true, ""));
        self.data.code = Some(input(window, cx, false, "123456"));
        self.data.username = Some(input(window, cx, false, ""));
        self.data.two_step = false;
        let local = self.account_me().is_some_and(|(_, me)| me.kind == pb::AccountKind::Local as i32);
        if local && let Some(key) = self.account_key() {
            let core = self.core.clone();
            let rx = self.core.spawn(async move { core.two_factor(&key).await });
            cx.spawn(async move |this, cx| {
                let Ok(Ok(res)) = rx.await else { return };
                let _ = this.update(cx, |this, cx| {
                    this.data.two_step = res.enabled;
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn delete_dialog(
        &mut self,
        key: &str,
        place: &str,
        me: &pb::User,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let standalone = me.kind == pb::AccountKind::Local as i32;
        let read = |s: &Option<Entity<InputState>>, cx: &Context<Self>| {
            s.as_ref().map(|s| s.read(cx).value().to_string()).unwrap_or_default()
        };
        let (password, code, username) =
            (read(&self.data.password, cx), read(&self.data.code, cx), read(&self.data.username, cx));
        let ready = if standalone {
            !password.is_empty() && (!self.data.two_step || code.trim().len() >= 6)
        } else {
            username.trim().to_lowercase() == me.username
        };
        let pending = self.data.pending;
        let lines = [
            t("accountsettings.privacy.lineProfile"),
            t("accountsettings.privacy.lineMessages"),
            t("accountsettings.privacy.lineServers"),
        ];
        let label =
            |text: String| div().text_xs().font_weight(FontWeight::BOLD).text_color(p.muted_foreground).child(text);
        let mut fields = div().flex().flex_col().gap(px(12.0));
        if standalone {
            if let Some(s) = &self.data.password {
                fields = fields.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(label(t("accountsettings.shared.yourPassword")))
                        .child(field(Input::new(s).appearance(false), p)),
                );
            }
            if self.data.two_step
                && let Some(s) = &self.data.code
            {
                fields = fields.child(motion::rise(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(label(t("accountsettings.shared.codeOrBackup")))
                        .child(field(Input::new(s).appearance(false), p)),
                    "delete-code-in",
                    Duration::ZERO,
                    -6.0,
                ));
            }
        } else if let Some(s) = &self.data.username {
            let name =
                if self.core.prefs().hides_personal() { "••••••".to_owned() } else { me.username.clone() };
            fields = fields.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(label(t_with("accountsettings.privacy.typeUsername", &[("username", Arg::Str(&name))])))
                    .child(field(Input::new(s).appearance(false), p)),
            );
        }
        let fields: AnyElement = match self.data.shake {
            Some(at) => motion::once(
                fields,
                SharedString::from(format!("delete-shake-{at:?}")),
                Duration::from_millis(400),
                |el, t| el.relative().left(px((t * std::f32::consts::TAU * 2.5).sin() * 8.0 * (1.0 - t))),
            ),
            None => fields.into_any_element(),
        };
        let k = key.to_owned();
        let gone = place.to_owned();
        let panel = div()
            .id("delete-panel")
            .w(px(512.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .rounded(radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .p(px(24.0))
            .shadow(crate::ui::settings_controls::shadow_xl())
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(t("accountsettings.privacy.dialogTitle")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(p.muted_foreground)
                            .child(t_with("accountsettings.privacy.dialogHint", &[("instance", Arg::Str(place))])),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .rounded(radius_2xl())
                    .bg(alpha(p.muted, 0.6))
                    .p(px(12.0))
                    .text_sm()
                    .children(lines.into_iter().enumerate().map(|(n, line)| {
                        motion::rise(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .child(icon("triangle-alert").size(px(16.0)).mt(px(2.0)).text_color(p.destructive))
                                .child(div().flex_1().child(line)),
                            SharedString::from(format!("delete-line-{n}")),
                            Duration::from_millis(100 + 50 * n as u64),
                            0.0,
                        )
                    })),
            )
            .child(fields)
            .when_some(self.data.error.clone(), |el, e| el.child(warn(e, p)))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        button("delete-keep", t("accountsettings.privacy.keep"), None, Look::Ghost, false, p)
                            .rounded(radius_xl())
                            .when(!pending, |el| {
                                el.on_click(cx.listener(|this, _, _, cx| {
                                    this.data.deleting = false;
                                    cx.notify();
                                }))
                            }),
                    )
                    .child(
                        button(
                            "delete-forever",
                            if pending {
                                t("accountsettings.privacy.deleting")
                            } else {
                                t("accountsettings.privacy.deleteForever")
                            },
                            None,
                            Look::Destructive,
                            false,
                            p,
                        )
                        .rounded(radius_xl())
                        .font_weight(FontWeight::BOLD)
                        .when(!ready, |el| el.opacity(0.6))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if pending {
                                return;
                            }
                            if !ready {
                                this.data.shake = Some(std::time::Instant::now());
                                cx.notify();
                                return;
                            }
                            this.data.pending = true;
                            this.data.error = None;
                            let (core, key) = (this.core.clone(), k.clone());
                            let (pw, c, u) = if standalone {
                                (password.clone(), code.trim().to_owned(), String::new())
                            } else {
                                (String::new(), String::new(), username.trim().to_owned())
                            };
                            let gone = gone.clone();
                            let rx = this.core.spawn({
                                let core = core.clone();
                                let key = key.clone();
                                async move { core.delete_account(&key, &pw, &c, &u).await }
                            });
                            cx.spawn(async move |this, cx| {
                                let Ok(result) = rx.await else { return };
                                let _ = this.update(cx, |this, cx| {
                                    this.data.pending = false;
                                    match result {
                                        Ok(()) => {
                                            this.data.deleting = false;
                                            this.core.remove_instance(&key);
                                            this.toast(
                                                "trash",
                                                t_with(
                                                    "accountsettings.privacy.gone",
                                                    &[("instance", Arg::Str(&gone))],
                                                ),
                                                cx,
                                            );
                                            this.close(cx);
                                        }
                                        Err(e) => {
                                            this.data.error = Some(e.message);
                                            this.data.shake = Some(std::time::Instant::now());
                                        }
                                    }
                                    cx.notify();
                                });
                            })
                            .detach();
                            cx.notify();
                        })),
                    ),
            );
        dialog(panel.into_any_element(), "delete-dialog", p, cx, |this, cx| {
            if !this.data.pending {
                this.data.deleting = false;
                cx.notify();
            }
        })
    }
}

/// A dialog over the whole settings screen: a dimmed scrim (a click on it closes) with the panel
/// rising in, the web's `Dialog`. Pages hand it to the screen through `PageState::overlay`.
pub(crate) fn dialog(
    panel: AnyElement,
    id: &'static str,
    p: &Palette,
    cx: &mut Context<SettingsView>,
    close: impl Fn(&mut SettingsView, &mut Context<SettingsView>) + 'static,
) -> AnyElement {
    motion::fade_in(
        div()
            .id(id)
            .absolute()
            .inset_0()
            .occlude()
            .bg(gpui_kit::hsla(0.0, 0.0, 0.0, if p.dark { 0.6 } else { 0.4 }))
            .flex()
            .items_center()
            .justify_center()
            .on_click(cx.listener(move |this, _, _, cx| close(this, cx)))
            .child(crate::ui::overlay::roomy(
                "dialog-room",
                motion::rise(div().child(panel), SharedString::from(format!("{id}-panel")), Duration::ZERO, 16.0),
            )),
        SharedString::from(format!("{id}-fade")),
        Duration::from_millis(160),
    )
    .into_any_element()
}
