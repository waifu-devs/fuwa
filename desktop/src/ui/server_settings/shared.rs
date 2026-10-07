//! Shared channels from a manager's side: adding another server's channel
//! with a code (preview first), requests waiting, channels shared either way
//! and what the other server's people may do, codes that still work, and
//! people kept out. `channel_share` is the same for one channel, in its
//! settings (the Share tab). The web's `settings/server/SharedChannels.tsx`.

use std::collections::HashSet;

use gpui_kit::Div;
use gpui_kit::component::input::{Input, InputEvent, InputState};

use super::roles::permission_name;
use super::*;
use crate::core::shared::{CodeLeft, SHAREABLE, code_left, find_share_code, share_code_instance, waiting};
use crate::ui::shared_marks::{glyph, server_picture, server_tag};

/// What a confirm strip ends.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Ask {
    /// Letting a waiting server in: a misclick hands it the whole channel.
    Approve,
    Disconnect,
    TurnDown,
    Cancel,
}

pub(super) struct Shared {
    /// The share code pasted, and the name the channel takes here.
    code: Entity<InputState>,
    name: Entity<InputState>,
    /// Where the code leads, for the code it was asked for.
    preview: Option<(String, pb::PreviewShareResponse)>,
    /// The category it goes in ("" for none).
    parent: String,
    looking: bool,
    asking: bool,
    look_error: Option<String>,
    ask_error: Option<String>,
    /// Reading the list failed.
    error: Option<String>,
    /// The server the list was asked for, so it's asked once.
    asked_for: Option<String>,
    confirm: Option<(String, Ask)>,
    confirm_error: Option<String>,
    /// Connections, codes and people with a change on its way.
    busy: HashSet<String>,
    /// A code just made in a channel's Share tab, to copy.
    fresh: Option<pb::ShareCode>,
    making: bool,
    make_error: Option<String>,
    /// The next code is for a server on another instance.
    elsewhere: bool,
    /// Fill the name box from the preview, or empty the code box, on the next draw (which has the window).
    name_for_preview: bool,
    clear_code: bool,
}

impl Shared {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ServerSettingsView>) -> (Self, Vec<Subscription>) {
        let code = cx.new(|cx| InputState::new(window, cx).placeholder(t("serversettings.sharedChannels.paste")));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(t("desktop.server.channels.namePlaceholder")));
        let subscriptions = vec![
            cx.subscribe_in(&code, window, |this: &mut ServerSettingsView, input, e: &InputEvent, window, cx| {
                match e {
                    InputEvent::PressEnter { .. } => this.look_up_share(cx),
                    InputEvent::Change => {
                        // Pasted with words around it: keep just the code.
                        let text = input.read(cx).value().to_string();
                        let found = find_share_code(&text).to_owned();
                        if !found.is_empty() && found != text {
                            input.update(cx, |s, cx| s.set_value(found.clone(), window, cx));
                        }
                        let s = &mut this.shared;
                        s.look_error = None;
                        s.ask_error = None;
                        if s.preview.as_ref().is_some_and(|(code, _)| *code != found) {
                            s.preview = None;
                        }
                        cx.notify();
                    }
                    _ => {}
                }
            }),
            cx.subscribe(&name, |_: &mut ServerSettingsView, _, e: &InputEvent, cx| {
                if let InputEvent::Change = e {
                    cx.notify()
                }
            }),
        ];
        let shared = Self {
            code,
            name,
            preview: None,
            parent: String::new(),
            looking: false,
            asking: false,
            look_error: None,
            ask_error: None,
            error: None,
            asked_for: None,
            confirm: None,
            confirm_error: None,
            busy: HashSet::new(),
            fresh: None,
            making: false,
            make_error: None,
            elsewhere: false,
            name_for_preview: false,
            clear_code: false,
        };
        (shared, subscriptions)
    }
}

/// An instance's message with its first letter up, as the web shows them.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn ms(at: Option<&prost_types::Timestamp>) -> i64 {
    at.map_or(0, |at| at.seconds * 1000 + i64::from(at.nanos) / 1_000_000)
}

/// How long a share code still works, as the web's `codeLeft` says it.
fn time_left(left: i64) -> String {
    match code_left(left) {
        CodeLeft::Expired => t("serversettings.sharedChannels.expired"),
        CodeLeft::Days(n) => t_with("desktop.server.shared.days", &[("count", Arg::Num(n))]),
        CodeLeft::Hours(n) => t_with("desktop.server.shared.hours", &[("count", Arg::Num(n))]),
        CodeLeft::Minutes(n) => t_with("desktop.server.shared.minutes", &[("count", Arg::Num(n))]),
        CodeLeft::FewMinutes => t("serversettings.sharedChannels.fewMinutes"),
    }
}

/// What a connection is, in a line, from this server's side.
fn connection_line(c: &pb::SharedConnection, home_name: &str, here: Option<&str>) -> String {
    let channel = format!("#{home_name}");
    match (c.home, waiting(c)) {
        (true, true) => t_with("serversettings.sharedChannels.wantsToShow", &[("channel", Arg::Str(&channel))]),
        (true, false) => t_with("serversettings.sharedChannels.sees", &[("channel", Arg::Str(&channel))]),
        (false, true) => t_with("serversettings.sharedChannels.waitingForApproval", &[("channel", Arg::Str(&channel))]),
        (false, false) => t_with(
            "serversettings.sharedChannels.hereAs",
            &[("channel", Arg::Str(&channel)), ("here", Arg::Str(&format!("#{}", here.unwrap_or(home_name))))],
        ),
    }
}

/// What the confirm strip says before ending a connection or a request: title, body, button, toast.
fn ask_copy(
    ask: Ask,
    c: &pb::SharedConnection,
    other: &str,
    home_name: &str,
    here: Option<&str>,
) -> (String, String, String, String) {
    let approved = || {
        t_with(
            "serversettings.sharedChannels.approved",
            &[("channel", Arg::Str(home_name)), ("server", Arg::Str(other))],
        )
    };
    let disconnected = || t_with("serversettings.sharedChannels.disconnected", &[("server", Arg::Str(other))]);
    match ask {
        Ask::Approve if !c.instance.is_empty() => (
            t_with(
                "desktop.server.shared.approveAskOn",
                &[("server", Arg::Str(other)), ("instance", Arg::Str(&c.instance)), ("channel", Arg::Str(home_name))],
            ),
            t_with("desktop.server.shared.approveHintOn", &[("channel", Arg::Str(home_name))]),
            t("serversettings.sharedChannels.approve"),
            approved(),
        ),
        Ask::Approve => (
            t_with(
                "desktop.server.shared.approveAsk",
                &[("server", Arg::Str(other)), ("channel", Arg::Str(home_name))],
            ),
            t_with("desktop.server.shared.approveHint", &[("channel", Arg::Str(home_name))]),
            t("serversettings.sharedChannels.approve"),
            approved(),
        ),
        Ask::TurnDown => (
            t_with("serversettings.sharedChannels.turnDownAsk", &[("server", Arg::Str(other))]),
            t_with("serversettings.sharedChannels.turnDownHint", &[("channel", Arg::Str(home_name))]),
            t("serversettings.applications.turnDown"),
            t_with("serversettings.applications.turnedDownDone", &[("name", Arg::Str(other))]),
        ),
        Ask::Cancel => (
            t("serversettings.sharedChannels.cancelAsk"),
            t_with("serversettings.sharedChannels.cancelHint", &[("server", Arg::Str(other))]),
            t("serversettings.sharedChannels.cancelRequest"),
            t("serversettings.sharedChannels.canceled"),
        ),
        Ask::Disconnect if c.home => (
            t_with(
                "serversettings.sharedChannels.stopAsk",
                &[("channel", Arg::Str(home_name)), ("server", Arg::Str(other))],
            ),
            t_with("serversettings.sharedChannels.stopHint", &[("server", Arg::Str(other))]),
            t("serversettings.sharedChannels.disconnect"),
            disconnected(),
        ),
        Ask::Disconnect => (
            t_with("serversettings.sharedChannels.removeAsk", &[("channel", Arg::Str(here.unwrap_or(home_name)))]),
            t_with("serversettings.sharedChannels.removeHint", &[("server", Arg::Str(other))]),
            t("serversettings.sharedChannels.disconnect"),
            disconnected(),
        ),
    }
}

/// What a connection with a server on another instance says beside that instance's key.
fn instance_note(c: &pb::SharedConnection) -> String {
    match (c.home, waiting(c)) {
        (true, true) => t("serversettings.sharedChannels.checkBeforeApproving"),
        (false, true) => t("serversettings.sharedChannels.theirFingerprint"),
        _ => t("desktop.server.shared.notAcross"),
    }
}

/// A section's heading, with how many are in it.
fn heading(title: &str, count: usize, p: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::EXTRA_BOLD)
        .text_color(p.muted_foreground)
        .child(title.to_uppercase())
        .when(count > 0, |el| {
            el.child(div().px(px(6.0)).rounded_full().bg(p.muted).text_size(px(10.0)).child(motion::count_up(
                SharedString::from(format!("shared-count-{title}")),
                count as f64,
                Duration::ZERO,
                |v| format!("{}", v.round()),
            )))
        })
}

/// Read, or one of what the home lets a guest's people do, on or struck through.
fn capability(on: bool, label: &str, glyph_name: &str, n: usize, id: &str, p: &Palette) -> impl IntoElement {
    let (fg, bg, border) = if on {
        (Hsla::from(p.primary), alpha(p.primary, 0.1), alpha(p.primary, 0.3))
    } else {
        (Hsla::from(p.muted_foreground), gpui_kit::transparent_black(), Hsla::from(p.border))
    };
    motion::rise(
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .h(px(26.0))
            .px(px(10.0))
            .rounded_full()
            .border_1()
            .border_color(border)
            .bg(bg)
            .text_xs()
            .line_height(px(16.0))
            .font_weight(FontWeight::BOLD)
            .text_color(fg)
            .child(icon(if on { glyph_name } else { "x" }).size(px(14.0)))
            .child(div().when(!on, |el| el.line_through()).child(label.to_owned())),
        SharedString::from(format!("cap-{id}-{n}")),
        Duration::from_millis(200 + n as u64 * 50),
        4.0,
    )
}

/// Where the other end is when it's on another instance: its host, and its key's fingerprint.
fn instance_line(instance: &str, fingerprint: &str, note: &str, p: &Palette) -> Div {
    div()
        .flex()
        .items_start()
        .gap(px(8.0))
        .text_sm()
        .line_height(px(20.0))
        .child(icon("globe").size(px(16.0)).mt(px(2.0)).text_color(p.primary))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(4.0))
                        .child(marked(
                            &t_with(
                                "serversettings.sharedChannels.otherInstance",
                                &[("instance", Arg::Str(&strong(instance)))],
                            ),
                            p,
                        ))
                        .child(div().text_color(p.muted_foreground).child(note.to_owned())),
                )
                .child(div().font_family("monospace").text_xs().line_height(px(16.0)).child(fingerprint.to_owned())),
        )
}

impl ServerSettingsView {
    /// Reads the server's shared channels once for this view; the event stream keeps them current.
    pub(super) fn load_shared(&mut self, cx: &mut Context<Self>) {
        if self.shared.asked_for.as_deref() == Some(self.server.as_str()) {
            return;
        }
        self.shared.asked_for = Some(self.server.clone());
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.list_connections(&key, &sid).await }, |this, result, cx| {
            this.shared.error = result.err().map(|e| capitalized(&e.message));
            cx.notify();
        });
    }

    fn toast(&self, glyph_name: &'static str, title: String, cx: &mut Context<Self>) {
        cx.emit(ServerSettingsEvent::Toast { icon: glyph_name, title });
    }

    fn shared_list(&self) -> Option<pb::ListConnectionsResponse> {
        self.core.shared.read(|s| s.instance(&self.key).and_then(|i| i.shared.get(&self.server).cloned()))
    }

    /// Whether sharing is on for the instance, and with other instances too.
    fn sharing(&self) -> (bool, bool, String) {
        self.core.shared.read(|s| {
            s.instance(&self.key).map_or((false, false, String::new()), |i| {
                let node = i.node.as_ref();
                (node.is_some_and(|n| n.shared_channels), node.is_some_and(|n| n.federation), i.url.clone())
            })
        })
    }

    fn look_up_share(&mut self, cx: &mut Context<Self>) {
        let text = self.shared.code.read(cx).value().to_string();
        let code = find_share_code(&text).to_owned();
        if code.is_empty() {
            self.shared.look_error = Some(t("desktop.server.shared.notCode"));
            cx.notify();
            return;
        }
        if self.shared.looking {
            return;
        }
        self.shared.looking = true;
        self.shared.look_error = None;
        let (core, key, sid, asked) = (self.core.clone(), self.key.clone(), self.server.clone(), code.clone());
        self.run(cx, async move { core.preview_share(&key, &sid, &asked).await }, move |this, result, cx| {
            this.shared.looking = false;
            match result {
                Ok(found) => {
                    this.shared.preview = Some((code, found));
                    this.shared.name_for_preview = true;
                }
                Err(err) => this.shared.look_error = Some(capitalized(&err.message)),
            }
            cx.notify();
        });
        cx.notify();
    }

    fn ask_to_connect(&mut self, cx: &mut Context<Self>) {
        let Some((code, preview)) = self.shared.preview.clone() else { return };
        if self.shared.asking {
            return;
        }
        let chosen = super::channels::slug(&self.shared.name.read(cx).value());
        if chosen.is_empty() {
            return;
        }
        self.shared.asking = true;
        self.shared.ask_error = None;
        let name = if chosen == preview.channel_name { String::new() } else { chosen.clone() };
        let parent = self.shared.parent.clone();
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let home = preview
            .home_server
            .as_ref()
            .map_or_else(|| t("serversettings.sharedChannels.theOtherServer"), |s| s.name.clone());
        self.run(
            cx,
            async move { core.accept_share(&key, &sid, &code, &name, &parent).await },
            move |this, result, cx| {
                this.shared.asking = false;
                match result {
                    Ok(()) => {
                        this.toast(
                            "send",
                            t_with(
                                "serversettings.sharedChannels.asked",
                                &[("server", Arg::Str(&home)), ("channel", Arg::Str(&chosen))],
                            ),
                            cx,
                        );
                        this.shared.preview = None;
                        this.shared.parent.clear();
                        this.shared.clear_code = true;
                    }
                    Err(err) => this.shared.ask_error = Some(capitalized(&err.message)),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// Runs a change to one connection, code or person, marking it busy until it's done.
    fn shared_change(
        &mut self,
        id: String,
        cx: &mut Context<Self>,
        work: impl Future<Output = Result<(), Problem>> + Send + 'static,
        done: Option<(&'static str, String)>,
    ) {
        if !self.shared.busy.insert(id.clone()) {
            return;
        }
        self.run(cx, work, move |this, result, cx| {
            this.shared.busy.remove(&id);
            match result {
                Ok(()) => {
                    if let Some((glyph_name, title)) = done {
                        this.toast(glyph_name, title, cx);
                    }
                }
                Err(err) => this.toast("circle-alert", capitalized(&err.message), cx),
            }
            cx.notify();
        });
        cx.notify();
    }

    fn answer_share(&mut self, c: &pb::SharedConnection, ask: Ask, done: String, cx: &mut Context<Self>) {
        let (core, key, sid, cid) = (self.core.clone(), self.key.clone(), self.server.clone(), c.id.clone());
        let id = c.id.clone();
        self.shared.confirm_error = None;
        self.shared.busy.insert(id.clone());
        self.run(
            cx,
            async move {
                match ask {
                    Ask::Approve => core.review_share(&key, &sid, &cid, true).await,
                    Ask::TurnDown => core.review_share(&key, &sid, &cid, false).await,
                    _ => core.disconnect_shared(&key, &sid, &cid).await,
                }
            },
            move |this, result, cx| {
                this.shared.busy.remove(&id);
                match result {
                    Ok(()) => {
                        this.shared.confirm = None;
                        let glyph = match ask {
                            Ask::Approve => "check",
                            Ask::TurnDown => "x",
                            _ => "unplug",
                        };
                        this.toast(glyph, done, cx);
                    }
                    Err(err) => this.shared.confirm_error = Some(capitalized(&err.message)),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn toggle_allowed(&mut self, c: &pb::SharedConnection, permission: P, on: bool, cx: &mut Context<Self>) {
        let mut next: Vec<P> = c.allowed().filter(|p| *p != permission).collect();
        if on {
            next.push(permission);
        }
        let (core, key, sid, cid) = (self.core.clone(), self.key.clone(), self.server.clone(), c.id.clone());
        let work = async move { core.update_connection(&key, &sid, &cid, next).await };
        self.shared_change(format!("{}/{}", c.id, permission as i32), cx, work, None);
    }

    fn delete_code(&mut self, code: &pb::ShareCode, cx: &mut Context<Self>) {
        let (core, key, sid, value) = (self.core.clone(), self.key.clone(), self.server.clone(), code.code.clone());
        let work = async move { core.delete_share_code(&key, &sid, &value).await };
        self.shared_change(code.code.clone(), cx, work, None);
    }

    fn let_back_in(&mut self, block: &pb::ChannelBlock, name: String, channel: String, cx: &mut Context<Self>) {
        let user_id = block.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
        let (core, key, sid, cid) =
            (self.core.clone(), self.key.clone(), self.server.clone(), block.channel_id.clone());
        let id = format!("{}/{user_id}", block.channel_id);
        let work = async move { core.block_from_channel(&key, &sid, &cid, &user_id, false).await };
        self.shared_change(
            id,
            cx,
            work,
            Some((
                "undo",
                t_with(
                    "serversettings.sharedChannels.letBackInDone",
                    &[("name", Arg::Str(&name)), ("channel", Arg::Str(&channel))],
                ),
            )),
        );
    }

    fn make_code(&mut self, channel_id: String, elsewhere: bool, cx: &mut Context<Self>) {
        if self.shared.making {
            return;
        }
        self.shared.making = true;
        self.shared.make_error = None;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(
            cx,
            async move { core.create_share_code(&key, &sid, &channel_id, elsewhere).await },
            |this, result, cx| {
                this.shared.making = false;
                match result {
                    Ok(code) => this.shared.fresh = Some(code),
                    Err(err) => this.shared.make_error = Some(capitalized(&err.message)),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn copy_code(&mut self, code: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(code.clone()));
        self.copied = Some((code, Instant::now()));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        cx.notify();
    }

    // ───────────────────────── The page ─────────────────────────

    pub(super) fn shared_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.load_shared(cx);
        let (on, _, url) = self.sharing();
        let list = self.shared_list();
        let (channels, can_kick) = self.core.shared.read(|s| {
            s.instance(&self.key).map_or((Vec::new(), false), |i| {
                (i.channels.get(&self.server).cloned().unwrap_or_default(), i.access(&self.server).has(P::KickMembers))
            })
        });
        let name_of = |id: &str| channels.iter().find(|c| c.id == id).map(|c| c.name.clone());

        let mut page = div().flex().flex_col().gap(px(28.0));
        page = page.child(if on {
            self.add_channel(&channels, &url, p, window, cx).into_any_element()
        } else {
            div()
                .p(px(16.0))
                .rounded(corner(16.0))
                .border_1()
                .border_dashed()
                .border_color(p.border)
                .text_sm()
                .line_height(px(20.0))
                .text_color(p.muted_foreground)
                .child(t("serversettings.sharedChannels.off"))
                .into_any_element()
        });
        if let Some(error) = &self.shared.error {
            return page
                .child(div().text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(error.clone()))
                .into_any_element();
        }
        let Some(list) = list else { return page.child(shimmer_rows(3, p)).into_any_element() };

        let now = now_ms();
        let requests: Vec<&pb::SharedConnection> = list.connections.iter().filter(|c| waiting(c)).collect();
        let active: Vec<&pb::SharedConnection> = list.connections.iter().filter(|c| !waiting(c)).collect();
        let codes: Vec<&pb::ShareCode> = list.codes.iter().filter(|c| ms(c.expires_at.as_ref()) > now).collect();

        if !requests.is_empty() {
            let mut section = div().flex().flex_col().gap(px(8.0)).child(heading(
                &t("serversettings.sharedChannels.waiting"),
                requests.len(),
                p,
            ));
            for (n, c) in requests.iter().enumerate() {
                section = section.child(self.connection_row(c, name_of(&c.channel_id).as_deref(), n, &url, p, cx));
            }
            page = page.child(section);
        }
        let mut section =
            div().flex().flex_col().gap(px(8.0)).child(heading(&t("serversettings.nav.shared"), active.len(), p));
        if active.is_empty() {
            section = section.child(nothing_shared(p));
        }
        for (n, c) in active.iter().enumerate() {
            section = section.child(self.connection_row(c, name_of(&c.channel_id).as_deref(), n, &url, p, cx));
        }
        page = page.child(section);
        if !codes.is_empty() {
            let mut section = div().flex().flex_col().gap(px(8.0)).child(heading(
                &t("serversettings.sharedChannels.codes"),
                codes.len(),
                p,
            ));
            for (n, code) in codes.iter().enumerate() {
                section = section.child(self.code_row(code, now, n, p, window, cx));
            }
            page = page.child(section);
        }
        if !list.blocks.is_empty() {
            let mut section = div().flex().flex_col().gap(px(8.0)).child(heading(
                &t("serversettings.sharedChannels.keptOut"),
                list.blocks.len(),
                p,
            ));
            for (n, b) in list.blocks.iter().enumerate() {
                section = section.child(self.block_row(b, name_of(&b.channel_id), can_kick, n, &url, p, window, cx));
            }
            page = page.child(section);
        }
        page.into_any_element()
    }

    /// Paste a code, see where it leads, pick a name and a place, and ask.
    fn add_channel(
        &mut self,
        channels: &[pb::Channel],
        url: &str,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        if std::mem::take(&mut self.shared.clear_code) {
            self.shared.code.update(cx, |s, cx| s.set_value("", window, cx));
        }
        if std::mem::take(&mut self.shared.name_for_preview)
            && let Some((_, preview)) = &self.shared.preview
        {
            let name = preview.channel_name.clone();
            self.shared.name.update(cx, |s, cx| s.set_value(name, window, cx));
        }
        let typed = !self.shared.code.read(cx).value().trim().is_empty();
        let looking = self.shared.looking;
        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(18.0))
            .rounded(corner(22.0))
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.background, 0.4))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex_none()
                            .size(px(40.0))
                            .rounded(corner(14.0))
                            .bg(alpha(p.primary, 0.15))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(glyph(20.0, p.primary)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div().font_weight(FontWeight::EXTRA_BOLD).child(t("serversettings.sharedChannels.add")),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .line_height(px(20.0))
                                    .text_color(p.muted_foreground)
                                    .child(t("serversettings.sharedChannels.addHint")),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .font_family("monospace")
                            .child(
                                super::pages::boxed(
                                    Input::new(&self.shared.code).appearance(false),
                                    44.0,
                                    super::pages::focused(&self.shared.code, window, cx),
                                    p,
                                )
                                .pl(px(28.0)),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(px(12.0))
                                    .top(px(14.0))
                                    .text_color(p.muted_foreground)
                                    .child(icon("key-round").size(px(16.0))),
                            ),
                    )
                    .child(
                        primary_button("share-preview", t("settings.controls.preview"), p)
                            .h(px(44.0))
                            .when(looking || !typed, |el| el.opacity(0.6))
                            .child(if looking {
                                spinner("share-preview-spin", 15.0, window)
                            } else {
                                icon("scan-eye").size(px(15.0)).into_any_element()
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.look_up_share(cx))),
                    ),
            );
        if let Some(error) = &self.shared.look_error {
            card = card.child(motion::rise(
                div().text_sm().line_height(px(20.0)).text_color(p.destructive).child(error.clone()),
                SharedString::from(format!("share-look-error-{error}")),
                Duration::ZERO,
                4.0,
            ));
        }
        let Some((code, preview)) = self.shared.preview.clone() else { return card };

        let categories: Vec<&pb::Channel> =
            channels.iter().filter(|c| c.r#type == pb::ChannelType::Category as i32).collect();
        let mut picks = div().flex().flex_wrap().gap(px(6.0)).child(
            chip(
                "share-parent-none".into(),
                &t("serversettings.channels.noCategory"),
                self.shared.parent.is_empty(),
                p,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.shared.parent.clear();
                cx.notify();
            })),
        );
        for c in categories {
            let id = c.id.clone();
            picks = picks.child(
                chip(SharedString::from(format!("share-parent-{id}")), &c.name, self.shared.parent == c.id, p)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.shared.parent = id.clone();
                        cx.notify();
                    })),
            );
        }
        let chosen = super::channels::slug(&self.shared.name.read(cx).value());
        let asking = self.shared.asking;
        let form = div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(preview_card(&preview, url, &code, p))
            .child(labeled(
                &t("serversettings.sharedChannels.nameHere"),
                Input::new(&self.shared.name).prefix(icon("hash").size(px(16.0)).text_color(p.muted_foreground)),
                p,
            ))
            .child(labeled(&t("serversettings.channels.category"), picks, p))
            .when_some(self.shared.ask_error.clone(), |el, e| {
                el.child(div().text_sm().line_height(px(20.0)).text_color(p.destructive).child(e))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .child(t("serversettings.sharedChannels.approveNote")),
                    )
                    .child(soft_button("share-not-now", t("serversettings.sharedChannels.notNow"), p).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.shared.preview = None;
                            cx.notify();
                        }),
                    ))
                    .child(
                        primary_button("share-ask", t("serversettings.sharedChannels.askToConnect"), p)
                            .when(asking || chosen.is_empty(), |el| el.opacity(0.6))
                            .child(if asking {
                                spinner("share-ask-spin", 15.0, window)
                            } else {
                                icon("send").size(px(15.0)).into_any_element()
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.ask_to_connect(cx))),
                    ),
            );
        card.child(motion::rise(form, SharedString::from(format!("share-preview-{code}")), Duration::ZERO, 24.0))
    }

    #[allow(clippy::too_many_arguments)]
    fn connection_row(
        &self,
        c: &pb::SharedConnection,
        here: Option<&str>,
        n: usize,
        url: &str,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let other =
            c.server.as_ref().map_or_else(|| t("serversettings.sharedChannels.anotherServer"), |s| s.name.clone());
        let home_name = if !c.home_channel_name.is_empty() {
            c.home_channel_name.clone()
        } else {
            here.map_or_else(|| t("serversettings.sharedChannels.aChannel"), str::to_owned)
        };
        let is_waiting = waiting(c);
        let busy = self.shared.busy.contains(&c.id);
        let id = c.id.clone();

        let picture = div()
            .relative()
            .flex_none()
            .when_some(c.server.as_ref(), |el, s| el.child(server_picture(s, url, 40.0, 12.0, p)))
            .child(
                div()
                    .absolute()
                    .right(px(-4.0))
                    .bottom(px(-4.0))
                    .size(px(20.0))
                    .rounded_full()
                    .bg(p.background)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(glyph(12.0, p.primary)),
            );
        let state = {
            let (text, fg, bg) = if is_waiting {
                (t("serversettings.sharedChannels.waiting"), Hsla::from(p.muted_foreground), Hsla::from(p.muted))
            } else {
                (t("serversettings.sharedChannels.connected"), Hsla::from(p.primary), alpha(p.primary, 0.12))
            };
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.0))
                .h(px(22.0))
                .px(px(8.0))
                .rounded_full()
                .bg(bg)
                .text_color(fg)
                .text_xs()
                .line_height(px(16.0))
                .font_weight(FontWeight::BOLD)
                .child(if is_waiting {
                    div().size(px(8.0)).rounded_full().bg(p.primary).into_any_element()
                } else {
                    motion::rise(
                        icon("check").size(px(12.0)),
                        SharedString::from(format!("conn-check-{id}")),
                        Duration::ZERO,
                        4.0,
                    )
                    .into_any_element()
                })
                .child(text)
        };
        let actions = if c.home && is_waiting {
            let (id2, id3) = (id.clone(), id.clone());
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    soft_button(
                        SharedString::from(format!("conn-down-{id}")),
                        t("serversettings.applications.turnDown"),
                        p,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.shared.confirm = Some((id2.clone(), Ask::TurnDown));
                        this.shared.confirm_error = None;
                        cx.notify();
                    })),
                )
                .child(
                    primary_button(
                        SharedString::from(format!("conn-approve-{id}")),
                        t("serversettings.sharedChannels.approve"),
                        p,
                    )
                    .when(busy, |el| el.opacity(0.6))
                    .child(icon("check").size(px(15.0)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.shared.confirm = Some((id3.clone(), Ask::Approve));
                        this.shared.confirm_error = None;
                        cx.notify();
                    })),
                )
                .into_any_element()
        } else {
            let ask = if is_waiting { Ask::Cancel } else { Ask::Disconnect };
            let id2 = id.clone();
            soft_button(
                SharedString::from(format!("conn-end-{id}")),
                if is_waiting { t("common.cancel") } else { t("serversettings.sharedChannels.disconnect") },
                p,
            )
            .text_color(p.destructive)
            .child(icon(if is_waiting { "x" } else { "unplug" }).size(px(15.0)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.shared.confirm = Some((id2.clone(), ask));
                this.shared.confirm_error = None;
                cx.notify();
            }))
            .into_any_element()
        };
        let head = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .child(picture)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_weight(FontWeight::BOLD).truncate().child(if c.home {
                        other.clone()
                    } else {
                        t_with("serversettings.sharedChannels.from", &[("server", Arg::Str(&other))])
                    }))
                    .child(
                        div()
                            .text_sm()
                            .line_height(px(20.0))
                            .text_color(p.muted_foreground)
                            .child(connection_line(c, &home_name, here)),
                    ),
            )
            .child(state)
            .child(actions);

        let mut card = div()
            .id(SharedString::from(format!("conn-{id}")))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(12.0))
            .rounded(corner(16.0))
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.background, 0.4))
            .hover({
                let b = alpha(p.primary, 0.3);
                move |s| s.border_color(b)
            })
            .child(head);
        if !c.instance.is_empty() {
            card = card.child(instance_line(&c.instance, &c.fingerprint, &instance_note(c), p));
        }
        if !c.checked_by.is_empty() {
            let providers = c.checked_by.join(", ");
            let note = if c.home {
                t_with("serversettings.sharedChannels.yourAutoMod", &[("providers", Arg::Str(&providers))])
            } else {
                t_with("serversettings.sharedChannels.theirAutoMod", &[("providers", Arg::Str(&providers))])
            };
            card = card.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.0))
                    .text_sm()
                    .line_height(px(20.0))
                    .text_color(p.muted_foreground)
                    .child(icon("eye").size(px(16.0)).mt(px(2.0)).text_color(p.primary))
                    .child(div().flex_1().min_w_0().child(note)),
            );
        }
        if !is_waiting {
            card = card.child(self.allowed(c, p, cx));
        }
        if let Some((_, ask)) = self.shared.confirm.as_ref().filter(|(cid, _)| *cid == c.id) {
            let ask = *ask;
            let (title, body, action, done) = ask_copy(ask, c, &other, &home_name, here);
            let c2 = c.clone();
            card = card.child(motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .p(px(12.0))
                    .rounded(corner(12.0))
                    .bg(alpha(if ask == Ask::Approve { p.primary } else { p.destructive }, 0.08))
                    .child(div().font_weight(FontWeight::EXTRA_BOLD).child(title))
                    .child(div().text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(body))
                    .when_some(self.shared.confirm_error.clone(), |el, e| {
                        el.child(div().text_sm().line_height(px(20.0)).text_color(p.destructive).child(e))
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.0))
                            .child(
                                soft_button(
                                    SharedString::from(format!("conn-keep-{id}")),
                                    if ask == Ask::Approve {
                                        t("desktop.server.shared.notYet")
                                    } else {
                                        t("serversettings.shared.keepIt")
                                    },
                                    p,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.shared.confirm = None;
                                    cx.notify();
                                })),
                            )
                            .child(
                                {
                                    let yes = SharedString::from(format!("conn-yes-{id}"));
                                    if ask == Ask::Approve {
                                        primary_button(yes, action, p)
                                    } else {
                                        danger_button(yes, action, p)
                                    }
                                }
                                .when(busy, |el| el.opacity(0.6))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !this.shared.busy.contains(&c2.id) {
                                        this.answer_share(&c2, ask, done.clone(), cx)
                                    }
                                })),
                            ),
                    ),
                SharedString::from(format!("conn-ask-{id}")),
                Duration::ZERO,
                6.0,
            ));
        }
        motion::rise(
            card,
            SharedString::from(format!("conn-in-{id}")),
            Duration::from_millis((n.min(10) * 30) as u64),
            10.0,
        )
        .into_any_element()
    }

    /// What the guest server's people may do: switches at the home, what they were given at the guest.
    fn allowed(&self, c: &pb::SharedConnection, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let on = |perm: P| c.allowed().any(|a| a == perm);
        if !c.home {
            let mut list = div().flex().flex_wrap().gap(px(6.0)).pl(px(52.0));
            for (n, (perm, _, glyph_name)) in SHAREABLE.iter().enumerate() {
                list = list.child(capability(on(*perm), &permission_name(*perm), glyph_name, n, &c.id, p));
            }
            return list.into_any_element();
        }
        // Three across where there's room, wrapping in narrower places (a channel's Share tab).
        let mut grid = div().flex().flex_wrap().gap(px(6.0)).p(px(6.0)).rounded(corner(12.0)).bg(alpha(p.muted, 0.4));
        for (perm, _, glyph_name) in SHAREABLE {
            let lit = on(perm);
            let saving = self.shared.busy.contains(&format!("{}/{}", c.id, perm as i32));
            let any_saving = self.shared.busy.iter().any(|b| b.starts_with(&format!("{}/", c.id)));
            let c2 = c.clone();
            grid = grid.child(
                div()
                    .flex_1()
                    .min_w(px(176.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(corner(9.0))
                    .child(
                        div()
                            .flex_none()
                            .size(px(24.0))
                            .rounded(corner(7.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(if lit { alpha(p.primary, 0.15) } else { p.muted.into() })
                            .text_color(if lit { p.primary } else { p.muted_foreground })
                            .child(icon(glyph_name).size(px(14.0))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .line_height(px(20.0))
                            .font_weight(FontWeight::BOLD)
                            .child(permission_name(perm)),
                    )
                    .child(if saving {
                        icon("loader-circle").size(px(16.0)).text_color(p.muted_foreground).into_any_element()
                    } else {
                        switch(
                            SharedString::from(format!("allow-{}-{}", c.id, perm as i32)),
                            lit,
                            any_saving,
                            cx,
                            move |this, v, cx| this.toggle_allowed(&c2, perm, v, cx),
                        )
                        .into_any_element()
                    }),
            );
        }
        grid.into_any_element()
    }

    fn code_row(
        &self,
        code: &pb::ShareCode,
        now: i64,
        n: usize,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let left = ms(code.expires_at.as_ref()) - now;
        let hide = self.core.prefs().streamer_mode;
        let deleting = self.shared.busy.contains(&code.code);
        let copied =
            self.copied.as_ref().is_some_and(|(c, at)| *c == code.code && at.elapsed() < Duration::from_millis(1400));
        let (value, gone) = (code.code.clone(), code.clone());
        let row = div()
            .flex()
            .items_center()
            .gap(px(14.0))
            .p(px(12.0))
            .rounded(corner(16.0))
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.background, 0.4))
            .child(
                div()
                    .flex_none()
                    .size(px(36.0))
                    .rounded(corner(12.0))
                    .bg(alpha(p.primary, 0.12))
                    .text_color(p.primary)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("key-round").size(px(16.0))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .line_height(px(20.0))
                            .font_weight(FontWeight::BOLD)
                            .truncate()
                            .child(format!("#{}", code.channel_name)),
                    )
                    .child(
                        div()
                            .font_family("monospace")
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .truncate()
                            .child(if hide { "••••••••".to_owned() } else { code.code.clone() }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_sm()
                    .line_height(px(20.0))
                    .text_color(if left < 86_400_000 { p.destructive } else { p.muted_foreground })
                    .child(icon("timer").size(px(14.0)))
                    .child(time_left(left)),
            )
            .child(
                icon_button(
                    SharedString::from(format!("code-copy-{}", code.code)),
                    if copied { "check" } else { "copy" },
                    p,
                )
                .when(copied, |el| el.text_color(p.primary))
                .on_click(cx.listener(move |this, _, _, cx| this.copy_code(value.clone(), cx))),
            )
            .child(if deleting {
                spinner(format!("code-del-spin-{}", code.code), 16.0, window)
            } else {
                icon_button_in(SharedString::from(format!("code-del-{}", code.code)), "trash", p, p.destructive)
                    .on_click(cx.listener(move |this, _, _, cx| this.delete_code(&gone, cx)))
                    .into_any_element()
            });
        motion::rise(
            row,
            SharedString::from(format!("code-in-{}", code.code)),
            Duration::from_millis((n.min(10) * 30) as u64),
            10.0,
        )
        .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn block_row(
        &self,
        b: &pb::ChannelBlock,
        channel: Option<String>,
        can_kick: bool,
        n: usize,
        url: &str,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = b.user.as_ref().map_or_else(|| t("common.someone"), user_name);
        let channel = channel.unwrap_or_else(|| t("serversettings.sharedChannels.aChannel"));
        let id = format!("{}/{}", b.channel_id, b.user.as_ref().map(|u| u.id.as_str()).unwrap_or(""));
        let lifting = self.shared.busy.contains(&id);
        let b2 = b.clone();
        let (name2, channel2) = (name.clone(), channel.clone());
        let row = div()
            .id(SharedString::from(format!("block-{id}")))
            .group("block")
            .flex()
            .items_center()
            .gap(px(12.0))
            .p(px(12.0))
            .rounded(corner(16.0))
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.background, 0.4))
            .child(div().opacity(0.55).group_hover("block", |s| s.opacity(1.0)).child(avatar(b.user.as_ref(), 40.0, p)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .font_weight(FontWeight::BOLD)
                            .child(div().min_w_0().truncate().child(name.clone()))
                            .when_some(b.server.as_ref(), |el, s| el.child(server_tag(s, url, p))),
                    )
                    .child(div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).truncate().child(
                        t_with(
                            "serversettings.sharedChannels.keptOutOf",
                            &[("channel", Arg::Str(&channel)), ("date", Arg::Str(&stamp(ms(b.created_at.as_ref()))))],
                        ),
                    )),
            )
            .when(can_kick, |el| {
                el.child(
                    soft_button(
                        SharedString::from(format!("block-lift-{id}")),
                        t("serversettings.sharedChannels.letBackIn"),
                        p,
                    )
                    .when(lifting, |el| el.opacity(0.6))
                    .child(if lifting {
                        spinner(format!("block-lift-spin-{id}"), 14.0, window)
                    } else {
                        icon("undo").size(px(14.0)).into_any_element()
                    })
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.let_back_in(&b2, name2.clone(), channel2.clone(), cx)),
                    ),
                )
            });
        motion::rise(
            row,
            SharedString::from(format!("block-in-{id}")),
            Duration::from_millis((n.min(10) * 30) as u64),
            10.0,
        )
        .into_any_element()
    }

    // ───────────────────────── One channel's Share tab ─────────────────────────

    /// Sharing one channel, in its settings: a code for another server's admins,
    /// the codes still out, and the server it's shared with. A channel shown from
    /// another server says where it comes from instead.
    pub(super) fn channel_share(
        &mut self,
        channel: &pb::Channel,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.load_shared(cx);
        if let Some(error) = &self.shared.error {
            return div()
                .text_sm()
                .line_height(px(20.0))
                .text_color(p.muted_foreground)
                .child(error.clone())
                .into_any_element();
        }
        let (on, federation, url) = self.sharing();
        let list = self.shared_list();
        let guests: Vec<pb::SharedConnection> = list
            .as_ref()
            .map(|l| l.connections.iter().filter(|c| c.channel_id == channel.id).cloned().collect())
            .unwrap_or_default();
        let shown = channel.shared.as_ref().is_some_and(|s| !s.home);
        if shown {
            let home = channel
                .shared
                .as_ref()
                .and_then(|s| s.home_server.as_ref())
                .map_or_else(|| t("serversettings.sharedChannels.anotherServer"), |s| s.name.clone());
            let mut out = div().flex().flex_col().gap(px(14.0)).child(
                div()
                    .text_sm()
                    .line_height(px(20.0))
                    .text_color(p.muted_foreground)
                    .child(t_with("serversettings.sharedChannels.comesFrom", &[("server", Arg::Str(&home))])),
            );
            for (n, c) in guests.iter().enumerate() {
                out = out.child(self.connection_row(c, Some(&channel.name), n, &url, p, cx));
            }
            return out.into_any_element();
        }

        let now = now_ms();
        let fresh = self.shared.fresh.clone().filter(|f| f.channel_id == channel.id);
        let codes: Vec<pb::ShareCode> = list
            .as_ref()
            .map(|l| {
                l.codes
                    .iter()
                    .filter(|c| {
                        c.channel_id == channel.id
                            && ms(c.expires_at.as_ref()) > now
                            && fresh.as_ref().is_none_or(|f| f.code != c.code)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        // One other server per channel, for now.
        let taken = !guests.is_empty();
        let maker: AnyElement = if let Some(code) = &fresh {
            self.fresh_code(code, now, p, cx)
        } else if on && !taken {
            let (making, elsewhere, cid) = (self.shared.making, self.shared.elsewhere, channel.id.clone());
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(16.0))
                .child(
                    primary_button("share-make", t("serversettings.sharedChannels.createCode"), p)
                        .when(making, |el| el.opacity(0.6))
                        .child(if making {
                            spinner("share-make-spin", 15.0, window)
                        } else {
                            icon("plus").size(px(15.0)).into_any_element()
                        })
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.make_code(cid.clone(), federation && elsewhere, cx)),
                        ),
                )
                .when(federation, |el| {
                    el.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(switch("share-elsewhere".into(), elsewhere, false, cx, |this, v, cx| {
                                this.shared.elsewhere = v;
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .child(
                                        div()
                                            .text_sm()
                                            .line_height(px(20.0))
                                            .font_weight(FontWeight::BOLD)
                                            .child(t("serversettings.sharedChannels.elsewhere")),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .line_height(px(16.0))
                                            .text_color(p.muted_foreground)
                                            .child(t("serversettings.sharedChannels.elsewhereHint")),
                                    ),
                            ),
                    )
                })
                .into_any_element()
        } else {
            div()
                .text_xs()
                .line_height(px(16.0))
                .text_color(p.muted_foreground)
                .child(if !on {
                    t("serversettings.sharedChannels.offShort")
                } else {
                    t("serversettings.sharedChannels.onlyOne")
                })
                .into_any_element()
        };
        let intro = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(16.0))
            .rounded(corner(16.0))
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.background, 0.4))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex_none()
                            .size(px(40.0))
                            .rounded(corner(14.0))
                            .bg(alpha(p.primary, 0.15))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(glyph(20.0, p.primary)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .line_height(px(20.0))
                            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t_with(
                                "serversettings.sharedChannels.shareTitle",
                                &[("channel", Arg::Str(&channel.name))],
                            )))
                            .child(div().text_color(p.muted_foreground).child(t_with(
                                "serversettings.sharedChannels.shareHint",
                                &[("channel", Arg::Str(&channel.name))],
                            ))),
                    ),
            )
            .child(maker)
            .when_some(self.shared.make_error.clone(), |el, e| {
                el.child(div().text_sm().line_height(px(20.0)).text_color(p.destructive).child(e))
            });

        let mut out = div().flex().flex_col().gap(px(20.0)).child(intro);
        if !codes.is_empty() {
            let mut section =
                div().flex().flex_col().gap(px(8.0)).child(heading(&t("serversettings.sharedChannels.codesOut"), 0, p));
            for (n, code) in codes.iter().enumerate() {
                section = section.child(self.code_row(code, now, n, p, window, cx));
            }
            out = out.child(section);
        }
        let mut with =
            div().flex().flex_col().gap(px(8.0)).child(heading(&t("serversettings.sharedChannels.sharedWith"), 0, p));
        if list.is_none() {
            with = with.child(shimmer_rows(1, p));
        } else if guests.is_empty() {
            with = with.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_sm()
                    .line_height(px(20.0))
                    .text_color(p.muted_foreground)
                    .child(icon("clipboard-paste").size(px(16.0)))
                    .child(t("serversettings.sharedChannels.noGuest"))
                    .child(icon("arrow-right").size(px(14.0)))
                    .child(t("serversettings.sharedChannels.sendCode")),
            );
        } else {
            for (n, c) in guests.iter().enumerate() {
                with = with.child(self.connection_row(c, Some(&channel.name), n, &url, p, cx));
            }
        }
        out.child(with).into_any_element()
    }

    /// A code just made, to copy.
    fn fresh_code(&self, code: &pb::ShareCode, now: i64, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let hide = self.core.prefs().streamer_mode;
        let copied =
            self.copied.as_ref().is_some_and(|(c, at)| *c == code.code && at.elapsed() < Duration::from_millis(1400));
        let value = code.code.clone();
        let works_for = {
            let time = time_left(ms(code.expires_at.as_ref()) - now);
            if share_code_instance(&code.code).is_empty() {
                t_with("serversettings.sharedChannels.worksForHere", &[("time", Arg::Str(&time))])
            } else {
                t_with("serversettings.sharedChannels.worksForAnywhere", &[("time", Arg::Str(&time))])
            }
        };
        motion::rise(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .p(px(12.0))
                .rounded(corner(12.0))
                .border_1()
                .border_color(alpha(p.primary, 0.3))
                .bg(alpha(p.primary, 0.05))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_family("monospace")
                                .text_sm()
                                .line_height(px(20.0))
                                .font_weight(FontWeight::BOLD)
                                .child(if hide { "••••••••".to_owned() } else { code.code.clone() }),
                        )
                        .child(
                            icon_button("fresh-copy", if copied { "check" } else { "copy" }, p)
                                .when(copied, |el| el.text_color(p.primary))
                                .on_click(cx.listener(move |this, _, _, cx| this.copy_code(value.clone(), cx))),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .text_xs()
                        .line_height(px(16.0))
                        .text_color(p.muted_foreground)
                        .child(icon("timer").size(px(12.0)))
                        .child(works_for),
                ),
            SharedString::from(format!("fresh-{}", code.code)),
            Duration::ZERO,
            12.0,
        )
        .into_any_element()
    }
}

/// Nothing shared yet: a floating glyph and where to start.
fn nothing_shared(p: &Palette) -> impl IntoElement {
    let primary = p.primary;
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.0))
        .p(px(32.0))
        .rounded(corner(24.0))
        .border_1()
        .border_dashed()
        .border_color(p.border)
        .child(motion::rise(
            div()
                .size(px(48.0))
                .rounded_full()
                .bg(alpha(primary, 0.15))
                .flex()
                .items_center()
                .justify_center()
                .child(glyph(24.0, primary)),
            "nothing-shared-in",
            Duration::from_millis(120),
            10.0,
        ))
        .child(div().font_weight(FontWeight::BOLD).child(t("serversettings.sharedChannels.nothingYet")))
        .child(
            div()
                .max_w(px(380.0))
                .text_center()
                .text_sm()
                .line_height(px(20.0))
                .text_color(p.muted_foreground)
                .child(t("serversettings.sharedChannels.nothingYetHint")),
        )
}

/// Where a code leads: whose channel it is, where what's said is kept, and what your people may do there.
fn preview_card(preview: &pb::PreviewShareResponse, url: &str, code: &str, p: &Palette) -> Div {
    let home = preview.home_server.as_ref();
    let home_name = home.map_or_else(|| t("serversettings.sharedChannels.theOtherServer"), |s| s.name.clone());
    let left = ms(preview.expires_at.as_ref()) - now_ms();
    let line = |glyph_name: &str, n: usize, body: Div| {
        motion::rise(
            div()
                .flex()
                .items_start()
                .gap(px(8.0))
                .child(icon(glyph_name).size(px(16.0)).mt(px(2.0)).text_color(p.primary))
                .child(body.flex_1().min_w_0()),
            SharedString::from(format!("preview-line-{code}-{n}")),
            Duration::from_millis(120 + n as u64 * 50),
            8.0,
        )
    };
    let mut body = div().flex().flex_col().gap(px(12.0)).p(px(16.0)).text_sm().line_height(px(20.0));
    if !preview.channel_topic.is_empty() {
        body = body.child(div().text_color(p.muted_foreground).child(preview.channel_topic.clone()));
    }
    if !preview.instance.is_empty() {
        body = body.child(instance_line(
            &preview.instance,
            &preview.fingerprint,
            &t("serversettings.sharedChannels.checkBeforeAsking"),
            p,
        ));
    }
    body = body.child(line(
        "database",
        1,
        div().child(t_with("serversettings.sharedChannels.storedOn", &[("server", Arg::Str(&home_name))])),
    ));
    if !preview.checked_by.is_empty() {
        body = body.child(line(
            "eye",
            2,
            div().child(t_with(
                "serversettings.sharedChannels.theirAutoModAlso",
                &[("providers", Arg::Str(&preview.checked_by.join(", ")))],
            )),
        ));
    }
    if preview.guest_count > 0 {
        let n = preview.guest_count;
        body = body.child(
            div()
                .text_color(p.muted_foreground)
                .child(t_with("serversettings.sharedChannels.alreadyShown", &[("count", Arg::Num(i64::from(n)))])),
        );
    }
    let allowed: Vec<i32> = preview.allowed.clone();
    let mut caps = div().flex().flex_wrap().gap(px(6.0)).child(capability(
        true,
        &t("serversettings.sharedChannels.read"),
        "eye",
        0,
        code,
        p,
    ));
    for (n, (perm, _, glyph_name)) in SHAREABLE.iter().enumerate() {
        let label = permission_name(*perm);
        caps = caps.child(capability(allowed.contains(&(*perm as i32)), &label, glyph_name, n + 1, code, p));
    }
    body = body.child(
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(div().font_weight(FontWeight::BOLD).child(t("serversettings.sharedChannels.whatTheyCanDo")))
            .child(caps)
            .child(
                div()
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(t("serversettings.sharedChannels.rolesNote")),
            ),
    );
    div()
        .rounded(corner(16.0))
        .border_1()
        .border_color(alpha(p.primary, 0.3))
        .bg(p.card)
        .overflow_hidden()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .p(px(16.0))
                .bg(alpha(p.primary, 0.08))
                .when_some(home, |el, s| el.child(server_picture(s, url, 48.0, 16.0, p)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_xs()
                                .line_height(px(16.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.muted_foreground)
                                .child(t_with(
                                    "serversettings.sharedChannels.from",
                                    &[("server", Arg::Str(&home_name))],
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .text_lg()
                                .line_height(px(28.0))
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .child(icon("hash").size(px(16.0)).text_color(p.muted_foreground))
                                .child(div().min_w_0().truncate().child(preview.channel_name.clone())),
                        ),
                )
                .when(left > 0, |el| {
                    el.child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .px(px(8.0))
                            .h(px(22.0))
                            .rounded_full()
                            .bg(p.muted)
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .child(icon("timer").size(px(12.0)))
                            .child(t_with(
                                "serversettings.sharedChannels.codeWorksFor",
                                &[("time", Arg::Str(&time_left(left)))],
                            )),
                    )
                }),
        )
        .child(body)
}
