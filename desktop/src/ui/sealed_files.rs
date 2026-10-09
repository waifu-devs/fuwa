//! The files an encrypted message carries (the web's `dm/SealedFiles.tsx`):
//! each fetched from this instance and opened on this device. Pictures show
//! inline once opened (small ones by themselves), and only when their own
//! bytes say they're a picture; everything else is a card to save. Also the
//! paperclip and the tray of files waiting to be sealed and sent
//! (`dm/EncryptedFiles.tsx`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Image, ImageFormat, InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _, WeakEntity, div, img, px,
};

use crate::core::attachments;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::sealed_files::{self, Outgoing};
use crate::core::vault::FileRef;
use crate::ui::app::{FuwaApp, Target};
use crate::ui::dm_view::seal;
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, radius_lg, radius_xl};
use crate::ui::widgets::icon;

/// The most room one picture takes in a message.
const MEDIA_BOX: (f32, f32) = (420.0, 320.0);
/// Pictures up to this size open by themselves.
const AUTO_OPEN_BYTES: i64 = 25 * 1024 * 1024;

/// Where opening one file stands.
#[derive(Clone)]
enum State {
    Opening,
    Open { bytes: Arc<Vec<u8>>, picture: Option<Arc<Image>> },
    Failed(String),
}

thread_local! {
    static FILES: RefCell<HashMap<String, State>> = RefCell::new(HashMap::new());
    /// Files picked to send, per conversation or channel.
    static PICKED: RefCell<HashMap<String, Vec<Outgoing>>> = RefCell::new(HashMap::new());
}

fn state_of(media_id: &str) -> Option<State> {
    FILES.with(|f| f.borrow().get(media_id).cloned())
}

/// Opens a file on this device, once (and again after it failed, when asked).
fn open(this: &WeakEntity<FuwaApp>, key: &str, file: &FileRef, cx: &mut gpui_kit::App) {
    if matches!(state_of(&file.media_id), Some(State::Opening | State::Open { .. })) {
        return;
    }
    FILES.with(|f| f.borrow_mut().insert(file.media_id.clone(), State::Opening));
    let (key, file, id) = (key.to_owned(), file.clone(), file.media_id.clone());
    let _ = this.update(cx, |app, cx| {
        let core = app.core.clone();
        app.run(cx, async move { core.open_dm_file(&key, &file).await }, move |_, result, cx| {
            let state = match result {
                Ok(bytes) => {
                    let format = match sealed_files::picture_kind(&bytes[..bytes.len().min(64)]) {
                        Some("png") => Some(ImageFormat::Png),
                        Some("jpeg") => Some(ImageFormat::Jpeg),
                        Some("gif") => Some(ImageFormat::Gif),
                        Some("webp") => Some(ImageFormat::Webp),
                        _ => None,
                    };
                    let picture = format.map(|f| Arc::new(Image::from_bytes(f, bytes.to_vec())));
                    State::Open { bytes, picture }
                }
                Err(problem) => State::Failed(problem.message),
            };
            FILES.with(|f| f.borrow_mut().insert(id.clone(), state));
            cx.notify();
        });
    });
}

/// Saves an opened file where the person picks, under its cleaned name.
fn save(this: &WeakEntity<FuwaApp>, name: String, bytes: Arc<Vec<u8>>, cx: &mut gpui_kit::App) {
    let _ = this.update(cx, |_, cx| {
        let dir = dirs::download_dir().or_else(dirs::home_dir).unwrap_or_default();
        let suggested: String = sealed_files::clean_name(&name)
            .chars()
            .map(|c| if matches!(c, '/' | '\\' | ':') { '_' } else { c })
            .collect::<String>()
            .trim_start_matches('.')
            .to_owned();
        let path = cx.prompt_for_new_path(&dir, Some(if suggested.is_empty() { "file" } else { &suggested }));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = path.await else { return };
            let written = std::fs::write(&path, bytes.as_slice());
            let _ = this.update(cx, |this, cx| match written {
                Ok(()) => {
                    let shown = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    this.toast("download", format!("Saved {shown}"), String::new(), None, None, cx);
                }
                Err(err) => this.toast("circle-alert", "Couldn't save that".into(), err.to_string(), None, None, cx),
            });
        })
        .detach();
    });
}

/// The files a message carries.
pub(crate) fn files_view(
    mid: &str,
    files: &[FileRef],
    p: &Palette,
    this: &WeakEntity<FuwaApp>,
    key: &str,
    cx: &mut gpui_kit::App,
) -> AnyElement {
    let mut out = div().mt(px(4.0)).flex().flex_col().gap(px(6.0));
    for (n, file) in files.iter().enumerate() {
        out = out.child(one_file(&format!("{mid}|{n}"), file, p, this, key, cx));
    }
    out.into_any_element()
}

fn one_file(
    id: &str,
    file: &FileRef,
    p: &Palette,
    this: &WeakEntity<FuwaApp>,
    key: &str,
    cx: &mut gpui_kit::App,
) -> AnyElement {
    let s = seal(p);
    let hinted = file.content_type.starts_with("image/") || file.width > 0;
    let auto = hinted && (if file.file_size > 0 { file.file_size } else { file.size }) <= AUTO_OPEN_BYTES;
    let state = state_of(&file.media_id);
    // Small pictures open by themselves when they show.
    if auto && state.is_none() {
        let (this, key, file) = (this.clone(), key.to_owned(), file.clone());
        cx.defer(move |cx| open(&this, &key, &file, cx));
    }
    if let Some(State::Open { bytes, picture: Some(picture) }) = &state {
        let (w, h) = attachments::fit_box(file.width as i32, file.height as i32, MEDIA_BOX);
        let (this, name, bytes) = (this.clone(), file.name.clone(), bytes.clone());
        return div()
            .id(SharedString::from(format!("sealed-pic|{id}")))
            .group("sealed")
            .relative()
            .w(px(w))
            .h(px(h))
            .max_w_full()
            .rounded(radius_xl())
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.muted, 0.6))
            .child(img(picture.clone()).size_full().rounded(radius_xl()).object_fit(ObjectFit::Contain))
            .child(
                div()
                    .id(SharedString::from(format!("sealed-save|{id}")))
                    .absolute()
                    .top(px(8.0))
                    .right(px(8.0))
                    .size(px(32.0))
                    .rounded(radius_lg())
                    .bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.55))
                    .text_color(gpui_kit::white())
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    // Fades in on the picture's hover, and grows under the pointer.
                    .opacity(0.0)
                    .group_hover("sealed", |st| st.opacity(1.0))
                    .hover(|st| st.opacity(1.0).scale(1.1))
                    .tooltip({
                        let name = file.name.clone();
                        move |window, cx| {
                            crate::ui::overlay::Tip::new(t_with(
                                "dms-calls.dm.sealed.download",
                                &[("name", Arg::Str(&name))],
                            ))
                            .build(window, cx)
                        }
                    })
                    .on_click(move |_, _, cx| save(&this, name.clone(), bytes.clone(), cx))
                    .child(icon("download").size(px(16.0))),
            )
            .into_any_element();
    }
    let opened = match &state {
        Some(State::Open { bytes, .. }) => Some(bytes.clone()),
        _ => None,
    };
    let busy = matches!(state, Some(State::Opening));
    let failed = match &state {
        Some(State::Failed(why)) => Some(why.clone()),
        _ => None,
    };
    let size = opened.as_ref().map_or(file.file_size, |b| b.len() as i64);
    let status: AnyElement = if let Some(why) = &failed {
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .text_color(p.destructive)
            .child(icon("shield-alert").size(px(12.0)))
            .child(capital(why))
            .into_any_element()
    } else if busy {
        div().child(t("dms-calls.dm.sealed.opening")).into_any_element()
    } else if size > 0 {
        div().child(attachments::format_bytes(size)).into_any_element()
    } else {
        div().child(t("dms-calls.dm.sealed.file")).into_any_element()
    };
    let card_button = |bid: String, glyph: &str, label: String| {
        let hover_bg = alpha(p.primary, 0.1);
        let hover_fg = p.primary;
        div()
            .id(SharedString::from(bid))
            .size(px(32.0))
            .flex_none()
            .rounded(radius_lg())
            .flex()
            .items_center()
            .justify_center()
            .text_color(p.muted_foreground)
            .cursor_pointer()
            // `whileHover={{ scale: 1.1 }}` and `whileTap={{ scale: 0.9 }}`.
            .hover(move |st| st.bg(hover_bg).text_color(hover_fg).scale(1.1))
            .active(|st| st.scale(0.9))
            .when(busy, |el| el.opacity(0.5))
            .tooltip(move |window, cx| crate::ui::overlay::Tip::new(label.clone()).build(window, cx))
            .child(icon(glyph).size(px(16.0)))
    };
    let show = (hinted && opened.is_none()).then(|| {
        let (this, key, file) = (this.clone(), key.to_owned(), file.clone());
        let glyph = if busy { "loader-circle" } else { "eye" };
        card_button(
            format!("sealed-show|{id}"),
            glyph,
            t_with("dms-calls.dm.sealed.show", &[("name", Arg::Str(&file.name))]),
        )
        .on_click(move |_, _, cx| {
            FILES.with(|f| {
                if matches!(f.borrow().get(&file.media_id), Some(State::Failed(_))) {
                    f.borrow_mut().remove(&file.media_id);
                }
            });
            open(&this, &key, &file, cx)
        })
    });
    let download = {
        let (this, key, file) = (this.clone(), key.to_owned(), file.clone());
        card_button(
            format!("sealed-download|{id}"),
            "download",
            t_with("dms-calls.dm.sealed.download", &[("name", Arg::Str(&file.name))]),
        )
        .on_click(move |_, _, cx| match state_of(&file.media_id) {
            Some(State::Open { bytes, .. }) => save(&this, file.name.clone(), bytes, cx),
            Some(State::Opening) => {}
            _ => {
                FILES.with(|f| f.borrow_mut().remove(&file.media_id));
                open(&this, &key, &file, cx);
                // Saved once it's open.
                let (this, file) = (this.clone(), file.clone());
                cx.spawn(async move |cx| {
                    for _ in 0..600 {
                        cx.background_executor().timer(Duration::from_millis(100)).await;
                        match state_of(&file.media_id) {
                            Some(State::Opening) => continue,
                            Some(State::Open { bytes, .. }) => {
                                cx.update(|cx| save(&this, file.name.clone(), bytes, cx));
                                break;
                            }
                            _ => break,
                        }
                    }
                })
                .detach();
            }
        })
    };
    div()
        .flex()
        .items_center()
        .gap(px(12.0))
        .w(px(384.0))
        .max_w_full()
        .px(px(12.0))
        .py(px(10.0))
        .rounded(radius_xl())
        .border_1()
        .border_color(if failed.is_some() { alpha(p.destructive, 0.5) } else { p.border.into() })
        .bg(alpha(p.muted, 0.4))
        .child(
            div().relative().child(crate::ui::attachments::badge(&file.name, 40.0)).child(
                div()
                    .absolute()
                    .right(px(-4.0))
                    .bottom(px(-4.0))
                    .size(px(16.0))
                    .rounded_full()
                    .bg(p.card)
                    .text_color(s.green)
                    .shadow(crate::ui::polls::shadow_sm())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("lock-keyhole").size(px(10.0))),
            ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .line_height(px(20.0))
                        .font_weight(gpui_kit::FontWeight::BOLD)
                        .child(attachments::short_name(&file.name, 48)),
                )
                .child(div().truncate().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(
                    motion::rise(
                        div().child(status),
                        SharedString::from(format!(
                            "sealed-status|{id}|{}",
                            if failed.is_some() {
                                "failed"
                            } else if busy {
                                "busy"
                            } else {
                                "idle"
                            }
                        )),
                        Duration::ZERO,
                        3.0,
                    ),
                )),
        )
        .children(show)
        .child(download)
        .into_any_element()
}

fn capital(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// Files on their way in an encrypted message (the web's `PendingFiles`).
pub(crate) fn pending_files(files: &[(String, i64)], p: &Palette) -> AnyElement {
    let s = seal(p);
    let mut out = div().mt(px(4.0)).flex().flex_col().gap(px(6.0));
    for (name, size) in files {
        out = out.child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .w(px(384.0))
                .max_w_full()
                .px(px(12.0))
                .py(px(10.0))
                .rounded(radius_xl())
                .border_1()
                .border_color(p.border)
                .bg(alpha(p.muted, 0.4))
                .child(crate::ui::attachments::badge(name, 40.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .line_height(px(20.0))
                                .font_weight(gpui_kit::FontWeight::BOLD)
                                .child(attachments::short_name(name, 48)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .line_height(px(16.0))
                                .text_color(p.muted_foreground)
                                .child(attachments::format_bytes(*size)),
                        ),
                )
                .child(icon("lock-keyhole").size(px(16.0)).text_color(s.green)),
        );
    }
    out.into_any_element()
}

// ───────────────────────── Picking files to send ─────────────────────────

/// The files picked to send in `place` (a conversation or a secure channel).
pub(crate) fn picked(place: &str) -> Vec<Outgoing> {
    PICKED.with(|p| p.borrow().get(place).cloned().unwrap_or_default())
}

/// Takes the files picked in `place`, to send them.
pub(crate) fn take_picked(place: &str) -> Vec<Outgoing> {
    PICKED.with(|p| p.borrow_mut().remove(place).unwrap_or_default())
}

impl FuwaApp {
    /// The paperclip (the web's `EncryptedAttach`): files are encrypted on this device before they go.
    pub(crate) fn encrypted_attach(&self, place: &str, ready: bool, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let place = place.to_owned();
        crate::ui::widgets::tool_button("enc-attach", "paperclip", false, p)
            .when(!ready, |el| el.opacity(0.5))
            .tooltip(|window, cx| crate::ui::overlay::Tip::new(t("dms-calls.dm.files.attachTitle")).build(window, cx))
            .on_click(cx.listener(move |this, _, _, cx| {
                if ready {
                    this.pick_sealed_files(place.clone(), cx)
                }
            }))
            .into_any_element()
    }

    fn pick_sealed_files(&mut self, place: String, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(t("dms-calls.dm.files.attach").into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let read = cx
                .background_executor()
                .spawn(async move {
                    paths
                        .into_iter()
                        .filter_map(|path| {
                            let bytes = std::fs::read(&path).ok()?;
                            let name = path.file_name()?.to_string_lossy().into_owned();
                            let content_type = attachments::content_type_of(&name).to_owned();
                            Some(Outgoing { name, content_type, bytes: Arc::new(bytes) })
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |_, cx| {
                PICKED.with(|p| {
                    let mut p = p.borrow_mut();
                    let list = p.entry(place.clone()).or_default();
                    list.extend(read);
                    list.truncate(sealed_files::MAX_FILES);
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// The files waiting to go (the web's `PickedTray`), each with a way to take it out.
    pub(crate) fn picked_tray(&self, place: &str, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let files = picked(place);
        if files.is_empty() {
            return None;
        }
        let mut tray = div().flex().flex_wrap().gap(px(8.0)).pb(px(8.0));
        for (n, file) in files.iter().enumerate() {
            let place = place.to_owned();
            let hover = p.muted;
            tray = tray.child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .max_w(px(240.0))
                    .pl(px(6.0))
                    .pr(px(4.0))
                    .py(px(4.0))
                    .rounded(radius_xl())
                    .border_1()
                    .border_color(p.border)
                    .bg(alpha(p.muted, 0.4))
                    .child(crate::ui::attachments::badge(&file.name, 28.0))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .line_height(px(16.0))
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .child(attachments::short_name(&file.name, 32)),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .line_height(px(14.0))
                                    .text_color(p.muted_foreground)
                                    .child(attachments::format_bytes(file.bytes.len() as i64)),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("picked-remove|{n}")))
                            .size(px(24.0))
                            .flex_none()
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(p.muted_foreground)
                            .cursor_pointer()
                            .hover(move |st| st.bg(hover))
                            .tooltip({
                                let label = t_with("dms-calls.dm.files.remove", &[("name", Arg::Str(&file.name))]);
                                move |window, cx| crate::ui::overlay::Tip::new(label.clone()).build(window, cx)
                            })
                            .on_click(cx.listener(move |_, _, _, cx| {
                                PICKED.with(|pk| {
                                    if let Some(list) = pk.borrow_mut().get_mut(&place)
                                        && n < list.len()
                                    {
                                        list.remove(n);
                                    }
                                });
                                cx.notify();
                            }))
                            .child(icon("x").size(px(14.0))),
                    ),
                SharedString::from(format!("picked-in|{n}|{}", file.name)),
                Duration::ZERO,
                6.0,
            ));
        }
        Some(tray.into_any_element())
    }

    /// Sends the picked files (with what's typed) in the open conversation or secure channel.
    pub(crate) fn send_picked(&mut self, text: String, cx: &mut Context<Self>) -> bool {
        let Some(Target::Dm { key, conversation: id } | Target::Secure { key, channel: id, .. }) = self.target() else {
            return false;
        };
        self.send_picked_in(key, id.clone(), id, text, None, cx)
    }

    /// Sends the files picked in `place` (a conversation, a secure channel, or
    /// one of its threads, with `thread` its message and whether the reply
    /// goes to the channel too).
    pub(crate) fn send_picked_in(
        &mut self,
        key: String,
        id: String,
        place: String,
        text: String,
        thread: Option<(i64, bool)>,
        cx: &mut Context<Self>,
    ) -> bool {
        let files = take_picked(&place);
        if files.is_empty() {
            return false;
        }
        let nonce = crate::core::dms::new_nonce();
        let sizes: Vec<(String, i64)> =
            files.iter().map(|f| (sealed_files::clean_name(&f.name), f.bytes.len() as i64)).collect();
        PENDING.with(|p| p.borrow_mut().insert(nonce, (place.clone(), text.clone(), sizes)));
        // An also-sent thread reply shows in the channel too while it goes.
        if let Some((_, true)) = thread {
            let sizes = PENDING.with(|p| p.borrow()[&nonce].2.clone());
            PENDING.with(|p| p.borrow_mut().insert(nonce + (1 << 62), (id.clone(), text.clone(), sizes)));
        }
        self.sync_list(cx);
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.send_dm_files(&key, &id, files, text, thread).await },
            move |this, result, cx| {
                PENDING.with(|p| {
                    let mut p = p.borrow_mut();
                    p.remove(&nonce);
                    p.remove(&(nonce + (1 << 62)));
                });
                crate::ui::dm_view::set_problem(&place, result.err().map(|e| e.0));
                this.sync_list(cx);
                cx.notify();
            },
        );
        true
    }
}

/// Each file's name and size.
pub(crate) type Sizes = Vec<(String, i64)>;
/// Files on their way: where, the caption, and each file's name and size.
type Going = (String, String, Sizes);

thread_local! {
    /// Files on their way, by a number of their own: where, the caption, and each file's name and size.
    static PENDING: RefCell<HashMap<u64, Going>> = RefCell::new(HashMap::new());
}

/// The files on their way in `place`, for its list.
pub(crate) fn pending_in(place: &str) -> Vec<(u64, String, Sizes)> {
    let mut out: Vec<_> = PENDING.with(|p| {
        p.borrow()
            .iter()
            .filter(|(_, (at, _, _))| at == place)
            .map(|(n, (_, text, files))| (*n, text.clone(), files.clone()))
            .collect()
    });
    out.sort_by_key(|(n, ..)| *n);
    out
}
