//! The Recordings page: first the ceiling on cameras in the server's voice
//! channels (the tallest picture and the most frames a second, for servers
//! whose members' connections can't take more), then what recordings on the
//! server keep, everyone's sound or their cameras and shared screens too,
//! where the instance lets servers keep video. Changing that ends the
//! recording going on; the next starts when someone presses Record. Beside
//! it, the files one person's hour would make. Both save from one bar.
//! The web's `settings/server/Recordings.tsx`.

use super::*;
use crate::ui::settings_controls::Opt;

#[derive(Default)]
pub(super) struct Recordings {
    /// The choice not saved yet; `None` is what the server keeps now.
    pub(super) draft: Option<bool>,
    /// The camera ceiling not saved yet (height, frames a second; 0 for none).
    pub(super) camera: Option<(i32, i32)>,
    /// Whether the instance lets servers keep video, once it says.
    allowed: Option<bool>,
    asked: bool,
    saving: bool,
}

/// The files one person's hour of recording makes, as (name, icon, about how big).
fn files(video: bool) -> Vec<(&'static str, &'static str, &'static str)> {
    let mut out = vec![("Mika.opus", "audio-lines", "4 MB")];
    if video {
        out.push(("Mika camera.webm", "video", "90 MB"));
        out.push(("Mika screen.webm", "monitor", "60 MB"));
    }
    out
}

/// Whether picking video is open: the instance allows it, or the server already keeps it (so it can be kept).
fn video_open(allowed: Option<bool>, keeps: bool) -> bool {
    keeps || allowed != Some(false)
}

impl ServerSettingsView {
    fn ask_recording_video(&mut self, cx: &mut Context<Self>) {
        if self.recordings.asked {
            return;
        }
        self.recordings.asked = true;
        let (core, key) = (self.core.clone(), self.key.clone());
        // Until the instance says, it may: saving would say if not.
        self.run(cx, async move { core.recording_video_allowed(&key).await }, |this, result, cx| {
            this.recordings.allowed = Some(result.unwrap_or(true));
            cx.notify();
        });
    }

    fn save_recordings(&mut self, video: Option<bool>, camera: Option<(i32, i32)>, cx: &mut Context<Self>) {
        self.recordings.saving = true;
        self.error = None;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let patch = ServerPatch {
            record_video: video,
            camera_max_height: camera.map(|c| c.0),
            camera_max_fps: camera.map(|c| c.1),
            ..ServerPatch::default()
        };
        self.run(cx, async move { core.update_server(&key, &sid, patch).await }, |this, result, cx| {
            this.recordings.saving = false;
            match result {
                Ok(_) => {
                    this.recordings.draft = None;
                    this.recordings.camera = None;
                    this.flash_saved(cx);
                }
                Err(err) => this.error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    pub(super) fn recordings_page(
        &mut self,
        server: &pb::Server,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (has_camera, has_video) = self.core.shared.read(|s| {
            s.instance(&self.key).map_or((false, false), |i| (i.has("camera-quality"), i.has("video-recordings")))
        });
        if has_video {
            self.ask_recording_video(cx);
        }
        let keeps = server.record_video;
        let video = self.recordings.draft.unwrap_or(keeps);
        let changed = video != keeps;
        let saved_camera = (server.camera_max_height, server.camera_max_fps);
        let camera = self.recordings.camera.unwrap_or(saved_camera);
        let camera_changed = camera != saved_camera;
        let open = video_open(self.recordings.allowed, keeps);
        if changed || camera_changed {
            let (video_out, camera_out) = (changed.then_some(video), camera_changed.then_some(camera));
            self.bar = Some(save_bar(
                "recordings-save-bar",
                usize::from(changed)
                    + usize::from(camera.0 != saved_camera.0)
                    + usize::from(camera.1 != saved_camera.1),
                self.recordings.saving,
                p,
                cx,
                |this, _, cx| {
                    this.recordings.draft = None;
                    this.recordings.camera = None;
                    this.error = None;
                    cx.notify();
                },
                move |this, _, cx| this.save_recordings(video_out, camera_out, cx),
            ));
        }
        let camera_section = has_camera.then(|| self.camera_ceiling(camera, saved_camera, p, window, cx));
        if !has_video {
            return div().flex().flex_col().children(camera_section).into_any_element();
        }

        let form_w = if self.wide { self.column - 288.0 - 40.0 } else { self.column };
        let mut video_opt =
            Opt::new(t("serversettings.recordings.video"), t("serversettings.recordings.videoHint"), "video");
        if !open {
            video_opt.disabled = Some(t("serversettings.recordings.videoOff"));
        }
        let choices = crate::ui::settings_controls::choice(
            "record-video",
            Some(usize::from(video)),
            vec![
                Opt::new(t("serversettings.recordings.sound"), t("serversettings.recordings.soundHint"), "audio-lines"),
                video_opt,
            ],
            form_w,
            p,
            window,
            cx,
            move |this: &mut Self, i, cx| {
                if i == 1 && !open {
                    return;
                }
                let keeps = this.server_record_video();
                this.recordings.draft = ((i == 1) != keeps).then_some(i == 1);
                cx.notify();
            },
        );

        let mut setting = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .line_height(px(24.0))
                            .child(t("serversettings.recordings.title")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .line_height(px(20.0))
                            .text_color(p.muted_foreground)
                            .child(t("serversettings.recordings.hint")),
                    ),
            )
            .child(choices);
        if !open {
            setting = setting.child(motion::rise(
                div().text_xs().text_color(p.muted_foreground).child(t("serversettings.recordings.videoOffHint")),
                "rec-video-off",
                Duration::ZERO,
                4.0,
            ));
        }
        if changed {
            setting = setting.child(motion::rise(
                div().text_xs().text_color(amber(p)).child(if video {
                    t("serversettings.recordings.changedVideo")
                } else {
                    t("serversettings.recordings.changedSound")
                }),
                SharedString::from(format!("rec-changed-{video}")),
                Duration::ZERO,
                4.0,
            ));
        }

        let mut list = div().flex().flex_col().gap(px(6.0));
        for (n, (name, glyph, size)) in files(video).into_iter().enumerate() {
            list = list.child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(crate::ui::theme::radius_xl())
                    .bg(alpha(p.background, 0.7))
                    .child(
                        div()
                            .size(px(28.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(crate::ui::theme::radius_lg())
                            .bg(alpha(p.primary, 0.1))
                            .text_color(p.primary)
                            .child(icon(glyph).size(px(16.0))),
                    )
                    .child(div().flex_1().min_w_0().truncate().text_sm().font_weight(FontWeight::BOLD).child(name))
                    .child(div().flex_none().text_xs().text_color(p.muted_foreground).child(format!("~{size}"))),
                SharedString::from(format!("rec-file-{name}")),
                Duration::from_millis(60 * n as u64),
                8.0,
            ));
        }
        let preview = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(12.0))
            .rounded(crate::ui::theme::radius_2xl())
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.muted, 0.4))
            .child(div().text_xs().text_color(p.muted_foreground).child(t("serversettings.recordings.preview")))
            .child(list);

        let setting = self.mark("record-video", setting.pb(px(20.0)), p);
        let recordings = crate::ui::settings_controls::with_preview(setting, preview, self.wide, p);
        div().flex().flex_col().children(camera_section).child(recordings).into_any_element()
    }

    /// The ceiling on cameras in the server's voice channels: a tallest
    /// picture and a frame rate, "No ceiling" first. A value set some other
    /// way shows as one more choice.
    fn camera_ceiling(
        &mut self,
        (height, fps): (i32, i32),
        saved: (i32, i32),
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::core::voice::ceiling::{CEILING_FRAME_RATES, CEILING_HEIGHTS};
        use crate::ui::settings_voice::{fps_label, height_label};
        let none = t("serversettings.camera.none");
        let heights = with_current(&CEILING_HEIGHTS, height);
        let rates = with_current(&CEILING_FRAME_RATES, fps);
        let row = |label: String, body: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(div().text_sm().font_weight(FontWeight::BOLD).child(label))
                .child(div().flex().child(body))
        };
        let (h_list, f_list) = (heights.clone(), rates.clone());
        let height_pick = crate::ui::settings_controls::segmented(
            "server-camera-height",
            heights.iter().map(|&h| (height_label(h, &none), None)).collect(),
            heights.iter().position(|&h| h as i32 == height).unwrap_or(0),
            if self.wide { 104.0 } else { 88.0 },
            p,
            window,
            cx,
            move |this: &mut Self, n, cx| {
                let saved = this.server_camera();
                let now = this.recordings.camera.unwrap_or(saved);
                let next = (h_list[n] as i32, now.1);
                this.recordings.camera = (next != saved).then_some(next);
                cx.notify();
            },
        );
        let fps_pick = crate::ui::settings_controls::segmented(
            "server-camera-fps",
            rates.iter().map(|&f| (fps_label(f, &none), None)).collect(),
            rates.iter().position(|&f| f as i32 == fps).unwrap_or(0),
            if self.wide { 104.0 } else { 88.0 },
            p,
            window,
            cx,
            move |this: &mut Self, n, cx| {
                let saved = this.server_camera();
                let now = this.recordings.camera.unwrap_or(saved);
                let next = (now.0, f_list[n] as i32);
                this.recordings.camera = (next != saved).then_some(next);
                cx.notify();
            },
        );
        let _ = saved;
        let section = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .line_height(px(24.0))
                            .child(t("serversettings.camera.title")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .line_height(px(20.0))
                            .text_color(p.muted_foreground)
                            .child(t("serversettings.camera.hint")),
                    ),
            )
            .child(row(t("serversettings.camera.resolution"), height_pick))
            .child(row(t("serversettings.camera.fps"), fps_pick));
        self.mark("camera-quality", section.pb(px(24.0)), p).into_any_element()
    }

    /// The camera ceiling the server keeps now.
    fn server_camera(&self) -> (i32, i32) {
        self.core.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.servers.iter().find(|sv| sv.id == self.server))
                .map_or((0, 0), |sv| (sv.camera_max_height, sv.camera_max_fps))
        })
    }

    fn server_record_video(&self) -> bool {
        self.core.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.servers.iter().find(|sv| sv.id == self.server))
                .is_some_and(|sv| sv.record_video)
        })
    }
}

/// The choices, with the one set now added (in order) when it isn't among them.
fn with_current(list: &[u32], now: i32) -> Vec<u32> {
    let mut out = list.to_vec();
    if let Ok(now) = u32::try_from(now)
        && !out.contains(&now)
    {
        // After "none", from the tallest down.
        let at = out.iter().skip(1).position(|&n| n < now).map_or(out.len(), |i| i + 1);
        out.insert(at, now);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_is_open_unless_the_instance_says_no() {
        assert!(video_open(None, false));
        assert!(video_open(Some(true), false));
        assert!(!video_open(Some(false), false));
        // A server already keeping video can still keep it, or go back to sound.
        assert!(video_open(Some(false), true));
    }

    #[test]
    fn a_ceiling_set_elsewhere_is_one_more_choice() {
        assert_eq!(with_current(&[0, 1080, 720, 480, 360], 720), [0, 1080, 720, 480, 360]);
        assert_eq!(with_current(&[0, 1080, 720, 480, 360], 900), [0, 1080, 900, 720, 480, 360]);
        assert_eq!(with_current(&[0, 1080, 720, 480, 360], 2160), [0, 2160, 1080, 720, 480, 360]);
        assert_eq!(with_current(&[0, 60, 30, 24, 15], 10), [0, 60, 30, 24, 15, 10]);
    }
}
