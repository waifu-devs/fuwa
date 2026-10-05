//! "Your account" in settings: your profile on an instance (picture, names,
//! pronouns, status, bio), your password, and the devices signed in to it.
//! Like the web app's `settings/account/` pages. Each instance keeps its own
//! account, so these pages pick one when you're signed in to several.

use std::time::Duration;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::account::{ProfilePatch, picture_type};
use crate::core::store::{Connection, user_name};
use crate::pb;
use crate::ui::motion;
use crate::ui::settings::SettingsView;
use crate::ui::text::{ms_of, when};
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{avatar, error_line, icon, labeled, primary_button, soft_button};

/// What the account pages hold while they're open.
pub struct AccountForm {
    /// The instance whose account is shown.
    pub key: Option<String>,
    /// The instance the fields were filled from.
    loaded: Option<String>,
    name: Entity<InputState>,
    pronouns: Entity<InputState>,
    status: Entity<InputState>,
    bio: Entity<TextareaState>,
    current: Entity<InputState>,
    new: Entity<InputState>,
    sessions: Option<Vec<pb::Session>>,
    busy: bool,
    uploading: bool,
    saved: Option<std::time::Instant>,
    error: Option<String>,
    password_error: Option<String>,
    password_saved: bool,
}

impl AccountForm {
    pub fn new(window: &mut Window, cx: &mut Context<SettingsView>) -> Self {
        Self {
            key: None,
            loaded: None,
            name: cx.new(|cx| InputState::new(window, cx).placeholder("How people see you")),
            pronouns: cx.new(|cx| InputState::new(window, cx).placeholder("she/her, they/them…")),
            status: cx.new(|cx| InputState::new(window, cx).placeholder("What are you up to?")),
            bio: cx.new(|cx| {
                TextareaState::new(window, cx).auto_grow(3, 10).placeholder("A little about you. Markdown works.")
            }),
            current: cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("Your password now")),
            new: cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("At least 8 characters")),
            sessions: None,
            busy: false,
            uploading: false,
            saved: None,
            error: None,
            password_error: None,
            password_saved: false,
        }
    }
}

/// The signed-in accounts, as (instance key, instance name, you).
fn accounts(view: &SettingsView) -> Vec<(String, String, pb::User)> {
    view.core.shared.read(|s| {
        s.order
            .iter()
            .filter_map(|k| s.instance(k))
            .filter(|i| i.connection != Connection::SignedOut)
            .filter_map(|i| Some((i.key.clone(), i.name(), i.me.clone()?)))
            .collect()
    })
}

impl SettingsView {
    /// The account the account pages show: the one picked, or the first.
    pub(crate) fn account_key(&mut self) -> Option<String> {
        let list = accounts(self);
        let key = self
            .account
            .key
            .clone()
            .filter(|k| list.iter().any(|(key, ..)| key == k))
            .or_else(|| list.first().map(|(key, ..)| key.clone()))?;
        self.account.key = Some(key.clone());
        Some(key)
    }

    /// Picks the account to show, and fills the fields from it once.
    fn account_ready(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<(String, pb::User)> {
        let list = accounts(self);
        let key = self.account.key.clone().filter(|k| list.iter().any(|(key, ..)| key == k));
        let (key, _, me) = match key {
            Some(k) => list.into_iter().find(|(key, ..)| *key == k)?,
            None => list.into_iter().next()?,
        };
        self.account.key = Some(key.clone());
        if self.account.loaded.as_deref() != Some(key.as_str()) {
            self.account.loaded = Some(key.clone());
            self.account.sessions = None;
            self.account.error = None;
            self.account.password_error = None;
            self.account.password_saved = false;
            let f = &self.account;
            let (name, status) = (me.display_name.clone(), me.status.clone());
            f.name.update(cx, |s, cx| s.set_value(name, window, cx));
            f.status.update(cx, |s, cx| s.set_value(status, window, cx));
            f.pronouns.update(cx, |s, cx| s.set_value("", window, cx));
            f.bio.update(cx, |s, cx| s.set_value("", window, cx));
            f.current.update(cx, |s, cx| s.set_value("", window, cx));
            f.new.update(cx, |s, cx| s.set_value("", window, cx));
            window.blur(cx);
            let (core, k, id) = (self.core.clone(), key.clone(), me.id.clone());
            let rx = self.core.spawn(async move { core.profile(&k, &id).await });
            cx.spawn_in(window, async move |this, cx| {
                let Ok(Ok(profile)) = rx.await else { return };
                let _ = this.update_in(cx, |this, window, cx| {
                    let f = &this.account;
                    f.pronouns.update(cx, |s, cx| s.set_value(profile.pronouns.clone(), window, cx));
                    f.bio.update(cx, |s, cx| s.set_value(profile.bio.clone(), window, cx));
                    window.blur(cx);
                    cx.notify();
                });
            })
            .detach();
            let (core, k) = (self.core.clone(), key.clone());
            let rx = self.core.spawn(async move { core.sessions(&k).await });
            cx.spawn(async move |this, cx| {
                let Ok(result) = rx.await else { return };
                let _ = this.update(cx, |this, cx| {
                    this.account.sessions = Some(result.unwrap_or_default());
                    cx.notify();
                });
            })
            .detach();
        }
        Some((key, me))
    }

    /// The row of instances to pick from, when there's more than one.
    pub(crate) fn account_picker(&self, current: &str, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let list = accounts(self);
        if list.len() < 2 {
            return None;
        }
        let streamer = self.core.prefs().streamer_mode;
        Some(
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.0))
                .children(list.into_iter().map(|(key, name, me)| {
                    let on = key == current;
                    div()
                        .id(SharedString::from(format!("acct-pick-{key}")))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .pl(px(6.0))
                        .pr(px(12.0))
                        .h(px(36.0))
                        .rounded_full()
                        .border_1()
                        .border_color(if on { p.primary } else { p.border })
                        .bg(if on { alpha(p.primary, 0.12) } else { p.card.into() })
                        .cursor_pointer()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.account.key = Some(key.clone());
                            cx.notify();
                        }))
                        .child(avatar(Some(&me), 24.0, p))
                        .child(if streamer { user_name(&me) } else { format!("{} · {name}", user_name(&me)) })
                }))
                .into_any_element(),
        )
    }

    pub(crate) fn profile_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some((key, me)) = self.account_ready(window, cx) else {
            return signed_out(p).into_any_element();
        };
        let f = &self.account;
        let saved = f.saved.is_some_and(|at| at.elapsed() < Duration::from_secs(3));
        let picture = div()
            .flex()
            .items_center()
            .gap(px(18.0))
            .child(div().relative().child(avatar(Some(&me), 88.0, p)).when(f.uploading, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded_full()
                        .bg(alpha(p.rail, 0.55))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(gpui_kit::white())
                        .child(icon("loader-circle").size(px(24.0))),
                )
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(
                                primary_button(
                                    "avatar-pick",
                                    if f.uploading { "Uploading…" } else { "Change picture" },
                                    p,
                                )
                                .h(px(36.0))
                                .text_sm()
                                .child(icon("image-up").size(px(16.0)))
                                .on_click(cx.listener(|this, _, window, cx| this.pick_avatar(window, cx))),
                            )
                            .when(!me.avatar_url.is_empty(), |el| {
                                el.child(soft_button("avatar-remove", "Remove", p).on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.save_profile(
                                            ProfilePatch { avatar_url: Some(String::new()), ..ProfilePatch::default() },
                                            cx,
                                        )
                                    },
                                )))
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.muted_foreground)
                            .child("PNG, JPEG, GIF or WebP. Square pictures look best."),
                    ),
            );
        let body = div()
            .flex()
            .flex_col()
            .gap(px(22.0))
            .when_some(self.account_picker(&key, p, cx), |el, picker| el.child(picker))
            .child(picture)
            .child(
                div()
                    .flex()
                    .gap(px(14.0))
                    .child(div().flex_1().child(labeled("Display name", Input::new(&f.name).large(), p)))
                    .child(div().w(px(200.0)).child(labeled("Pronouns", Input::new(&f.pronouns).large(), p))),
            )
            .child(labeled("Status", Input::new(&f.status).large(), p))
            .child(labeled(
                "About me",
                div()
                    .px(px(12.0))
                    .py(px(10.0))
                    .rounded(corner(12.0))
                    .bg(p.card)
                    .border_1()
                    .border_color(p.border)
                    .child(Textarea::new(&f.bio).appearance(false)),
                p,
            ))
            .when_some(error_line(f.error.as_deref(), p), |el, e| el.child(e))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(primary_button("profile-save", if f.busy { "Saving…" } else { "Save changes" }, p).on_click(
                        cx.listener(|this, _, _, cx| {
                            let f = &this.account;
                            let patch = ProfilePatch {
                                display_name: Some(f.name.read(cx).value().trim().to_owned()),
                                pronouns: Some(f.pronouns.read(cx).value().trim().to_owned()),
                                status: Some(f.status.read(cx).value().trim().to_owned()),
                                bio: Some(f.bio.read(cx).value().trim().to_owned()),
                                avatar_url: None,
                            };
                            this.save_profile(patch, cx);
                        }),
                    ))
                    .when(saved, |el| {
                        el.child(motion::rise(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.success)
                                .child(icon("check").size(px(16.0)))
                                .child("Saved"),
                            "profile-saved",
                            Duration::ZERO,
                            6.0,
                        ))
                    }),
            );
        body.into_any_element()
    }

    fn save_profile(&mut self, patch: ProfilePatch, cx: &mut Context<Self>) {
        let Some(key) = self.account.key.clone() else { return };
        if self.account.busy {
            return;
        }
        self.account.busy = true;
        self.account.error = None;
        let core = self.core.clone();
        let rx = self.core.spawn(async move { core.update_profile(&key, patch).await });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.account.busy = false;
                match result {
                    Ok(_) => this.account.saved = Some(std::time::Instant::now()),
                    Err(err) => this.account.error = Some(err.message),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Asks the system for a picture, uploads it and sets it.
    fn pick_avatar(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.account.key.clone() else { return };
        if self.account.uploading {
            return;
        }
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose a picture".into()),
        });
        let core = self.core.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let Some(kind) = picture_type(&name) else {
                let _ = this.update(cx, |this, cx| {
                    this.account.error = Some("That isn't a picture fuwa can use (PNG, JPEG, GIF or WebP).".into());
                    cx.notify();
                });
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.account.uploading = true;
                this.account.error = None;
                cx.notify();
            });
            let rx = core.spawn({
                let core = core.clone();
                async move {
                    let bytes = crate::core::account::read_picture(&path).await?;
                    let url = core.upload_picture(&key, pb::MediaPurpose::Avatar, kind, bytes).await?;
                    core.update_profile(&key, ProfilePatch { avatar_url: Some(url), ..ProfilePatch::default() }).await
                }
            });
            let result = rx.await;
            let _ = this.update(cx, |this, cx| {
                this.account.uploading = false;
                match result {
                    Ok(Ok(_)) => this.account.saved = Some(std::time::Instant::now()),
                    Ok(Err(err)) => this.account.error = Some(err.message),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn security_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some((key, me)) = self.account_ready(window, cx) else {
            return signed_out(p).into_any_element();
        };
        let f = &self.account;
        let linked = me.kind == pb::AccountKind::Linked as i32;
        let password: AnyElement = if linked {
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .p(px(16.0))
                .rounded(corner(16.0))
                .bg(p.secondary)
                .child(icon("link").size(px(18.0)).text_color(p.primary))
                .child(
                    div()
                        .text_sm()
                        .child("You sign in here with waifu.dev, so there's no password to change on this instance."),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(14.0))
                .child(
                    div()
                        .flex()
                        .gap(px(14.0))
                        .child(div().flex_1().child(labeled(
                            "Current password",
                            Input::new(&f.current).large().mask_toggle(),
                            p,
                        )))
                        .child(div().flex_1().child(labeled(
                            "New password",
                            Input::new(&f.new).large().mask_toggle(),
                            p,
                        ))),
                )
                .when_some(error_line(f.password_error.as_deref(), p), |el, e| el.child(e))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .child(
                            primary_button("password-save", "Change password", p)
                                .on_click(cx.listener(|this, _, window, cx| this.change_password(window, cx))),
                        )
                        .when(f.password_saved, |el| {
                            el.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(p.success)
                                    .child(icon("check").size(px(16.0)))
                                    .child("Changed. Your other devices were signed out."),
                            )
                        }),
                )
                .into_any_element()
        };

        let mut devices = div().flex().flex_col().gap(px(10.0));
        match &f.sessions {
            None => {
                devices = devices.child(div().text_sm().text_color(p.muted_foreground).child("Getting your devices…"))
            }
            Some(list) => {
                for (n, session) in list.iter().enumerate() {
                    let (glyph, label) = device_label(&session.user_agent);
                    let seen = ms_of(session.last_active_at.as_ref());
                    let id = session.id.clone();
                    devices = devices.child(motion::rise(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(14.0))
                            .p(px(14.0))
                            .rounded(corner(16.0))
                            .bg(p.card)
                            .border_1()
                            .border_color(if session.current { p.primary } else { p.border })
                            .child(
                                div()
                                    .size(px(40.0))
                                    .rounded(corner(12.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(alpha(p.primary, 0.12))
                                    .text_color(p.primary)
                                    .child(icon(glyph).size(px(20.0))),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().font_weight(FontWeight::BOLD).child(label))
                                    .child(div().text_sm().text_color(p.muted_foreground).child(if session.current {
                                        "This computer, right now".to_owned()
                                    } else if seen > 0 {
                                        format!("Last seen {}", when(seen).to_lowercase())
                                    } else {
                                        "Signed in".to_owned()
                                    })),
                            )
                            .when(!session.current, |el| {
                                el.child(
                                    soft_button(SharedString::from(format!("revoke-{id}")), "Sign out", p)
                                        .on_click(cx.listener(move |this, _, _, cx| this.revoke(Some(id.clone()), cx))),
                                )
                            }),
                        SharedString::from(format!("session-{}", session.id)),
                        Duration::from_millis(40 * n.min(10) as u64),
                        8.0,
                    ));
                }
            }
        }
        let others = f.sessions.as_ref().is_some_and(|l| l.iter().any(|s| !s.current));
        div()
            .flex()
            .flex_col()
            .gap(px(28.0))
            .when_some(self.account_picker(&key, p, cx), |el, picker| el.child(picker))
            .child(section("Password", password, p))
            .child(section(
                "Signed-in devices",
                div().flex().flex_col().gap(px(12.0)).child(devices).when(others, |el| {
                    el.child(
                        div().child(
                            soft_button("revoke-all", "Sign out everywhere else", p)
                                .child(icon("log-out").size(px(16.0)))
                                .on_click(cx.listener(|this, _, _, cx| this.revoke(None, cx))),
                        ),
                    )
                }),
                p,
            ))
            .into_any_element()
    }

    fn change_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.account.key.clone() else { return };
        let current = self.account.current.read(cx).value().to_string();
        let new = self.account.new.read(cx).value().to_string();
        if current.is_empty() || new.is_empty() {
            self.account.password_error = Some("Fill in both passwords.".into());
            cx.notify();
            return;
        }
        self.account.password_error = None;
        self.account.password_saved = false;
        let core = self.core.clone();
        let rx = self.core.spawn(async move {
            core.change_password(&key, &current, &new).await?;
            core.sessions(&key).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(sessions) => {
                        this.account.password_saved = true;
                        this.account.sessions = Some(sessions);
                        this.account.current.update(cx, |s, cx| s.set_value("", window, cx));
                        this.account.new.update(cx, |s, cx| s.set_value("", window, cx));
                        window.blur(cx);
                    }
                    Err(err) => this.account.password_error = Some(err.message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Signs one device out, or every other one.
    fn revoke(&mut self, session: Option<String>, cx: &mut Context<Self>) {
        let Some(key) = self.account.key.clone() else { return };
        let core = self.core.clone();
        let rx = self.core.spawn(async move {
            match &session {
                Some(id) => core.revoke_session(&key, id).await?,
                None => {
                    core.revoke_other_sessions(&key).await?;
                }
            }
            core.sessions(&key).await
        });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(sessions) => this.account.sessions = Some(sessions),
                    Err(err) => this.account.password_error = Some(err.message),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn section(title: &str, body: impl IntoElement, p: &Palette) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .text_size(px(11.0))
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(p.muted_foreground)
                .child(title.to_uppercase()),
        )
        .child(body)
}

fn signed_out(p: &Palette) -> impl IntoElement {
    div()
        .p(px(18.0))
        .rounded(corner(16.0))
        .bg(p.secondary)
        .text_color(p.muted_foreground)
        .child("Sign in to an instance first. Your account lives there.")
}

/// A device, from what it said it was when it signed in.
pub fn device_label(agent: &str) -> (&'static str, String) {
    let lower = agent.to_lowercase();
    let os = if lower.contains("windows") {
        "Windows"
    } else if lower.contains("macos") || lower.contains("mac os") || lower.contains("macintosh") {
        "macOS"
    } else if lower.contains("android") {
        "Android"
    } else if lower.contains("iphone") || lower.contains("ipad") || lower.contains("ios") {
        "iOS"
    } else if lower.contains("linux") {
        "Linux"
    } else {
        ""
    };
    let (glyph, app) = if lower.starts_with("fuwa-desktop") {
        ("monitor", "fuwa desktop")
    } else if lower.contains("android") || lower.contains("iphone") || lower.contains("mobile") {
        ("smartphone", "Browser")
    } else if lower.contains("firefox") {
        ("globe", "Firefox")
    } else if lower.contains("edg/") {
        ("globe", "Edge")
    } else if lower.contains("chrome") {
        ("globe", "Chrome")
    } else if lower.contains("safari") {
        ("globe", "Safari")
    } else {
        ("laptop", "A device")
    };
    (glyph, if os.is_empty() { app.to_owned() } else { format!("{app} on {os}") })
}

#[cfg(test)]
mod tests {
    #[test]
    fn devices_read_like_people_say_them() {
        assert_eq!(super::device_label("fuwa-desktop/0.1.0 (linux; x86_64)").1, "fuwa desktop on Linux");
        assert_eq!(
            super::device_label(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120 Safari/537.36"
            )
            .1,
            "Chrome on Windows"
        );
        assert_eq!(super::device_label("").1, "A device");
    }
}
