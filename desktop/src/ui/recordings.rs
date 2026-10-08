//! A voice channel's recordings on the server (the web's
//! components/calls/Recordings.tsx): one track per person, each an Ogg Opus
//! file (and, when the server records video, their camera and shared
//! screen as WebM), for the people with Record there. Downloads come as
//! they are, a file each or all of them in a .zip, lined up from the
//! recording's start.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike as _, Local, TimeZone as _, Timelike as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px, relative,
};
use parking_lot::Mutex;

use crate::core::attachments::format_bytes;
use crate::core::calls::{Part, safe_name, zip};
use crate::core::i18n::{Arg, t, t_with};
use crate::pb;
use crate::ui::app::FuwaApp;
use crate::ui::call_parts::red;
use crate::ui::motion;
use crate::ui::overlay::scrim;
use crate::ui::text::ms_of;
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_3xl, radius_xl};
use crate::ui::widgets::{icon, pal};

/// What one person's file is downloading, by (recording, person, part): 0 to 1.
type Progress = Arc<Mutex<HashMap<(String, String, Part), f32>>>;

/// The open recordings dialog.
pub(crate) struct Recordings {
    key: String,
    server: String,
    channel: String,
    channel_name: String,
    list: Option<Vec<pb::Recording>>,
    /// Bytes used, the server's cap, and how many days recordings are kept.
    usage: Option<(i64, Option<i64>, Option<i64>)>,
    problem: Option<String>,
    progress: Progress,
    /// The recording whose bin asked "Delete?", and one being deleted.
    confirming: Option<String>,
    deleting: Option<String>,
    _poll: Option<gpui_kit::Task<()>>,
}

/// "Booth 2026-10-03 09.41": what files from a recording are named after.
fn base_name(channel: &str, rec: &pb::Recording) -> String {
    let at = Local.timestamp_millis_opt(ms_of(rec.started_at.as_ref())).single().unwrap_or_else(Local::now);
    format!("{channel} {}", at.format("%Y-%m-%d %H.%M"))
}

/// "4:07" or "1:02:33", from milliseconds.
fn length(ms: i64) -> String {
    crate::core::calls::clock((ms.max(0) as f64 / 1000.0).round() as u64)
}

impl FuwaApp {
    /// The web's `RecordingsButton`: opens the channel's recordings, with a
    /// red dot while the server records it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn recordings_button(
        &self,
        key: &str,
        server: &str,
        channel: &str,
        states: &[pb::VoiceState],
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let live = states.iter().any(|s| s.server_record);
        let hover = p.muted;
        let fg = p.foreground;
        let (k, s, c) = (key.to_owned(), server.to_owned(), channel.to_owned());
        let background = p.background;
        div()
            .id("recordings-button")
            .relative()
            .size(px(36.0))
            .flex_none()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_color(p.muted_foreground)
            .hover(move |st| st.bg(hover).text_color(fg))
            .active(|st| st.opacity(0.85))
            .tooltip(|window, cx| {
                crate::ui::overlay::Tip::new(t("dms-calls.calls.recordings.buttonTitle")).build(window, cx)
            })
            .on_click(cx.listener(move |this, _, _, cx| this.open_recordings(&k, &s, &c, cx)))
            .child(icon("audio-lines").size(px(18.0)))
            .when(live, |el| {
                let ping = div().absolute().size(px(8.0)).rounded_full().bg(alpha(red(), 0.6));
                let ping = motion::ambient(ping, "rec-button-ping", Duration::from_secs(1), window, |el, t| {
                    let s = 8.0 * (1.0 + t);
                    el.opacity(1.0 - t).size(px(s)).top(px((8.0 - s) / 2.0)).left(px((8.0 - s) / 2.0))
                });
                el.child(
                    div()
                        .absolute()
                        .top(px(6.0))
                        .right(px(6.0))
                        .size(px(8.0))
                        .child(ping)
                        .child(div().absolute().inset_0().rounded_full().bg(red()).border_2().border_color(background)),
                )
            })
            .into_any_element()
    }

    fn open_recordings(&mut self, key: &str, server: &str, channel: &str, cx: &mut Context<Self>) {
        let channel_name = self
            .core
            .shared
            .read(|s| s.instance(key).and_then(|i| i.channel(server, channel)).map(|c| c.name.clone()))
            .unwrap_or_default();
        self.calls.recordings = Some(Recordings {
            key: key.to_owned(),
            server: server.to_owned(),
            channel: channel.to_owned(),
            channel_name,
            list: None,
            usage: None,
            problem: None,
            progress: Arc::default(),
            confirming: None,
            deleting: None,
            _poll: None,
        });
        self.load_recordings(cx);
        // While one is going on, its tracks grow: look again every few seconds.
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let going = this.update(cx, |this, cx| {
                    let r = this.calls.recordings.as_ref()?;
                    let live = this.core.shared.read(|s| {
                        s.instance(&r.key)
                            .and_then(|i| i.voice.get(&r.server))
                            .is_some_and(|l| l.iter().any(|v| v.channel_id == r.channel && v.server_record))
                    });
                    let going = live || r.list.as_ref().is_some_and(|l| l.iter().any(|x| x.ended_at.is_none()));
                    if going {
                        this.load_recordings(cx);
                    }
                    Some(())
                });
                if !matches!(going, Ok(Some(()))) {
                    break;
                }
            }
        });
        if let Some(r) = self.calls.recordings.as_mut() {
            r._poll = Some(poll);
        }
        cx.notify();
    }

    fn load_recordings(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.calls.recordings.as_ref() else { return };
        let (core, key, server, channel) = (self.core.clone(), r.key.clone(), r.server.clone(), r.channel.clone());
        let rx = self.core.spawn(async move { core.recordings(&key, &server, &channel).await });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                let Some(r) = this.calls.recordings.as_mut() else { return };
                match result {
                    Ok(res) => {
                        r.list = Some(res.recordings);
                        r.usage = Some((res.used_bytes, res.cap_bytes, res.keep_days));
                        r.problem = None;
                    }
                    Err(problem) => r.problem = Some(problem.message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Downloads some of a recording's files: one as it is, or several in a .zip.
    fn download_recording_files(
        &mut self,
        rec: pb::Recording,
        files: Vec<(pb::RecordingTrack, Part)>,
        cx: &mut Context<Self>,
    ) {
        let Some(r) = self.calls.recordings.as_ref() else { return };
        if files.is_empty() {
            return;
        }
        let names: HashMap<String, String> = self.core.shared.read(|s| {
            let i = s.instance(&r.key);
            files
                .iter()
                .map(|(track, _)| {
                    (
                        track.user_id.clone(),
                        i.map(|i| i.display_name(Some(&r.server), &track.user_id)).unwrap_or_default(),
                    )
                })
                .collect()
        });
        let base = base_name(&r.channel_name, &rec);
        let file_name = |track: &pb::RecordingTrack, part: Part| {
            format!(
                "{base} - {}{}",
                safe_name(names.get(&track.user_id).map(String::as_str).unwrap_or("")),
                part.suffix()
            )
        };
        let one = files.len() == 1;
        let suggested = if one { file_name(&files[0].0, files[0].1) } else { format!("{}.zip", safe_name(&base)) };
        let named: Vec<(pb::RecordingTrack, Part, String)> = files
            .into_iter()
            .map(|(track, part)| {
                let name = file_name(&track, part);
                (track, part, name)
            })
            .collect();
        let progress = r.progress.clone();
        for (track, part, _) in &named {
            progress.lock().insert((rec.id.clone(), track.user_id.clone(), *part), 0.0);
        }
        let (core, key, server) = (self.core.clone(), r.key.clone(), r.server.clone());
        let dir = dirs::download_dir().or_else(dirs::home_dir).unwrap_or_default();
        let path = cx.prompt_for_new_path(&dir, Some(&suggested));
        let started = Local.timestamp_millis_opt(ms_of(rec.started_at.as_ref())).single().unwrap_or_else(Local::now);
        let when = (
            started.year().max(1980) as u32,
            started.month(),
            started.day(),
            started.hour(),
            started.minute(),
            started.second(),
        );
        cx.spawn(async move |this, cx| {
            let clear = |this: &gpui_kit::WeakEntity<FuwaApp>, cx: &mut gpui_kit::AsyncApp| {
                let _ = this.update(cx, |this, cx| {
                    if let Some(r) = this.calls.recordings.as_ref() {
                        r.progress.lock().retain(|(id, ..), _| *id != rec.id);
                    }
                    cx.notify();
                });
            };
            let Ok(Ok(Some(path))) = path.await else {
                clear(&this, cx);
                return;
            };
            let rx = core.spawn({
                let (core, progress, rec_id) = (core.clone(), progress.clone(), rec.id.clone());
                async move {
                    let mut out = Vec::new();
                    for (track, part, name) in named {
                        let slot = (rec_id.clone(), track.user_id.clone(), part);
                        let progress = progress.clone();
                        let data = core
                            .download_recording(&key, &server, &rec_id, &track, part, move |f| {
                                progress.lock().insert(slot.clone(), f);
                            })
                            .await?;
                        out.push((name, data));
                    }
                    let bytes = if out.len() == 1 { out.remove(0).1 } else { zip(&out, when) };
                    std::fs::write(&path, bytes).map_err(|e| {
                        crate::core::api::Problem::new(tonic::Code::Internal, format!("Couldn't save the file: {e}"))
                    })?;
                    Ok::<_, crate::core::api::Problem>(path)
                }
            });
            // The bars fill as the bytes come.
            let ticker = {
                let this = this.clone();
                cx.spawn(async move |cx| {
                    loop {
                        cx.background_executor().timer(Duration::from_millis(120)).await;
                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                            break;
                        }
                    }
                })
            };
            let result = rx.await;
            drop(ticker);
            clear(&this, cx);
            let _ = this.update(cx, |this, cx| match result {
                Ok(Ok(path)) => {
                    let shown = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    this.toast("download", format!("Saved {shown}"), String::new(), None, None, cx);
                }
                Ok(Err(problem)) => this.toast(
                    "circle-alert",
                    t_with("dms-calls.calls.recordings.downloadFailed", &[("problem", Arg::Str(&problem.message))]),
                    String::new(),
                    None,
                    None,
                    cx,
                ),
                Err(_) => {}
            });
        })
        .detach();
    }

    fn delete_recording(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(r) = self.calls.recordings.as_mut() else { return };
        r.deleting = Some(id.clone());
        let (core, key, server) = (self.core.clone(), r.key.clone(), r.server.clone());
        let rx = self.core.spawn({
            let id = id.clone();
            async move { core.delete_recording(&key, &server, &id).await }
        });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                if let Some(r) = this.calls.recordings.as_mut() {
                    r.deleting = None;
                    r.confirming = None;
                    if result.is_ok()
                        && let Some(list) = r.list.as_mut()
                    {
                        list.retain(|x| x.id != id);
                    }
                }
                match result {
                    Ok(()) => {
                        this.toast("trash-2", t("dms-calls.calls.recordings.deleted"), String::new(), None, None, cx);
                        this.load_recordings(cx);
                    }
                    Err(problem) => this.toast(
                        "circle-alert",
                        t_with("dms-calls.calls.recordings.deleteFailed", &[("problem", Arg::Str(&problem.message))]),
                        String::new(),
                        None,
                        None,
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The dialog, over everything, while it's open.
    pub(crate) fn render_recordings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let r = self.calls.recordings.as_ref()?;
        let p = pal(cx);
        let title = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .text_size(px(20.0))
            .line_height(px(28.0))
            .font_weight(FontWeight::EXTRA_BOLD)
            .child(icon("server").size(px(20.0)).text_color(p.primary))
            .child(t("dms-calls.calls.recordings.title"));
        let description = div()
            .mt(px(4.0))
            .text_sm()
            .line_height(px(20.0))
            .text_color(p.muted_foreground)
            .child(t_with("dms-calls.calls.recordings.description", &[("channel", Arg::Str(&r.channel_name))]));
        let fg = p.foreground;
        let hover = p.muted;
        let close = div()
            .id("recordings-close")
            .absolute()
            .top(px(16.0))
            .right(px(16.0))
            .size(px(32.0))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_color(p.muted_foreground)
            .hover(move |s| s.bg(hover).text_color(fg))
            .on_click(cx.listener(|this, _, _, cx| {
                this.calls.recordings = None;
                cx.notify();
            }))
            .child(icon("x").size(px(16.0)));
        let body = self.recordings_body(&p, window, cx);
        let card = div()
            .id("recordings-card")
            .relative()
            .w(px(672.0))
            .max_h(relative(0.92))
            .overflow_y_scroll()
            .rounded(radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .text_color(p.foreground)
            .p(px(24.0))
            .shadow(crate::ui::settings_controls::shadow_xl())
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(close)
            .child(div().mb(px(20.0)).pr(px(32.0)).child(title).child(description))
            .child(body);
        Some(
            motion::fade_in(
                scrim("recordings-scrim", &p)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.calls.recordings = None;
                            cx.notify();
                        }),
                    )
                    .child(motion::rise(card, "recordings-in", Duration::ZERO, 40.0)),
                "recordings-fade",
                Duration::from_millis(200),
            )
            .into_any_element(),
        )
    }

    fn recordings_body(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(r) = self.calls.recordings.as_ref() else { return div().into_any_element() };
        if let (Some(problem), None) = (&r.problem, &r.list) {
            return div()
                .text_sm()
                .line_height(px(20.0))
                .font_weight(FontWeight::BOLD)
                .text_color(p.destructive)
                .child(problem.clone())
                .into_any_element();
        }
        let Some(list) = r.list.clone() else {
            return div()
                .py(px(40.0))
                .flex()
                .justify_center()
                .text_color(p.muted_foreground)
                .child(motion::ambient(
                    icon("loader-circle").size(px(24.0)),
                    "recordings-loading",
                    Duration::from_secs(1),
                    window,
                    |el, t| el.rotate(gpui_kit::radians(t * std::f32::consts::TAU)),
                ))
                .into_any_element();
        };
        let usage = r.usage.and_then(|u| usage_strip(u, p));
        if list.is_empty() {
            let bob = motion::ambient(
                div()
                    .size(px(48.0))
                    .rounded(radius_2xl())
                    .bg(alpha(p.primary, 0.1))
                    .text_color(p.primary)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("audio-lines").size(px(24.0))),
                "recordings-empty-bob",
                Duration::from_millis(2400),
                window,
                |el, t| el.relative().top(px(-4.0 * (t * std::f32::consts::PI).sin())),
            );
            return div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .children(usage)
                .child(motion::rise(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(radius_3xl())
                        .border_1()
                        .border_dashed()
                        .border_color(p.border)
                        .px(px(24.0))
                        .py(px(40.0))
                        .child(bob)
                        .child(div().font_weight(FontWeight::BOLD).child(t("dms-calls.calls.recordings.emptyTitle")))
                        .child(
                            div()
                                .max_w(px(320.0))
                                .text_center()
                                .text_sm()
                                .line_height(px(20.0))
                                .text_color(p.muted_foreground)
                                .child(t("dms-calls.calls.recordings.emptyText")),
                        ),
                    "recordings-empty",
                    Duration::ZERO,
                    8.0,
                ))
                .into_any_element();
        }
        let mut cards = div().flex().flex_col().gap(px(12.0));
        for (n, rec) in list.iter().enumerate() {
            cards = cards.child(self.recording_card(rec, n, p, window, cx));
        }
        div().flex().flex_col().gap(px(12.0)).children(usage).child(cards).into_any_element()
    }

    fn recording_card(
        &self,
        rec: &pb::Recording,
        n: usize,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(r) = self.calls.recordings.as_ref() else { return div().into_any_element() };
        let live = rec.ended_at.is_none();
        let started = ms_of(rec.started_at.as_ref());
        let now = crate::core::dms::now_ms();
        let span = if live {
            now - started
        } else {
            rec.tracks.iter().map(|t| t.duration_ms).max().unwrap_or(0).max(ms_of(rec.ended_at.as_ref()) - started)
        };
        let (starter, names, mine, may_delete) = self.core.shared.read(|s| {
            let i = s.instance(&r.key);
            let starter = i.map(|i| i.display_name(Some(&r.server), &rec.started_by)).unwrap_or_default();
            let names: HashMap<String, (String, Option<pb::User>)> = rec
                .tracks
                .iter()
                .map(|t| {
                    let name = i.map(|i| i.display_name(Some(&r.server), &t.user_id)).unwrap_or_default();
                    (t.user_id.clone(), (name, i.and_then(|i| i.users.get(&t.user_id).cloned())))
                })
                .collect();
            let mine = i.and_then(|i| i.me.as_ref()).is_some_and(|m| m.id == rec.started_by);
            let manage = i.is_some_and(|i| i.access(&r.server).has_in(&r.channel, pb::Permission::ManageChannels));
            (starter, names, mine, mine || manage)
        });
        let _ = mine;
        let progress = r.progress.lock().clone();
        let busy = progress.keys().any(|(id, ..)| *id == rec.id);
        let everything: Vec<(pb::RecordingTrack, Part)> =
            rec.tracks.iter().flat_map(|t| Part::of(t).into_iter().map(move |p| (t.clone(), p))).collect();
        let date = Local
            .timestamp_millis_opt(started)
            .single()
            .map(|d| d.format("%b %-d, %I:%M %p").to_string())
            .unwrap_or_default();
        let summary = div()
            .min_w(px(160.0))
            .flex_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().child(date))
                    .when(rec.video, |el| {
                        el.child(
                            div()
                                .id(SharedString::from(format!("rec-video|{}", rec.id)))
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(px(4.0))
                                .rounded_full()
                                .bg(alpha(p.primary, 0.1))
                                .px(px(6.0))
                                .py(px(2.0))
                                .text_size(px(11.2))
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.primary)
                                .tooltip(|window, cx| {
                                    crate::ui::overlay::Tip::new(t("dms-calls.calls.recordings.videoTitle"))
                                        .build(window, cx)
                                })
                                .child(icon("video").size(px(14.0)))
                                .child(t("dms-calls.calls.recordings.withVideo")),
                        )
                    }),
            )
            .child(if live {
                let now_text = t_with("dms-calls.calls.recordings.now", &[("time", Arg::Str(&length(span)))]);
                let line = t_with(
                    "dms-calls.calls.recordings.liveLine",
                    &[("now", Arg::Str("\u{1}")), ("name", Arg::Str(&starter))],
                );
                let (before, after) = line.split_once('\u{1}').unwrap_or((&line, ""));
                div()
                    .flex()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(before.to_owned())
                    .child(div().font_weight(FontWeight::BOLD).text_color(red()).child(now_text))
                    .child(after.to_owned())
            } else {
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(t_with(
                        "dms-calls.calls.recordings.doneLine",
                        &[
                            ("time", Arg::Str(&length(span))),
                            ("size", Arg::Str(&format_bytes(rec.size_bytes))),
                            ("name", Arg::Str(&starter)),
                        ],
                    ))
            });
        let wave = {
            let (bg, fg): (gpui_kit::Hsla, gpui_kit::Hsla) =
                if live { (alpha(red(), 0.12), red().into()) } else { (alpha(p.primary, 0.1), p.primary.into()) };
            let mut bars = div().h(px(16.0)).flex().items_center().gap(px(3.0));
            for (i, h) in [0.55f32, 1.0, 0.7, 0.4].into_iter().enumerate() {
                let bar = div().w(px(3.0)).h(px(16.0 * h)).rounded_full().bg(fg);
                let bar = if live {
                    motion::ambient(
                        bar,
                        SharedString::from(format!("rec-wave|{}|{i}", rec.id)),
                        Duration::from_millis(1100),
                        window,
                        move |el, t| {
                            let t = (t + i as f32 * 0.13).rem_euclid(1.0);
                            let k = if t < 0.33 {
                                h + (0.25 - h) * t / 0.33
                            } else if t < 0.66 {
                                0.25 + 0.75 * (t - 0.33) / 0.33
                            } else {
                                1.0 + (h - 1.0) * (t - 0.66) / 0.34
                            };
                            el.h(px(16.0 * k))
                        },
                    )
                } else {
                    bar.into_any_element()
                };
                bars = bars.child(bar);
            }
            div()
                .size(px(40.0))
                .flex_none()
                .rounded(radius_2xl())
                .bg(bg)
                .flex()
                .items_center()
                .justify_center()
                .child(bars)
        };
        let id = rec.id.clone();
        let confirming = r.confirming.as_deref() == Some(rec.id.as_str());
        let removing = r.deleting.as_deref() == Some(rec.id.as_str());
        let actions = (!live).then(|| {
            let rec_all = rec.clone();
            let all_hover = alpha(p.secondary, 0.8);
            let all = div()
                .id(SharedString::from(format!("rec-all|{}", rec.id)))
                .h(px(32.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .rounded(radius_xl())
                .bg(p.secondary)
                .text_sm()
                .line_height(px(20.0))
                .font_weight(FontWeight::BOLD)
                .when(busy || everything.is_empty(), |el| el.opacity(0.5))
                .when(!busy && !everything.is_empty(), |el| {
                    let files = everything.clone();
                    el.cursor_pointer().hover(move |s| s.bg(all_hover)).on_click(cx.listener(move |this, _, _, cx| {
                        this.download_recording_files(rec_all.clone(), files.clone(), cx)
                    }))
                })
                .child(icon("file-archive").size(px(16.0)))
                .child(format!("{} .zip", t("dms-calls.calls.recordings.all")));
            let bin: AnyElement = if !may_delete {
                div().into_any_element()
            } else if confirming {
                let yes = id.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        div()
                            .id(SharedString::from(format!("rec-delete|{}", rec.id)))
                            .h(px(32.0))
                            .px(px(12.0))
                            .flex()
                            .items_center()
                            .rounded(radius_xl())
                            .bg(p.destructive)
                            .text_color(gpui_kit::white())
                            .text_sm()
                            .line_height(px(20.0))
                            .font_weight(FontWeight::BOLD)
                            .cursor_pointer()
                            .when(removing, |el| el.opacity(0.5))
                            .on_click(cx.listener(move |this, _, _, cx| this.delete_recording(yes.clone(), cx)))
                            .child(if removing {
                                t("dms-calls.calls.recordings.deleting")
                            } else {
                                t("dms-calls.calls.recordings.delete")
                            }),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("rec-keep|{}", rec.id)))
                            .h(px(32.0))
                            .px(px(12.0))
                            .flex()
                            .items_center()
                            .rounded(radius_xl())
                            .text_sm()
                            .line_height(px(20.0))
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.8))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(r) = this.calls.recordings.as_mut() {
                                    r.confirming = None;
                                }
                                cx.notify();
                            }))
                            .child(t("dms-calls.calls.recordings.keepIt")),
                    )
                    .into_any_element()
            } else {
                let danger = p.destructive;
                let danger_bg = alpha(p.destructive, 0.1);
                let ask = id.clone();
                div()
                    .id(SharedString::from(format!("rec-bin|{}", rec.id)))
                    .size(px(32.0))
                    .rounded(radius_xl())
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(p.muted_foreground)
                    .hover(move |s| s.bg(danger_bg).text_color(danger))
                    .tooltip(|window, cx| {
                        crate::ui::overlay::Tip::new(t("dms-calls.calls.recordings.deleteTitle")).build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(r) = this.calls.recordings.as_mut() {
                            r.confirming = Some(ask.clone());
                        }
                        cx.notify();
                    }))
                    .child(icon("trash").size(px(16.0)))
                    .into_any_element()
            };
            div().ml_auto().flex_none().flex().items_center().gap(px(4.0)).child(all).child(bin)
        });
        let head = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(12.0))
            .px(px(16.0))
            .pt(px(14.0))
            .pb(px(8.0))
            .child(wave)
            .child(summary)
            .children(actions);
        let mut tracks = div().flex().flex_col().px(px(8.0)).pb(px(8.0));
        for (i, track) in rec.tracks.iter().enumerate() {
            let (name, user) = names.get(&track.user_id).cloned().unwrap_or_default();
            let parts = Part::of(track);
            let fill = parts
                .iter()
                .filter_map(|part| progress.get(&(rec.id.clone(), track.user_id.clone(), *part)).copied())
                .fold(None::<f32>, |a, b| Some(a.map_or(b, |a| a.max(b))));
            let hover = alpha(p.muted, 0.6);
            let mut row = div()
                .id(SharedString::from(format!("rec-track|{}|{}", rec.id, track.user_id)))
                .relative()
                .flex()
                .items_center()
                .gap(px(10.0))
                .overflow_hidden()
                .rounded(radius_2xl())
                .px(px(8.0))
                .py(px(6.0))
                .hover(move |s| s.bg(hover))
                .when_some(fill, |el, f| {
                    el.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(relative(f.max(0.02)))
                            .bg(alpha(p.primary, 0.12)),
                    )
                })
                .child(crate::ui::call_parts::person_avatar(user.as_ref(), &track.user_id, 28.0, 12.0))
                .child(
                    div()
                        .relative()
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_sm()
                        .line_height(px(20.0))
                        .font_weight(FontWeight::BOLD)
                        .child(name.clone()),
                )
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .text_xs()
                        .line_height(px(16.0))
                        .text_color(p.muted_foreground)
                        .child(format_bytes(track.size_bytes + track.camera_bytes + track.screen_bytes)),
                );
            for part in parts {
                let (glyph, live_key, their) = match part {
                    Part::Sound => {
                        ("download", "dms-calls.calls.recordings.liveSound", "dms-calls.calls.recordings.theirSound")
                    }
                    Part::Camera => {
                        ("video", "dms-calls.calls.recordings.liveCamera", "dms-calls.calls.recordings.theirCamera")
                    }
                    Part::Screen => {
                        ("monitor", "dms-calls.calls.recordings.liveScreen", "dms-calls.calls.recordings.theirScreen")
                    }
                };
                if live {
                    if part != Part::Sound {
                        row = row.child(
                            div()
                                .id(SharedString::from(format!("rec-live|{}|{}|{glyph}", rec.id, track.user_id)))
                                .relative()
                                .text_color(p.muted_foreground)
                                .tooltip(move |window, cx| crate::ui::overlay::Tip::new(t(live_key)).build(window, cx))
                                .child(icon(glyph).size(px(14.0))),
                        );
                    }
                    continue;
                }
                let getting = progress.contains_key(&(rec.id.clone(), track.user_id.clone(), part));
                let (fg, bg) = (p.primary, alpha(p.primary, 0.1));
                let (rec_one, track_one) = (rec.clone(), track.clone());
                row = row.child(
                    div()
                        .id(SharedString::from(format!("rec-get|{}|{}|{glyph}", rec.id, track.user_id)))
                        .relative()
                        .size(px(32.0))
                        .flex_none()
                        .rounded(radius_xl())
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(p.muted_foreground)
                        .when(getting, |el| el.opacity(0.6))
                        .when(!getting, |el| {
                            el.cursor_pointer().hover(move |s| s.bg(bg).text_color(fg)).on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.download_recording_files(rec_one.clone(), vec![(track_one.clone(), part)], cx)
                                },
                            ))
                        })
                        .tooltip(move |window, cx| crate::ui::overlay::Tip::new(t(their)).build(window, cx))
                        .child(if getting {
                            motion::ambient(
                                icon("loader-circle").size(px(16.0)),
                                SharedString::from(format!("rec-spin|{}|{}|{glyph}", rec.id, track.user_id)),
                                Duration::from_secs(1),
                                window,
                                |el, t| el.rotate(gpui_kit::radians(t * std::f32::consts::TAU)),
                            )
                        } else {
                            icon(glyph).size(px(16.0)).into_any_element()
                        }),
                );
            }
            tracks =
                tracks.child(motion::slide_in(row, SharedString::from(format!("rec-track-in|{}|{i}", rec.id)), -8.0));
        }
        if rec.tracks.is_empty() {
            tracks = tracks.child(
                div().px(px(8.0)).py(px(6.0)).text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(t(
                    if live { "dms-calls.calls.recordings.silentLive" } else { "dms-calls.calls.recordings.silent" },
                )),
            );
        }
        let card = div()
            .overflow_hidden()
            .rounded(radius_3xl())
            .border_1()
            .border_color(if live { alpha(red(), 0.4) } else { p.border.into() })
            .bg(alpha(p.background, 0.6))
            .when(live, |el| {
                el.shadow(vec![
                    gpui_kit::BoxShadow {
                        color: gpui_kit::hsla(359.0 / 360.0, 0.83, 0.59, 0.15),
                        offset: gpui_kit::point(px(0.0), px(0.0)),
                        blur_radius: px(0.0),
                        spread_radius: px(1.0),
                        inset: false,
                    },
                    gpui_kit::BoxShadow {
                        color: gpui_kit::hsla(359.0 / 360.0, 0.83, 0.59, 0.5),
                        offset: gpui_kit::point(px(0.0), px(12.0)),
                        blur_radius: px(40.0),
                        spread_radius: px(-16.0),
                        inset: false,
                    },
                ])
            })
            .child(head)
            .child(tracks);
        motion::rise(
            card,
            SharedString::from(format!("rec-card|{}", rec.id)),
            Duration::from_millis(50 * n.min(6) as u64),
            14.0,
        )
        .into_any_element()
    }
}

/// How much the server's recordings take, against its cap, and how long they're kept.
fn usage_strip((used, cap, keep): (i64, Option<i64>, Option<i64>), p: &Palette) -> Option<AnyElement> {
    if cap.is_none() && keep.is_none() {
        return None;
    }
    let mut strip = div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .rounded(radius_2xl())
        .border_1()
        .border_color(p.border)
        .bg(alpha(p.muted, 0.4))
        .px(px(14.0))
        .py(px(12.0));
    if let Some(cap) = cap {
        let share = if cap > 0 { (used as f32 / cap as f32).min(1.0) } else { 1.0 };
        let full = used >= cap;
        let bar: gpui_kit::Hsla = if full {
            p.destructive.into()
        } else if share > 0.8 {
            gpui_kit::rgb(0xf59e0b).into()
        } else {
            p.primary.into()
        };
        strip = strip
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .gap(px(12.0))
                    .text_sm()
                    .line_height(px(20.0))
                    .child(div().font_weight(FontWeight::BOLD).when(full, |el| el.text_color(p.destructive)).child(t(
                        if full { "dms-calls.calls.recordings.full" } else { "dms-calls.calls.recordings.usage" },
                    )))
                    .child(div().flex_none().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(
                        t_with(
                            "dms-calls.calls.recordings.usedOf",
                            &[("used", Arg::Str(&format_bytes(used))), ("cap", Arg::Str(&format_bytes(cap)))],
                        ),
                    )),
            )
            .child(
                div()
                    .h(px(8.0))
                    .overflow_hidden()
                    .rounded_full()
                    .bg(p.muted)
                    .child(div().h_full().w(relative(share)).rounded_full().bg(bar)),
            );
    }
    if let Some(days) = keep {
        strip = strip.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_xs()
                .line_height(px(16.0))
                .text_color(p.muted_foreground)
                .child(icon("hourglass").size(px(14.0)))
                .child(t_with("dms-calls.calls.recordings.keep", &[("count", Arg::Num(days))])),
        );
    }
    Some(motion::rise(strip, "recordings-usage", Duration::ZERO, -6.0).into_any_element())
}
