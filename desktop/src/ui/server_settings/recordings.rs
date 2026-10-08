//! The Recordings page: what recordings on the server keep, everyone's sound
//! or their cameras and shared screens too, where the instance lets servers
//! keep video. Changing it ends the recording going on; the next starts when
//! someone presses Record. Beside it, the files one person's hour would make.
//! The web's `settings/server/Recordings.tsx`.

use super::*;
use crate::ui::settings_controls::Opt;

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

    pub(super) fn recordings_page(
        &mut self,
        server: &pb::Server,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        crate::ui::settings_controls::with_preview(setting, preview, self.wide, p)
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
