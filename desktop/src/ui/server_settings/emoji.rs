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
    path: PathBuf,
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
    let unreadable = || "That picture couldn't be read.".to_owned();
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
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
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
            prompt: Some("Choose pictures".into()),
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
                self.error = Some("Emoji are PNG, JPEG, GIF or WebP pictures.".into());
                continue;
            };
            let name = unique_emoji_name(&emoji_name_from_file(&file), &taken);
            taken.insert(name.to_lowercase());
            let id = self.emojis.next;
            self.emojis.next += 1;
            self.emojis.pending.push(Pending { id, path: path.clone(), name: name.clone(), done: None, error: None });

            let (core, key, sid, svg) = (self.core.clone(), self.key.clone(), self.server.clone(), cx.svg_renderer());
            self.run(
                cx,
                async move {
                    let bytes = tokio::fs::read(&path).await.map_err(|err| {
                        Problem::new(tonic::Code::NotFound, format!("Couldn't read that file: {err}"))
                    })?;
                    let (bytes, kind) = tokio::task::spawn_blocking(move || shrink(bytes, kind, svg))
                        .await
                        .map_err(|_| Problem::new(tonic::Code::Internal, "That picture couldn't be read."))?
                        .map_err(|msg| Problem::new(tonic::Code::InvalidArgument, msg))?;
                    core.add_emoji(&key, &sid, &name, kind, bytes).await
                },
                move |this, result, cx| {
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
                },
            );
        }
        cx.notify();
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
            self.error = Some("Emoji names are 2 to 32 letters, digits and underscores.".into());
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
                    .gap(px(6.0))
                    .py(px(28.0))
                    .child(motion::once(
                        div().text_size(px(40.0)).child("🫥"),
                        "emoji-empty-bob",
                        Duration::from_millis(1400),
                        |el, t| {
                            let wave = (t * std::f32::consts::TAU).sin();
                            el.relative().top(px(-4.0 * wave.abs()))
                        },
                    ))
                    .child(div().font_weight(FontWeight::BOLD).child("No emoji yet"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(p.muted_foreground)
                            .child("Add the first and it shows up when people type a colon."),
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
                    .text_color(amber)
                    .child(icon("image-plus").size(px(14.0)))
                    .child("This server has all the emoji it can hold. Delete one to make room."),
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
                    .hover(move |s| s.bg(hover_bg).border_color(hover_border))
                    .active(|s| s.top(px(1.0)))
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
            .child(div().font_weight(FontWeight::EXTRA_BOLD).child("Drop pictures here, or pick some"))
            .child(div().max_w(px(400.0)).text_xs().text_color(p.muted_foreground).child(
                "Several at once is fine. Each is shrunk to 128 pixels; GIFs keep moving. \
                 The file name becomes the emoji's name, and you can change it.",
            ))
            .child(
                div()
                    .mt(px(4.0))
                    .px(px(10.0))
                    .py(px(2.0))
                    .rounded_full()
                    .bg(p.muted)
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .child(match cap {
                        Some(cap) => format!("{count} of {cap}"),
                        None => format!("{count} emoji"),
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
            None if pending.done.is_some() => "Added".into(),
            None => "Uploading…".into(),
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
                .child(img(pending.path.clone()).size(px(40.0)).flex_none().object_fit(ObjectFit::Contain))
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
        let about = format!(
            "{} · {}{}",
            creator.map(user_name).unwrap_or_else(|| "Someone".into()),
            size_label(emoji.size),
            if emoji.animated { " · moves" } else { "" }
        );
        let end = if self.emojis.confirming.as_deref() == Some(id.as_str()) {
            let yes = id.clone();
            motion::slide_in(
                div()
                    .flex()
                    .gap(px(4.0))
                    .child(
                        danger_button(SharedString::from(format!("emoji-delete-yes-{id}")), "Delete", p)
                            .h(px(32.0))
                            .px(px(12.0))
                            .text_xs()
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
                .child(
                    img(SharedString::from(emoji.url.clone()))
                        .size(px(40.0))
                        .flex_none()
                        .object_fit(ObjectFit::Contain),
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
