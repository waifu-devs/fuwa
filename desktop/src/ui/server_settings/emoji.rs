//! The Emoji page: the server's own emoji. Drop pictures in or pick some
//! (several at once), each shrunk to 128 pixels and named after its file;
//! rename one in place, or delete it. They show up for everyone as `:name:`.
//! The web's `settings/server/Emoji.tsx`.

use std::collections::HashSet;
use std::path::PathBuf;

use gpui_kit::{AnimationExt as _, ExternalPaths, Image, ImageFormat, ObjectFit, StyledImage as _, SvgRenderer, img};

use super::*;
use crate::core::account::picture_type;
use crate::core::server_admin::{emoji_name_from_file, emoji_name_ok, unique_emoji_name};

/// The longest side an emoji is kept at, as on the web.
const SIDE: usize = 128;

/// How wide each emoji is in the two-column grid.
const CELL: f32 = 336.0;

/// A picture on its way up.
struct Pending {
    id: u64,
    /// What it'll look like, once it's been checked and shrunk.
    preview: Option<Arc<Image>>,
    name: String,
    /// The emoji it became, kept out of the list while the check shows.
    done: Option<String>,
    error: Option<String>,
}

pub(super) struct Emojis {
    /// The server's cap: `None` until asked, then `Some(None)` for no cap.
    cap: Option<Option<i64>>,
    asking: bool,
    pending: Vec<Pending>,
    next: u64,
    /// The emoji being renamed, and the box its name is typed in.
    renaming: Option<String>,
    rename: Entity<InputState>,
    confirming: Option<String>,
    busy: Option<String>,
    /// When a file that isn't a picture was refused, so the drop zone shakes.
    refused: Option<Instant>,
}

impl Emojis {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ServerSettingsView>) -> (Self, Vec<Subscription>) {
        let rename = cx.new(|cx| InputState::new(window, cx));
        let subscriptions =
            vec![cx.subscribe(&rename, |this: &mut ServerSettingsView, _, e: &InputEvent, cx| match e {
                InputEvent::PressEnter { .. } | InputEvent::Blur => this.finish_rename(cx),
                InputEvent::Change => cx.notify(),
                InputEvent::Focus => {}
            })];
        let emojis = Self {
            cap: None,
            asking: false,
            pending: Vec::new(),
            next: 0,
            renaming: None,
            rename,
            confirming: None,
            busy: None,
            refused: None,
        };
        (emojis, subscriptions)
    }
}

/// The biggest file taken as an emoji, checked before it's read.
const MAX_BYTES: u64 = 10 * 1024 * 1024;
/// The longest side a picture may have before it's decoded.
const MAX_SIDE: u32 = 4096;
/// How many pixels a moving picture may hold over all its frames (about 256 MB decoded).
const MAX_FRAME_PIXELS: u64 = 64 * 1024 * 1024;

fn be16(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?)))
}

fn le16(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?)))
}

fn le24(b: &[u8], at: usize) -> Option<u32> {
    let x = b.get(at..at + 3)?;
    Some(u32::from(x[0]) | u32::from(x[1]) << 8 | u32::from(x[2]) << 16)
}

/// A picture's size (and frames, for a GIF) read from its header, so a
/// picture made to be huge once decoded is refused before it is.
fn header_size(b: &[u8], kind: &str) -> Option<(u32, u32, u64)> {
    match kind {
        "image/png" => {
            (b.get(..8)? == b"\x89PNG\r\n\x1a\n" && b.get(12..16)? == b"IHDR").then_some(())?;
            Some((be16(b, 16)? << 16 | be16(b, 18)?, be16(b, 20)? << 16 | be16(b, 22)?, 1))
        }
        "image/jpeg" => {
            let mut at = 2;
            while at + 9 < b.len() {
                if b[at] != 0xff {
                    return None;
                }
                let marker = b[at + 1];
                if marker == 0xff {
                    at += 1;
                    continue;
                }
                if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
                    return Some((be16(b, at + 7)?, be16(b, at + 5)?, 1));
                }
                at += 2 + be16(b, at + 2)? as usize;
            }
            None
        }
        "image/webp" => {
            (b.get(..4)? == b"RIFF" && b.get(8..12)? == b"WEBP").then_some(())?;
            match b.get(12..16)? {
                b"VP8 " => Some((le16(b, 26)? & 0x3fff, le16(b, 28)? & 0x3fff, 1)),
                b"VP8L" => {
                    let x = b.get(21..25)?;
                    let w = 1 + (u32::from(x[0]) | (u32::from(x[1]) & 0x3f) << 8);
                    let h = 1 + (u32::from(x[1]) >> 6 | u32::from(x[2]) << 2 | (u32::from(x[3]) & 0xf) << 10);
                    Some((w, h, 1))
                }
                b"VP8X" => Some((1 + le24(b, 24)?, 1 + le24(b, 27)?, 1)),
                _ => None,
            }
        }
        "image/gif" => {
            (b.get(..4)? == b"GIF8").then_some(())?;
            let (w, h) = (le16(b, 6)?, le16(b, 8)?);
            let mut at = 13 + if b[10] & 0x80 != 0 { 3 << ((b[10] & 7) + 1) } else { 0 };
            let skip_blocks = |mut at: usize| -> Option<usize> {
                loop {
                    let len = *b.get(at)? as usize;
                    at += 1 + len;
                    if len == 0 {
                        return Some(at);
                    }
                }
            };
            let mut frames = 0u64;
            loop {
                match *b.get(at)? {
                    0x21 => at = skip_blocks(at + 2)?,
                    0x2c => {
                        frames += 1;
                        let flags = *b.get(at + 9)?;
                        at += 10 + if flags & 0x80 != 0 { 3 << ((flags & 7) + 1) } else { 0 };
                        at = skip_blocks(at + 1)?;
                    }
                    0x3b => return Some((w, h, frames.max(1))),
                    _ => return None,
                }
            }
        }
        _ => None,
    }
}

/// Why a picture can't be an emoji, judged from its header alone.
fn too_big(bytes: &[u8], kind: &str) -> Option<String> {
    let Some((w, h, frames)) = header_size(bytes, kind) else {
        return Some(t("desktop.server.emoji.unreadable"));
    };
    if w == 0 || h == 0 {
        return Some(t("desktop.server.emoji.unreadable"));
    }
    if w > MAX_SIDE || h > MAX_SIDE {
        return Some(t_with(
            "desktop.server.emoji.tooBig",
            &[
                ("width", Arg::Str(&w.to_string())),
                ("height", Arg::Str(&h.to_string())),
                ("max", Arg::Str(&MAX_SIDE.to_string())),
            ],
        ));
    }
    if frames * u64::from(w) * u64::from(h) > MAX_FRAME_PIXELS {
        return Some(t("desktop.server.emoji.tooManyFrames"));
    }
    None
}

/// A picture made at most [`SIDE`] pixels on its longest side, as a PNG. GIFs
/// stay as they are, so they keep moving; small pictures aren't touched, nor
/// ones whose file is already smaller than the PNG would be.
fn shrink(bytes: Vec<u8>, kind: &'static str, svg: SvgRenderer) -> Result<(Vec<u8>, &'static str), String> {
    let format = match kind {
        "image/gif" => return Ok((bytes, kind)),
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        _ => ImageFormat::Webp,
    };
    let unreadable = || t("desktop.server.emoji.unreadable");
    let picture = Image::from_bytes(format, bytes.clone()).to_image_data(svg).map_err(|_| unreadable())?;
    let s = picture.size(0);
    let (sw, sh) = (i32::from(s.width).max(0) as usize, i32::from(s.height).max(0) as usize);
    let px = picture.as_bytes(0).filter(|b| sw > 0 && sh > 0 && b.len() >= sw * sh * 4).ok_or_else(unreadable)?;
    if sw.max(sh) <= SIDE {
        return Ok((bytes, kind));
    }
    let (dw, dh) =
        if sw >= sh { (SIDE, (sh * SIDE).div_ceil(sw).max(1)) } else { ((sw * SIDE).div_ceil(sh).max(1), SIDE) };
    // Each pixel the average of the ones it covers, weighted by how opaque
    // they are so see-through edges don't go dark.
    let mut rgba = Vec::with_capacity(dw * dh * 4);
    for y in 0..dh {
        let (y0, y1) = (y * sh / dh, ((y + 1) * sh / dh).max(y * sh / dh + 1).min(sh));
        for x in 0..dw {
            let (x0, x1) = (x * sw / dw, ((x + 1) * sw / dw).max(x * sw / dw + 1).min(sw));
            let (mut bgr, mut a, mut n) = ([0f32; 3], 0f32, 0f32);
            for row in y0..y1 {
                for col in x0..x1 {
                    let at = (row * sw + col) * 4;
                    let alpha = f32::from(px[at + 3]);
                    for c in 0..3 {
                        bgr[c] += f32::from(px[at + c]) * alpha;
                    }
                    a += alpha;
                    n += 1.0;
                }
            }
            let color = |c: f32| if a > 0.0 { (c / a).round().clamp(0.0, 255.0) as u8 } else { 0 };
            // BGRA back to RGBA.
            rgba.extend_from_slice(&[color(bgr[2]), color(bgr[1]), color(bgr[0]), (a / n).round() as u8]);
        }
    }
    let small = crate::ui::png::png(dw as u32, dh as u32, &rgba, true);
    // A photo can be smaller as the JPEG it came as; then that goes up instead.
    Ok(if small.len() < bytes.len() { (small, "image/png") } else { (bytes, kind) })
}

fn size_label(bytes: i64) -> String {
    match bytes {
        b if b < 1024 => t_with("desktop.server.emoji.bytes", &[("size", Arg::Str(&b.to_string()))]),
        b if b < 1024 * 1024 => {
            t_with("desktop.server.emoji.kilobytes", &[("size", Arg::Str(&format!("{:.1}", b as f64 / 1024.0)))])
        }
        b => t_with("desktop.updates.size", &[("size", Arg::Str(&format!("{:.1}", b as f64 / (1024.0 * 1024.0))))]),
    }
}

impl ServerSettingsView {
    fn ask_emoji_cap(&mut self, cx: &mut Context<Self>) {
        if self.emojis.asking || self.emojis.cap.is_some() {
            return;
        }
        self.emojis.asking = true;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.emoji_cap(&key, &sid).await }, |this, result, cx| {
            this.emojis.asking = false;
            // Not knowing the cap only hides it; adding still works.
            this.emojis.cap = Some(result.ok().flatten());
            cx.notify();
        });
    }

    fn pick_emoji(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(t("desktop.server.emoji.choosePictures").into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let _ = this.update(cx, |this, cx| this.add_emoji(paths, cx));
        })
        .detach();
    }

    /// Sends each picture up as an emoji named after its file.
    fn add_emoji(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let mut taken: HashSet<String> = self.core.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.emojis.get(&self.server))
                .map(|list| list.iter().map(|e| e.name.to_lowercase()).collect())
                .unwrap_or_default()
        });
        taken.extend(self.emojis.pending.iter().map(|p| p.name.to_lowercase()));
        self.error = None;
        for path in paths {
            let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let Some(kind) = picture_type(&file) else {
                self.emojis.refused = Some(Instant::now());
                self.error = Some(t("desktop.server.emoji.badType"));
                continue;
            };
            let name = unique_emoji_name(&emoji_name_from_file(&file), &taken);
            taken.insert(name.to_lowercase());
            let id = self.emojis.next;
            self.emojis.next += 1;
            self.emojis.pending.push(Pending { id, preview: None, name: name.clone(), done: None, error: None });

            // First read, check and shrink it; only then is it shown or sent.
            let svg = cx.svg_renderer();
            self.run(
                cx,
                async move {
                    let unreadable =
                        |_: std::io::Error| Problem::new(tonic::Code::NotFound, t("desktop.look.cantRead"));
                    let size = tokio::fs::metadata(&path).await.map_err(unreadable)?.len();
                    if size > MAX_BYTES {
                        return Err(Problem::new(
                            tonic::Code::InvalidArgument,
                            t_with(
                                "desktop.server.emoji.fileTooBig",
                                &[("size", Arg::Num((MAX_BYTES / (1024 * 1024)) as i64))],
                            ),
                        ));
                    }
                    let bytes = tokio::fs::read(&path).await.map_err(unreadable)?;
                    if let Some(why) = too_big(&bytes, kind) {
                        return Err(Problem::new(tonic::Code::InvalidArgument, why));
                    }
                    tokio::task::spawn_blocking(move || shrink(bytes, kind, svg))
                        .await
                        .map_err(|_| Problem::new(tonic::Code::Internal, t("desktop.server.emoji.unreadable")))?
                        .map_err(|msg| Problem::new(tonic::Code::InvalidArgument, msg))
                },
                move |this, result, cx| {
                    match result {
                        Ok((bytes, kind)) => this.send_emoji(id, name, bytes, kind, cx),
                        Err(err) => {
                            if let Some(p) = this.emojis.pending.iter_mut().find(|p| p.id == id) {
                                p.error = Some(err.message);
                            }
                        }
                    }
                    cx.notify();
                },
            );
        }
        cx.notify();
    }

    fn send_emoji(&mut self, id: u64, name: String, bytes: Vec<u8>, kind: &'static str, cx: &mut Context<Self>) {
        let format = match kind {
            "image/gif" => ImageFormat::Gif,
            "image/png" => ImageFormat::Png,
            "image/jpeg" => ImageFormat::Jpeg,
            _ => ImageFormat::Webp,
        };
        if let Some(p) = self.emojis.pending.iter_mut().find(|p| p.id == id) {
            p.preview = Some(Arc::new(Image::from_bytes(format, bytes.clone())));
        }
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.add_emoji(&key, &sid, &name, kind, bytes).await }, move |this, result, cx| {
            let Some(p) = this.emojis.pending.iter_mut().find(|p| p.id == id) else { return };
            match result {
                Ok(emoji) => {
                    p.done = Some(emoji.id);
                    // The check shows for a moment, then the emoji takes its place.
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(Duration::from_millis(600)).await;
                        let _ = this.update(cx, |this, cx| {
                            this.emojis.pending.retain(|p| p.id != id);
                            cx.notify();
                        });
                    })
                    .detach();
                }
                Err(err) => p.error = Some(err.message),
            }
            cx.notify();
        });
    }

    fn start_rename(&mut self, emoji: &pb::Emoji, window: &mut Window, cx: &mut Context<Self>) {
        self.emojis.renaming = Some(emoji.id.clone());
        self.emojis.confirming = None;
        let name = emoji.name.clone();
        self.emojis.rename.update(cx, |s, cx| {
            s.set_value(name, window, cx);
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        cx.notify();
    }

    fn finish_rename(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.emojis.renaming.take() else { return };
        let name = self.emojis.rename.read(cx).value().trim().replace(char::is_whitespace, "_");
        let current = self.core.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.emojis.get(&self.server))
                .and_then(|list| list.iter().find(|e| e.id == id).map(|e| e.name.clone()))
        });
        cx.notify();
        if current.as_deref().is_none_or(|c| c == name) {
            return;
        }
        if !emoji_name_ok(&name) {
            self.error = Some(t("serversettings.emoji.badName"));
            return;
        }
        self.error = None;
        self.emojis.busy = Some(id.clone());
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.rename_emoji(&key, &sid, &id, &name).await }, |this, result, cx| {
            this.emojis.busy = None;
            match result {
                Ok(_) => this.flash_saved(cx),
                Err(err) => this.error = Some(err.message),
            }
            cx.notify();
        });
    }

    fn remove_emoji(&mut self, id: String, cx: &mut Context<Self>) {
        self.emojis.busy = Some(id.clone());
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.delete_emoji(&key, &sid, &id).await }, |this, result, cx| {
            this.emojis.busy = None;
            this.emojis.confirming = None;
            if let Err(err) = result {
                this.error = Some(err.message);
            }
            cx.notify();
        });
        cx.notify();
    }

    pub(super) fn emoji_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.ask_emoji_cap(cx);
        let (mut emojis, people): (Vec<pb::Emoji>, HashMap<String, pb::User>) = self.core.shared.read(|s| {
            let Some(i) = s.instance(&self.key) else { return (Vec::new(), HashMap::new()) };
            let people = i
                .members
                .get(&self.server)
                .into_iter()
                .flatten()
                .filter_map(|m| m.user.clone())
                .map(|u| (u.id.clone(), u))
                .collect();
            (i.emojis.get(&self.server).cloned().unwrap_or_default(), people)
        });
        emojis.sort_by_key(|e| e.name.to_lowercase());
        let count = emojis.len();
        let cap = self.emojis.cap.flatten();
        let sending = self.emojis.pending.iter().filter(|p| p.error.is_none() && p.done.is_none()).count();
        let full = cap.is_some_and(|c| (count + sending) as i64 >= c);

        let mut page = div().flex().flex_col().gap(px(20.0)).child(self.emoji_drop(count, cap, full, p, cx));

        if count == 0 && self.emojis.pending.is_empty() {
            page = page.child(motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .py(px(32.0))
                    .child(motion::once(
                        div().text_size(px(36.0)).line_height(px(40.0)).child("🫥"),
                        "emoji-empty-bob",
                        Duration::from_millis(1400),
                        |el, t| {
                            let wave = (t * std::f32::consts::TAU).sin();
                            el.relative().top(px(-4.0 * wave.abs()))
                        },
                    ))
                    .child(div().font_weight(FontWeight::BOLD).child(t("serversettings.emoji.none")))
                    .child(
                        div()
                            .text_sm()
                            .line_height(px(20.0))
                            .text_color(p.muted_foreground)
                            .child(t("serversettings.emoji.noneHint")),
                    ),
                "emoji-empty",
                Duration::from_millis(80),
                8.0,
            ));
        } else {
            let mut grid = div().flex().flex_wrap().gap(px(8.0));
            for pending in &self.emojis.pending {
                grid = grid.child(self.pending_row(pending, p, window, cx));
            }
            let landing: HashSet<&str> = self.emojis.pending.iter().filter_map(|p| p.done.as_deref()).collect();
            for (n, emoji) in emojis.iter().filter(|e| !landing.contains(e.id.as_str())).enumerate() {
                let creator = people.get(&emoji.creator_id);
                grid = grid.child(self.emoji_row(emoji, creator, n, p, window, cx));
            }
            page = page.child(grid);
        }
        if full {
            let amber = amber(p);
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(amber)
                    .child(icon("image-plus").size(px(14.0)))
                    .child(t("serversettings.emoji.full")),
            );
        }
        page.into_any_element()
    }

    fn emoji_drop(
        &self,
        count: usize,
        cap: Option<i64>,
        full: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (primary, hover_bg, drag_bg) = (p.primary, alpha(p.primary, 0.05), alpha(p.primary, 0.12));
        let hover_border = alpha(p.primary, 0.5);
        let zone = div()
            .id("emoji-drop")
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.0))
            .p(px(24.0))
            .rounded(corner(24.0))
            .border_2()
            .border_dashed()
            .border_color(p.border)
            .text_center()
            .when(full, |el| el.opacity(0.6))
            .when(!full, |el| {
                el.cursor_pointer()
                    .hover(move |s| s.bg(hover_bg).border_color(hover_border).translate_y(px(-2.0)))
                    .active(|s| s.scale(0.98))
                    .on_click(cx.listener(|this, _, _, cx| this.pick_emoji(cx)))
                    .drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(drag_bg).border_color(primary))
                    .on_drop(
                        cx.listener(|this, paths: &ExternalPaths, _, cx| this.add_emoji(paths.paths().to_vec(), cx)),
                    )
            })
            .child(
                div()
                    .size(px(48.0))
                    .rounded(corner(16.0))
                    .bg(alpha(p.primary, 0.15))
                    .text_color(p.primary)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(motion::once(
                        icon("face-slightly-smiling-plus").size(px(24.0)),
                        "emoji-drop-wiggle",
                        Duration::from_millis(900),
                        |el, t| {
                            // A wiggle that settles.
                            el.rotate(gpui_kit::radians((t * std::f32::consts::TAU * 2.0).sin() * 0.2 * (1.0 - t)))
                        },
                    )),
            )
            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("serversettings.emoji.drop")))
            .child(
                div()
                    .max_w(px(384.0))
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(t("serversettings.emoji.dropHint")),
            )
            .child(
                div()
                    .mt(px(4.0))
                    .px(px(10.0))
                    .py(px(2.0))
                    .rounded_full()
                    .bg(p.muted)
                    .text_xs()
                    .line_height(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .child(match cap {
                        Some(cap) => t_with(
                            "serversettings.emoji.countOf",
                            &[("count", Arg::Num(count as i64)), ("cap", Arg::Num(cap))],
                        ),
                        None => t_with("serversettings.emoji.count", &[("count", Arg::Num(count as i64))]),
                    }),
            );
        // Shakes once when a file that isn't a picture is refused.
        match self.emojis.refused.filter(|at| at.elapsed() < Duration::from_millis(500)) {
            Some(at) => zone
                .with_animation(
                    SharedString::from(format!("emoji-refused-{at:?}")),
                    gpui_kit::Animation::new(Duration::from_millis(400)),
                    |el, t| el.relative().left(px((t * std::f32::consts::TAU * 2.5).sin() * 8.0 * (1.0 - t))),
                )
                .into_any_element(),
            None => zone.into_any_element(),
        }
    }

    fn pending_row(&self, pending: &Pending, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let id = pending.id;
        let failed = pending.error.is_some();
        let fill = alpha(p.primary, 0.1);
        let bar = if failed {
            None
        } else if pending.done.is_some() {
            Some(div().absolute().top_0().bottom_0().left_0().w_full().bg(fill).into_any_element())
        } else {
            // Most of the way while it's sent; the last bit when the server says yes.
            Some(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .bg(fill)
                    .with_animation(
                        SharedString::from(format!("emoji-send-{id}")),
                        gpui_kit::Animation::new(Duration::from_millis(1600)).with_easing(gpui_kit::ease_out_quint()),
                        |el, t| el.w(gpui_kit::relative(t * 0.9)),
                    )
                    .into_any_element(),
            )
        };
        let end = if failed {
            icon_button(SharedString::from(format!("emoji-dismiss-{id}")), "x", p)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.emojis.pending.retain(|p| p.id != id);
                    cx.notify();
                }))
                .into_any_element()
        } else if pending.done.is_some() {
            motion::rise(
                icon("check").size(px(20.0)).text_color(gpui_kit::rgb(0x10b981)),
                SharedString::from(format!("emoji-done-{id}")),
                Duration::ZERO,
                6.0,
            )
            .into_any_element()
        } else {
            motion::ambient(
                icon("loader-circle").size(px(20.0)).text_color(p.muted_foreground),
                SharedString::from(format!("emoji-spin-{id}")),
                Duration::from_millis(900),
                window,
                |el, t| el.rotate(gpui_kit::radians(t * std::f32::consts::TAU)),
            )
        };
        let status = match &pending.error {
            Some(e) => e.clone(),
            None if pending.done.is_some() => t("serversettings.emoji.added"),
            None => t("serversettings.emoji.uploading"),
        };
        motion::rise(
            div()
                .id(SharedString::from(format!("emoji-pending-{id}")))
                .relative()
                .w(px(CELL))
                .flex()
                .items_center()
                .gap(px(12.0))
                .p(px(12.0))
                .rounded(corner(16.0))
                .border_1()
                .overflow_hidden()
                .border_color(if failed { alpha(p.destructive, 0.5) } else { p.border.into() })
                .when(failed, |el| el.bg(alpha(p.destructive, 0.05)))
                .children(bar)
                .child(match pending.preview.clone() {
                    Some(picture) => {
                        img(picture).size(px(40.0)).flex_none().object_fit(ObjectFit::Contain).into_any_element()
                    }
                    None => div()
                        .size(px(40.0))
                        .flex_none()
                        .rounded(corner(10.0))
                        .bg(alpha(p.muted, 0.6))
                        .into_any_element(),
                })
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .child(div().truncate().font_weight(FontWeight::BOLD).child(format!(":{}:", pending.name)))
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .line_height(px(16.0))
                                .text_color(if failed { p.destructive } else { p.muted_foreground })
                                .child(status),
                        ),
                )
                .child(div().relative().child(end)),
            SharedString::from(format!("emoji-pending-in-{id}")),
            Duration::ZERO,
            8.0,
        )
        .into_any_element()
    }

    fn emoji_row(
        &self,
        emoji: &pb::Emoji,
        creator: Option<&pb::User>,
        n: usize,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = emoji.id.clone();
        let group = SharedString::from(format!("emoji-{id}"));
        let renaming = self.emojis.renaming.as_deref() == Some(id.as_str());
        let busy = self.emojis.busy.as_deref() == Some(id.as_str());
        let hover_border = alpha(p.primary, 0.3);
        let name = if renaming {
            let typed = self.emojis.rename.read(cx).value().to_string();
            let bad = !emoji_name_ok(typed.trim());
            div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .font_weight(FontWeight::BOLD)
                .child(div().text_color(p.muted_foreground).child(":"))
                .child(
                    div()
                        .w(px(180.0))
                        .when(bad, |el| el.text_color(p.destructive))
                        .child(Input::new(&self.emojis.rename).small()),
                )
                .child(div().text_color(p.muted_foreground).child(":"))
                .into_any_element()
        } else {
            let e = emoji.clone();
            let hover = alpha(p.primary, 0.08);
            div()
                .id(SharedString::from(format!("emoji-name-{id}")))
                .flex()
                .items_center()
                .px(px(4.0))
                .ml(px(-4.0))
                .rounded(corner(6.0))
                .cursor_text()
                .font_weight(FontWeight::BOLD)
                .hover(move |s| s.bg(hover))
                .on_click(cx.listener(move |this, _, window, cx| this.start_rename(&e, window, cx)))
                .child(div().text_color(p.muted_foreground).child(":"))
                .child(div().min_w_0().truncate().child(emoji.name.clone()))
                .child(div().text_color(p.muted_foreground).child(":"))
                .when(busy, |el| {
                    el.child(div().ml(px(4.0)).child(motion::ambient(
                        icon("loader-circle").size(px(14.0)).text_color(p.muted_foreground),
                        SharedString::from(format!("emoji-saving-{n}")),
                        Duration::from_millis(900),
                        window,
                        |el, t| el.rotate(gpui_kit::radians(t * std::f32::consts::TAU)),
                    )))
                })
                .into_any_element()
        };
        let by = creator.map(user_name).unwrap_or_else(|| t("common.someone"));
        let about = if emoji.animated {
            format!("{by} · {} · {}", size_label(emoji.size), t("serversettings.emoji.moves"))
        } else {
            format!("{by} · {}", size_label(emoji.size))
        };
        let end = if self.emojis.confirming.as_deref() == Some(id.as_str()) {
            let yes = id.clone();
            motion::slide_in(
                div()
                    .flex()
                    .gap(px(4.0))
                    .child(
                        danger_button(
                            SharedString::from(format!("emoji-delete-yes-{id}")),
                            t("serversettings.shared.delete"),
                            p,
                        )
                        .h(px(32.0))
                        .px(px(12.0))
                        .text_xs()
                        .line_height(px(16.0))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.emojis.busy.is_none() {
                                this.remove_emoji(yes.clone(), cx)
                            }
                        })),
                    )
                    .child(icon_button(SharedString::from(format!("emoji-keep-{id}")), "x", p).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.emojis.confirming = None;
                            cx.notify();
                        },
                    ))),
                SharedString::from(format!("emoji-confirm-{id}")),
                8.0,
            )
            .into_any_element()
        } else {
            let ask = id.clone();
            let red = p.destructive;
            div()
                .opacity(0.5)
                .id(SharedString::from(format!("{group}-tools")))
                .group_hover(group.clone(), |s| s.opacity(1.0))
                .child(icon_button_in(SharedString::from(format!("emoji-delete-{id}")), "trash", p, red).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.emojis.confirming = Some(ask.clone());
                        cx.notify();
                    }),
                ))
                .into_any_element()
        };
        motion::rise(
            div()
                .id(SharedString::from(format!("emoji-row-{id}")))
                .group(group)
                .w(px(CELL))
                .flex()
                .items_center()
                .gap(px(12.0))
                .p(px(12.0))
                .rounded(corner(16.0))
                .border_1()
                .border_color(p.border)
                .bg(alpha(p.background, 0.5))
                .hover(move |s| s.border_color(hover_border))
                // The picture grows and tips under the pointer (`scale: 1.25, rotate: -8`).
                .child(
                    div()
                        .id(SharedString::from(format!("emoji-pic-{id}")))
                        .flex_none()
                        .hover(|s| s.scale(1.25).rotate(gpui_kit::radians(-8f32.to_radians())))
                        .child(
                            img(SharedString::from(emoji.url.clone())).size(px(40.0)).object_fit(ObjectFit::Contain),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(div().flex().min_w_0().child(name))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .text_xs()
                                .line_height(px(16.0))
                                .text_color(p.muted_foreground)
                                .child(avatar(creator, 16.0, p))
                                .child(div().min_w_0().truncate().child(about)),
                        ),
                )
                .child(end),
            SharedString::from(format!("emoji-in-{id}")),
            Duration::from_millis((n.min(12) * 25) as u64),
            8.0,
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gif(w: u16, h: u16, frames: usize) -> Vec<u8> {
        let mut b = b"GIF89a".to_vec();
        b.extend_from_slice(&w.to_le_bytes());
        b.extend_from_slice(&h.to_le_bytes());
        b.extend_from_slice(&[0, 0, 0]);
        // A comment, then the frames: each a descriptor, a code size and one tiny block.
        b.extend_from_slice(&[0x21, 0xfe, 2, b'h', b'i', 0]);
        for _ in 0..frames {
            b.push(0x2c);
            b.extend_from_slice(&[0, 0, 0, 0]);
            b.extend_from_slice(&w.to_le_bytes());
            b.extend_from_slice(&h.to_le_bytes());
            b.extend_from_slice(&[0, 2, 1, 0x44, 0]);
        }
        b.push(0x3b);
        b
    }

    #[test]
    fn pictures_are_sized_from_their_headers() {
        let png = crate::ui::png::png(300, 20, &vec![0; 300 * 20 * 4], false);
        assert_eq!(header_size(&png, "image/png"), Some((300, 20, 1)));

        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0];
        jpeg.extend_from_slice(&[0xff, 0xc0, 0, 17, 8, 0x01, 0x2c, 0x02, 0x58, 3]);
        jpeg.extend_from_slice(&[0; 12]);
        assert_eq!(header_size(&jpeg, "image/jpeg"), Some((600, 300, 1)));

        assert_eq!(header_size(&gif(64, 32, 3), "image/gif"), Some((64, 32, 3)));
        assert_eq!(header_size(b"nonsense at all", "image/png"), None);
    }

    #[test]
    fn huge_pictures_are_refused_before_decoding() {
        assert!(too_big(&gif(128, 128, 10), "image/gif").is_none());
        assert!(too_big(&gif(2048, 2048, 40), "image/gif").is_some(), "too many big frames");
        let mut png = crate::ui::png::png(1, 1, &[0; 4], false);
        png[16..20].copy_from_slice(&50_000u32.to_be_bytes());
        assert!(too_big(&png, "image/png").is_some(), "a header claiming 50000 pixels wide");
        assert!(too_big(b"GIF89a", "image/gif").is_some(), "cut short");
    }
}
