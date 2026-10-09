//! "Your account" pages for the instance on screen, as the web's
//! `settings/account/Profile.tsx`, `Account.tsx` (`Password`) and
//! `Devices.tsx`: the profile beside a live copy of your card (names,
//! pictures, color, effect, status that clears by itself, about me), the
//! password with its strength meter, and every device signed in.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, img,
    px, rgb,
};

use crate::core::account::ProfilePatch;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::pictures::PictureKind;
use crate::core::profile_items::{offered_effects, resolve_decoration, resolve_effect};
use crate::core::store::{Connection, user_name};
use crate::pb;
use crate::ui::motion;
use crate::ui::settings::SettingsView;
use crate::ui::settings_controls::{At, Look, button, caps, chips, field, hint, save_bar, segmented, warn};
use crate::ui::text::ms_of;
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_3xl, radius_lg, radius_xl};
use crate::ui::widgets::{avatar, icon};

const BIO_MAX: usize = 2000;
pub(crate) const PASSWORD_MIN: usize = 8;
pub(crate) const PASSWORD_MAX: usize = 256;

/// Colors for the banner and the card, picked to sit well on every theme.
const COLORS: [u32; 12] = [
    0xff6b9d, 0xf472b6, 0xc084fc, 0x818cf8, 0x60a5fa, 0x22d3ee, 0x34d399, 0xa3e635, 0xfbbf24, 0xfb923c, 0xf87171,
    0x94a3b8,
];

/// When a status clears by itself. `Keep` leaves the time it already has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Clear {
    Keep,
    Never,
    Minutes30,
    Hour1,
    Hours4,
    Today,
}

const CLEARS: [Clear; 5] = [Clear::Never, Clear::Minutes30, Clear::Hour1, Clear::Hours4, Clear::Today];

fn clear_label(c: Clear, kept: Option<i64>) -> String {
    match c {
        Clear::Keep => {
            t_with("accountsettings.profile.clearAt", &[("time", Arg::Str(&kept.map(at_time).unwrap_or_default()))])
        }
        Clear::Never => t("accountsettings.profile.clearNever"),
        Clear::Minutes30 => t_with("accountsettings.profile.clearMinutes", &[("count", Arg::Num(30))]),
        Clear::Hour1 => t_with("accountsettings.profile.clearHours", &[("count", Arg::Num(1))]),
        Clear::Hours4 => t_with("accountsettings.profile.clearHours", &[("count", Arg::Num(4))]),
        Clear::Today => t("accountsettings.profile.clearToday"),
    }
}

/// When a status set now clears, as unix ms.
fn clears_at(c: Clear, kept: Option<i64>) -> Option<i64> {
    use chrono::{Local, TimeZone as _};
    let now = Local::now();
    let later = |m: i64| Some(now.timestamp_millis() + m * 60_000);
    match c {
        Clear::Keep => kept,
        Clear::Never => None,
        Clear::Minutes30 => later(30),
        Clear::Hour1 => later(60),
        Clear::Hours4 => later(240),
        Clear::Today => {
            let end = now.date_naive().and_hms_opt(23, 59, 59)?;
            Local.from_local_datetime(&end).single().map(|d| d.timestamp_millis())
        }
    }
}

/// "4:30 PM" today, "Tue 4:30 PM" another day.
fn at_time(ms: i64) -> String {
    use chrono::{Local, TimeZone as _};
    let Some(at) = Local.timestamp_millis_opt(ms).single() else { return String::new() };
    let time = crate::ui::text::clock(ms);
    if at.date_naive() == Local::now().date_naive() { time } else { format!("{} {time}", at.format("%a")) }
}

fn is_link(value: &str) -> bool {
    let v = value.trim();
    v.is_empty() || ((v.starts_with("https://") || v.starts_with("http://")) && !v.contains(char::is_whitespace))
}

/// Your profile as it's saved, to edit over.
#[derive(Clone, Default, PartialEq)]
pub(crate) struct Draft {
    name: String,
    pronouns: String,
    status: String,
    expires: Option<i64>,
    bio: String,
    avatar: String,
    banner: String,
    /// 0xRRGGBB, or -1 for the color fuwa picks from your id.
    pub(crate) accent: i32,
    effect: String,
    /// One of the instance's decorations, or "" for none.
    decoration: String,
}

/// How you look in one server, for the card's preview: a nickname, an effect and a decoration
/// there ("" for your own).
pub(crate) struct ServerLook<'a> {
    pub server_id: &'a str,
    pub nickname: &'a str,
    pub effect: &'a str,
    pub decoration: &'a str,
    /// When you joined it, unix milliseconds (0 when unknown).
    pub joined: i64,
}

/// Which picture field.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Avatar,
    Banner,
}

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
    avatar_link: Entity<InputState>,
    banner_link: Entity<InputState>,
    color_text: Entity<InputState>,
    current: Entity<InputState>,
    new: Entity<InputState>,
    again: Entity<InputState>,
    /// The saved profile, once read (or once reading it failed).
    base: Option<Draft>,
    ready: bool,
    created_at: i64,
    avatar: String,
    banner: String,
    accent: i32,
    effect: String,
    decoration: String,
    clear: Clear,
    bio_preview: bool,
    links: (bool, bool),
    any_color: bool,
    uploading: Option<Kind>,
    picture_error: Option<(Kind, String)>,
    /// The picture being framed before it's uploaded.
    cropper: Option<crate::ui::cropper::CropSlot>,
    saving: bool,
    error: Option<String>,
    /// Password page.
    show_password: bool,
    password_busy: bool,
    password_error: Option<String>,
    password_done: bool,
    password_shake: Option<std::time::Instant>,
    /// Devices page.
    pub(crate) sessions: Option<Vec<pb::Session>>,
    sessions_error: Option<String>,
    leaving: std::collections::HashSet<String>,
    confirm_all: bool,
    signing_out: bool,
}

impl AccountForm {
    pub fn new(window: &mut Window, cx: &mut Context<SettingsView>) -> Self {
        let input = |placeholder: String, window: &mut Window, cx: &mut Context<SettingsView>| {
            let state = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
            cx.subscribe(&state, |_, _, _: &InputEvent, cx| cx.notify()).detach();
            state
        };
        let masked = |window: &mut Window, cx: &mut Context<SettingsView>| {
            let state = cx.new(|cx| InputState::new(window, cx).masked(true));
            cx.subscribe(&state, |_, _, _: &InputEvent, cx| cx.notify()).detach();
            state
        };
        let bio = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(5, 14).placeholder(t("accountsettings.profile.bioPlaceholder"))
        });
        cx.subscribe(&bio, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        Self {
            key: None,
            loaded: None,
            name: input(String::new(), window, cx),
            pronouns: input(t("accountsettings.profile.pronounsPlaceholder"), window, cx),
            status: input(t("accountsettings.profile.statusPlaceholder"), window, cx),
            bio,
            avatar_link: input("https://…".into(), window, cx),
            banner_link: input("https://…".into(), window, cx),
            color_text: input("#ff6b9d".into(), window, cx),
            current: masked(window, cx),
            new: masked(window, cx),
            again: masked(window, cx),
            base: None,
            ready: false,
            created_at: 0,
            avatar: String::new(),
            banner: String::new(),
            accent: -1,
            effect: String::new(),
            decoration: String::new(),
            clear: Clear::Never,
            bio_preview: false,
            links: (false, false),
            any_color: false,
            uploading: None,
            picture_error: None,
            cropper: None,
            saving: false,
            error: None,
            show_password: false,
            password_busy: false,
            password_error: None,
            password_done: false,
            password_shake: None,
            sessions: None,
            sessions_error: None,
            leaving: Default::default(),
            confirm_all: false,
            signing_out: false,
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
    /// The account the account pages show: the instance on screen, or the first signed in.
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

    /// Fills the fields from the account once (again when the instance changes).
    pub(crate) fn account_ready(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<(String, pb::User)> {
        let (key, me) = self.account_me()?;
        if self.account.loaded.as_deref() == Some(key.as_str()) {
            return Some((key, me));
        }
        self.account.loaded = Some(key.clone());
        self.account.sessions = None;
        self.account.base = None;
        self.account.ready = false;
        self.account.error = None;
        let base = Draft {
            name: me.display_name.clone(),
            status: me.status.clone(),
            expires: me.status_expires_at.as_ref().map(|t| ms_of(Some(t))).filter(|ms| *ms > 0),
            avatar: me.avatar_url.clone(),
            accent: -1,
            decoration: me.decoration_id.clone(),
            ..Draft::default()
        };
        self.load_draft(&base, window, cx);
        self.account.base = Some(base);
        let (core, k, id) = (self.core.clone(), key.clone(), me.id.clone());
        let rx = self.core.spawn(async move { core.profile(&k, &id).await });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                if let Ok(profile) = result {
                    this.take_profile(&profile, window, cx);
                }
                this.account.ready = true;
                cx.notify();
            });
        })
        .detach();
        Some((key, me))
    }

    /// The saved profile came: it's what edits are measured against now.
    fn take_profile(&mut self, profile: &pb::Profile, window: &mut Window, cx: &mut Context<Self>) {
        let me = profile.user.clone().or_else(|| self.account_me().map(|(_, me)| me)).unwrap_or_default();
        let base = Draft {
            name: me.display_name.clone(),
            pronouns: profile.pronouns.clone(),
            status: me.status.clone(),
            expires: me.status_expires_at.as_ref().map(|t| ms_of(Some(t))).filter(|ms| *ms > 0),
            bio: profile.bio.clone(),
            avatar: me.avatar_url.clone(),
            banner: profile.banner_url.clone(),
            accent: profile.accent_color.unwrap_or(-1),
            effect: profile.effect.clone(),
            decoration: me.decoration_id.clone(),
        };
        self.account.created_at = ms_of(profile.created_at.as_ref());
        self.load_draft(&base, window, cx);
        self.account.base = Some(base);
    }

    fn load_draft(&mut self, d: &Draft, window: &mut Window, cx: &mut Context<Self>) {
        let f = &mut self.account;
        f.name.update(cx, |s, cx| s.set_value(d.name.clone(), window, cx));
        f.pronouns.update(cx, |s, cx| s.set_value(d.pronouns.clone(), window, cx));
        f.status.update(cx, |s, cx| s.set_value(d.status.clone(), window, cx));
        f.bio.update(cx, |s, cx| s.set_value(d.bio.clone(), window, cx));
        f.avatar_link.update(cx, |s, cx| s.set_value(d.avatar.clone(), window, cx));
        f.banner_link.update(cx, |s, cx| s.set_value(d.banner.clone(), window, cx));
        f.avatar = d.avatar.clone();
        f.banner = d.banner.clone();
        f.accent = d.accent;
        f.effect = d.effect.clone();
        f.decoration = d.decoration.clone();
        f.clear = if !d.status.is_empty() && d.expires.is_some() { Clear::Keep } else { Clear::Never };
        f.error = None;
    }

    /// The draft as the fields hold it now.
    fn draft(&self, cx: &Context<Self>) -> Draft {
        let f = &self.account;
        Draft {
            name: f.name.read(cx).value().to_string(),
            pronouns: f.pronouns.read(cx).value().to_string(),
            status: f.status.read(cx).value().to_string(),
            expires: None,
            bio: f.bio.read(cx).value().to_string(),
            avatar: f.avatar.clone(),
            banner: f.banner.clone(),
            accent: f.accent,
            effect: f.effect.clone(),
            decoration: f.decoration.clone(),
        }
    }

    /// How many fields differ from what's saved, and whether the status (or when it clears) does.
    fn changes(&self, cx: &Context<Self>) -> (usize, bool) {
        let Some(base) = &self.account.base else { return (0, false) };
        let d = self.draft(cx);
        let base_clear = if !base.status.is_empty() && base.expires.is_some() { Clear::Keep } else { Clear::Never };
        let status = d.status != base.status || (!d.status.trim().is_empty() && self.account.clear != base_clear);
        let n = [
            d.name != base.name,
            d.pronouns != base.pronouns,
            d.bio != base.bio,
            d.avatar != base.avatar,
            d.banner != base.banner,
            d.accent != base.accent,
            d.effect != base.effect,
            d.decoration != base.decoration,
        ]
        .iter()
        .filter(|x| **x)
        .count();
        (n + usize::from(status), status)
    }

    fn save_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.account.key.clone() else { return };
        let Some(base) = self.account.base.clone() else { return };
        if self.account.saving {
            return;
        }
        let d = self.draft(cx);
        let problem = if d.name.trim().is_empty() {
            Some(t("accountsettings.profile.nameEmpty"))
        } else if !is_link(&d.avatar) {
            Some(t("accountsettings.profile.avatarLink"))
        } else if !is_link(&d.banner) {
            Some(t("accountsettings.profile.bannerLink"))
        } else {
            None
        };
        if let Some(problem) = problem {
            self.account.error = Some(problem);
            cx.notify();
            return;
        }
        let (_, status_changed) = self.changes(cx);
        let differs = |a: &str, b: &str| (a != b).then(|| a.trim().to_owned());
        let patch = ProfilePatch {
            display_name: differs(&d.name, &base.name),
            pronouns: differs(&d.pronouns, &base.pronouns),
            bio: differs(&d.bio, &base.bio),
            avatar_url: differs(&d.avatar, &base.avatar),
            banner_url: differs(&d.banner, &base.banner),
            accent_color: (d.accent != base.accent).then_some(d.accent),
            effect: (d.effect != base.effect).then(|| d.effect.clone()),
            status: status_changed.then(|| d.status.trim().to_owned()),
            status_expires_at: if status_changed { clears_at(self.account.clear, base.expires) } else { None },
            decoration_id: (d.decoration != base.decoration).then(|| d.decoration.clone()),
        };
        self.account.saving = true;
        self.account.error = None;
        let picked = (
            patch.effect.as_ref().is_some_and(|e| !e.is_empty()),
            patch.decoration_id.as_ref().is_some_and(|d| !d.is_empty()),
        );
        let core = self.core.clone();
        let rx = self.core.spawn(async move { core.update_profile(&key, patch).await });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.account.saving = false;
                match result {
                    Ok(profile) => {
                        if picked.0 {
                            crate::core::reports::used("profile-effect/picked");
                        }
                        if picked.1 {
                            crate::core::reports::used("profile-decoration/picked");
                        }
                        this.take_profile(&profile, window, cx);
                    }
                    Err(err) => this.account.error = Some(err.message),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Your profile as it's saved.
    pub(crate) fn saved_draft(&self) -> Draft {
        self.account.base.clone().unwrap_or_default()
    }

    fn discard_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(base) = self.account.base.clone() {
            self.load_draft(&base, window, cx);
        }
        cx.notify();
    }

    /// Asks the system for a picture, frames it, and uploads it for the avatar or banner;
    /// saving is the page's, from the save bar.
    fn pick_picture(&mut self, kind: Kind, cx: &mut Context<Self>) {
        if self.account.key.is_none() || self.account.uploading.is_some() || self.account.cropper.is_some() {
            return;
        }
        let shape = if kind == Kind::Avatar { PictureKind::Avatar } else { PictureKind::Banner };
        crate::ui::cropper::choose(
            self.core.clone(),
            shape,
            t("desktop.account.choosePicture"),
            cx,
            |this| &mut this.account.cropper,
            move |this, error, cx| {
                this.account.picture_error = Some((kind, error));
                cx.notify();
            },
            Rc::new(move |this: &mut Self, bytes, mime, cx: &mut Context<Self>| {
                this.upload_picture(kind, bytes, mime, cx)
            }),
        );
    }

    fn upload_picture(&mut self, kind: Kind, bytes: Vec<u8>, mime: &'static str, cx: &mut Context<Self>) {
        let Some(key) = self.account.key.clone() else { return };
        self.account.uploading = Some(kind);
        self.account.picture_error = None;
        let purpose = if kind == Kind::Avatar { pb::MediaPurpose::Avatar } else { pb::MediaPurpose::Banner };
        let rx = self.core.spawn({
            let core = self.core.clone();
            async move { core.upload_picture(&key, purpose, mime, bytes).await }
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            let _ = this.update(cx, |this, cx| {
                this.account.uploading = None;
                match result {
                    Ok(Ok(url)) => match kind {
                        Kind::Avatar => this.account.avatar = url,
                        Kind::Banner => this.account.banner = url,
                    },
                    Ok(Err(err)) => this.account.picture_error = Some((kind, err.message)),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn profile_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some((key, me)) = self.account_ready(window, cx) else { return div().into_any_element() };
        if let Some(cropper) = crate::ui::cropper::layer(&self.account.cropper) {
            self.state.overlay = Some(cropper);
        }
        // Instances from before profile effects (or decorations) don't say, and can't keep one.
        let (effects_on, decorations_on, items) = self.core.shared.read(|s| match s.instance(&key) {
            Some(i) => (i.effects_on(), i.decorations_on(), i.profile_items.clone()),
            None => (false, false, Vec::new()),
        });
        let ready = self.account.ready;
        let d = self.draft(cx);
        let form_w = if self.wide { self.column - 40.0 - 288.0 } else { self.column };

        let name_hint = if d.name.trim().is_empty() {
            warn(t("accountsettings.profile.pickName"), p)
        } else {
            hint(t("accountsettings.profile.nameHint"), p)
        };
        let mut rows: Vec<(&'static str, String, Option<AnyElement>, AnyElement)> = vec![
            (
                "display-name",
                t("settings.nav.displayName"),
                Some(name_hint),
                field(Input::new(&self.account.name).appearance(false), p).into_any_element(),
            ),
            (
                "pronouns",
                t("settings.nav.pronouns"),
                Some(hint(t("accountsettings.profile.pronounsHint"), p)),
                div()
                    .max_w(px(320.0))
                    .child(field(Input::new(&self.account.pronouns).appearance(false), p))
                    .when(!ready, |el| el.opacity(0.5))
                    .into_any_element(),
            ),
            (
                "avatar",
                t("settings.nav.avatar"),
                Some(if is_link(&d.avatar) {
                    hint(t("accountsettings.profile.avatarHint"), p)
                } else {
                    warn(t("accountsettings.profile.linkHttps"), p)
                }),
                self.picture_field(Kind::Avatar, &me, p, cx),
            ),
            (
                "banner",
                t("settings.nav.banner"),
                Some(if is_link(&d.banner) {
                    hint(t("accountsettings.profile.bannerHint"), p)
                } else {
                    warn(t("accountsettings.profile.linkHttps"), p)
                }),
                self.picture_field(Kind::Banner, &me, p, cx),
            ),
            (
                "profile-color",
                t("settings.nav.profileColor"),
                Some(hint(t("accountsettings.profile.colorHint"), p)),
                self.color_picker(&me, ready, p, window, cx),
            ),
        ];
        if effects_on {
            let about = match resolve_effect(&d.effect, &items) {
                Some(spec) => crate::ui::profile_effect::effect_text(&spec).1,
                None => t("accountsettings.profile.effectHint"),
            };
            let offered = offered_effects(&items);
            rows.push((
                "profile-effect",
                t("settings.nav.profileEffect"),
                Some(hint(about, p)),
                self.effect_picker(
                    "profile",
                    &d.effect,
                    &me.id,
                    d.accent,
                    &offered,
                    None,
                    ready,
                    form_w,
                    p,
                    cx,
                    |this, id, cx| {
                        this.account.effect = id;
                        cx.notify();
                    },
                ),
            ));
        }
        let decorations: Vec<pb::ProfileItem> =
            items.iter().filter(|i| i.kind == pb::ProfileItemKind::Decoration as i32).cloned().collect();
        if decorations_on && (!decorations.is_empty() || !d.decoration.is_empty()) {
            let about = resolve_decoration(&d.decoration, &items)
                .map(|i| i.description.clone())
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| t("accountsettings.decorations.hint"));
            let picker = self.decoration_picker(
                "profile",
                &d.decoration,
                &me,
                &decorations,
                None,
                ready,
                form_w,
                p,
                cx,
                |this, id, cx| {
                    this.account.decoration = id;
                    cx.notify();
                },
            );
            rows.push((
                "avatar-decoration",
                t("accountsettings.decorations.label"),
                Some(hint(about, p)),
                motion::slide_in(div().child(picker), "avatar-decoration", 12.0).into_any_element(),
            ));
        }
        rows.push((
            "status",
            t("settings.nav.status"),
            Some(hint(t("accountsettings.profile.statusHint"), p)),
            self.status_field(&d, p, cx),
        ));
        let count = d.bio.chars().count();
        let bio_hint = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .text_sm()
            .text_color(p.muted_foreground)
            .child(t_with(
                "accountsettings.profile.markdown",
                &[
                    ("bold", Arg::Str(&format!("**{}**", t("accountsettings.profile.bold")))),
                    ("italics", Arg::Str(&format!("*{}*", t("accountsettings.profile.italics")))),
                    ("code", Arg::Str(&format!("`{}`", t("accountsettings.profile.code")))),
                ],
            ))
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .when(count * 10 > BIO_MAX * 9, |el| el.text_color(rgb(0xf59e0b)))
                    .child(format!("{count} / {BIO_MAX}")),
            )
            .into_any_element();
        rows.push(("about-me", t("settings.nav.aboutMe"), Some(bio_hint), self.bio_field(&d, ready, p, window, cx)));
        let username = if self.core.prefs().hides_personal() {
            "@••••••".to_owned()
        } else {
            format!("@{}", me.username)
        };
        rows.push((
            "username",
            t("accountsettings.profile.username"),
            Some(hint(t("accountsettings.profile.usernameHint"), p)),
            div().text_sm().font_weight(FontWeight::BOLD).child(username).into_any_element(),
        ));
        let n_rows = rows.len();
        let mut form = div().flex().flex_col();
        for (n, (id, label, h, body)) in rows.into_iter().enumerate() {
            form = form.child(self.row(id, &label, h, At::of(n, n_rows), body, p));
        }
        let (changed, _) = self.changes(cx);
        if changed > 0 {
            self.holding = true;
        }
        let alarm = self.alarm();
        let bar = save_bar(
            "profile",
            changed,
            self.account.saving,
            self.account.error.as_deref(),
            alarm,
            p,
            window,
            cx,
            |this, window, cx| this.save_profile(window, cx),
            |this, window, cx| this.discard_profile(window, cx),
        );
        let preview = self.profile_preview(&me, &d, None, p, cx);
        div()
            .flex()
            .flex_col()
            .child(crate::ui::settings_controls::with_preview(form, preview, self.wide, p))
            .child(bar)
            .into_any_element()
    }

    /// The card others open from your name, as the draft would make it (the web's `ProfileCard`).
    pub(crate) fn profile_preview(
        &mut self,
        me: &pb::User,
        d: &Draft,
        server: Option<ServerLook>,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let nickname = server.as_ref().map(|s| s.nickname);
        let joined = server.as_ref().map_or(0, |s| s.joined);
        // What it wears: in a server the picks there, else your own (the draft's), as others see it.
        let (effect, decoration) = self.account_key().map_or((None, None), |key| {
            self.core.shared.read(|s| {
                let Some(i) = s.instance(&key) else { return (None, None) };
                let effect = i.effects_on().then(|| match &server {
                    Some(look) if !look.effect.is_empty() => resolve_effect(look.effect, i.server_list(look.server_id)),
                    _ => resolve_effect(&d.effect, &i.profile_items),
                });
                let decoration = i.decorations_on().then(|| match &server {
                    Some(look) if !look.decoration.is_empty() => {
                        resolve_decoration(look.decoration, i.server_list(look.server_id))
                    }
                    _ => resolve_decoration(&d.decoration, &i.profile_items),
                });
                (effect.flatten(), decoration.flatten().map(|item| item.picture_url.clone()))
            })
        });
        // Your dot, where the instance follows who's online.
        let presence = self.account_key().and_then(|key| {
            self.core.shared.read(|s| {
                let people = s.instance(&key)?.people.as_ref()?;
                Some(crate::ui::presence::shown(people.get(&me.id)))
            })
        });
        let mut user = me.clone();
        user.display_name = if d.name.trim().is_empty() { me.username.clone() } else { d.name.trim().to_owned() };
        if let Some(nick) = nickname.map(str::trim).filter(|n| !n.is_empty()) {
            user.display_name = nick.to_owned();
        }
        if is_link(&d.avatar) {
            user.avatar_url = d.avatar.trim().to_owned();
        }
        let status = d.status.trim().to_owned();
        let pronouns = d.pronouns.trim().to_owned();
        let bio = d.bio.trim().to_owned();
        let ready = self.account.ready;
        let banner =
            banner_of(&me.id, if is_link(&d.banner) { d.banner.trim().to_owned() } else { String::new() }, d.accent, p)
                .h(px(112.0));
        let hidden = self.core.prefs().hides_personal();
        let effect =
            effect.map(|spec| self.effect_layer("preview-card", &spec, &me.id, d.accent, (288.0, 420.0), true, cx));
        let card = div()
            .relative()
            .overflow_hidden()
            .rounded(radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .shadow(crate::ui::settings_controls::shadow_xl())
            .child(banner.rounded_t(radius_3xl()))
            .child(
                div()
                    .relative()
                    .px(px(16.0))
                    .pb(px(16.0))
                    .child(
                        div()
                            .mt(px(-44.0))
                            .flex()
                            .items_end()
                            .gap(px(8.0))
                            .child(
                                // Your picture in a ring of the card's color, as the card draws it.
                                div()
                                    .flex_none()
                                    .size(px(86.0))
                                    .relative()
                                    .child(
                                        div()
                                            .absolute()
                                            .left(px(-1.0))
                                            .top(px(-1.0))
                                            .size(px(88.0))
                                            .rounded_full()
                                            .bg(p.card),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .left(px(3.0))
                                            .top(px(3.0))
                                            .size(px(80.0))
                                            .child(crate::ui::widgets::decorated(
                                                avatar(Some(&user), 80.0, p),
                                                80.0,
                                                decoration.as_deref(),
                                            ))
                                            .children(presence.map(|status| {
                                                div().absolute().right(px(-3.0)).bottom(px(-3.0)).child(
                                                    crate::ui::presence::ringed_dot(
                                                        status,
                                                        20.0,
                                                        5.0,
                                                        p.card.into(),
                                                        p,
                                                    ),
                                                )
                                            })),
                                    ),
                            )
                            .when(!status.is_empty(), |el| {
                                el.child(
                                    div()
                                        .mb(px(36.0))
                                        .min_w_0()
                                        .rounded(radius_2xl())
                                        .rounded_bl(px(6.0))
                                        .border_1()
                                        .border_color(p.border)
                                        .bg(p.card)
                                        .px(px(12.0))
                                        .py(px(6.0))
                                        .text_sm()
                                        .shadow(crate::ui::settings_controls::shadow_sm())
                                        .child(status),
                                )
                            }),
                    )
                    .child(
                        div()
                            .mt(px(8.0))
                            .child(
                                div().text_xl().font_weight(FontWeight::EXTRA_BOLD).truncate().child(user_name(&user)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_x(px(6.0))
                                    .text_sm()
                                    .text_color(p.muted_foreground)
                                    .child(if hidden {
                                        "@••••••".to_owned()
                                    } else {
                                        format!("@{}", user.username)
                                    })
                                    .when(!pronouns.is_empty(), |el| {
                                        el.child(
                                            div()
                                                .rounded_full()
                                                .bg(p.muted)
                                                .px(px(8.0))
                                                .py(px(2.0))
                                                .text_xs()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(alpha(p.foreground, 0.8))
                                                .child(pronouns),
                                        )
                                    }),
                            ),
                    )
                    .when(!bio.is_empty() || !ready, |el| {
                        el.child(
                            div()
                                .mt(px(12.0))
                                .rounded(radius_2xl())
                                .bg(alpha(p.muted, 0.6))
                                .p(px(12.0))
                                .child(
                                    caps(&t("workspace.profile.aboutMe"), p)
                                        .font_weight(FontWeight::EXTRA_BOLD)
                                        .mb(px(4.0)),
                                )
                                .child(if bio.is_empty() {
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(6.0))
                                        .py(px(4.0))
                                        .child(div().h(px(12.0)).w(relative_w(0.9)).rounded(px(4.0)).bg(p.muted))
                                        .child(div().h(px(12.0)).w(relative_w(0.66)).rounded(px(4.0)).bg(p.muted))
                                        .into_any_element()
                                } else {
                                    div()
                                        .text_sm()
                                        .child(
                                            crate::ui::text::markdown(
                                                "profile-preview-bio",
                                                crate::ui::text::images_as_links(&bio),
                                            )
                                            .w_full(),
                                        )
                                        .into_any_element()
                                }),
                        )
                    })
                    .when(self.account.created_at > 0 || joined > 0, |el| {
                        use chrono::TimeZone as _;
                        let day = |ms: i64| {
                            chrono::Local
                                .timestamp_millis_opt(ms)
                                .single()
                                .map(|d| d.format("%b %-d, %Y").to_string())
                                .unwrap_or_default()
                        };
                        let since = self.account.created_at;
                        el.child(
                            div()
                                .mt(px(12.0))
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .text_xs()
                                .text_color(p.muted_foreground)
                                .when(since > 0, |el| {
                                    el.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(icon("calendar-heart").size(px(14.0)))
                                            .child(t_with(
                                                "workspace.profile.since",
                                                &[("date", Arg::Str(&day(since)))],
                                            )),
                                    )
                                })
                                .when(joined > 0, |el| {
                                    el.child(
                                        div().pl(px(20.0)).child(t_with(
                                            "workspace.profile.joined",
                                            &[("date", Arg::Str(&day(joined)))],
                                        )),
                                    )
                                }),
                        )
                    }),
            )
            .children(effect);
        card.into_any_element()
    }

    /// A picture you set by uploading one, or from a link (the web's `PictureField`, without
    /// the cropper: pictures go up as they are).
    fn picture_field(&mut self, kind: Kind, me: &pb::User, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let f = &self.account;
        let (value, link_open, link) = match kind {
            Kind::Avatar => (f.avatar.clone(), f.links.0, f.avatar_link.clone()),
            Kind::Banner => (f.banner.clone(), f.links.1, f.banner_link.clone()),
        };
        let busy = f.uploading == Some(kind);
        let id = if kind == Kind::Avatar { "avatar" } else { "banner" };
        let shown = (!value.is_empty() && is_link(&value)).then(|| value.clone());
        let fallback: AnyElement = match kind {
            Kind::Avatar => {
                let mut u = me.clone();
                u.avatar_url = String::new();
                avatar(Some(&u), 80.0, p).into_any_element()
            }
            Kind::Banner => banner_of(&me.id, String::new(), f.accent, p).size_full().into_any_element(),
        };
        let tile = div()
            .id(SharedString::from(format!("pic-{id}")))
            .group(SharedString::from(format!("pic-{id}")))
            .relative()
            .flex_none()
            .overflow_hidden()
            .border_1()
            .border_color(p.border)
            .bg(p.muted)
            .cursor_pointer()
            .map(|el| match kind {
                Kind::Avatar => el.size(px(80.0)).rounded_full(),
                Kind::Banner => el.w(px(240.0)).h(px(96.0)).rounded(radius_2xl()),
            })
            .on_click(cx.listener(move |this, _, _, cx| this.pick_picture(kind, cx)))
            .child(match &shown {
                // Rounded itself: the tile doesn't clip it to its corners.
                Some(url) => img(SharedString::from(url.clone()))
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .map(|el| match kind {
                        Kind::Avatar => el.rounded_full(),
                        Kind::Banner => el.rounded(radius_2xl()),
                    })
                    .into_any_element(),
                None => fallback,
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(2.0))
                    .bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.45))
                    .text_color(rgb(0xffffff))
                    .text_size(px(10.4))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .when(kind == Kind::Avatar, |el| el.rounded_full())
                    .opacity(if busy { 1.0 } else { 0.0 })
                    .id(SharedString::from(format!("pic-{id}-shade")))
                    .group_hover(SharedString::from(format!("pic-{id}")), |s| s.opacity(1.0))
                    .child(icon(if busy { "loader-circle" } else { "camera" }).size(px(20.0)))
                    .child(if busy { String::new() } else { t("workspace.picture.changeShort").to_uppercase() }),
            );
        let actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .child(
                button(
                    SharedString::from(format!("pic-{id}-pick")),
                    if value.is_empty() {
                        t("workspace.picture.uploadShort")
                    } else {
                        t("workspace.picture.changeShort")
                    },
                    Some("image-up"),
                    Look::Outline,
                    false,
                    p,
                )
                .rounded(radius_xl())
                .on_click(cx.listener(move |this, _, _, cx| this.pick_picture(kind, cx))),
            )
            .when(!value.is_empty(), |el| {
                el.child(
                    button(
                        SharedString::from(format!("pic-{id}-remove")),
                        t("system.picture.remove"),
                        Some("trash"),
                        Look::Ghost,
                        false,
                        p,
                    )
                    .rounded(radius_xl())
                    .text_color(p.muted_foreground)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        match kind {
                            Kind::Avatar => this.account.avatar.clear(),
                            Kind::Banner => this.account.banner.clear(),
                        }
                        let link = if kind == Kind::Avatar {
                            this.account.avatar_link.clone()
                        } else {
                            this.account.banner_link.clone()
                        };
                        link.update(cx, |s, cx| s.set_value("", window, cx));
                        this.account.picture_error = None;
                        cx.notify();
                    })),
                )
            })
            .child({
                let fg = p.foreground;
                div()
                    .id(SharedString::from(format!("pic-{id}-link")))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .rounded(radius_lg())
                    .px(px(6.0))
                    .py(px(4.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .cursor_pointer()
                    .hover(move |s| s.text_color(fg))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        match kind {
                            Kind::Avatar => this.account.links.0 = !this.account.links.0,
                            Kind::Banner => this.account.links.1 = !this.account.links.1,
                        }
                        cx.notify();
                    }))
                    .child(icon("link").size(px(14.0)))
                    .child(if link_open || !is_link(&value) {
                        t("workspace.picture.hideLink")
                    } else {
                        t("workspace.picture.useLink")
                    })
            });
        // The link field follows what's typed into it.
        let typed = link.read(cx).value().to_string();
        if (link_open || !is_link(&value)) && typed != value {
            match kind {
                Kind::Avatar => self.account.avatar = typed,
                Kind::Banner => self.account.banner = typed,
            }
        }
        let error = self.account.picture_error.as_ref().filter(|(k, _)| *k == kind).map(|(_, e)| e.clone());
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(div().flex().items_center().gap(px(16.0)).child(tile).child(actions))
            .when(link_open || !is_link(&value), |el| {
                el.child(motion::rise(
                    div().pt(px(4.0)).child(field(Input::new(&link).appearance(false), p)),
                    SharedString::from(format!("pic-{id}-link-in")),
                    Duration::ZERO,
                    -6.0,
                ))
            })
            .when_some(error, |el, e| el.child(warn(e, p)))
            .into_any_element()
    }

    /// Swatches for the profile color: fuwa's pick, a palette, and any color at all.
    fn color_picker(
        &mut self,
        me: &pb::User,
        ready: bool,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let value = self.account.accent;
        let custom = value >= 0 && !COLORS.contains(&(value as u32));
        let ring = |el: gpui_kit::Stateful<gpui_kit::Div>, on: bool| {
            // The web's ring-2 ring-primary ring-offset-2.
            div()
                .flex_none()
                .size(px(44.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .border_2()
                .border_color(if on { p.primary.into() } else { alpha(p.primary, 0.0) })
                .child(el)
        };
        let swatch = |id: String| {
            div()
                .id(SharedString::from(id))
                .size(px(36.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.translate_y(px(-2.0)))
                .active(|s| s.scale(0.9))
        };
        let mut row = div().flex().flex_wrap().gap(px(0.0)).when(!ready, |el| el.opacity(0.5));
        row = row.child(ring(
            crate::ui::widgets::hue_gradient(
                &me.id,
                18.0,
                div().size(px(36.0)).rounded_full().flex().items_center().justify_center(),
            )
            .id("color-auto")
            .cursor_pointer()
            .hover(|s| s.translate_y(px(-2.0)))
            .active(|s| s.scale(0.9))
            .on_click(cx.listener(|this, _, _, cx| {
                this.account.accent = -1;
                cx.notify();
            }))
            .child(icon("sparkles").size(px(16.0)).text_color(rgb(0xffffff))),
            value < 0,
        ));
        for c in COLORS {
            let on = value == c as i32;
            row = row.child(ring(
                swatch(format!("color-{c:06x}"))
                    .bg(rgb(c))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.account.accent = c as i32;
                        cx.notify();
                    }))
                    .when(on, |el| el.child(icon("check").size(px(16.0)).text_color(rgb(0xffffff)))),
                on,
            ));
        }
        let rainbow = if custom { rgb(value as u32).into() } else { gpui_kit::hsla(0.9, 0.7, 0.65, 1.0) };
        row = row.child(ring(
            swatch("color-any".into())
                .bg(rainbow)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.account.any_color = !this.account.any_color;
                    cx.notify();
                }))
                .child(icon("pipette").size(px(16.0)).text_color(rgb(0xffffff))),
            custom,
        ));
        // Any color: typed as #rrggbb.
        if self.account.any_color {
            let typed = self.account.color_text.read(cx).value().to_string();
            if let Some(c) = crate::core::themes::parse_hex(typed.trim())
                && c as i32 != value
            {
                self.account.accent = c as i32;
            }
            let _ = window;
        }
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(row)
            .when(self.account.any_color, |el| {
                el.child(motion::rise(
                    div().w(px(200.0)).child(field(Input::new(&self.account.color_text).appearance(false), p)),
                    "color-any-in",
                    Duration::ZERO,
                    -6.0,
                ))
            })
            .into_any_element()
    }

    /// Your status, a button to clear it, and when it clears by itself.
    fn status_field(&mut self, d: &Draft, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let base = self.account.base.clone().unwrap_or_default();
        let kept = base.expires.filter(|_| !base.status.is_empty());
        let mut options: Vec<Clear> = Vec::new();
        if kept.is_some() {
            options.push(Clear::Keep);
        }
        options.extend(CLEARS);
        let chosen = options.iter().position(|c| *c == self.account.clear).unwrap_or(0);
        let labels = options.iter().map(|c| clear_label(*c, kept)).collect();
        let (hover_bg, hover_fg) = (p.muted, p.foreground);
        div()
            .flex()
            .flex_col()
            .child(
                div().relative().child(field(Input::new(&self.account.status).appearance(false), p).pr(px(44.0))).when(
                    !d.status.is_empty(),
                    |el| {
                        el.child(
                            div()
                                .id("status-clear")
                                .absolute()
                                .top(px(6.0))
                                .right(px(6.0))
                                .size(px(32.0))
                                .rounded(radius_lg())
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(p.muted_foreground)
                                .cursor_pointer()
                                .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.account.status.update(cx, |s, cx| s.set_value("", window, cx));
                                    cx.notify();
                                }))
                                .child(icon("x").size(px(16.0))),
                        )
                    },
                ),
            )
            .when(!d.status.trim().is_empty(), |el| {
                el.child(motion::rise(
                    div()
                        .child(
                            div()
                                .pt(px(12.0))
                                .pb(px(8.0))
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.muted_foreground)
                                .child(t("accountsettings.profile.clearAfter")),
                        )
                        .child(chips("status-clear", labels, chosen, p, cx, move |this, n, cx| {
                            this.account.clear = options[n];
                            cx.notify();
                        })),
                    "status-clear-in",
                    Duration::ZERO,
                    -6.0,
                ))
            })
            .into_any_element()
    }

    /// About me, written or previewed as it will show.
    fn bio_field(
        &mut self,
        d: &Draft,
        ready: bool,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let preview = self.account.bio_preview;
        let tabs = segmented(
            "bio-tab",
            vec![
                (t("accountsettings.profile.write"), Some("pencil-line")),
                (t("accountsettings.profile.preview"), Some("eye")),
            ],
            usize::from(preview),
            96.0,
            p,
            window,
            cx,
            |this, n, cx| {
                this.account.bio_preview = n == 1;
                cx.notify();
            },
        );
        let body: AnyElement = if preview {
            motion::slide_in(
                div()
                    .min_h(px(128.0))
                    .rounded(radius_xl())
                    .border_1()
                    .border_color(p.border)
                    .bg(alpha(p.muted, 0.4))
                    .px(px(12.0))
                    .py(px(8.0))
                    .text_sm()
                    .child(if d.bio.trim().is_empty() {
                        div()
                            .text_color(p.muted_foreground)
                            .child(t("accountsettings.profile.nothingToPreview"))
                            .into_any_element()
                    } else {
                        crate::ui::text::markdown("bio-preview", crate::ui::text::images_as_links(&d.bio))
                            .w_full()
                            .into_any_element()
                    }),
                "bio-preview-in",
                10.0,
            )
            .into_any_element()
        } else {
            motion::slide_in(
                div()
                    .min_h(px(128.0))
                    .rounded(radius_xl())
                    .border_1()
                    .border_color(p.border)
                    .px(px(12.0))
                    .py(px(8.0))
                    .text_sm()
                    .when(!ready, |el| el.opacity(0.5))
                    .child(Textarea::new(&self.account.bio).appearance(false)),
                "bio-write-in",
                -10.0,
            )
            .into_any_element()
        };
        div().flex().flex_col().gap(px(8.0)).child(div().flex().child(tabs)).child(body).into_any_element()
    }

    // ───────────────────────── Password ─────────────────────────

    pub(crate) fn password_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(key) = self.account_key() else { return div().into_any_element() };
        let place = self.place(&key);
        if self.account.password_done {
            return motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(12.0))
                    .rounded(radius_3xl())
                    .border_1()
                    .border_color(p.border)
                    .bg(p.card)
                    .px(px(24.0))
                    .py(px(40.0))
                    .child(
                        div()
                            .size(px(56.0))
                            .rounded_full()
                            .bg(rgb(0x10b981))
                            .text_color(rgb(0xffffff))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon("check").size(px(28.0))),
                    )
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(t("accountsettings.password.changed")),
                    )
                    .child(
                        div()
                            .max_w(px(384.0))
                            .text_sm()
                            .text_color(p.muted_foreground)
                            .text_center()
                            .child(t_with("accountsettings.password.changedHint", &[("instance", Arg::Str(&place))])),
                    )
                    .child(
                        button("password-done", t("accountsettings.shared.done"), None, Look::Outline, false, p)
                            .mt(px(8.0))
                            .rounded(radius_xl())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.account.password_done = false;
                                cx.notify();
                            })),
                    ),
                "password-done-in",
                Duration::ZERO,
                10.0,
            )
            .into_any_element();
        }
        let current = self.account.current.read(cx).value().to_string();
        let next = self.account.new.read(cx).value().to_string();
        let again = self.account.again.read(cx).value().to_string();
        let len = next.chars().count();
        let too_short = len > 0 && len < PASSWORD_MIN;
        let mismatch = !again.is_empty() && again != next;
        let same = !next.is_empty() && next == current;
        let show = self.account.show_password;
        for state in [&self.account.current, &self.account.new, &self.account.again] {
            state.update(cx, |s, cx| s.set_masked(!show, window, cx));
        }
        let new_hint = if same {
            warn(t("accountsettings.password.same"), p)
        } else if too_short {
            warn(t_with("accountsettings.password.min", &[("count", Arg::Num(PASSWORD_MIN as i64))]), p)
        } else {
            hint(
                t_with(
                    "accountsettings.password.range",
                    &[("min", Arg::Num(PASSWORD_MIN as i64)), ("max", Arg::Num(PASSWORD_MAX as i64))],
                ),
                p,
            )
        };
        let level = strength(&next);
        let rows = [
            (
                "current-password",
                t("accountsettings.password.current"),
                None,
                self.password_input("pw-current", &self.account.current.clone(), false, p, window, cx),
            ),
            (
                "new-password",
                t("accountsettings.password.new"),
                Some(new_hint),
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(self.password_input("pw-new", &self.account.new.clone(), false, p, window, cx))
                    .child(strength_meter(level, !next.is_empty(), p, window, cx))
                    .into_any_element(),
            ),
            (
                "confirm-password",
                t("accountsettings.password.again"),
                mismatch.then(|| warn(t("accountsettings.password.mismatch"), p)),
                self.password_input(
                    "pw-again",
                    &self.account.again.clone(),
                    !again.is_empty() && again == next,
                    p,
                    window,
                    cx,
                ),
            ),
        ];
        let mut form = div().max_w(px(448.0)).flex().flex_col();
        for (n, (id, label, h, body)) in rows.into_iter().enumerate() {
            form = form.child(self.row(id, &label, h, At::of(n, 3), body, p));
        }
        let ready = !current.is_empty() && (PASSWORD_MIN..=PASSWORD_MAX).contains(&len) && again == next && !same;
        let busy = self.account.password_busy;
        let form = form
            .when_some(self.account.password_error.clone(), |el, e| el.child(div().pb(px(12.0)).child(warn(e, p))))
            .child(
                div().flex().child(
                    button(
                        "password-save",
                        if busy {
                            t("accountsettings.password.changing")
                        } else {
                            t("accountsettings.password.change")
                        },
                        Some("key-round"),
                        Look::Primary,
                        false,
                        p,
                    )
                    .rounded(radius_xl())
                    .px(px(20.0))
                    .font_weight(FontWeight::BOLD)
                    .when(!ready || busy, |el| el.opacity(0.5))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if ready && !busy {
                            this.change_password(window, cx)
                        } else {
                            this.account.password_shake = Some(std::time::Instant::now());
                            cx.notify();
                        }
                    })),
                ),
            );
        match self.account.password_shake {
            Some(at) => motion::once(
                form,
                SharedString::from(format!("pw-shake-{at:?}")),
                Duration::from_millis(400),
                |el, t| el.relative().left(px((t * std::f32::consts::TAU * 2.5).sin() * 8.0 * (1.0 - t))),
            ),
            None => form.into_any_element(),
        }
    }

    /// A password field with a button that shows what's typed, and a tick once it matches.
    fn password_input(
        &self,
        id: &'static str,
        state: &Entity<InputState>,
        matches: bool,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let show = self.account.show_password;
        let (hover_bg, hover_fg) = (p.muted, p.foreground);
        // The green check while the two match: it springs in from nothing and shrinks away
        // when they stop matching (the web's `Matches`).
        let going = motion::kept(&format!("{id}-match"), matches.then_some(&()), window, cx);
        let check = || {
            div()
                .absolute()
                .top(px(12.0))
                .right(px(48.0))
                .size(px(20.0))
                .rounded_full()
                .bg(rgb(0x10b981))
                .text_color(rgb(0xffffff))
                .flex()
                .items_center()
                .justify_center()
                .child(icon("check").size(px(14.0)))
        };
        div()
            .relative()
            .child(field(Input::new(state).appearance(false), p).pr(px(80.0)))
            .when(matches, |el| {
                el.child(motion::spring_in(
                    check(),
                    SharedString::from(format!("{id}-match")),
                    (600.0, 18.0),
                    Duration::ZERO,
                    |el, t| el.opacity(t.clamp(0.0, 1.0)).scale(t.max(0.0)),
                ))
            })
            .when_some(going, |el, ((), t)| {
                let e = crate::ui::settings_controls::gone(t);
                el.child(check().opacity(1.0 - e).scale(1.0 - e))
            })
            .child(
                div()
                    .id(SharedString::from(format!("{id}-eye")))
                    .absolute()
                    .top(px(6.0))
                    .right(px(6.0))
                    .size(px(32.0))
                    .rounded(radius_lg())
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(p.muted_foreground)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                    .active(|s| s.scale(0.9))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.account.show_password = !this.account.show_password;
                        cx.notify();
                    }))
                    // The eye swaps with a turn as it opens or shuts.
                    .child(motion::pop(
                        div().child(icon(if show { "eye-off" } else { "eye" }).size(px(16.0))),
                        SharedString::from(format!("{id}-eye-{show}")),
                        0.6,
                        -40.0,
                        Duration::ZERO,
                    )),
            )
            .into_any_element()
    }

    fn change_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.account.key.clone() else { return };
        let current = self.account.current.read(cx).value().to_string();
        let new = self.account.new.read(cx).value().to_string();
        self.account.password_busy = true;
        self.account.password_error = None;
        let core = self.core.clone();
        let rx = self.core.spawn(async move { core.change_password(&key, &current, &new).await });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.account.password_busy = false;
                match result {
                    Ok(()) => {
                        this.account.password_done = true;
                        this.account.sessions = None;
                        for s in [this.account.current.clone(), this.account.new.clone(), this.account.again.clone()] {
                            s.update(cx, |s, cx| s.set_value("", window, cx));
                        }
                    }
                    Err(err) => {
                        this.account.password_error = Some(err.message);
                        this.account.password_shake = Some(std::time::Instant::now());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    // ───────────────────────── Devices ─────────────────────────

    fn load_sessions(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.account.key.clone() else { return };
        let core = self.core.clone();
        let rx = self.core.spawn(async move { core.sessions(&key).await });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(list) => {
                        this.account.sessions = Some(list);
                        this.account.sessions_error = None;
                    }
                    Err(err) => this.account.sessions_error = Some(err.message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn devices_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some((key, _)) = self.account_ready(window, cx) else { return div().into_any_element() };
        if self.account.sessions.is_none() && self.account.sessions_error.is_none() && !self.account.signing_out {
            self.account.signing_out = true;
            self.load_sessions(cx);
        }
        if self.account.sessions.is_some() {
            self.account.signing_out = false;
        }
        let place = self.place(&key);
        if let (Some(e), None) = (&self.account.sessions_error, &self.account.sessions) {
            return div()
                .flex()
                .flex_col()
                .items_start()
                .gap(px(12.0))
                .rounded(radius_2xl())
                .border_1()
                .border_color(alpha(p.destructive, 0.4))
                .bg(alpha(p.destructive, 0.05))
                .p(px(16.0))
                .child(warn(e.clone(), p))
                .child(
                    button("devices-retry", t("accountsettings.shared.tryAgain"), None, Look::Outline, true, p)
                        .rounded(radius_xl())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.account.sessions_error = None;
                            cx.notify();
                        })),
                )
                .into_any_element();
        }
        let sessions = self.account.sessions.clone();
        let current = sessions.as_ref().and_then(|l| l.iter().find(|s| s.current).cloned());
        let others: Vec<pb::Session> = sessions.iter().flatten().filter(|s| !s.current).cloned().collect();
        let shimmer = |op: f32| div().h(px(72.0)).rounded(radius_2xl()).bg(p.muted).opacity(op);
        let head = |text: String| caps(&text, p).font_weight(FontWeight::EXTRA_BOLD).mb(px(12.0));
        let this_device = div().child(head(t("accountsettings.devices.thisDevice"))).child(match &current {
            Some(s) => device_row(s, true, p).into_any_element(),
            None => shimmer(1.0).into_any_element(),
        });
        let mut list = div().flex().flex_col().gap(px(8.0));
        match &sessions {
            None => list = list.child(shimmer(1.0)).child(shimmer(0.6)),
            Some(_) if others.is_empty() => {
                list = list.child(motion::rise(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .rounded(radius_2xl())
                        .border_1()
                        .border_dashed()
                        .border_color(p.border)
                        .p(px(16.0))
                        .child(
                            div()
                                .size(px(40.0))
                                .flex_none()
                                .rounded(radius_xl())
                                .bg(alpha(rgb(0x10b981), 0.15))
                                .text_color(rgb(0x10b981))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon("shield-check").size(px(20.0))),
                        )
                        .child(
                            div()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::BOLD)
                                        .child(t("accountsettings.devices.onlyThis")),
                                )
                                .child(div().text_sm().text_color(p.muted_foreground).child(t_with(
                                    "accountsettings.devices.onlyThisHint",
                                    &[("instance", Arg::Str(&place))],
                                ))),
                        ),
                    "devices-none",
                    Duration::ZERO,
                    8.0,
                ))
            }
            Some(_) => {
                for (n, s) in others.iter().enumerate() {
                    let id = s.id.clone();
                    let leaving = self.account.leaving.contains(&id);
                    let (hover_bg, hover_fg) = (alpha(p.destructive, 0.1), p.destructive);
                    let out = div()
                        .id(SharedString::from(format!("revoke-{id}")))
                        .flex_none()
                        .h(px(32.0))
                        .px(px(10.0))
                        .rounded(radius_xl())
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(p.muted_foreground)
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                        .on_click(cx.listener(move |this, _, _, cx| this.revoke(Some(id.clone()), cx)))
                        .child(icon("log-out").size(px(16.0)))
                        .child(if leaving {
                            t("accountsettings.shared.signingOut")
                        } else {
                            t("accountsettings.shared.signOut")
                        });
                    list = list.child(motion::rise(
                        device_row(s, false, p).child(out),
                        SharedString::from(format!("session-{}", s.id)),
                        Duration::from_millis(40 * n.min(10) as u64),
                        12.0,
                    ));
                }
            }
        }
        let count_title = if sessions.is_some() {
            t_with("accountsettings.devices.othersCount", &[("count", Arg::Num(others.len() as i64))])
        } else {
            t("accountsettings.devices.others")
        };
        let mut page =
            div().flex().flex_col().gap(px(32.0)).child(this_device).child(div().child(head(count_title)).child(list));
        if others.len() > 1 {
            let confirm = self.account.confirm_all;
            let n = others.len() as i64;
            page = page.child(motion::rise(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(12.0))
                    .rounded(radius_2xl())
                    .border_1()
                    .border_color(p.border)
                    .p(px(16.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .child(t("accountsettings.devices.signOutAll")),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(p.muted_foreground)
                                    .child(t("accountsettings.devices.signOutAllHint")),
                            ),
                    )
                    .child(if confirm {
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(
                                button("devices-cancel", t("common.cancel"), None, Look::Ghost, true, p)
                                    .rounded(radius_xl())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.account.confirm_all = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                button(
                                    "devices-all",
                                    t_with("accountsettings.devices.signOutCount", &[("count", Arg::Num(n))]),
                                    None,
                                    Look::Destructive,
                                    true,
                                    p,
                                )
                                .rounded(radius_xl())
                                .font_weight(FontWeight::BOLD)
                                .on_click(cx.listener(|this, _, _, cx| this.revoke(None, cx))),
                            )
                            .into_any_element()
                    } else {
                        button(
                            "devices-ask",
                            t("accountsettings.devices.signOutAllButton"),
                            Some("log-out"),
                            Look::DangerOutline,
                            true,
                            p,
                        )
                        .rounded(radius_xl())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.account.confirm_all = true;
                            cx.notify();
                        }))
                        .into_any_element()
                    }),
                "devices-all-in",
                Duration::ZERO,
                -8.0,
            ));
        }
        page.child(self.message_backup(&key, p, window, cx)).into_any_element()
    }

    /// Signs one device out, or every other one.
    fn revoke(&mut self, session: Option<String>, cx: &mut Context<Self>) {
        let Some(key) = self.account.key.clone() else { return };
        if let Some(id) = &session {
            self.account.leaving.insert(id.clone());
        }
        let core = self.core.clone();
        let one = session.clone();
        let rx = self.core.spawn(async move {
            let n = match &one {
                Some(id) => {
                    core.revoke_session(&key, id).await?;
                    1
                }
                None => core.revoke_other_sessions(&key).await?,
            };
            Ok::<_, crate::core::api::Problem>((n, core.sessions(&key).await?))
        });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                if let Some(id) = &session {
                    this.account.leaving.remove(id);
                }
                this.account.confirm_all = false;
                match result {
                    Ok((n, sessions)) => {
                        this.account.sessions = Some(sessions);
                        let line = if session.is_some() {
                            t("accountsettings.devices.signedOutOne")
                        } else {
                            t_with("accountsettings.devices.signedOutCount", &[("count", Arg::Num(i64::from(n)))])
                        };
                        this.toast("log-out", line, cx);
                    }
                    Err(err) => this.toast("circle-alert", err.message, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

/// `w` as a share of the parent's width.
fn relative_w(share: f32) -> gpui_kit::DefiniteLength {
    gpui_kit::relative(share)
}

/// A rough read of a password's strength, from 0 to 4, for the meter (the web's `strength`).
fn strength(password: &str) -> usize {
    let len = password.chars().count();
    if len < PASSWORD_MIN {
        return 0;
    }
    let kinds = [
        password.chars().any(|c| c.is_ascii_lowercase()),
        password.chars().any(|c| c.is_ascii_uppercase()),
        password.chars().any(|c| c.is_ascii_digit()),
        password.chars().any(|c| !c.is_ascii_alphanumeric()),
    ]
    .iter()
    .filter(|x| **x)
    .count();
    let long = if len >= 16 {
        2
    } else if len >= 12 {
        1
    } else {
        0
    };
    (kinds + long).saturating_sub(1).clamp(1, 4)
}

/// Four bars that fill as the new password gets harder to guess.
fn strength_meter(
    level: usize,
    any: bool,
    p: &Palette,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    let (label, tone): (&str, gpui_kit::Hsla) = match level {
        0 => ("accountsettings.password.tooShort", alpha(p.muted_foreground, 0.4)),
        1 => ("accountsettings.password.weak", p.destructive.into()),
        2 => ("accountsettings.password.fair", rgb(0xf59e0b).into()),
        3 => ("accountsettings.password.good", rgb(0x10b981).into()),
        _ => ("accountsettings.password.strong", rgb(0x10b981).into()),
    };
    let mut bars = div().flex_1().flex().gap(px(4.0));
    for n in 1..=4 {
        let fill =
            motion::follow(SharedString::from(format!("pw-bar-{n}")), if level >= n { 1.0 } else { 0.0 }, window, cx);
        bars = bars.child(
            div()
                .flex_1()
                .h(px(6.0))
                .rounded_full()
                .bg(p.muted)
                .overflow_hidden()
                .child(div().h_full().w(gpui_kit::relative(fill.clamp(0.0, 1.0))).rounded_full().bg(tone)),
        );
    }
    div()
        .flex()
        .items_center()
        .gap(px(12.0))
        .child(bars)
        .child(
            div()
                .w(px(64.0))
                .text_right()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(p.muted_foreground)
                .child(motion::swap_text("pw-strength", if any { t(label) } else { " ".to_owned() }, 12.0, window, cx)),
        )
        .into_any_element()
}

/// A banner: the picture, or the profile color (or fuwa's pick from the id) shaded toward the bottom.
pub(crate) fn banner_of(user_id: &str, picture: String, accent: i32, p: &Palette) -> gpui_kit::Div {
    let _ = p;
    let base =
        if accent < 0 { crate::ui::widgets::hue_gradient(user_id, 0.0, div()) } else { div().bg(rgb(accent as u32)) };
    base.relative()
        .overflow_hidden()
        .w_full()
        .when(accent >= 0, |el| {
            el.child(div().absolute().inset_0().bg(gpui_kit::linear_gradient(
                135.0,
                gpui_kit::linear_color_stop(gpui_kit::hsla(0.0, 0.0, 0.0, 0.0), 0.5),
                gpui_kit::linear_color_stop(gpui_kit::hsla(0.0, 0.0, 0.0, 0.45), 1.0),
            )))
        })
        .when(!picture.is_empty(), |el| {
            el.child(img(SharedString::from(picture)).absolute().inset_0().size_full().object_fit(ObjectFit::Cover))
        })
}

/// One device: what it is, when it was last used and when it signed in.
fn device_row(s: &pb::Session, here: bool, p: &Palette) -> gpui_kit::Stateful<gpui_kit::Div> {
    let (glyph, label) = device_label(&s.user_agent);
    let created = ms_of(s.created_at.as_ref());
    let active = ms_of(s.last_active_at.as_ref()).max(created);
    let mut line = if here {
        t("accountsettings.devices.usingNow")
    } else if active > 0 {
        active_ago(active)
    } else {
        t("accountsettings.devices.notUsed")
    };
    if created > 0 {
        use chrono::TimeZone as _;
        let day = chrono::Local
            .timestamp_millis_opt(created)
            .single()
            .map(|d| d.format("%b %-d, %Y").to_string())
            .unwrap_or_default();
        line.push_str(&format!(" · {}", t_with("accountsettings.devices.signedIn", &[("date", Arg::Str(&day))])));
    }
    let hover = alpha(p.primary, 0.3);
    div()
        .id(SharedString::from(format!("device-{}", s.id)))
        .flex()
        .items_center()
        .gap(px(12.0))
        .rounded(radius_2xl())
        .border_1()
        .p(px(12.0))
        .pr(px(8.0))
        .map(|el| {
            if here {
                el.border_color(alpha(p.primary, 0.4)).bg(alpha(p.primary, 0.05))
            } else {
                el.border_color(p.border).bg(p.card).hover(move |s| s.border_color(hover))
            }
        })
        .child(
            div()
                .relative()
                .size(px(44.0))
                .flex_none()
                .rounded(radius_xl())
                .flex()
                .items_center()
                .justify_center()
                .map(|el| {
                    if here {
                        el.bg(p.primary).text_color(p.primary_foreground)
                    } else {
                        el.bg(p.muted).text_color(p.muted_foreground)
                    }
                })
                .child(icon(glyph).size(px(20.0)))
                .when(here, |el| {
                    el.child(
                        div()
                            .absolute()
                            .right(px(-2.0))
                            .bottom(px(-2.0))
                            .size(px(12.0))
                            .rounded_full()
                            .bg(rgb(0x10b981))
                            .border_2()
                            .border_color(p.card),
                    )
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(div().text_sm().font_weight(FontWeight::BOLD).truncate().child(label))
                .child(div().text_xs().text_color(p.muted_foreground).truncate().child(line)),
        )
}

/// "Active now", "Active 5 minutes ago", "Active yesterday" (the web's `activeAgo`).
fn active_ago(ms: i64) -> String {
    let now = crate::core::dms::now_ms();
    let minutes = ((now - ms) / 60_000).max(0);
    if minutes < 6 {
        return t("common.device.activeNow");
    }
    let when = if minutes < 60 {
        format!("{minutes} minutes ago")
    } else if minutes < 60 * 24 {
        let h = (minutes as f64 / 60.0).round() as i64;
        if h == 1 { "1 hour ago".to_owned() } else { format!("{h} hours ago") }
    } else {
        let d = (minutes as f64 / 60.0 / 24.0).round() as i64;
        if d == 1 {
            "yesterday".to_owned()
        } else if d < 30 {
            format!("{d} days ago")
        } else {
            let m = (d as f64 / 30.0).round() as i64;
            if m == 1 { "last month".to_owned() } else { format!("{m} months ago") }
        }
    };
    t_with("common.device.active", &[("when", Arg::Str(&when))])
}

/// A device, from what it said it was when it signed in (the web's `describeDevice` and `deviceName`).
pub fn device_label(agent: &str) -> (&'static str, String) {
    let has = |s: &str| agent.contains(s);
    let lower = agent.to_lowercase();
    let browser = if lower.contains("fuwa-desktop") {
        t("common.device.desktop")
    } else if has("Edg/") || has("EdgA/") || has("EdgiOS/") || has("Edge/") {
        "Edge".into()
    } else if has("OPR/") || has("Opera") {
        "Opera".into()
    } else if has("SamsungBrowser") {
        "Samsung Internet".into()
    } else if has("Vivaldi") {
        "Vivaldi".into()
    } else if has("Firefox/") || has("FxiOS") {
        "Firefox".into()
    } else if has("CriOS") || has("Chrome/") || has("Chromium") {
        "Chrome".into()
    } else if has("Safari/") {
        "Safari".into()
    } else if ["tonic", "grpc", "okhttp", "curl", "python", "go-http"].iter().any(|s| lower.contains(s)) {
        t("common.device.script")
    } else {
        String::new()
    };
    let script = ["tonic", "grpc", "okhttp", "curl", "python", "go-http"].iter().any(|s| lower.contains(s))
        && !lower.contains("fuwa-desktop");
    let os = if has("iPhone") {
        "iPhone"
    } else if has("iPad") {
        "iPad"
    } else if has("Android") {
        "Android"
    } else if has("CrOS") {
        "ChromeOS"
    } else if has("Windows") || lower.contains("windows") {
        "Windows"
    } else if has("Mac OS X") || has("Macintosh") || lower.contains("macos") {
        "macOS"
    } else if has("Linux") || lower.contains("linux") {
        "Linux"
    } else {
        ""
    };
    let glyph = if has("iPad") || has("Tablet") {
        "tablet"
    } else if has("Mobi") || has("iPhone") || (has("Android") && has("Mobile")) {
        "smartphone"
    } else if script {
        "terminal"
    } else {
        "laptop"
    };
    let name = if !browser.is_empty() && !os.is_empty() {
        t_with("common.device.on", &[("browser", Arg::Str(&browser)), ("os", Arg::Str(os))])
    } else if !browser.is_empty() {
        browser
    } else if !os.is_empty() {
        os.to_owned()
    } else {
        t("common.device.unknown")
    };
    (glyph, name)
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
        assert_eq!(super::device_label("").1, "An unknown device");
    }

    #[test]
    fn passwords_get_stronger_with_length_and_kinds() {
        assert_eq!(super::strength("short"), 0);
        assert_eq!(super::strength("abcdefgh"), 1);
        assert_eq!(super::strength("abcdefgh1A"), 2);
        assert_eq!(super::strength("abcdefgh1A!xyzwq"), 4);
    }
}
