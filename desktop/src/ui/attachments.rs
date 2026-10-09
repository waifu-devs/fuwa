//! Files with messages, as the web app has them: pictures shown (side by side
//! when there are several) and opened large on a click, anything else as a
//! card to save. The composer's paperclip (or dropping files on it) uploads
//! files for the server as soon as they're picked, and they go with the next
//! message.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, Context, ExternalPaths, FontWeight, InteractiveElement, IntoElement,
    ObjectFit, ParentElement as _, SharedString, SpringConfig, StatefulInteractiveElement as _, Styled,
    StyledImage as _, WeakEntity, Window, div, img, px, radians, rgb, sampled_easing,
};

use crate::core::attachments::{self, Family, Look, MAX_FILES};
use crate::core::i18n::{Arg, t, t_with};
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp, Target};
use crate::ui::motion;
use crate::ui::overlay::shade;
use crate::ui::theme::{Palette, alpha, radius_xl};
use crate::ui::widgets::icon;

/// Tailwind's `shadow-2xl`, under a picture opened large.
fn shadow_2xl() -> Vec<gpui_kit::BoxShadow> {
    vec![gpui_kit::BoxShadow {
        color: gpui_kit::Hsla { h: 0.0, s: 0.0, l: 0.0, a: 0.25 },
        offset: gpui_kit::point(px(0.0), px(25.0)),
        blur_radius: px(50.0),
        spread_radius: px(-12.0),
        inset: false,
    }]
}

/// The most room one picture takes in a message.
const MEDIA_BOX: (f32, f32) = (420.0, 320.0);
/// Pictures side by side, when a message has more than one.
const TILE: f32 = 200.0;

/// A file picked for the next message.
pub struct Staged {
    id: u64,
    name: String,
    size: i64,
    /// Where it is on disk, to send again if it didn't go.
    path: PathBuf,
    /// A picture, shown from disk while it uploads.
    preview: Option<PathBuf>,
    state: Upload,
}

pub enum Upload {
    Uploading,
    Ready(pb::Attachment),
    Failed(String),
}

/// The files waiting to go with the next message, and where.
#[derive(Default)]
pub struct Files {
    place: Option<(String, String, String)>,
    staged: Vec<Staged>,
    next: u64,
}

/// The files waiting to go, summed up for the send button.
#[derive(Default, Clone, Copy)]
pub struct FilesState {
    pub uploading: bool,
    pub broken: bool,
    pub share: f32,
    pub any: bool,
}

fn family_icon(family: Family) -> (&'static str, gpui_kit::Rgba) {
    let tint = rgb;
    match family {
        Family::Archive => ("archive", tint(0xf59e0b)),
        Family::Code => ("file-code", tint(0x0ea5e9)),
        Family::Document | Family::Pdf => ("file-text", tint(if family == Family::Pdf { 0xf43f5e } else { 0x3b82f6 })),
        Family::Sheet => ("file-spreadsheet", tint(0x10b981)),
        Family::Slides => ("presentation", tint(0xf97316)),
        Family::Text => ("file-text", tint(0x94a3b8)),
        Family::Audio => ("file-music", tint(0x8b5cf6)),
        Family::Video => ("file-video-camera", tint(0xd946ef)),
        Family::Picture => ("file-image", tint(0xec4899)),
        Family::Other => ("file", tint(0xa78bfa)),
    }
}

/// A file's icon in its family's tint.
pub(crate) fn badge(name: &str, size: f32) -> impl IntoElement {
    let (glyph, tint) = family_icon(attachments::family_of(&attachments::clean_name(name)));
    div()
        .size(px(size))
        .flex_none()
        .rounded(if size >= 40.0 { crate::ui::theme::radius_xl() } else { crate::ui::theme::radius_lg() })
        .flex()
        .items_center()
        .justify_center()
        .bg(alpha(tint, 0.14))
        .text_color(tint)
        .child(icon(glyph).size(px(size / 2.0)))
}

/// A file coming in with a message that just came, `n`th of its files (the
/// web's `enter`): a fade while it rises 6px and grows from 97%, 40ms after
/// the one before, on `stiffness: 460, damping: 30`.
fn enter<E: IntoElement + Styled + 'static>(el: E, id: SharedString, n: usize, animate: bool) -> AnyElement {
    if !animate {
        return el.into_any_element();
    }
    let (duration, easing) = sampled_easing(SpringConfig::new(460.0, 30.0, 1.0), 0.002);
    let delay = Duration::from_millis(40 * n as u64);
    let total = delay + duration;
    let start = delay.as_secs_f32() / total.as_secs_f32();
    el.with_animation(
        id,
        Animation::new(total).with_easing(move |t| if t <= start { 0.0 } else { easing((t - start) / (1.0 - start)) }),
        |el, t| el.opacity(t.clamp(0.0, 1.0)).translate_y(px((1.0 - t) * 6.0)).scale(0.97 + 0.03 * t),
    )
    .into_any_element()
}

/// The files a message came with. Only files on this instance load; each
/// picture keeps its box before it loads, so the list doesn't jump.
/// `animate` for a message that just came in, whose files come in one by one.
pub(crate) fn attachments_view(
    mid: &str,
    files: &[pb::Attachment],
    instance: &str,
    p: &Palette,
    this: &WeakEntity<FuwaApp>,
    key: &str,
    animate: bool,
) -> AnyElement {
    let shown = |f: &&pb::Attachment| {
        attachments::look_of(&f.content_type) == Look::Picture && attachments::on_instance(&f.url, instance)
    };
    // Each with its place among the message's files, for its right-click menu.
    let pictures: Vec<(usize, &pb::Attachment)> = files.iter().enumerate().filter(|(_, f)| shown(f)).collect();
    let rest: Vec<&pb::Attachment> = files.iter().filter(|f| !shown(f)).collect();
    let tiled = pictures.len() > 1;
    let shown_pictures = pictures.len();
    let mut out = div().mt(px(4.0)).flex().flex_col().gap(px(6.0));
    if !pictures.is_empty() {
        let mut grid = div().flex().flex_wrap().gap(px(6.0)).when(tiled, |el| el.max_w(px(TILE * 2.0 + 6.0)));
        for (n, (place, file)) in pictures.into_iter().enumerate() {
            let (w, h) = if tiled { (TILE, TILE) } else { attachments::fit_box(file.width, file.height, MEDIA_BOX) };
            let open = Dialog::Picture {
                key: key.to_owned(),
                url: file.url.clone(),
                name: file.filename.clone(),
                width: file.width,
                height: file.height,
                bytes: file.size,
            };
            let this = this.clone();
            let picture = div()
                .id(SharedString::from(format!("pic|{mid}|{n}")))
                .w(px(w))
                .h(px(h))
                .rounded(crate::ui::theme::radius_xl())
                .overflow_hidden()
                .border_1()
                .border_color(p.border)
                .bg(alpha(p.muted, 0.6))
                .cursor_pointer()
                // `whileHover={{ scale: 1.01 }}` and `whileTap={{ scale: 0.98 }}`.
                .hover(|s| s.scale(1.01))
                .active(|s| s.scale(0.98))
                .on_mouse_down(gpui_kit::MouseButton::Right, {
                    let (this, mid) = (this.clone(), mid.to_owned());
                    // Says which picture it was; the message's own handler opens the menu.
                    move |_, _, cx| _ = this.update(cx, |this, _| this.right_picture = Some((mid.clone(), place)))
                })
                .on_click(move |_, window, cx| {
                    let _ = this.update(cx, |this, cx| this.open_dialog(open.clone(), window, cx));
                })
                .child(
                    img(SharedString::from(file.url.clone()))
                        .size_full()
                        .rounded(px((f32::from(crate::ui::theme::radius_xl()) - 1.0).max(0.0)))
                        .object_fit(if tiled { ObjectFit::Cover } else { ObjectFit::Contain })
                        .with_loading({
                            let bg = alpha(p.foreground, 0.04);
                            move || div().size_full().bg(bg).into_any_element()
                        })
                        .with_fallback({
                            let fg = p.muted_foreground;
                            move || {
                                div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(fg)
                                    .child(icon("file-image").size(px(24.0)))
                                    .into_any_element()
                            }
                        }),
                );
            grid = grid.child(enter(picture, SharedString::from(format!("pic-in|{mid}|{n}")), n, animate));
        }
        out = out.child(grid);
    }
    for (n, file) in rest.into_iter().enumerate() {
        let card = file_card(&format!("{mid}|{n}"), file, p, this, key);
        out = out.child(enter(card, SharedString::from(format!("file-in|{mid}|{n}")), shown_pictures + n, animate));
    }
    out.into_any_element()
}

/// A file to save: its icon, name and size, and a download button.
/// It lifts a pixel under the pointer, its badge tilting (`FileCard`).
fn file_card(
    id: &str,
    file: &pb::Attachment,
    p: &Palette,
    this: &WeakEntity<FuwaApp>,
    key: &str,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let (this, key, url, name, bytes) =
        (this.clone(), key.to_owned(), file.url.clone(), file.filename.clone(), file.size);
    let (hover_bg, hover_fg) = (p.muted, p.primary);
    div()
        .id(SharedString::from(format!("file|{id}")))
        .hover(|s| s.translate_y(px(-1.0)))
        .flex()
        .items_center()
        .gap(px(12.0))
        .w(px(448.0))
        .max_w_full()
        .p(px(12.0))
        .rounded(crate::ui::theme::radius_2xl())
        .bg(crate::ui::theme::mix(p.chat_surface.into(), p.card, 0.7))
        .border_1()
        .border_color(p.border)
        .shadow(crate::ui::polls::shadow_sm())
        .child(
            div()
                .id(SharedString::from(format!("file-badge|{id}")))
                .flex_none()
                .hover(|s| s.rotate(radians((-8.0f32).to_radians())).scale(1.06))
                .child(badge(&file.filename, 40.0)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .truncate()
                        .child(attachments::short_name(&file.filename, 40)),
                )
                .child(div().text_xs().text_color(p.muted_foreground).child(attachments::format_bytes(file.size))),
        )
        // The web's download button: muted, the muted fill and the primary on hover.
        .child(
            div()
                .id(SharedString::from(format!("save|{id}")))
                .size(px(32.0))
                .flex_none()
                .rounded(crate::ui::theme::radius_lg())
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(p.muted_foreground)
                .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                .child(icon("download").size(px(16.0)))
                .tooltip(|window, cx| crate::ui::overlay::Tip::new("Save").build(window, cx))
                .on_click(move |_, _, cx| {
                    let _ =
                        this.update(cx, |this, cx| this.save_file(key.clone(), url.clone(), name.clone(), bytes, cx));
                }),
        )
}

impl FuwaApp {
    /// Files go in a server's plain channels, never shared ones, for those who may attach them.
    pub(crate) fn can_attach(&self) -> bool {
        let Some(Target::Channel { key, server, channel }) = self.target() else { return false };
        self.core.shared.read(|s| {
            s.instance(&key).is_some_and(|i| {
                i.access(&server).has_in(&channel, pb::Permission::AttachFiles)
                    && i.channel(&server, &channel).is_some_and(|c| c.shared.is_none())
            })
        })
    }

    fn files_place(&self) -> Option<(String, String, String)> {
        match self.target()? {
            Target::Channel { key, server, channel } => Some((key, server, channel)),
            _ => None,
        }
    }

    pub(crate) fn attach_button(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        crate::ui::widgets::tool_button("attach", "paperclip", false, p)
            .tooltip(|window, cx| crate::ui::overlay::Tip::new(t("chat.files.attach")).build(window, cx))
            .on_click(cx.listener(|this, _, _, cx| this.pick_files(cx)))
            .into_any_element()
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let _ = this.update(cx, |this, cx| this.add_files(paths, cx));
        })
        .detach();
    }

    /// Picked or dropped files: each starts uploading at once.
    pub(crate) fn add_files(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if !self.can_attach() {
            return;
        }
        let Some(place) = self.files_place() else { return };
        if self.files.place.as_ref() != Some(&place) {
            self.files = Files { place: Some(place.clone()), ..Default::default() };
        }
        let paths: Vec<PathBuf> = paths.into_iter().filter(|p| p.is_file()).collect();
        if self.files.staged.len() + paths.len() > MAX_FILES {
            self.toast(
                "paperclip",
                format!("You can send up to {MAX_FILES} files at once"),
                "The rest weren't added.".into(),
                None,
                None,
                cx,
            );
        }
        let room = MAX_FILES.saturating_sub(self.files.staged.len());
        for path in paths.into_iter().take(room) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let size = std::fs::metadata(&path).map(|m| m.len() as i64).unwrap_or(0);
            let picture = attachments::look_of(attachments::content_type_of(&name)) == Look::Picture;
            let id = self.files.next;
            self.files.next += 1;
            self.files.staged.push(Staged {
                id,
                name,
                size,
                preview: picture.then(|| path.clone()),
                path: path.clone(),
                state: Upload::Uploading,
            });
            self.upload_staged(id, path, cx);
        }
        cx.notify();
    }

    /// Sends a staged file up (again).
    fn upload_staged(&mut self, id: u64, path: PathBuf, cx: &mut Context<Self>) {
        let Some((key, server, _)) = self.files.place.clone() else { return };
        let core = self.core.clone();
        self.run(cx, async move { core.upload_attachment(&key, &server, &path).await }, move |this, result, cx| {
            if let Some(s) = this.files.staged.iter_mut().find(|s| s.id == id) {
                s.state = match result {
                    Ok(file) => Upload::Ready(file),
                    Err(problem) => {
                        // Said as a sentence, like the web's.
                        let mut why = problem.message;
                        if let Some(first) = why.get(..1) {
                            why = first.to_uppercase() + &why[1..];
                        }
                        Upload::Failed(why)
                    }
                };
            }
            cx.notify();
        });
    }

    fn retry_file(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(s) = self.files.staged.iter_mut().find(|s| s.id == id) else { return };
        s.state = Upload::Uploading;
        let path = s.path.clone();
        self.upload_staged(id, path, cx);
        cx.notify();
    }

    fn remove_file(&mut self, id: u64, cx: &mut Context<Self>) {
        self.files.staged.retain(|s| s.id != id);
        cx.notify();
    }

    /// Whether files wait to go here.
    pub(crate) fn has_files(&self) -> bool {
        self.files_place().is_some_and(|place| self.files.place.as_ref() == Some(&place))
            && !self.files.staged.is_empty()
    }

    /// The files ready to go with the message being sent, taken from the
    /// tray; Err with why not when some are still uploading or didn't upload.
    pub(crate) fn take_files(&mut self) -> Result<Vec<pb::Attachment>, &'static str> {
        if !self.has_files() {
            return Ok(Vec::new());
        }
        if self.files.staged.iter().any(|s| matches!(s.state, Upload::Uploading)) {
            return Err("Your files are still uploading.");
        }
        if self.files.staged.iter().any(|s| matches!(s.state, Upload::Failed(_))) {
            return Err("Remove the files that didn't upload first.");
        }
        Ok(std::mem::take(&mut self.files.staged)
            .into_iter()
            .filter_map(|s| match s.state {
                Upload::Ready(file) => Some(file),
                _ => None,
            })
            .collect())
    }

    /// How the files waiting here are going: still uploading, any broken, and
    /// how much of them is up (0 to 1, by files, since uploads don't report bytes).
    pub(crate) fn files_state(&self) -> FilesState {
        if !self.has_files() {
            return FilesState::default();
        }
        let staged = &self.files.staged;
        let up = staged.iter().filter(|s| matches!(s.state, Upload::Ready(_))).count();
        FilesState {
            uploading: staged.iter().any(|s| matches!(s.state, Upload::Uploading)),
            broken: staged.iter().any(|s| matches!(s.state, Upload::Failed(_))),
            share: up as f32 / staged.len() as f32,
            any: true,
        }
    }

    /// Clears files that were for somewhere else.
    pub(crate) fn forget_files_elsewhere(&mut self) {
        if self.files.place.is_some() && self.files.place != self.files_place() {
            self.files = Files::default();
        }
    }

    /// The files going with the next message, inside the composer above the
    /// box (the web's `StagedTray`): a card each, with how its upload is going.
    pub(crate) fn file_tray(&self, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.has_files() {
            return None;
        }
        let mut tray = div().flex().gap(px(8.0)).mx(px(-4.0)).px(px(4.0)).pt(px(4.0)).pb(px(8.0));
        for s in &self.files.staged {
            tray = tray.child(self.staged_card(s, p, cx));
        }
        Some(
            div()
                .id("staged-tray")
                .overflow_x_scroll()
                .child(motion::rise(tray, "staged-tray-rise", Duration::ZERO, 8.0))
                .into_any_element(),
        )
    }

    fn staged_card(&self, s: &Staged, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = s.id;
        let failed = match &s.state {
            Upload::Failed(why) => Some(why.clone()),
            _ => None,
        };
        let done = matches!(s.state, Upload::Ready(_));
        let uploading = matches!(s.state, Upload::Uploading);
        let shown = s.preview.is_some();
        let white = gpui_kit::rgb(0xffffff);
        let tip = failed.clone().unwrap_or_else(|| {
            t_with(
                "chat.files.nameAndSize",
                &[("name", Arg::Str(&s.name)), ("size", Arg::Str(&attachments::format_bytes(s.size)))],
            )
        });
        let mut card = div()
            .id(SharedString::from(format!("staged|{id}")))
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .h(px(96.0))
            .w(px(144.0))
            .overflow_hidden()
            .rounded(radius_xl())
            .border_1()
            .border_color(if failed.is_some() { alpha(p.destructive, 0.6) } else { p.border.into() })
            .bg(if failed.is_some() { alpha(p.destructive, 0.05) } else { alpha(p.muted, 0.4) })
            .tooltip(move |window, cx| crate::ui::overlay::Tip::new(tip.clone()).build(window, cx));
        card = match &s.preview {
            Some(path) => card.child(
                img(path.clone())
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .rounded(radius_xl())
                    .object_fit(ObjectFit::Cover)
                    .opacity(if done { 1.0 } else { 0.6 }),
            ),
            None => card
                .child(div().flex_1().flex().items_center().justify_center().pt(px(4.0)).child(badge(&s.name, 40.0))),
        };
        let mut caption = div().relative().mt_auto().px(px(8.0)).pb(px(6.0));
        if shown {
            caption = caption.pt(px(16.0)).text_color(white).bg(gpui_kit::linear_gradient(
                0.0,
                gpui_kit::linear_color_stop(gpui_kit::hsla(0.0, 0.0, 0.0, 0.7), 0.0),
                gpui_kit::linear_color_stop(gpui_kit::hsla(0.0, 0.0, 0.0, 0.0), 1.0),
            ));
        }
        caption = caption
            .child(
                div()
                    .truncate()
                    .text_size(px(11.2))
                    .line_height(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .child(attachments::short_name(&s.name, 22)),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(10.4))
                    .line_height(px(14.0))
                    .text_color(if failed.is_some() {
                        p.destructive.into()
                    } else if shown {
                        alpha(white, 0.75)
                    } else {
                        p.muted_foreground.into()
                    })
                    .child(failed.clone().unwrap_or_else(|| attachments::format_bytes(s.size))),
            );
        card = card.child(caption);
        // How it's going: uploads don't count their bytes here, so a bar
        // sweeps along the bottom until it's up (the web's fills).
        if uploading {
            card =
                card.child(div().absolute().bottom_0().left_0().h(px(4.0)).w(px(48.0)).bg(p.primary).with_animation(
                    SharedString::from(format!("staged-bar|{id}")),
                    Animation::new(Duration::from_millis(1200)).repeat(),
                    |el, t| el.left(px(-48.0 + t * 192.0)),
                ));
        }
        if done {
            // A check pops in and fades once it's up.
            card = card.child(motion::once(
                div()
                    .absolute()
                    .top(px(6.0))
                    .left(px(6.0))
                    .size(px(20.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.primary)
                    .text_color(p.primary_foreground)
                    .child(icon("check").size(px(12.0))),
                SharedString::from(format!("staged-done|{id}")),
                Duration::from_millis(1100),
                |el, t| {
                    let opacity = if t < 0.4 { (t / 0.25).min(1.0) } else { 1.0 - (t - 0.4) / 0.6 };
                    el.opacity(opacity.clamp(0.0, 1.0))
                },
            ));
        }
        let round = |name: &str, glyph: &str, hover: gpui_kit::Rgba| {
            div()
                .id(SharedString::from(format!("{name}|{id}")))
                .size(px(24.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(alpha(p.card, 0.9))
                .text_color(p.foreground)
                .shadow(vec![gpui_kit::BoxShadow {
                    color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.08),
                    offset: gpui_kit::point(px(0.0), px(1.0)),
                    blur_radius: px(2.0),
                    spread_radius: px(0.0),
                    inset: false,
                }])
                .cursor_pointer()
                .hover(move |s| s.text_color(hover))
                .child(icon(glyph).size(px(14.0)))
        };
        let buttons = div()
            .absolute()
            .top(px(4.0))
            .right(px(4.0))
            .flex()
            .gap(px(4.0))
            .when(failed.is_some(), |el| {
                let name = s.name.clone();
                el.child(
                    round("staged-retry", "rotate-cw", p.primary)
                        .tooltip(move |window, cx| {
                            crate::ui::overlay::Tip::new(t_with("chat.files.retry", &[("name", Arg::Str(&name))]))
                                .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.retry_file(id, cx))),
                )
            })
            .child({
                let name = s.name.clone();
                round("staged-remove", "x", p.destructive)
                    .tooltip(move |window, cx| {
                        crate::ui::overlay::Tip::new(t_with("chat.files.remove", &[("name", Arg::Str(&name))]))
                            .build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.remove_file(id, cx)))
            });
        motion::rise(card.child(buttons), SharedString::from(format!("staged-rise|{id}")), Duration::ZERO, 10.0)
            .into_any_element()
    }

    /// While files are dragged over the window: a sheet over everything saying
    /// where they'll go (the web's `DropOverlay`); dropping them anywhere adds
    /// them to the next message. It's always there, unseen, until a drag of
    /// files passes over it.
    pub(crate) fn drop_overlay(&self, p: &Palette, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let gate = self.send_gate()?;
        if !gate.can_attach || gate.pending || gate.timed_out() || !gate.can_send || self.dialog.is_some() {
            return None;
        }
        let Some(Target::Channel { key, server, channel }) = self.target() else { return None };
        let name = self
            .core
            .shared
            .read(|s| s.instance(&key).and_then(|i| i.channel(&server, &channel).map(|c| c.name.clone())))?;
        let size = window.viewport_size();
        let sheet = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.0))
            .max_w(px(384.0))
            .px(px(40.0))
            .py(px(32.0))
            .rounded(crate::ui::theme::radius_3xl())
            .border_2()
            .border_dashed()
            .border_color(alpha(p.primary, 0.6))
            .bg(p.card)
            .shadow(vec![gpui_kit::BoxShadow {
                color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.25),
                offset: gpui_kit::point(px(0.0), px(25.0)),
                blur_radius: px(50.0),
                spread_radius: px(-12.0),
                inset: false,
            }])
            .child(
                div()
                    .size(px(56.0))
                    .rounded(crate::ui::theme::radius_2xl())
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(alpha(p.primary, 0.15))
                    .text_color(p.primary)
                    .child(icon("upload").size(px(28.0)).with_animation(
                        "drop-bob",
                        Animation::new(Duration::from_millis(1200)).repeat(),
                        |el, t| {
                            let y = -6.0 * (t * std::f32::consts::PI).sin();
                            el.transform(gpui_kit::Transformation::translate(gpui_kit::point(px(0.0), px(y))))
                        },
                    )),
            )
            .child(
                div()
                    .text_size(px(18.0))
                    .line_height(px(28.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(p.foreground)
                    .text_center()
                    .child(t_with("chat.files.dropToChannel", &[("channel", Arg::Str(&name))])),
            )
            .child(div().text_sm().text_color(p.muted_foreground).text_center().child(t("chat.files.dropNote")));
        let shade = alpha(p.background, 0.6);
        Some(
            gpui_kit::deferred(
                gpui_kit::anchored().position(gpui_kit::point(px(0.0), px(0.0))).child(
                    div()
                        .id("drop-overlay")
                        .w(size.width)
                        .h(size.height)
                        .flex()
                        .items_center()
                        .justify_center()
                        .p(px(24.0))
                        .opacity(0.0)
                        .drag_over::<ExternalPaths>(move |s, _, _, _| s.opacity(1.0).bg(shade))
                        .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                            cx.stop_propagation();
                            this.add_files(paths.paths().to_vec(), cx)
                        }))
                        .child(sheet),
                ),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }

    /// Tints the composer while files are dragged over it (the overlay takes the drop).
    pub(crate) fn droppable<E: InteractiveElement + gpui_kit::Styled + 'static>(
        &self,
        el: E,
        p: &Palette,
        _cx: &mut Context<Self>,
    ) -> E {
        if !self.can_attach() {
            return el;
        }
        let (bg, border) = (alpha(p.primary, 0.08), p.primary);
        el.drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(bg).border_color(border))
    }

    /// Asks where to save a file, then saves it there.
    pub(crate) fn save_file(&mut self, key: String, url: String, name: String, bytes: i64, cx: &mut Context<Self>) {
        let dir = dirs::download_dir().or_else(dirs::home_dir).unwrap_or_default();
        // Only the name goes to the system's dialog, never a path someone wrote into it.
        let suggested: String = attachments::clean_name(&name)
            .chars()
            .map(|c| if matches!(c, '/' | '\\' | ':') { '_' } else { c })
            .collect::<String>()
            .trim_start_matches('.')
            .to_owned();
        let path = cx.prompt_for_new_path(&dir, Some(if suggested.is_empty() { "file" } else { &suggested }));
        let core = self.core.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = path.await else { return };
            let rx = core.spawn({
                let core = core.clone();
                let path = path.clone();
                async move { core.save_attachment(&key, &url, bytes, &path).await }
            });
            let result = rx.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(Ok(())) => {
                    let shown = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    this.toast("download", format!("Saved {shown}"), String::new(), None, None, cx);
                }
                Ok(Err(problem)) => {
                    this.toast("circle-alert", "Couldn't save that".into(), problem.message, None, None, cx)
                }
                Err(_) => {}
            });
        })
        .detach();
    }

    /// A picture opened large, over everything.
    pub(crate) fn render_picture(&self, open: &Dialog, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Dialog::Picture { key, url, name, width, height, bytes } = open else { return div().into_any_element() };
        let (size, bytes) = ((*width, *height), *bytes);
        let view = window.viewport_size();
        let room = (f32::from(view.width) - 160.0, f32::from(view.height) - 180.0);
        let (w, h) = attachments::fit_box(size.0, size.1, (room.0.max(200.0), room.1.max(200.0)));
        let (key, url_owned, name_owned) = (key.to_owned(), url.to_owned(), name.to_owned());
        // The web's caption: white on black at 60%, its buttons lit white at 15% on hover.
        let round = |id: &'static str, glyph: &str| {
            let hover = gpui_kit::Hsla { h: 0.0, s: 0.0, l: 1.0, a: 0.15 };
            div()
                .id(id)
                .size(px(32.0))
                .flex_none()
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(icon(glyph).size(px(16.0)))
        };
        let white = |a: f32| gpui_kit::Hsla { h: 0.0, s: 0.0, l: 1.0, a };
        let caption = div()
            .max_w_full()
            .flex()
            .items_center()
            .gap(px(8.0))
            .pl(px(16.0))
            .pr(px(6.0))
            .py(px(6.0))
            .rounded_full()
            .bg(gpui_kit::Hsla { h: 0.0, s: 0.0, l: 0.0, a: 0.6 })
            .text_sm()
            .text_color(white(1.0))
            .child(div().min_w_0().truncate().font_weight(FontWeight::BOLD).child(attachments::short_name(name, 48)))
            .when(bytes > 0, |el| {
                el.child(div().flex_none().text_color(white(0.7)).child(attachments::format_bytes(bytes)))
            })
            .child(
                round("picture-save", "download")
                    .tooltip(|window, cx| crate::ui::overlay::Tip::new("Save").build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.save_file(key.clone(), url_owned.clone(), name_owned.clone(), bytes, cx)
                    })),
            )
            .child(
                // It turns a quarter under the pointer (`hover:rotate-90`).
                round("picture-close", "x")
                    .hover(|s| s.rotate(radians(std::f32::consts::FRAC_PI_2)))
                    .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
            );
        motion::fade_in(
            // The web's `bg-black/80 backdrop-blur-sm`.
            shade("picture-scrim", 0.8, 4.0).on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))).child(
                div()
                    .id("picture-panel")
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(12.0))
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(motion::grow_in(
                        img(SharedString::from(url.to_owned()))
                            .w(px(w))
                            .h(px(h))
                            .rounded(radius_xl())
                            .shadow(shadow_2xl())
                            .object_fit(ObjectFit::Contain),
                        "picture-zoom",
                        0.92,
                    ))
                    .child(motion::rise(caption, "picture-caption", Duration::from_millis(50), 8.0)),
            ),
            "picture-fade",
            Duration::from_millis(160),
        )
        .into_any_element()
    }
}
