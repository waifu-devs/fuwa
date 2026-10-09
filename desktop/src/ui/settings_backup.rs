//! The message backup on the Devices page, as the web's
//! `settings/account/MessageBackup.tsx`: off (set it up), waiting for its
//! recovery key (restore, or start over), restoring, or on (how much room it
//! uses, a new key, turning it off), and the new key shown once to save.

use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, Entity, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb,
};

use crate::core::backup::{self, Status};
use crate::core::i18n::{Arg, t, t_with};
use crate::ui::motion;
use crate::ui::settings::SettingsView;
use crate::ui::settings_controls::{IconHover, Look, button, button_with, caps, field};
use crate::ui::settings_data::bytes;
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_xl};
use crate::ui::widgets::icon;

#[derive(Default)]
pub(crate) struct BackupForm {
    recovery_key: Option<String>,
    saved: bool,
    copied: bool,
    busy: bool,
    asking: Option<&'static str>,
    text: Option<Entity<InputState>>,
}

impl SettingsView {
    fn backup_act(&mut self, key: &str, what: &'static str, cx: &mut Context<Self>) {
        let Ok(engine) = backup::engine(&self.core, key) else { return };
        self.backup.busy = true;
        self.backup.asking = None;
        let text = self.backup.text.as_ref().map(|s| s.read(cx).value().to_string()).unwrap_or_default();
        let rx = self.core.spawn(async move {
            match what {
                "create" => engine.backup_create(false).await.map(Some),
                "replace" => engine.backup_create(true).await.map(Some),
                "restore" => engine.backup_restore(&text).await.map(|_| None),
                _ => engine.backup_remove().await.map(|_| None),
            }
        });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.backup.busy = false;
                match result {
                    Ok(Some(key)) => {
                        this.backup.recovery_key = Some(key);
                        this.backup.saved = false;
                    }
                    Ok(None) => {}
                    Err(e) => this.toast("circle-alert", e.message, cx),
                }
                cx.notify();
            });
        })
        .detach();
        // Restoring moves its bar along: look again while it runs.
        cx.spawn(async move |this, cx| {
            for _ in 0..600 {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                let Ok(done) = this.update(cx, |this, cx| {
                    cx.notify();
                    !this.backup.busy
                }) else {
                    return;
                };
                if done {
                    return;
                }
            }
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn message_backup(
        &mut self,
        key: &str,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = backup::state(key);
        if matches!(state.status, Status::Unsupported) || backup::engine(&self.core, key).is_err() {
            return div().into_any_element();
        }
        let card = |glyph: &'static str, title: String, tone: (Hsla, Hsla), body: gpui_kit::Div| {
            div()
                .flex()
                .gap(px(12.0))
                .rounded(radius_2xl())
                .border_1()
                .border_color(p.border)
                .bg(p.card)
                .p(px(16.0))
                .child(
                    div()
                        .size(px(40.0))
                        .flex_none()
                        .rounded(radius_xl())
                        .bg(tone.0)
                        .text_color(tone.1)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(glyph).size(px(20.0))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(div().mb(px(4.0)).text_sm().font_weight(FontWeight::BOLD).child(title))
                        .child(body),
                )
        };
        let muted = (p.muted.into(), p.muted_foreground.into());
        let primary = (alpha(p.primary, 0.15), p.primary.into());
        let ok = (alpha(rgb(0x10b981), 0.15), rgb(0x10b981).into());
        let warn_tone = (alpha(rgb(0xf59e0b), 0.15), rgb(0xd97706).into());
        let busy = self.backup.busy;
        let k = key.to_owned();
        let ask = |this: &Self, question: String, confirm: String, what: &'static str, cx: &mut Context<Self>| {
            let k = k.clone();
            div()
                .mt(px(12.0))
                .rounded(radius_xl())
                .border_1()
                .border_color(alpha(p.destructive, 0.3))
                .bg(alpha(p.destructive, 0.05))
                .p(px(12.0))
                .child(div().text_sm().child(question))
                .child(
                    div()
                        .mt(px(8.0))
                        .flex()
                        .gap(px(8.0))
                        .child(
                            button("backup-cancel", t("common.cancel"), None, Look::Ghost, true, p)
                                .rounded(radius_xl())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.backup.asking = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            button(
                                "backup-confirm",
                                if this.backup.busy { t("accountsettings.shared.working") } else { confirm },
                                None,
                                Look::Destructive,
                                true,
                                p,
                            )
                            .rounded(radius_xl())
                            .font_weight(FontWeight::BOLD)
                            .on_click(cx.listener(move |this, _, _, cx| this.backup_act(&k, what, cx))),
                        ),
                )
        };
        let body: AnyElement = if let Some(recovery) = self.backup.recovery_key.clone() {
            let (copy_text, file) = (
                recovery.clone(),
                format!(
                    "{}\n\n{}\n\n{}\n",
                    t("accountsettings.backup.fileTitle"),
                    recovery,
                    t("accountsettings.backup.fileNote")
                ),
            );
            let copied = self.backup.copied;
            div()
                .rounded(radius_2xl())
                .border_1()
                .border_color(alpha(p.primary, 0.4))
                .bg(alpha(p.primary, 0.05))
                .p(px(16.0))
                .child(div().text_sm().font_weight(FontWeight::BOLD).child(t("accountsettings.backup.save")))
                .child(
                    div()
                        .mt(px(4.0))
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child(t("accountsettings.backup.saveHint")),
                )
                .child(
                    div()
                        .mt(px(12.0))
                        .rounded(radius_xl())
                        .border_1()
                        .border_color(p.border)
                        .bg(p.background)
                        .px(px(12.0))
                        .py(px(10.0))
                        .text_size(px(12.8))
                        .font_family("monospace")
                        .child(recovery.clone()),
                )
                .child(
                    div()
                        .mt(px(12.0))
                        .flex()
                        .flex_wrap()
                        .gap(px(8.0))
                        .child(
                            button(
                                "key-copy",
                                if copied {
                                    t("accountsettings.shared.copied")
                                } else {
                                    t("accountsettings.shared.copy")
                                },
                                Some(if copied { "check" } else { "copy" }),
                                Look::Outline,
                                true,
                                p,
                            )
                            .rounded(radius_xl())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
                                this.backup.copied = true;
                                this.backup.saved = true;
                                cx.notify();
                            })),
                        )
                        .child(
                            button(
                                "key-download",
                                t("accountsettings.shared.download"),
                                Some("download"),
                                Look::Outline,
                                true,
                                p,
                            )
                            .rounded(radius_xl())
                            .on_click(cx.listener(move |_, _, _, cx| {
                                let dir = dirs::download_dir().or_else(dirs::home_dir).unwrap_or_default();
                                let path = cx.prompt_for_new_path(&dir, Some("fuwa-recovery-key.txt"));
                                let file = file.clone();
                                cx.spawn(async move |this, cx| {
                                    let Ok(Ok(Some(path))) = path.await else { return };
                                    if std::fs::write(&path, file).is_ok() {
                                        let _ = this.update(cx, |this, cx| {
                                            this.backup.saved = true;
                                            cx.notify();
                                        });
                                    }
                                })
                                .detach();
                            })),
                        )
                        .child(div().flex_1())
                        .child(
                            button("key-saved", t("accountsettings.shared.savedIt"), None, Look::Primary, true, p)
                                .rounded(radius_xl())
                                .font_weight(FontWeight::BOLD)
                                .when(!self.backup.saved, |el| el.opacity(0.5))
                                .when(self.backup.saved, |el| {
                                    el.on_click(cx.listener(|this, _, _, cx| {
                                        this.backup.recovery_key = None;
                                        this.backup.copied = false;
                                        cx.notify();
                                    }))
                                }),
                        ),
                )
                .into_any_element()
        } else {
            match state.status {
                Status::Loading => div().h(px(96.0)).rounded(radius_2xl()).bg(p.muted).into_any_element(),
                Status::Off => card(
                    "clock-arrow-left",
                    t("accountsettings.backup.off"),
                    muted,
                    div()
                        .child(
                            div().text_sm().text_color(p.muted_foreground).child(t("accountsettings.backup.offHint")),
                        )
                        .child(div().mt(px(12.0)).flex().child({
                            let k = key.to_owned();
                            button(
                                "backup-setup",
                                if busy {
                                    t("accountsettings.backup.settingUp")
                                } else {
                                    t("accountsettings.backup.setUp")
                                },
                                Some("key-round"),
                                Look::Primary,
                                true,
                                p,
                            )
                            .rounded(radius_xl())
                            .font_weight(FontWeight::BOLD)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.backup.busy {
                                    this.backup_act(&k, "create", cx)
                                }
                            }))
                        })),
                )
                .into_any_element(),
                Status::Locked => {
                    let input = match &self.backup.text {
                        Some(s) => s.clone(),
                        None => {
                            let s = cx.new(|cx| InputState::new(window, cx).placeholder("XXXX-XXXX-XXXX-…"));
                            cx.subscribe(&s, |_, _, _: &InputEvent, cx| cx.notify()).detach();
                            self.backup.text = Some(s.clone());
                            s
                        }
                    };
                    let has_text = !input.read(cx).value().trim().is_empty();
                    let k2 = key.to_owned();
                    let mut body = div()
                        .child(
                            div()
                                .text_sm()
                                .text_color(p.muted_foreground)
                                .child(t("accountsettings.backup.lockedHint")),
                        )
                        .child(
                            div()
                                .mt(px(12.0))
                                .flex()
                                .gap(px(8.0))
                                .child(
                                    div().flex_1().min_w_0().child(
                                        field(Input::new(&input).appearance(false), p)
                                            .h(px(36.0))
                                            .text_xs()
                                            .font_family("monospace"),
                                    ),
                                )
                                .child(
                                    button(
                                        "backup-restore",
                                        if busy {
                                            t("accountsettings.backup.restoringShort")
                                        } else {
                                            t("accountsettings.backup.restore")
                                        },
                                        None,
                                        Look::Primary,
                                        true,
                                        p,
                                    )
                                    .rounded(radius_xl())
                                    .font_weight(FontWeight::BOLD)
                                    .when(busy || !has_text, |el| el.opacity(0.5))
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            if !this.backup.busy && has_text {
                                                this.backup_act(&k2, "restore", cx)
                                            }
                                        },
                                    )),
                                ),
                        );
                    body = if self.backup.asking == Some("replace") {
                        body.child(ask(
                            self,
                            t("accountsettings.backup.lostAsk"),
                            t("accountsettings.backup.startOver"),
                            "replace",
                            cx,
                        ))
                    } else {
                        let fg = p.foreground;
                        body.child(
                            div()
                                .id("backup-lost")
                                .mt(px(8.0))
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.muted_foreground)
                                .cursor_pointer()
                                .hover(move |s| s.text_color(fg).underline())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.backup.asking = Some("replace");
                                    cx.notify();
                                }))
                                .child(t("accountsettings.backup.lost")),
                        )
                    };
                    card("lock-keyhole", t("accountsettings.backup.locked"), primary, body).into_any_element()
                }
                Status::Restoring => {
                    let share =
                        if state.total > 0 { (state.restored as f32 / state.total as f32).min(1.0) } else { 0.0 };
                    let fill = motion::follow("backup-restore-bar", share, window, cx);
                    card(
                        "clock-arrow-left",
                        t("accountsettings.backup.restoring"),
                        primary,
                        div()
                            .child(
                                div()
                                    .mt(px(4.0))
                                    .h(px(8.0))
                                    .overflow_hidden()
                                    .rounded_full()
                                    .bg(p.muted)
                                    .child(div().h_full().w(gpui_kit::relative(fill)).rounded_full().bg(p.primary)),
                            )
                            .child(div().mt(px(8.0)).text_xs().text_color(p.muted_foreground).child(t_with(
                                "accountsettings.backup.parts",
                                &[("restored", Arg::Num(state.restored)), ("count", Arg::Num(state.total))],
                            ))),
                    )
                    .into_any_element()
                }
                _ => {
                    let full = state.status == Status::Full;
                    let share =
                        if state.max_size > 0 { (state.size as f32 / state.max_size as f32).min(1.0) } else { 0.0 };
                    let mut line = t("accountsettings.backup.onHint");
                    if state.updated_at > 0 {
                        let minutes = (crate::core::dms::now_ms() - state.updated_at) / 60_000;
                        line.push(' ');
                        line.push_str(&if minutes < 6 {
                            t("accountsettings.backup.lastSavedNow")
                        } else {
                            t_with(
                                "accountsettings.backup.lastSaved",
                                &[("when", Arg::Str(&format!("{minutes} minutes ago")))],
                            )
                        });
                    }
                    let mut body = div()
                        .child(div().text_sm().text_color(p.muted_foreground).child(line))
                        .child(div().mt(px(12.0)).h(px(6.0)).overflow_hidden().rounded_full().bg(p.muted).child(
                            div().h_full().w(gpui_kit::relative(share)).rounded_full().bg(if full {
                                rgb(0xf59e0b)
                            } else {
                                rgb(0x10b981)
                            }),
                        ))
                        .child(div().mt(px(6.0)).text_xs().text_color(p.muted_foreground).child(t_with(
                            "accountsettings.backup.size",
                            &[
                                ("used", Arg::Str(&bytes(state.size.max(0) as u64))),
                                ("total", Arg::Str(&bytes(state.max_size.max(0) as u64))),
                            ],
                        )));
                    body = match self.backup.asking {
                        Some("off") => body.child(ask(
                            self,
                            t("accountsettings.backup.offAsk"),
                            t("accountsettings.shared.turnOff"),
                            "off",
                            cx,
                        )),
                        Some(_) => body.child(ask(
                            self,
                            t("accountsettings.backup.newKeyAsk"),
                            t("accountsettings.backup.startOver"),
                            "replace",
                            cx,
                        )),
                        None => body.child(
                            div()
                                .mt(px(12.0))
                                .flex()
                                .flex_wrap()
                                .gap(px(8.0))
                                .child(
                                    // The web's arrow stays still here.
                                    button_with(
                                        "backup-new-key",
                                        if full {
                                            t("accountsettings.backup.startOverHere")
                                        } else {
                                            t("accountsettings.backup.newKey")
                                        },
                                        Some("rotate-ccw"),
                                        IconHover::Still,
                                        Look::Outline,
                                        true,
                                        p,
                                    )
                                    .rounded(radius_xl())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.backup.asking = Some("replace");
                                        cx.notify();
                                    })),
                                )
                                .child(
                                    button(
                                        "backup-off",
                                        t("accountsettings.shared.turnOff"),
                                        None,
                                        Look::Ghost,
                                        true,
                                        p,
                                    )
                                    .rounded(radius_xl())
                                    .text_color(p.muted_foreground)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.backup.asking = Some("off");
                                        cx.notify();
                                    })),
                                ),
                        ),
                    };
                    card(
                        "check",
                        if full { t("accountsettings.backup.full") } else { t("accountsettings.backup.on") },
                        if full { warn_tone } else { ok },
                        body,
                    )
                    .into_any_element()
                }
            }
        };
        let key_shown = self.backup.recovery_key.is_some();
        div()
            .child(caps(&t("accountsettings.backup.title"), p).font_weight(FontWeight::EXTRA_BOLD).mb(px(12.0)))
            .child(motion::rise(div().child(body), "backup-card", Duration::ZERO, 10.0))
            .when_some(state.problem.filter(|_| !key_shown), |el, problem| {
                el.child(
                    div().mt(px(8.0)).text_xs().font_weight(FontWeight::BOLD).text_color(rgb(0xd97706)).child(problem),
                )
            })
            .into_any_element()
    }
}
