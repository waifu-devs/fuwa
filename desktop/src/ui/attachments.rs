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
    ObjectFit, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _,
    WeakEntity, Window, div, img, px, rgb,
};

use crate::core::attachments::{self, Family, Look, MAX_FILES};
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp, Target};
use crate::ui::motion;
use crate::ui::overlay::scrim;
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{icon, icon_button};

/// The most room one picture takes in a message.
const MEDIA_BOX: (f32, f32) = (420.0, 320.0);
/// Pictures side by side, when a message has more than one.
const TILE: f32 = 200.0;

/// A file picked for the next message.
pub struct Staged {
    id: u64,
    name: String,
    size: i64,
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
fn badge(name: &str, size: f32) -> impl IntoElement {
    let (glyph, tint) = family_icon(attachments::family_of(&attachments::clean_name(name)));
    div()
        .size(px(size))
        .flex_none()
        .rounded(corner(12.0))
        .flex()
        .items_center()
        .justify_center()
        .bg(alpha(tint, 0.14))
        .text_color(tint)
        .child(icon(glyph).size(px(size / 2.0)))
}

/// The files a message came with. Only files on this instance load; each
/// picture keeps its box before it loads, so the list doesn't jump.
pub(crate) fn attachments_view(
    mid: &str,
    files: &[pb::Attachment],
    instance: &str,
    p: &Palette,
    this: &WeakEntity<FuwaApp>,
    key: &str,
) -> AnyElement {
    let shown = |f: &&pb::Attachment| {
        attachments::look_of(&f.content_type) == Look::Picture && attachments::on_instance(&f.url, instance)
    };
    let pictures: Vec<&pb::Attachment> = files.iter().filter(shown).collect();
    let rest: Vec<&pb::Attachment> = files.iter().filter(|f| !shown(f)).collect();
    let tiled = pictures.len() > 1;
    let mut out = div().mt(px(4.0)).flex().flex_col().gap(px(6.0));
    if !pictures.is_empty() {
        let mut grid = div().flex().flex_wrap().gap(px(6.0)).when(tiled, |el| el.max_w(px(TILE * 2.0 + 6.0)));
        for (n, file) in pictures.into_iter().enumerate() {
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
            grid = grid.child(
                div()
                    .id(SharedString::from(format!("pic|{mid}|{n}")))
                    .w(px(w))
                    .h(px(h))
                    .rounded(corner(12.0))
                    .overflow_hidden()
                    .bg(alpha(p.foreground, 0.06))
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.92))
                    .on_click(move |_, window, cx| {
                        let _ = this.update(cx, |this, cx| this.open_dialog(open.clone(), window, cx));
                    })
                    .child(
                        img(SharedString::from(file.url.clone()))
                            .size_full()
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
                    ),
            );
        }
        out = out.child(grid);
    }
    for (n, file) in rest.into_iter().enumerate() {
        out = out.child(file_card(&format!("{mid}|{n}"), file, p, this, key));
    }
    out.into_any_element()
}

/// A file to save: its icon, name and size, and a download button.
fn file_card(id: &str, file: &pb::Attachment, p: &Palette, this: &WeakEntity<FuwaApp>, key: &str) -> impl IntoElement {
    let (this, key, url, name, bytes) =
        (this.clone(), key.to_owned(), file.url.clone(), file.filename.clone(), file.size);
    div()
        .flex()
        .items_center()
        .gap(px(12.0))
        .w(px(360.0))
        .max_w_full()
        .p(px(10.0))
        .rounded(corner(14.0))
        .bg(p.card)
        .border_1()
        .border_color(p.border)
        .child(badge(&file.filename, 40.0))
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
        .child(
            icon_button(SharedString::from(format!("save|{id}")), "download", p)
                .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Save").build(window, cx))
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
        icon_button("attach", "paperclip", p)
            .size(px(36.0))
            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Attach files").build(window, cx))
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
                state: Upload::Uploading,
            });
            let (core, (key, server, _)) = (self.core.clone(), place.clone());
            self.run(cx, async move { core.upload_attachment(&key, &server, &path).await }, move |this, result, cx| {
                if let Some(s) = this.files.staged.iter_mut().find(|s| s.id == id) {
                    s.state = match result {
                        Ok(file) => Upload::Ready(file),
                        Err(problem) => Upload::Failed(problem.message),
                    };
                }
                cx.notify();
            });
        }
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

    /// Clears files that were for somewhere else.
    pub(crate) fn forget_files_elsewhere(&mut self) {
        if self.files.place.is_some() && self.files.place != self.files_place() {
            self.files = Files::default();
        }
    }

    /// The files waiting to go, above the message box.
    pub(crate) fn file_tray(&self, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.has_files() {
            return None;
        }
        let mut tray = div().flex().flex_wrap().gap(px(8.0)).pb(px(8.0));
        for s in &self.files.staged {
            let id = s.id;
            let face: AnyElement = match &s.preview {
                Some(path) => img(path.clone())
                    .size(px(44.0))
                    .rounded(corner(10.0))
                    .object_fit(ObjectFit::Cover)
                    .into_any_element(),
                None => badge(&s.name, 44.0).into_any_element(),
            };
            let (line, color) = match &s.state {
                Upload::Uploading => ("Uploading…".to_owned(), p.muted_foreground),
                Upload::Ready(_) => (attachments::format_bytes(s.size), p.muted_foreground),
                Upload::Failed(why) => (why.clone(), p.destructive),
            };
            tray = tray.child(motion::rise(
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .w(px(220.0))
                    .p(px(8.0))
                    .rounded(corner(14.0))
                    .bg(alpha(p.foreground, 0.05))
                    .border_1()
                    .border_color(if matches!(s.state, Upload::Failed(_)) { p.destructive } else { p.border })
                    .child(face)
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
                                    .child(attachments::short_name(&s.name, 26)),
                            )
                            .child(div().text_xs().truncate().text_color(color).child(line)),
                    )
                    .when(matches!(s.state, Upload::Uploading), |el| {
                        el.child(icon("loader-circle").size(px(14.0)).text_color(p.muted_foreground).with_animation(
                            SharedString::from(format!("file-up|{id}")),
                            Animation::new(Duration::from_millis(900)).repeat(),
                            |el, t| el.rotate(gpui_kit::percentage(t)),
                        ))
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("file-drop|{id}")))
                            .absolute()
                            .top(px(-6.0))
                            .right(px(-6.0))
                            .size(px(20.0))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(p.card)
                            .border_1()
                            .border_color(p.border)
                            .cursor_pointer()
                            .hover(|s| s.text_color(p.destructive))
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_file(id, cx)))
                            .child(icon("x").size(px(12.0))),
                    ),
                SharedString::from(format!("file-rise|{id}")),
                Duration::ZERO,
                8.0,
            ));
        }
        Some(tray.into_any_element())
    }

    /// Lets files be dropped on the composer.
    pub(crate) fn droppable<E: InteractiveElement + gpui_kit::Styled + 'static>(
        &self,
        el: E,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> E {
        if !self.can_attach() {
            return el;
        }
        let (bg, border) = (alpha(p.primary, 0.08), p.primary);
        el.drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(bg).border_color(border))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.add_files(paths.paths().to_vec(), cx)))
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
        let p = crate::ui::widgets::pal(cx);
        let view = window.viewport_size();
        let room = (f32::from(view.width) - 160.0, f32::from(view.height) - 180.0);
        let (w, h) = attachments::fit_box(size.0, size.1, (room.0.max(200.0), room.1.max(200.0)));
        let (key, url_owned, name_owned) = (key.to_owned(), url.to_owned(), name.to_owned());
        motion::fade_in(
            scrim("picture-scrim", &p).on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))).child(
                motion::rise(
                    div()
                        .id("picture-panel")
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(12.0))
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            img(SharedString::from(url.to_owned()))
                                .w(px(w))
                                .h(px(h))
                                .rounded(corner(14.0))
                                .object_fit(ObjectFit::Contain),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .px(px(14.0))
                                .py(px(6.0))
                                .rounded_full()
                                .bg(p.card)
                                .border_1()
                                .border_color(p.border)
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::BOLD)
                                        .child(attachments::short_name(name, 48)),
                                )
                                .child(
                                    icon_button("picture-save", "download", &p)
                                        .tooltip(|window, cx| {
                                            gpui_kit::component::tooltip::Tooltip::new("Save").build(window, cx)
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.save_file(
                                                key.clone(),
                                                url_owned.clone(),
                                                name_owned.clone(),
                                                bytes,
                                                cx,
                                            )
                                        })),
                                )
                                .child(
                                    icon_button("picture-close", "x", &p)
                                        .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
                                ),
                        ),
                    "picture-rise",
                    Duration::ZERO,
                    16.0,
                ),
            ),
            "picture-fade",
            Duration::from_millis(160),
        )
        .into_any_element()
    }
}
