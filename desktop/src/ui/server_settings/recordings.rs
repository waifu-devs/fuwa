//! The Recordings page: what recordings on the server keep, everyone's sound
//! or their cameras and shared screens too, where the instance lets servers
//! keep video. Changing it ends the recording going on; the next starts when
//! someone presses Record. Beside it, the files one person's hour would make.
//! The web's `settings/server/Recordings.tsx`.

use super::*;

#[derive(Default)]
pub(super) struct Recordings {
    /// The choice not saved yet; `None` is what the server keeps now.
    pub(super) draft: Option<bool>,
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

    fn save_recordings(&mut self, video: bool, cx: &mut Context<Self>) {
        self.recordings.saving = true;
        self.error = None;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let patch = ServerPatch { record_video: Some(video), ..ServerPatch::default() };
        self.run(cx, async move { core.update_server(&key, &sid, patch).await }, |this, result, cx| {
            this.recordings.saving = false;
            match result {
                Ok(_) => {
                    this.recordings.draft = None;
                    this.flash_saved(cx);
                }
                Err(err) => this.error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    pub(super) fn recordings_page(&mut self, server: &pb::Server, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        self.ask_recording_video(cx);
        let keeps = server.record_video;
        let video = self.recordings.draft.unwrap_or(keeps);
        let changed = video != keeps;
        let open = video_open(self.recordings.allowed, keeps);
        if changed {
            self.bar = Some(save_bar(
                "recordings-save-bar",
                1,
                self.recordings.saving,
                p,
                cx,
                |this, _, cx| {
                    this.recordings.draft = None;
                    this.error = None;
                    cx.notify();
                },
                move |this, _, cx| this.save_recordings(video, cx),
            ));
        }

        let option = |id: &'static str, on: bool, enabled: bool, glyph: &'static str, label: &str, hint: &str| {
            let hover = alpha(p.primary, 0.06);
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(12.0))
                .p(px(12.0))
                .rounded(corner(14.0))
                .border_1()
                .border_color(if on { alpha(p.primary, 0.6) } else { p.border.into() })
                .when(on, |el| el.bg(alpha(p.primary, 0.08)))
                .when(!enabled, |el| el.opacity(0.5))
                .when(enabled && !on, |el| el.cursor_pointer().hover(move |s| s.bg(hover)))
                .child(crate::ui::settings::radio(on, p))
                .child(
                    div()
                        .size(px(32.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(corner(10.0))
                        .bg(alpha(p.primary, 0.1))
                        .text_color(p.primary)
                        .child(icon(glyph).size(px(16.0))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().font_weight(FontWeight::BOLD).child(label.to_owned()))
                        .child(div().text_sm().text_color(p.muted_foreground).child(hint.to_owned())),
                )
        };
        let pick = |video: bool| {
            cx.listener(move |this: &mut Self, _, _, cx| {
                let keeps = this.server_record_video();
                this.recordings.draft = (video != keeps).then_some(video);
                cx.notify();
            })
        };
        let choices = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                option("rec-sound", !video, true, "audio-lines", "Sound only", "A track per person, as Ogg Opus.")
                    .on_click(pick(false)),
            )
            .child(
                option(
                    "rec-video",
                    video,
                    open,
                    "video",
                    "Sound and video",
                    if open {
                        "Each person's camera and shared screen too, as WebM files next to their sound."
                    } else {
                        "This instance doesn't let servers record video"
                    },
                )
                .when(open, |el| el.on_click(pick(true))),
            );

        let mut setting = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().font_weight(FontWeight::EXTRA_BOLD).child("What recordings keep"))
                    .child(div().text_sm().text_color(p.muted_foreground).child(
                        "For recordings on the server, which people with Record start in a voice channel. \
                         Recordings on someone's own device are always sound only.",
                    )),
            )
            .child(choices);
        if !open {
            setting = setting.child(motion::rise(
                div().text_xs().text_color(p.muted_foreground).child(
                    "This instance doesn't let servers keep video in recordings. Whoever runs it can turn it on \
                     (Video in recordings, under Calls in its settings).",
                ),
                "rec-video-off",
                Duration::ZERO,
                4.0,
            ));
        }
        if changed {
            setting = setting.child(motion::rise(
                div().text_xs().text_color(amber(p)).child(if video {
                    "Everyone in the call sees “Recording with video”. Pictures take far more room than sound, \
                     against the same storage cap. A recording going on ends; the next, when someone presses \
                     Record, keeps video."
                } else {
                    "A recording going on ends; the next, when someone presses Record, keeps sound only."
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
                    .rounded(corner(12.0))
                    .bg(alpha(p.background, 0.7))
                    .child(
                        div()
                            .size(px(28.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(corner(8.0))
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
            .w(px(240.0))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(12.0))
            .rounded(corner(16.0))
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.muted, 0.4))
            .child(div().text_xs().text_color(p.muted_foreground).child("An hour-long recording, for one person:"))
            .child(list);

        div()
            .flex()
            .items_start()
            .gap(px(24.0))
            .child(div().flex_1().min_w_0().child(setting))
            .child(preview)
            .into_any_element()
    }

    fn server_record_video(&self) -> bool {
        self.core.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.servers.iter().find(|sv| sv.id == self.server))
                .is_some_and(|sv| sv.record_video)
        })
    }
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
}
