//! The rest of the Welcome & onboarding page: the server's banner (picked,
//! its focal point dragged, checked in the shapes it's shown in) and accent
//! color, and the onboarding steps new members go through. One save bar
//! saves them with the welcome screen, in the web's order: the server, the
//! welcome screen, the onboarding. The web's
//! `settings/server/WelcomeAndOnboarding.tsx`.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::StyledImage as _;
use gpui_kit::{Bounds, ImgResourceLoader, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Resource, canvas, img};

use super::roles::switch;
use super::*;
use crate::core::onboarding::{self as onb, PICK, RULES, SAY_HELLO};
use crate::ui::banner::{accent, cover, hex, hue_of, on_accent, parse_hex};
use crate::ui::overlay::emoji_tile;
use crate::ui::text::{WIDE, tracked};
use crate::ui::theme::{radius_2xl, radius_lg, radius_xl};

/// The shapes a banner is shown in, as the web lists them: a name, and its width and height.
fn crops() -> [(String, f32, f32); 3] {
    [
        (t("serversettings.welcome.cropPhone"), 390.0, 128.0),
        (t("serversettings.welcome.cropBrowse"), 300.0, 80.0),
        (t("serversettings.welcome.cropDialog"), 672.0, 160.0),
    ]
}

/// A step being edited, with keys for it and its choices that survive moving them.
struct Step {
    key: u64,
    step: pb::OnboardingStep,
    options: Vec<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Open {
    Roles,
    Channels,
    Emoji,
}

pub(super) struct Onboard {
    saved: Option<pb::Onboarding>,
    loading: bool,
    enabled: bool,
    steps: Vec<Step>,
    next: u64,
    /// Every box on the page, by the step or choice it's in and which one.
    fields: HashMap<(u64, &'static str), Entity<InputState>>,
    field_changes: Vec<Subscription>,
    /// A choice whose roles, channels or emoji are open under it.
    open: Option<(u64, Open)>,
    /// The banner, its focus and accent as they're being edited (`None` until filled).
    look: Option<(String, (i32, i32), i32)>,
    hex: Entity<InputState>,
    /// The banner's link, typed in ("Use a link"), and whether its box is open.
    link: Entity<InputState>,
    link_open: bool,
    uploading: bool,
    /// Where the focal point editor is on screen, for turning a click into a focus.
    focus_box: Rc<Cell<Option<Bounds<Pixels>>>>,
    dragging: bool,
    pub(super) saving: bool,
}

impl Onboard {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ServerSettingsView>) -> (Self, Vec<Subscription>) {
        let hex = cx.new(|cx| InputState::new(window, cx).placeholder("#ff88aa"));
        let mut subs = vec![cx.subscribe(&hex, |this: &mut ServerSettingsView, input, e: &InputEvent, cx| {
            if let InputEvent::Change = e {
                if let Some(color) = parse_hex(&input.read(cx).value())
                    && let Some(look) = this.onboard.look.as_mut()
                {
                    look.2 = color;
                }
                cx.notify()
            }
        })];
        let link = cx.new(|cx| InputState::new(window, cx).placeholder("https://…"));
        subs.push(cx.subscribe(&link, |this: &mut ServerSettingsView, input, e: &InputEvent, cx| {
            if let InputEvent::Change = e {
                let typed = input.read(cx).value().trim().to_string();
                if let Some(look) = this.onboard.look.as_mut()
                    && (typed.is_empty() || typed.starts_with("https://") || typed.starts_with("http://"))
                    && look.0 != typed
                {
                    look.0 = typed;
                    look.1 = (50, 50);
                }
                cx.notify()
            }
        }));
        let onboard = Self {
            saved: None,
            loading: false,
            enabled: false,
            steps: Vec::new(),
            next: 0,
            fields: HashMap::new(),
            field_changes: Vec::new(),
            open: None,
            look: None,
            hex,
            link,
            link_open: false,
            uploading: false,
            focus_box: Rc::new(Cell::new(None)),
            dragging: false,
            saving: false,
        };
        (onboard, subs)
    }
}

/// The banner, focus and accent a server has, as the editor holds them (-1 for no accent).
fn look_of(server: &pb::Server) -> (String, (i32, i32), i32) {
    (server.banner_url.clone(), (server.banner_focus_x, server.banner_focus_y), server.accent_color.unwrap_or(-1))
}

/// Up to five colors that stand out in a picture (BGRA bytes, `w`×`h`), most
/// common first: saturated, not too dark or light, a hue apart.
pub(crate) fn swatches(bgra: &[u8], w: usize, h: usize) -> Vec<i32> {
    if w == 0 || h == 0 || bgra.len() < w * h * 4 {
        return Vec::new();
    }
    // Twelve hues, each adding up its pixels' colors.
    let mut buckets = [(0u64, 0u64, 0u64, 0u64); 12];
    let step = ((w * h) / 4000).max(1);
    for n in (0..w * h).step_by(step) {
        let px = &bgra[n * 4..n * 4 + 4];
        let (b, g, r, a) = (px[0], px[1], px[2], px[3]);
        if a < 128 {
            continue;
        }
        let c: Hsla = gpui_kit::Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 }.into();
        if c.s < 0.3 || c.l < 0.2 || c.l > 0.85 {
            continue;
        }
        let bucket = &mut buckets[((c.h * 12.0) as usize).min(11)];
        bucket.0 += 1;
        bucket.1 += u64::from(r);
        bucket.2 += u64::from(g);
        bucket.3 += u64::from(b);
    }
    let mut ranked: Vec<_> = buckets.iter().filter(|b| b.0 > 0).collect();
    ranked.sort_by_key(|b| std::cmp::Reverse(b.0));
    ranked.into_iter().take(5).map(|&(n, r, g, b)| ((r / n) << 16 | (g / n) << 8 | (b / n)) as i32).collect()
}

/// How many of an onboarding's two parts differ from what's saved.
fn onboarding_changes(saved: &pb::Onboarding, now: &pb::Onboarding) -> usize {
    let saved = onb::cleaned(saved.clone());
    [saved.enabled != now.enabled, saved.steps != now.steps].into_iter().filter(|c| *c).count()
}

impl ServerSettingsView {
    fn load_onboarding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.onboard.loading || self.onboard.saved.is_some() {
            return;
        }
        self.onboard.loading = true;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let rx = core.spawn({
            let core = core.clone();
            async move { core.onboarding(&key, &sid).await }
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.onboard.loading = false;
                match result {
                    Ok(onboarding) => this.fill_onboarding(onboarding, window, cx),
                    Err(err) => {
                        this.onboard.saved = Some(pb::Onboarding::default());
                        this.error = Some(err.message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Fills the steps from the onboarding as saved.
    pub(super) fn fill_onboarding(&mut self, onboarding: pb::Onboarding, window: &mut Window, cx: &mut Context<Self>) {
        let o = &mut self.onboard;
        o.enabled = onboarding.enabled;
        o.steps.clear();
        o.fields.clear();
        o.field_changes.clear();
        o.open = None;
        for step in &onboarding.steps {
            self.add_step(step.clone(), window, cx);
        }
        self.onboard.saved = Some(onboarding);
    }

    /// Fills the banner, focus and accent from the server as saved.
    pub(super) fn fill_look(&mut self, server: &pb::Server, window: &mut Window, cx: &mut Context<Self>) {
        let look = look_of(server);
        let text = if look.2 >= 0 { hex(look.2) } else { String::new() };
        self.onboard.hex.update(cx, |s, cx| s.set_value(text, window, cx));
        self.onboard.look = Some(look);
    }

    fn add_step(&mut self, step: pb::OnboardingStep, window: &mut Window, cx: &mut Context<Self>) {
        let o = &mut self.onboard;
        o.next += 1;
        let key = o.next;
        let options = step
            .options
            .iter()
            .map(|_| {
                o.next += 1;
                o.next
            })
            .collect::<Vec<_>>();
        self.field(key, "title", &step.title, &t("serversettings.onboarding.title"), window, cx);
        self.field(key, "description", &step.description, &t("serversettings.onboarding.description"), window, cx);
        self.field(key, "hello", &step.hello, &t("join.onboarding.hello"), window, cx);
        for (k, option) in options.iter().zip(&step.options) {
            self.field(*k, "label", &option.label, &t("serversettings.onboarding.choicePlaceholder"), window, cx);
            self.field(*k, "description", &option.description, &t("serversettings.onboarding.choiceAbout"), window, cx);
        }
        self.onboard.steps.push(Step { key, step, options });
    }

    fn add_option(&mut self, step_key: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.onboard.next += 1;
        let key = self.onboard.next;
        self.field(key, "label", "", &t("serversettings.onboarding.choicePlaceholder"), window, cx);
        self.field(key, "description", "", &t("serversettings.onboarding.choiceAbout"), window, cx);
        if let Some(s) = self.onboard.steps.iter_mut().find(|s| s.key == step_key)
            && s.step.options.len() < onb::MAX_OPTIONS
        {
            s.step.options.push(onb::new_option());
            s.options.push(key);
        }
        cx.notify();
    }

    /// A box on the page, made once and kept by its place.
    fn field(
        &mut self,
        key: u64,
        name: &'static str,
        value: &str,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = SharedString::from(placeholder.to_owned());
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let value = value.to_owned();
        input.update(cx, |s, cx| s.set_value(value, window, cx));
        self.onboard.field_changes.push(cx.subscribe(&input, |_: &mut Self, _, e: &InputEvent, cx| {
            if let InputEvent::Change = e {
                cx.notify()
            }
        }));
        self.onboard.fields.insert((key, name), input);
    }

    fn text(&self, key: u64, name: &'static str, cx: &Context<Self>) -> String {
        self.onboard.fields.get(&(key, name)).map(|f| f.read(cx).value().to_string()).unwrap_or_default()
    }

    /// The onboarding as it would be saved now.
    fn onboarding_draft(&self, cx: &Context<Self>) -> pb::Onboarding {
        let steps = self
            .onboard
            .steps
            .iter()
            .map(|s| {
                let mut step = s.step.clone();
                step.title = self.text(s.key, "title", cx);
                step.description = self.text(s.key, "description", cx);
                step.hello = if step.kind == SAY_HELLO { self.text(s.key, "hello", cx) } else { String::new() };
                for (option, k) in step.options.iter_mut().zip(&s.options) {
                    option.label = self.text(*k, "label", cx);
                    option.description = self.text(*k, "description", cx);
                }
                step
            })
            .collect();
        onb::cleaned(pb::Onboarding { enabled: self.onboard.enabled, steps, set_by: String::new() })
    }

    /// How many things on the page aren't saved: the banner's three, and the onboarding's two.
    pub(super) fn look_and_onboarding_changes(&self, server: &pb::Server, cx: &Context<Self>) -> usize {
        let look = match &self.onboard.look {
            Some((url, focus, color)) => {
                let (u, f, c) = look_of(server);
                [*url != u, *focus != f, *color != c].into_iter().filter(|c| *c).count()
            }
            None => 0,
        };
        let steps = self.onboard.saved.as_ref().map(|s| onboarding_changes(s, &self.onboarding_draft(cx))).unwrap_or(0);
        look + steps
    }

    /// Puts the banner and the steps back as they're saved.
    pub(super) fn discard_look_and_onboarding(
        &mut self,
        server: &pb::Server,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.fill_look(server, window, cx);
        if let Some(saved) = self.onboard.saved.clone() {
            self.fill_onboarding(saved, window, cx);
        }
    }

    /// Saves everything on the page that changed, in order: the server's
    /// banner and color, the welcome screen, then the onboarding.
    pub(super) fn save_everything(
        &mut self,
        server: &pb::Server,
        welcome: Option<pb::WelcomeScreen>,
        cx: &mut Context<Self>,
    ) {
        let patch = self.onboard.look.clone().filter(|l| *l != look_of(server)).map(|(url, focus, color)| {
            let (u, f, c) = look_of(server);
            ServerPatch {
                banner_url: (url != u).then_some(url),
                banner_focus: (focus != f).then_some(focus),
                accent_color: (color != c).then_some(color),
                ..ServerPatch::default()
            }
        });
        let draft = self.onboarding_draft(cx);
        let onboarding =
            self.onboard.saved.as_ref().filter(|s| onboarding_changes(s, &draft) > 0).map(|_| draft.clone());
        if onboarding.as_ref().is_some_and(|o| o.enabled && o.steps.is_empty()) {
            self.error = Some(t("desktop.server.onboarding.needsStep"));
            cx.notify();
            return;
        }
        self.onboard.saving = true;
        self.welcome.saving = true;
        self.error = None;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let rx = core.spawn({
            let core = core.clone();
            async move {
                if let Some(patch) = patch {
                    core.update_server(&key, &sid, patch).await?;
                }
                let screen = match welcome {
                    Some(screen) => Some(core.set_welcome_screen(&key, &sid, screen).await?),
                    None => None,
                };
                let onboarding = match onboarding {
                    Some(o) => Some(core.set_onboarding(&key, &sid, o).await?),
                    None => None,
                };
                Ok::<_, Problem>((screen, onboarding))
            }
        });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.onboard.saving = false;
                this.welcome.saving = false;
                match result {
                    Ok((screen, onboarding)) => {
                        if let Some(screen) = screen {
                            this.welcome.saved = Some(screen);
                        }
                        // The steps stay as they are, with the ids the server gave new ones.
                        if let Some(saved) = onboarding {
                            this.onboard.enabled = saved.enabled;
                            for (s, new) in this.onboard.steps.iter_mut().zip(&saved.steps) {
                                s.step.id = new.id.clone();
                                for (o, n) in s.step.options.iter_mut().zip(&new.options) {
                                    o.id = n.id.clone();
                                }
                            }
                            this.onboard.saved = Some(saved);
                        }
                        this.flash_saved(cx);
                    }
                    Err(err) => this.error = Some(err.message),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Asks the system for a picture, frames it, and uploads it as the banner, to save with the rest.
    fn pick_banner(&mut self, cx: &mut Context<Self>) {
        if self.onboard.uploading || self.cropper.is_some() {
            return;
        }
        crate::ui::cropper::choose(
            self.core.clone(),
            crate::core::pictures::PictureKind::Banner,
            t("desktop.server.onboarding.chooseBanner"),
            cx,
            |this| &mut this.cropper,
            |this, error, cx| {
                this.error = Some(error);
                cx.notify();
            },
            Rc::new(|this: &mut Self, bytes, mime, cx: &mut Context<Self>| this.upload_banner(bytes, mime, cx)),
        );
    }

    fn upload_banner(&mut self, bytes: Vec<u8>, mime: &'static str, cx: &mut Context<Self>) {
        self.onboard.uploading = true;
        self.error = None;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(
            cx,
            async move { core.upload_picture_for(&key, &sid, pb::MediaPurpose::Banner, mime, bytes).await },
            |this, result, cx| {
                this.onboard.uploading = false;
                match result {
                    // A new picture starts with its middle in focus.
                    Ok(url) => {
                        if let Some(look) = this.onboard.look.as_mut() {
                            look.0 = url;
                            look.1 = (50, 50);
                        }
                    }
                    Err(err) => this.error = Some(err.message),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// Moves the focus to where the pointer is over the picture.
    fn focus_at(&mut self, at: gpui_kit::Point<Pixels>, cx: &mut Context<Self>) {
        let Some(bounds) = self.onboard.focus_box.get() else { return };
        let Some(look) = self.onboard.look.as_mut() else { return };
        let x = ((at.x - bounds.origin.x) / bounds.size.width * 100.0).round() as i32;
        let y = ((at.y - bounds.origin.y) / bounds.size.height * 100.0).round() as i32;
        look.1 = (x.clamp(0, 100), y.clamp(0, 100));
        cx.notify();
    }

    fn nudge_focus(&mut self, dx: i32, dy: i32, cx: &mut Context<Self>) {
        if let Some(look) = self.onboard.look.as_mut() {
            look.1 = ((look.1.0 + dx).clamp(0, 100), (look.1.1 + dy).clamp(0, 100));
        }
        cx.notify();
    }

    /// The server as it would look with the banner and color being edited.
    pub(super) fn server_as_drafted(&self, server: &pb::Server) -> pb::Server {
        let mut s = server.clone();
        if let Some((url, focus, color)) = &self.onboard.look {
            s.banner_url = url.clone();
            (s.banner_focus_x, s.banner_focus_y) = *focus;
            s.accent_color = (*color >= 0).then_some(*color);
        }
        s
    }

    /// Banner and color.
    pub(super) fn look_section(
        &mut self,
        server: &pb::Server,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.onboard.look.is_none() {
            self.fill_look(server, window, cx);
        }
        let Some((url, focus, color)) = self.onboard.look.clone() else { return div().into_any_element() };
        let drafted = self.server_as_drafted(server);
        let tint = accent(&drafted);
        let uploading = self.onboard.uploading;

        let size = (!url.is_empty())
            .then(|| {
                match window.use_asset::<ImgResourceLoader>(&Resource::Uri(SharedString::from(url.clone()).into()), cx)
                {
                    Some(Ok(image)) => {
                        let s = image.size(0);
                        let (w, h) = (i32::from(s.width) as usize, i32::from(s.height) as usize);
                        let colors = image.as_bytes(0).map(|b| swatches(b, w, h)).unwrap_or_default();
                        Some((w as f32, h as f32, colors))
                    }
                    _ => None,
                }
            })
            .flatten();

        // The picture, as the web's `PictureField` for a banner: the tile (click to change it),
        // then upload or change, remove, and setting it from a link.
        let link_open = self.onboard.link_open;
        let tile = div()
            .id("banner-tile")
            .group("banner-tile")
            .relative()
            .w(px(240.0))
            .h(px(96.0))
            .flex_none()
            .rounded(radius_2xl())
            .border_1()
            .border_color(p.border)
            .bg(p.muted)
            .overflow_hidden()
            .cursor_pointer()
            .active(|s| s.opacity(0.9))
            .on_click(cx.listener(|this, _, _, cx| this.pick_banner(cx)))
            .child(if url.is_empty() {
                crate::ui::banner::server_banner(&drafted, 238.0, 94.0, None, window, cx)
            } else {
                crate::ui::widgets::picture(url.clone())
                    .size_full()
                    .object_fit(gpui_kit::ObjectFit::Cover)
                    .into_any_element()
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
                    .text_color(gpui_kit::white())
                    .text_size(px(10.4))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .opacity(if uploading { 1.0 } else { 0.0 })
                    .id("banner-tile-shade")
                    .group_hover("banner-tile", |s| s.opacity(1.0))
                    .child(if uploading {
                        spinner("banner-busy", 20.0, window)
                    } else {
                        icon("camera").size(px(20.0)).into_any_element()
                    })
                    .child(if uploading { String::new() } else { t("workspace.picture.changeShort").to_uppercase() }),
            );
        let (fg, destructive) = (p.foreground, p.destructive);
        let actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .child(
                crate::ui::settings_controls::button(
                    "banner-pick",
                    if url.is_empty() {
                        t("workspace.picture.uploadShort")
                    } else {
                        t("workspace.picture.changeShort")
                    },
                    Some("image-up"),
                    crate::ui::settings_controls::Look::Outline,
                    false,
                    p,
                )
                .rounded(radius_xl())
                .when(uploading, |el| el.opacity(0.5))
                .on_click(cx.listener(|this, _, _, cx| this.pick_banner(cx))),
            )
            .when(!url.is_empty(), |el| {
                el.child(
                    super::pages::hover_button(
                        "banner-remove",
                        t("system.picture.remove"),
                        Some("trash"),
                        false,
                        p,
                        move |s| s.text_color(destructive),
                    )
                    .text_color(p.muted_foreground)
                    .font_weight(FontWeight::MEDIUM)
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some(look) = this.onboard.look.as_mut() {
                            look.0.clear();
                            look.1 = (50, 50);
                        }
                        this.onboard.link.update(cx, |s, cx| s.set_value("", window, cx));
                        cx.notify();
                    })),
                )
            })
            .child(
                div()
                    .id("banner-link")
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .rounded(radius_lg())
                    .px(px(6.0))
                    .py(px(4.0))
                    .text_xs()
                    .line_height(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .cursor_pointer()
                    .hover(move |s| s.text_color(fg))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.onboard.link_open = !this.onboard.link_open;
                        if this.onboard.link_open {
                            let now = this.onboard.look.as_ref().map(|l| l.0.clone()).unwrap_or_default();
                            this.onboard.link.update(cx, |s, cx| s.set_value(now, window, cx));
                        }
                        cx.notify();
                    }))
                    .child(icon("link").size(px(14.0)))
                    .child(if link_open { t("workspace.picture.hideLink") } else { t("workspace.picture.useLink") }),
            );
        let link_box = link_open.then(|| {
            motion::rise(
                div().pt(px(4.0)).child(super::pages::boxed(
                    Input::new(&self.onboard.link).appearance(false),
                    44.0,
                    super::pages::focused(&self.onboard.link, window, cx),
                    p,
                )),
                "banner-link-box",
                Duration::ZERO,
                -6.0,
            )
        });
        let buttons = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(div().flex().items_center().gap(px(16.0)).child(tile).child(actions))
            .children(link_box);

        // The focal point: the whole picture, with a dot to drag where it matters.
        let focal = match &size {
            Some((iw, ih, _)) => {
                let (bw, bh) = {
                    let w = 340.0;
                    let h = (w * ih / iw).min(220.0);
                    (h * iw / ih, h)
                };
                let cell = self.onboard.focus_box.clone();
                let ring = alpha(p.background, 0.9);
                let dot = div()
                    .absolute()
                    .left(px(bw * focus.0 as f32 / 100.0 - 11.0))
                    .top(px(bh * focus.1 as f32 / 100.0 - 11.0))
                    .size(px(22.0))
                    .rounded_full()
                    .border_2()
                    .border_color(gpui_kit::white())
                    .bg(Hsla { a: 0.6, ..tint })
                    .shadow(vec![gpui_kit::BoxShadow {
                        color: ring,
                        offset: gpui_kit::point(px(0.0), px(0.0)),
                        blur_radius: px(6.0),
                        spread_radius: px(2.0),
                        inset: false,
                    }]);
                let arrows = div()
                    .flex()
                    .gap(px(4.0))
                    .children(
                        [
                            ("left", -2, 0, "arrow-left"),
                            ("right", 2, 0, "arrow-right"),
                            ("up", 0, -2, "arrow-up"),
                            ("down", 0, 2, "arrow-down"),
                        ]
                        .map(|(name, dx, dy, glyph)| {
                            icon_button(SharedString::from(format!("focus-{name}")), glyph, p).on_click(cx.listener(
                                move |this, e: &gpui_kit::ClickEvent, _, cx| {
                                    // Shift goes further.
                                    let far = if e.modifiers().shift { 5 } else { 1 };
                                    this.nudge_focus(dx * far, dy * far, cx)
                                },
                            ))
                        }),
                    )
                    .child(div().ml(px(6.0)).text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(
                        t_with(
                            "desktop.server.onboarding.focus",
                            &[("x", Arg::Num(focus.0.into())), ("y", Arg::Num(focus.1.into()))],
                        ),
                    ));
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("serversettings.welcome.focalPoint")))
                    .child(
                        div()
                            .text_sm()
                            .line_height(px(20.0))
                            .text_color(p.muted_foreground)
                            .child(t("desktop.server.onboarding.focusHint")),
                    )
                    .child(
                        div()
                            .id("banner-focus")
                            .relative()
                            .w(px(bw))
                            .h(px(bh))
                            .rounded(corner(12.0))
                            .overflow_hidden()
                            .cursor_crosshair()
                            .child(crate::ui::widgets::picture(url.clone()).absolute().inset_0().size_full())
                            .child(
                                canvas(move |bounds, _, _| cell.set(Some(bounds)), |_, _, _, _| {})
                                    .absolute()
                                    .inset_0(),
                            )
                            .child(dot)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                                    this.onboard.dragging = true;
                                    this.focus_at(e.position, cx);
                                }),
                            )
                            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                                if this.onboard.dragging && e.pressed_button == Some(MouseButton::Left) {
                                    this.focus_at(e.position, cx);
                                } else {
                                    this.onboard.dragging = false;
                                }
                            })),
                    )
                    .child(arrows)
                    .child(div().flex().flex_wrap().gap(px(12.0)).children(crops().map(|(name, cw, ch)| {
                        // Each shape at a third of its size, cut as it will be.
                        let (w, h) = (cw * 0.4, ch * 0.4);
                        let (l, t, sw, sh) = cover(*iw, *ih, w, h, focus);
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div().relative().w(px(w)).h(px(h)).rounded(corner(8.0)).overflow_hidden().child(
                                    crate::ui::widgets::picture(url.clone())
                                        .absolute()
                                        .left(px(l))
                                        .top(px(t))
                                        .w(px(sw))
                                        .h(px(sh)),
                                ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.4))
                                    .line_height(px(16.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(p.muted_foreground)
                                    .child(tracked(name.to_uppercase(), WIDE)),
                            )
                    })))
                    .into_any_element()
            }
            None if !url.is_empty() => shimmer_rows(1, p).into_any_element(),
            None => div().into_any_element(),
        };

        // The accent: the server's own hue, colors from the banner, or any.
        let own = {
            let c: Hsla = hsla(hue_of(&server.id) as f32 / 360.0, 0.70, 0.58, 1.0);
            (-1, c, t("serversettings.welcome.ownHue"))
        };
        let mut options = vec![own];
        for c in size.as_ref().map(|s| s.2.clone()).unwrap_or_default() {
            options.push((c, gpui_kit::rgb(c as u32).into(), hex(c)));
        }
        let ring = p.foreground;
        let swatch_row = div().flex().flex_wrap().items_center().gap(px(8.0)).children(
            options.into_iter().enumerate().map(|(n, (value, shown, name))| {
                let on = color == value;
                // The web's `Swatch`: a 36px button, a ring when picked and a tick at its corner.
                let face = div()
                    .size_full()
                    .rounded_full()
                    .bg(shown)
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(n == 0, |el| el.child(icon("wand-sparkles").size(px(14.0)).text_color(gpui_kit::white())));
                div()
                    .id(SharedString::from(format!("accent-{value}")))
                    .relative()
                    .size(px(36.0))
                    .p(px(4.0))
                    .rounded_full()
                    .when(on, |el| el.border_2().border_color(ring).p(px(2.0)))
                    .cursor_pointer()
                    .hover(|s| s.scale(1.1))
                    .active(|s| s.scale(0.9))
                    .tooltip(move |window, cx| crate::ui::overlay::Tip::new(name.clone()).build(window, cx))
                    .child(face)
                    .when(on, |el| {
                        el.child(
                            div()
                                .absolute()
                                .right(px(-4.0))
                                .bottom(px(-4.0))
                                .size(px(16.0))
                                .rounded_full()
                                .bg(p.foreground)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon("check").size(px(10.0)).text_color(p.background)),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(look) = this.onboard.look.as_mut() {
                            look.2 = value;
                        }
                        let text = if value >= 0 { hex(value) } else { String::new() };
                        this.onboard.hex.update(cx, |s, cx| s.set_value(text, window, cx));
                        cx.notify();
                    }))
            }),
        );
        // Any color: a pill with the color and its hex (`h-9 rounded-full border`).
        let custom = color >= 0 && !size.as_ref().is_some_and(|s| s.2.contains(&color));
        let hex_pill = div()
            .h(px(36.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(8.0))
            .rounded_full()
            .border_1()
            .border_color(if custom { p.foreground.into() } else { Hsla::from(p.border) })
            .text_xs()
            .font_weight(FontWeight::BOLD)
            .text_color(p.muted_foreground)
            .child(
                div()
                    .size(px(20.0))
                    .flex_none()
                    .rounded_full()
                    .border_1()
                    .border_color(p.border)
                    .when(color >= 0, |el| el.bg(gpui_kit::rgb(color as u32 & 0xff_ffff))),
            )
            .child(div().w(px(80.0)).font_family("monospace").child(Input::new(&self.onboard.hex).appearance(false)));
        let accent_part = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_sm()
                            .line_height(px(20.0))
                            .font_weight(FontWeight::BOLD)
                            .child(icon("palette").size(px(16.0)).text_color(p.muted_foreground))
                            .child(t("serversettings.nav.accentColor")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .child(t("serversettings.welcome.accentHint")),
                    ),
            )
            .child(div().flex().flex_wrap().items_center().gap(px(8.0)).child(swatch_row).child(hex_pill));

        div()
            .flex()
            .flex_col()
            .gap(px(20.0))
            // Its heading is the page's "Banner and color" section, as on the web.
            .child(buttons)
            .child(focal)
            .child(accent_part)
            .into_any_element()
    }

    /// Onboarding: the switch and the steps.
    pub(super) fn onboarding_section(
        &mut self,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.load_onboarding(window, cx);
        if self.onboard.saved.is_none() {
            return shimmer_rows(2, p).into_any_element();
        }
        let (roles, channels, emojis, look, access, has_rules) = self.core.shared.read(|s| {
            let i = s.instance(&self.key);
            let mut channels: Vec<pb::Channel> = i
                .and_then(|i| i.channels.get(&self.server))
                .into_iter()
                .flatten()
                .filter(|c| {
                    matches!(
                        pb::ChannelType::try_from(c.r#type),
                        Ok(pb::ChannelType::Text | pb::ChannelType::Announcement)
                    )
                })
                .cloned()
                .collect();
            channels.sort_by_key(|c| c.position);
            let mut roles: Vec<pb::Role> = i.and_then(|i| i.roles.get(&self.server)).cloned().unwrap_or_default();
            roles.retain(|r| r.id != self.server);
            roles.sort_by_key(|r| -r.position);
            (
                roles,
                channels,
                i.and_then(|i| i.emojis.get(&self.server)).cloned().unwrap_or_default(),
                i.map(|i| crate::ui::mentions::Look::of(i, &self.server)).unwrap_or_default(),
                i.map(|i| i.access(&self.server)).unwrap_or_default(),
                i.and_then(|i| i.server(&self.server)).is_some_and(|s| s.has_rules),
            )
        });
        let enabled = self.onboard.enabled;
        // The web's `Toggle` over a rule.
        let toggle = div().pb(px(20.0)).border_b_1().border_color(alpha(p.border, 0.7)).child(
            crate::ui::settings_controls::toggle(
                "onboarding-on",
                &t("serversettings.onboarding.enabled"),
                Some(&t("serversettings.onboarding.enabledHint")),
                enabled,
                false,
                p,
                window,
                cx,
                |this: &mut Self, on, cx| {
                    this.onboard.enabled = on;
                    cx.notify();
                },
            ),
        );

        let count = self.onboard.steps.len();
        let mut list = div().flex().flex_col().gap(px(10.0));
        for at in 0..count {
            list = list.child(self.step_card(at, &roles, &channels, &emojis, &look, &access, p, cx));
        }
        let has_rules_step = self.onboard.steps.iter().any(|s| s.step.kind == RULES);
        let first_text = channels.iter().find(|c| c.r#type == pb::ChannelType::Text as i32).map(|c| c.id.clone());
        let mut adds = div().flex().flex_wrap().gap(px(8.0));
        if count < onb::MAX_STEPS {
            let kinds = [
                (PICK, "list-checks", t("serversettings.onboarding.kindPick")),
                (RULES, "scroll-text", t("serversettings.onboarding.kindRules")),
                (SAY_HELLO, "hand", t("serversettings.onboarding.kindHello")),
            ];
            for (kind, glyph, label) in kinds {
                if kind == RULES && (!has_rules || has_rules_step) {
                    continue;
                }
                let first = first_text.clone();
                adds = adds.child(
                    crate::ui::settings_controls::button(
                        SharedString::from(format!("onb-add-{kind}")),
                        "",
                        Some("plus"),
                        crate::ui::settings_controls::Look::Outline,
                        false,
                        p,
                    )
                    .rounded(radius_xl())
                    .border_dashed()
                    .child(icon(glyph).size(px(16.0)).text_color(p.muted_foreground))
                    .child(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.add_step(onb::new_step(kind, first.as_deref()), window, cx);
                        cx.notify();
                    })),
                );
            }
        }
        let steps = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .py(px(20.0))
            .child(
                div()
                    .child(
                        div()
                            .line_height(px(24.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(t("serversettings.onboarding.steps")),
                    )
                    .child(div().text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(t_with(
                        "serversettings.onboarding.stepsHint",
                        &[("max", Arg::Num(onb::MAX_STEPS as i64))],
                    ))),
            )
            .child(list)
            .child(adds);
        div().flex().flex_col().child(toggle).child(steps).into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn step_card(
        &self,
        at: usize,
        roles: &[pb::Role],
        channels: &[pb::Channel],
        emojis: &[pb::Emoji],
        look: &crate::ui::mentions::Look,
        access: &crate::core::permissions::Access,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = &self.onboard.steps[at];
        let (key, kind) = (s.key, s.step.kind);
        let count = self.onboard.steps.len();
        let (glyph, hint) = match kind {
            PICK => ("list-checks", t("serversettings.onboarding.kindPickHint")),
            RULES => ("scroll-text", t("serversettings.onboarding.kindRulesHint")),
            _ => ("hand", t("serversettings.onboarding.kindHelloHint")),
        };
        let field = |name| self.onboard.fields.get(&(key, name)).cloned();
        let head = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .size(px(28.0))
                    .rounded(corner(9.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(alpha(p.primary, 0.14))
                    .text_color(p.primary)
                    .child(icon(glyph).size(px(15.0))),
            )
            .child(div().flex_1().text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(hint))
            .child(
                icon_button(SharedString::from(format!("onb-up-{key}")), "chevron-up", p)
                    .when(at == 0, |el| el.opacity(0.3))
                    .on_click(cx.listener(move |this, _, _, cx| this.move_step(key, -1, cx))),
            )
            .child(
                icon_button(SharedString::from(format!("onb-down-{key}")), "chevron-down", p)
                    .when(at + 1 == count, |el| el.opacity(0.3))
                    .on_click(cx.listener(move |this, _, _, cx| this.move_step(key, 1, cx))),
            )
            .child(icon_button(SharedString::from(format!("onb-remove-{key}")), "trash", p).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.onboard.steps.retain(|s| s.key != key);
                    cx.notify();
                },
            )));
        let mut body = div().flex().flex_col().gap(px(10.0)).child(head);
        if let Some(title) = field("title") {
            body = body.child(labeled(&t("serversettings.onboarding.title"), Input::new(&title), p));
        }
        if let Some(description) = field("description") {
            body = body.child(Input::new(&description));
        }
        let mut flags = div().flex().flex_wrap().gap(px(18.0));
        if kind != RULES {
            flags = flags.child(flag(
                format!("onb-skip-{key}"),
                &t("serversettings.onboarding.skippable"),
                &t("serversettings.onboarding.skippableHint"),
                s.step.skippable,
                p,
                cx,
                move |this, on| this.step_mut(key, |s| s.skippable = on),
            ));
        }
        if kind == PICK {
            flags = flags.child(flag(
                format!("onb-multi-{key}"),
                &t("serversettings.onboarding.multiple"),
                &t("serversettings.onboarding.multipleHint"),
                s.step.multiple,
                p,
                cx,
                move |this, on| this.step_mut(key, |s| s.multiple = on),
            ));
        }
        body = body.child(flags);
        match kind {
            PICK => {
                let mut options = div().flex().flex_col().gap(px(8.0));
                for (n, k) in s.options.iter().enumerate() {
                    options = options.child(self.option_row(
                        key,
                        *k,
                        &s.step.options[n],
                        roles,
                        channels,
                        emojis,
                        look,
                        access,
                        p,
                        cx,
                    ));
                }
                body = body.child(options);
                if s.step.options.len() < onb::MAX_OPTIONS {
                    body = body.child(
                        soft_button(
                            SharedString::from(format!("onb-add-option-{key}")),
                            t("serversettings.onboarding.addChoice"),
                            p,
                        )
                        .rounded(radius_xl())
                        .border_dashed()
                        .on_click(cx.listener(move |this, _, window, cx| this.add_option(key, window, cx))),
                    );
                }
            }
            SAY_HELLO => {
                let picked = s.step.channel_id.clone();
                let options: Vec<(String, String)> =
                    channels.iter().map(|c| (c.id.clone(), format!("# {}", c.name))).collect();
                let mut chips = div().flex().flex_wrap().gap(px(6.0));
                for (id, label) in options {
                    let on = id == picked;
                    chips =
                        chips.child(chip(SharedString::from(format!("onb-ch-{key}-{id}")), &label, on, p).on_click(
                            cx.listener(move |this, _, _, cx| {
                                let id = id.clone();
                                this.step_mut(key, |s| s.channel_id = id);
                                cx.notify();
                            }),
                        ));
                }
                body = body.child(labeled(&t("serversettings.shared.pickChannel"), chips, p));
                if let Some(hello) = field("hello") {
                    body = body.child(labeled(&t("desktop.server.onboarding.helloLabel"), Input::new(&hello), p));
                }
            }
            _ => {}
        }
        motion::rise(
            div()
                .p(px(14.0))
                .rounded(corner(16.0))
                .border_1()
                .border_color(p.border)
                .bg(alpha(p.background, 0.6))
                .child(body),
            SharedString::from(format!("onb-step-card-{key}")),
            Duration::ZERO,
            8.0,
        )
        .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn option_row(
        &self,
        step_key: u64,
        key: u64,
        option: &pb::OnboardingOption,
        roles: &[pb::Role],
        channels: &[pb::Channel],
        emojis: &[pb::Emoji],
        look: &crate::ui::mentions::Look,
        access: &crate::core::permissions::Access,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.onboard.open.filter(|(k, _)| *k == key).map(|(_, o)| o);
        let toggle_open = move |what: Open| {
            move |this: &mut Self, _: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.onboard.open = if this.onboard.open == Some((key, what)) { None } else { Some((key, what)) };
                cx.notify();
            }
        };
        let emoji = div()
            .id(SharedString::from(format!("onb-emoji-{key}")))
            .flex_none()
            .cursor_pointer()
            .on_click(cx.listener(toggle_open(Open::Emoji)))
            .child(emoji_tile(&option.emoji, look, p.primary.into(), "sparkles"));

        let line = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(emoji)
            .child(
                div().flex_1().min_w_0().flex().flex_col().gap(px(4.0)).children(
                    ["label", "description"]
                        .into_iter()
                        .filter_map(|name| self.onboard.fields.get(&(key, name)).map(|f| Input::new(f).small())),
                ),
            )
            .child(
                soft_button(
                    SharedString::from(format!("onb-roles-{key}")),
                    match option.role_ids.len() {
                        0 => t("serversettings.nav.roles"),
                        n => t_with("desktop.server.onboarding.roles", &[("count", Arg::Num(n as i64))]),
                    },
                    p,
                )
                .rounded(radius_lg())
                .border_dashed()
                .on_click(cx.listener(toggle_open(Open::Roles))),
            )
            .child(
                soft_button(
                    SharedString::from(format!("onb-channels-{key}")),
                    match option.channel_ids.len() {
                        0 => t("serversettings.nav.channels"),
                        n => t_with("desktop.server.onboarding.channels", &[("count", Arg::Num(n as i64))]),
                    },
                    p,
                )
                .rounded(radius_lg())
                .border_dashed()
                .on_click(cx.listener(toggle_open(Open::Channels))),
            )
            .child(icon_button(SharedString::from(format!("onb-option-remove-{key}")), "x", p).on_click(cx.listener(
                move |this, _, _, cx| {
                    if let Some(s) = this.onboard.steps.iter_mut().find(|s| s.key == step_key)
                        && let Some(at) = s.options.iter().position(|k| *k == key)
                    {
                        s.options.remove(at);
                        s.step.options.remove(at);
                    }
                    cx.notify();
                },
            )));
        let choices: Option<AnyElement> = open.map(|what| match what {
            Open::Roles => {
                let mut chips = div().flex().flex_wrap().gap(px(6.0));
                for role in roles {
                    let on = option.role_ids.contains(&role.id);
                    let strong = !onb::harmless(&role.permissions);
                    let above = !access.above(role.position);
                    let shown = if strong {
                        format!("@{} · {}", role.name, t("serversettings.onboarding.moderates"))
                    } else if above {
                        format!("@{} · {}", role.name, t("serversettings.onboarding.aboveYou"))
                    } else {
                        format!("@{}", role.name)
                    };
                    let (id, blocked) = (role.id.clone(), (strong || above) && !on);
                    chips = chips.child(
                        chip(SharedString::from(format!("onb-role-{key}-{}", role.id)), &shown, on, p)
                            .when(blocked, |el| el.opacity(0.45))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !blocked {
                                    this.option_mut(step_key, key, |o| flip(&mut o.role_ids, &id));
                                }
                                cx.notify();
                            })),
                    );
                }
                chips.into_any_element()
            }
            Open::Channels => {
                let mut chips = div().flex().flex_wrap().gap(px(6.0));
                for c in channels {
                    let on = option.channel_ids.contains(&c.id);
                    let id = c.id.clone();
                    chips = chips.child(
                        chip(SharedString::from(format!("onb-chan-{key}-{}", c.id)), &format!("# {}", c.name), on, p)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.option_mut(step_key, key, |o| flip(&mut o.channel_ids, &id));
                                cx.notify();
                            })),
                    );
                }
                chips.into_any_element()
            }
            Open::Emoji => {
                let mut grid = div().flex().flex_wrap();
                let everyday = crate::ui::emoji::standard()
                    .iter()
                    .flat_map(|g| &g.emojis)
                    .take(48)
                    .map(|e| (e.char.clone(), None));
                let own = emojis.iter().map(|e| (crate::ui::emoji::token(e), Some(e.url.clone())));
                for (n, (value, url)) in own.chain(everyday).enumerate() {
                    let face = match url {
                        Some(url) => crate::ui::widgets::picture(url).size(px(22.0)).into_any_element(),
                        None => div().text_size(px(19.0)).child(value.clone()).into_any_element(),
                    };
                    let hover = alpha(p.primary, 0.12);
                    grid = grid.child(
                        div()
                            .id(SharedString::from(format!("onb-e-{key}-{n}")))
                            .size(px(34.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(corner(10.0))
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let v = value.clone();
                                this.option_mut(step_key, key, |o| o.emoji = v);
                                this.onboard.open = None;
                                cx.notify();
                            }))
                            .child(face),
                    );
                }
                div()
                    .id(SharedString::from(format!("onb-egrid-{key}")))
                    .max_h(px(150.0))
                    .overflow_y_scroll()
                    .child(grid)
                    .into_any_element()
            }
        });
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(8.0))
            .rounded(corner(12.0))
            .bg(p.secondary)
            .child(line)
            .when_some(choices, |el, c| {
                el.child(motion::rise(
                    div().px(px(4.0)).child(c),
                    SharedString::from(format!("onb-open-{key}")),
                    Duration::ZERO,
                    6.0,
                ))
            })
            .into_any_element()
    }

    fn step_mut(&mut self, key: u64, change: impl FnOnce(&mut pb::OnboardingStep)) {
        if let Some(s) = self.onboard.steps.iter_mut().find(|s| s.key == key) {
            change(&mut s.step);
        }
    }

    fn option_mut(&mut self, step_key: u64, key: u64, change: impl FnOnce(&mut pb::OnboardingOption)) {
        if let Some(s) = self.onboard.steps.iter_mut().find(|s| s.key == step_key)
            && let Some(at) = s.options.iter().position(|k| *k == key)
        {
            change(&mut s.step.options[at]);
        }
    }

    fn move_step(&mut self, key: u64, by: isize, cx: &mut Context<Self>) {
        let steps = &mut self.onboard.steps;
        let Some(at) = steps.iter().position(|s| s.key == key) else { return };
        let to = at as isize + by;
        if to >= 0 && (to as usize) < steps.len() {
            steps.swap(at, to as usize);
        }
        cx.notify();
    }

    /// The first step as a newcomer sees it, beside the welcome screen's preview.
    /// The first step of onboarding as new members get it (the web's `OnboardingFlow` in preview).
    pub(super) fn onboarding_card(&self, server: &pb::Server, p: &Palette, cx: &Context<Self>) -> Option<AnyElement> {
        let draft = self.onboarding_draft(cx);
        if !draft.enabled {
            return None;
        }
        let step = draft.steps.first()?;
        let tint = accent(server);
        let total = draft.steps.len();
        let mut dots = div().flex().gap(px(5.0));
        for n in 0..total {
            dots = dots.child(div().h(px(5.0)).w(px(if n == 0 { 16.0 } else { 5.0 })).rounded_full().bg(if n == 0 {
                tint
            } else {
                alpha(p.muted_foreground, 0.3)
            }));
        }
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(16.0))
            .child(
                div().text_xs().line_height(px(16.0)).font_weight(FontWeight::EXTRA_BOLD).text_color(tint).child(
                    tracked(
                        t_with("join.onboarding.stepOf", &[("step", Arg::Num(1)), ("total", Arg::Num(total as i64))])
                            .to_uppercase(),
                        WIDE,
                    ),
                ),
            )
            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(if step.title.is_empty() {
                t("desktop.server.onboarding.untitled")
            } else {
                step.title.clone()
            }));
        if !step.description.is_empty() {
            body = body.child(
                div()
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(crate::ui::text::markdown("onb-preview-words", step.description.clone())),
            );
        }
        for option in step.options.iter().take(4) {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .p(px(6.0))
                    .rounded(corner(10.0))
                    .border_1()
                    .border_color(p.border)
                    .bg(p.secondary)
                    .child(div().text_size(px(15.0)).child(
                        if option.emoji.starts_with('<') || option.emoji.is_empty() {
                            "✨".to_owned()
                        } else {
                            option.emoji.clone()
                        },
                    ))
                    .child(div().truncate().text_xs().line_height(px(16.0)).font_weight(FontWeight::BOLD).child(
                        if option.label.is_empty() {
                            t("desktop.server.onboarding.aChoice")
                        } else {
                            option.label.clone()
                        },
                    )),
            );
        }
        body = body.child(
            div().flex().items_center().justify_between().child(dots).child(
                div()
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(corner(8.0))
                    .bg(tint)
                    .text_color(on_accent(tint))
                    .text_xs()
                    .line_height(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .child(format!(
                        "{} →",
                        if total == 1 { t("join.onboarding.finish") } else { t("join.onboarding.next") }
                    )),
            ),
        );
        Some(div().p(px(8.0)).child(body).into_any_element())
    }
}

/// Adds or takes away one id, five at most.
fn flip(ids: &mut Vec<String>, id: &str) {
    if let Some(at) = ids.iter().position(|i| i == id) {
        ids.remove(at);
    } else if ids.len() < onb::MAX_PICKS {
        ids.push(id.to_owned());
    }
}

/// A switch with its name and a line under it.
fn flag(
    id: String,
    title: &str,
    about: &str,
    on: bool,
    p: &Palette,
    cx: &mut Context<ServerSettingsView>,
    set: impl Fn(&mut ServerSettingsView, bool) + 'static,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(switch(id.into(), on, false, cx, move |this, on, cx| {
            set(this, on);
            cx.notify();
        }))
        .child(
            div()
                .child(div().text_sm().line_height(px(20.0)).font_weight(FontWeight::BOLD).child(title.to_owned()))
                .child(div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(about.to_owned())),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swatches_find_the_colors_that_stand_out() {
        // Half a strong red, a quarter blue, a quarter grey (left out).
        let (w, h) = (8, 8);
        let mut bytes = Vec::new();
        for n in 0..w * h {
            let (b, g, r) = match n % 4 {
                0 | 1 => (20, 20, 230),
                2 => (230, 40, 30),
                _ => (128, 128, 128),
            };
            bytes.extend([b, g, r, 255]);
        }
        let found = swatches(&bytes, w, h);
        assert_eq!(found, vec![0xe61414, 0x1e28e6]);
        assert!(swatches(&[], 0, 0).is_empty());
    }

    #[test]
    fn picks_stop_at_five() {
        let mut ids = Vec::new();
        for n in 0..7 {
            flip(&mut ids, &n.to_string());
        }
        assert_eq!(ids.len(), 5);
        flip(&mut ids, "0");
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn saved_onboarding_compares_as_cleaned() {
        let saved = pb::Onboarding {
            enabled: true,
            steps: vec![pb::OnboardingStep { kind: RULES, title: "Our rules".into(), ..Default::default() }],
            set_by: "someone".into(),
        };
        let now = onb::cleaned(saved.clone());
        assert_eq!(onboarding_changes(&saved, &now), 0);
        let off = pb::Onboarding { enabled: false, ..now };
        assert_eq!(onboarding_changes(&saved, &off), 1);
    }
}
