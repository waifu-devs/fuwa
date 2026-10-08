//! The Webhooks page: addresses other apps (CI, feeds, alerts, anything made
//! for Discord webhooks) post messages to, each into one channel. Make one,
//! name it, give it a picture, pick its channel, copy its address, try it,
//! or give it a new address. The web's `settings/server/Webhooks.tsx`.

use gpui_kit::AnimationExt as _;

use super::*;
use crate::core::server_admin::{WebhookPatch, webhook_url};

/// Names new webhooks get, to be changed.
const NAMES: [&str; 6] = ["Captain Hook", "Fuwa Hook", "Hooky", "Post Bot", "Little Messenger", "Paper Plane"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ask {
    Reset,
    Delete,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Save,
    Test,
    Reset,
    Picture,
}

pub(super) struct Hooks {
    list: Option<Vec<pb::Webhook>>,
    creators: People,
    loading: bool,
    open: Option<String>,
    creating: bool,
    how_to: bool,
    name: Entity<InputState>,
    /// Which webhook the name box was filled for.
    filled_for: Option<String>,
    busy: Option<(String, Busy)>,
    asking: Option<(String, Ask)>,
    /// The webhook whose address is shown in full.
    shown: Option<String>,
    /// When a test post went through, to say so for a moment.
    sent: Option<(String, Instant)>,
}

impl Hooks {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ServerSettingsView>) -> (Self, Vec<Subscription>) {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Captain Hook"));
        let subscriptions = vec![cx.subscribe(&name, |this: &mut ServerSettingsView, _, e: &InputEvent, cx| match e {
            InputEvent::PressEnter { .. } | InputEvent::Blur => this.rename_hook(cx),
            _ => cx.notify(),
        })];
        let hooks = Self {
            list: None,
            creators: People::new(),
            loading: false,
            open: None,
            creating: false,
            how_to: false,
            name,
            filled_for: None,
            busy: None,
            asking: None,
            shown: None,
            sent: None,
        };
        (hooks, subscriptions)
    }
}

/// A webhook as people see it in chat: its name and picture, as a profile.
fn as_user(w: &pb::Webhook) -> pb::User {
    pb::User {
        id: w.id.clone(),
        display_name: w.name.clone(),
        username: w.name.clone(),
        avatar_url: w.avatar_url.clone(),
        ..Default::default()
    }
}

fn is_text(c: &pb::Channel) -> bool {
    matches!(
        pb::ChannelType::try_from(c.r#type).unwrap_or(pb::ChannelType::Text),
        pb::ChannelType::Text | pb::ChannelType::Announcement
    )
}

fn channel_glyph(c: Option<&pb::Channel>) -> &'static str {
    match c.map(|c| pb::ChannelType::try_from(c.r#type).unwrap_or(pb::ChannelType::Text)) {
        Some(pb::ChannelType::Announcement) => "megaphone",
        _ => "hash",
    }
}

impl ServerSettingsView {
    fn load_hooks(&mut self, cx: &mut Context<Self>) {
        if self.hooks.loading || self.hooks.list.is_some() {
            return;
        }
        self.hooks.loading = true;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.webhooks(&key, &sid).await }, |this, result, cx| {
            this.hooks.loading = false;
            match result {
                Ok((list, creators)) => {
                    this.hooks.list = Some(list);
                    this.hooks.creators = creators;
                }
                Err(err) => {
                    this.hooks.list = Some(Vec::new());
                    this.error = Some(err.message);
                }
            }
            cx.notify();
        });
    }

    /// Reads the list again without the loading look: how many each posted, and when.
    fn refresh_hooks(&mut self, cx: &mut Context<Self>) {
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.webhooks(&key, &sid).await }, |this, result, cx| {
            if let Ok((list, creators)) = result {
                this.hooks.list = Some(list);
                this.hooks.creators.extend(creators);
                cx.notify();
            }
        });
    }

    fn put_hook(&mut self, w: pb::Webhook) {
        if let Some(list) = self.hooks.list.as_mut() {
            match list.iter_mut().find(|x| x.id == w.id) {
                Some(x) => *x = w,
                None => list.push(w),
            }
        }
    }

    pub(super) fn text_channels(&self) -> Vec<pb::Channel> {
        self.core.shared.read(|s| {
            let mut list: Vec<pb::Channel> = s
                .instance(&self.key)
                .and_then(|i| i.channels.get(&self.server))
                .into_iter()
                .flatten()
                .filter(|c| is_text(c))
                .cloned()
                .collect();
            list.sort_by_key(|c| c.position);
            list
        })
    }

    fn new_hook(&mut self, cx: &mut Context<Self>) {
        if self.hooks.creating {
            return;
        }
        let Some(channel) = self.text_channels().into_iter().next() else {
            self.error = Some(t("serversettings.webhooks.needChannel"));
            cx.notify();
            return;
        };
        self.hooks.creating = true;
        self.error = None;
        let name = NAMES[self.hooks.list.as_ref().map_or(0, Vec::len) % NAMES.len()];
        let me = self.core.shared.read(|s| s.instance(&self.key).and_then(|i| i.me.clone()));
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(
            cx,
            async move { core.create_webhook(&key, &sid, &channel.id, name).await },
            move |this, result, cx| {
                this.hooks.creating = false;
                match result {
                    Ok(w) => {
                        if let Some(me) = me {
                            this.hooks.creators.insert(me.id.clone(), me);
                        }
                        this.hooks.open = Some(w.id.clone());
                        this.put_hook(w);
                    }
                    Err(err) => this.error = Some(err.message),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn hook(&self, id: &str) -> Option<pb::Webhook> {
        self.hooks.list.as_ref()?.iter().find(|w| w.id == id).cloned()
    }

    fn save_hook(&mut self, w: pb::Webhook, patch: WebhookPatch, cx: &mut Context<Self>) {
        self.hooks.busy = Some((w.id.clone(), Busy::Save));
        self.error = None;
        let (core, key) = (self.core.clone(), self.key.clone());
        self.run(cx, async move { core.update_webhook(&key, &w, patch).await }, |this, result, cx| {
            this.hooks.busy = None;
            match result {
                Ok(w) => this.put_hook(w),
                Err(err) => {
                    this.error = Some(err.message);
                    // The box shows the name it has again.
                    this.hooks.filled_for = None;
                }
            }
            cx.notify();
        });
        cx.notify();
    }

    fn rename_hook(&mut self, cx: &mut Context<Self>) {
        let Some(w) = self.hooks.filled_for.as_deref().and_then(|id| self.hook(id)) else { return };
        let name = self.hooks.name.read(cx).value().trim().to_owned();
        if name == w.name {
            return;
        }
        if name.is_empty() {
            self.hooks.filled_for = None;
            cx.notify();
            return;
        }
        self.save_hook(w, WebhookPatch { name: Some(name), ..WebhookPatch::default() }, cx);
    }

    /// Asks the system for a picture, frames it, and makes it the webhook's.
    fn pick_hook_picture(&mut self, w: pb::Webhook, cx: &mut Context<Self>) {
        if self.cropper.is_some() {
            return;
        }
        crate::ui::cropper::choose(
            self.core.clone(),
            crate::core::pictures::PictureKind::Avatar,
            t("desktop.account.choosePicture"),
            cx,
            |this| &mut this.cropper,
            |this, error, cx| {
                this.error = Some(error);
                cx.notify();
            },
            Rc::new(move |this: &mut Self, bytes, mime, cx: &mut Context<Self>| {
                this.upload_hook_picture(w.clone(), bytes, mime, cx)
            }),
        );
    }

    fn upload_hook_picture(&mut self, w: pb::Webhook, bytes: Vec<u8>, mime: &'static str, cx: &mut Context<Self>) {
        self.hooks.busy = Some((w.id.clone(), Busy::Picture));
        let (core, key) = (self.core.clone(), self.key.clone());
        self.run(
            cx,
            async move {
                let url = core.upload_picture(&key, pb::MediaPurpose::Avatar, mime, bytes).await?;
                core.update_webhook(&key, &w, WebhookPatch { avatar_url: Some(url), ..WebhookPatch::default() }).await
            },
            |this, result, cx| {
                this.hooks.busy = None;
                match result {
                    Ok(w) => this.put_hook(w),
                    Err(err) => this.error = Some(err.message),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn test_hook(&mut self, w: pb::Webhook, cx: &mut Context<Self>) {
        self.hooks.busy = Some((w.id.clone(), Busy::Test));
        self.error = None;
        let (core, key) = (self.core.clone(), self.key.clone());
        let id = w.id.clone();
        let content = format!(
            "👋 {}",
            t_with("serversettings.webhooks.testMessage", &[("name", Arg::Str(&format!("**{}**", w.name)))])
        );
        self.run(cx, async move { core.test_webhook(&key, &w, &content).await }, move |this, result, cx| {
            this.hooks.busy = None;
            match result {
                Ok(()) => {
                    this.hooks.sent = Some((id, Instant::now()));
                    this.refresh_hooks(cx);
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(Duration::from_millis(1700)).await;
                        let _ = this.update(cx, |_, cx| cx.notify());
                    })
                    .detach();
                }
                Err(err) => this.error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    fn answer_hook(&mut self, w: pb::Webhook, ask: Ask, cx: &mut Context<Self>) {
        self.hooks.asking = None;
        let (core, key, sid, id) = (self.core.clone(), self.key.clone(), self.server.clone(), w.id.clone());
        match ask {
            Ask::Reset => {
                self.hooks.busy = Some((id.clone(), Busy::Reset));
                self.run(cx, async move { core.reset_webhook(&key, &sid, &id).await }, |this, result, cx| {
                    this.hooks.busy = None;
                    match result {
                        Ok(w) => {
                            this.put_hook(w);
                            this.flash_saved(cx);
                        }
                        Err(err) => this.error = Some(err.message),
                    }
                    cx.notify();
                });
            }
            Ask::Delete => {
                let gone = id.clone();
                self.run(cx, async move { core.delete_webhook(&key, &sid, &id).await }, move |this, result, cx| {
                    match result {
                        Ok(()) => {
                            if let Some(list) = this.hooks.list.as_mut() {
                                list.retain(|w| w.id != gone);
                            }
                        }
                        Err(err) => this.error = Some(err.message),
                    }
                    cx.notify();
                });
            }
        }
        cx.notify();
    }

    pub(super) fn webhooks_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.load_hooks(cx);
        let channels = self.text_channels();
        let base = self.core.api(&self.key).map(|a| a.url.clone()).unwrap_or_default();

        // Keep the open one's name box filled for it.
        if let Some(w) = self.hooks.open.as_deref().and_then(|id| self.hook(id))
            && self.hooks.filled_for.as_deref() != Some(w.id.as_str())
        {
            self.hooks.filled_for = Some(w.id.clone());
            self.hooks.name.update(cx, |s, cx| s.set_value(w.name.clone(), window, cx));
        }

        let mut page = div().flex().flex_col().gap(px(20.0)).child(self.hooks_header(p, window, cx));
        match &self.hooks.list {
            None => {
                for n in 0..2 {
                    page = page.child(div().h(px(64.0)).rounded(corner(16.0)).bg(alpha(p.muted, 0.6)).with_animation(
                        SharedString::from(format!("hook-shimmer-{n}")),
                        gpui_kit::Animation::new(Duration::from_millis(1200)).repeat(),
                        |el, t| el.opacity(0.5 + 0.5 * (t * std::f32::consts::TAU).sin().abs()),
                    ));
                }
            }
            Some(list) if list.is_empty() => {
                page = page.child(motion::rise(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(8.0))
                        .py(px(32.0))
                        .child(motion::once(
                            div().text_size(px(36.0)).line_height(px(40.0)).child("🪝"),
                            "hook-empty-bob",
                            Duration::from_millis(1200),
                            |el, t| el.relative().top(px(-6.0 * (t * std::f32::consts::PI).sin())),
                        ))
                        .child(div().font_weight(FontWeight::BOLD).child(t("serversettings.webhooks.none")))
                        .child(
                            div()
                                .text_sm()
                                .line_height(px(20.0))
                                .text_color(p.muted_foreground)
                                .child(t("serversettings.webhooks.noneHint")),
                        ),
                    "hook-empty",
                    Duration::from_millis(80),
                    8.0,
                ));
            }
            Some(list) => {
                let list = list.clone();
                let mut cards = div().flex().flex_col().gap(px(8.0));
                for (n, w) in list.iter().enumerate() {
                    cards = cards.child(self.hook_card(w, &channels, &base, n, p, window, cx));
                }
                page = page.child(cards);
            }
        }
        // The secret part shows only while that webhook's address is shown.
        let example = match self.hooks.list.as_ref().and_then(|l| l.first()) {
            Some(w) if self.hooks.shown.as_deref() == Some(w.id.as_str()) => webhook_url(&base, w),
            Some(w) => webhook_url(&base, &pb::Webhook { token: "…".into(), ..w.clone() }),
            None => format!("{}/webhooks/…", base.trim_end_matches('/')),
        };
        page.child(self.how_apps_post(&example, p, cx)).into_any_element()
    }

    fn hooks_header(&self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let creating = self.hooks.creating;
        // Faint lines with dots drifting along them once: messages on their way in.
        let mut wires = div().absolute().inset_0();
        for (i, top) in [0.22f32, 0.5, 0.78].into_iter().enumerate() {
            let line = alpha(p.primary, 0.18);
            wires = wires
                .child(div().absolute().left_0().right_0().top(gpui_kit::relative(top)).h(px(1.0)).bg(line))
                .child(motion::once(
                    div()
                        .absolute()
                        .size(px(5.0))
                        .rounded_full()
                        .bg(alpha(p.primary, 0.6))
                        .top(gpui_kit::relative(top))
                        .mt(px(-2.0)),
                    SharedString::from(format!("hook-wire-{i}")),
                    Duration::from_millis(1800 + i as u64 * 500),
                    |el, t| el.left(gpui_kit::relative(t)).opacity((t * std::f32::consts::PI).sin()),
                ));
        }
        div()
            .relative()
            .overflow_hidden()
            .rounded(corner(24.0))
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.primary, 0.06))
            .p(px(20.0))
            .child(wires)
            .child(
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(16.0))
                    .child(
                        div()
                            .size(px(48.0))
                            .flex_none()
                            .rounded(corner(16.0))
                            .bg(alpha(p.primary, 0.15))
                            .text_color(p.primary)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(motion::once(
                                icon("webhook").size(px(24.0)),
                                "hook-icon-sway",
                                Duration::from_millis(1400),
                                |el, t| {
                                    el.rotate(gpui_kit::radians((t * std::f32::consts::TAU).sin() * 0.14 * (1.0 - t)))
                                },
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("serversettings.webhooks.title")))
                            .child(
                                div()
                                    .text_sm()
                                    .line_height(px(20.0))
                                    .text_color(p.muted_foreground)
                                    .child(t("serversettings.webhooks.intro")),
                            ),
                    )
                    .child(
                        super::pages::act(
                            "hook-new",
                            t("serversettings.webhooks.new"),
                            if creating {
                                spinner("hook-new-spin", 16.0, window)
                            } else {
                                icon("plus").size(px(16.0)).into_any_element()
                            },
                            crate::ui::settings_controls::Look::Primary,
                            p,
                        )
                        .when(creating || self.hooks.list.is_none(), |el| el.opacity(0.5))
                        .on_click(cx.listener(|this, _, _, cx| this.new_hook(cx))),
                    ),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn hook_card(
        &self,
        w: &pb::Webhook,
        channels: &[pb::Channel],
        base: &str,
        n: usize,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = w.id.clone();
        let open = self.hooks.open.as_deref() == Some(id.as_str());
        let channel = channels.iter().find(|c| c.id == w.channel_id);
        let busy = self.hooks.busy.as_ref().filter(|(b, _)| *b == id).map(|(_, k)| *k);
        let user = as_user(w);
        let place = channel.map(|c| c.name.clone()).unwrap_or_else(|| t("serversettings.invites.deletedChannel"));
        let count = w.messages;
        let about = match w.last_used_at.as_ref() {
            Some(at) => t_with(
                "serversettings.webhooks.lineUsed",
                &[
                    ("channel", Arg::Str(&place)),
                    ("count", Arg::Num(count)),
                    ("when", Arg::Str(&stamp(at.seconds * 1000))),
                ],
            ),
            None => {
                t_with("serversettings.webhooks.line", &[("channel", Arg::Str(&place)), ("count", Arg::Num(count))])
            }
        };
        let toggle = id.clone();
        let chevron =
            motion::follow(SharedString::from(format!("hook-chev-{id}")), if open { 1.0 } else { 0.0 }, window, cx);
        let hover_border = alpha(p.primary, 0.3);
        let head = div()
            .id(SharedString::from(format!("hook-head-{id}")))
            .flex()
            .items_center()
            .gap(px(12.0))
            .p(px(12.0))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.hooks.open =
                    if this.hooks.open.as_deref() == Some(toggle.as_str()) { None } else { Some(toggle.clone()) };
                this.hooks.asking = None;
                cx.notify();
            }))
            .child(avatar(Some(&user), 40.0, p))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(div().min_w_0().truncate().font_weight(FontWeight::BOLD).child(w.name.clone()))
                            .child(app_badge(SharedString::from(format!("hook-app-{id}")), "APP", p)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .child(icon(channel_glyph(channel)).size(px(12.0)))
                            .child(div().min_w_0().truncate().child(about)),
                    ),
            )
            .when(busy == Some(Busy::Save), |el| {
                el.child(div().text_color(p.muted_foreground).child(spinner(format!("hook-save-{id}"), 16.0, window)))
            })
            .child(
                icon("chevron-down")
                    .size(px(16.0))
                    .text_color(p.muted_foreground)
                    .rotate(gpui_kit::radians(chevron * std::f32::consts::PI)),
            );

        let card = div()
            .id(SharedString::from(format!("hook-{id}")))
            .rounded(corner(16.0))
            .border_1()
            .bg(alpha(p.background, 0.5))
            .map(|el| {
                if open {
                    el.border_color(alpha(p.primary, 0.4)).shadow_lg()
                } else {
                    el.border_color(p.border).hover(move |s| s.border_color(hover_border))
                }
            })
            .child(head)
            .when(open, |el| el.child(self.hook_body(w, channels, base, busy, p, window, cx)));
        motion::rise(
            card,
            SharedString::from(format!("hook-in-{id}")),
            Duration::from_millis((n.min(8) * 30) as u64),
            10.0,
        )
        .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn hook_body(
        &self,
        w: &pb::Webhook,
        channels: &[pb::Channel],
        base: &str,
        busy: Option<Busy>,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = w.id.clone();
        let url = webhook_url(base, w);
        let shown = self.hooks.shown.as_deref() == Some(id.as_str());
        let copied = self.copied.as_ref().is_some_and(|(c, at)| *c == id && at.elapsed() < Duration::from_secs(2));
        let sent =
            self.hooks.sent.as_ref().is_some_and(|(s, at)| *s == id && at.elapsed() < Duration::from_millis(1600));
        let asking = self.hooks.asking.as_ref().filter(|(a, _)| *a == id).map(|(_, k)| *k);
        let creator = self.hooks.creators.get(&w.creator_id);

        // The picture: click to change it.
        let picture = {
            let w2 = w.clone();
            let shade = gpui_kit::hsla(0.0, 0.0, 0.0, 0.45);
            div()
                .id(SharedString::from(format!("hook-pic-{id}")))
                .relative()
                .flex_none()
                .size(px(72.0))
                .rounded_full()
                .cursor_pointer()
                .group("hook-pic")
                .on_click(cx.listener(move |this, _, _, cx| this.pick_hook_picture(w2.clone(), cx)))
                .child(avatar(Some(&as_user(w)), 72.0, p))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(gpui_kit::white())
                        .bg(shade)
                        .opacity(if busy == Some(Busy::Picture) { 1.0 } else { 0.0 })
                        .group_hover("hook-pic", |s| s.opacity(1.0))
                        .child(if busy == Some(Busy::Picture) {
                            spinner(format!("hook-pic-spin-{id}"), 20.0, window)
                        } else {
                            icon("camera").size(px(20.0)).into_any_element()
                        }),
                )
        };

        let mut picks = div().flex().flex_wrap().gap(px(6.0));
        for c in channels {
            let on = c.id == w.channel_id;
            let (w2, cid) = (w.clone(), c.id.clone());
            picks = picks.child(
                chip(SharedString::from(format!("hook-ch-{id}-{}", c.id)), &format!("# {}", c.name), on, p).on_click(
                    cx.listener(move |this, _, _, cx| {
                        if !on {
                            this.save_hook(
                                w2.clone(),
                                WebhookPatch { channel_id: Some(cid.clone()), ..WebhookPatch::default() },
                                cx,
                            )
                        }
                    }),
                ),
            );
        }

        // The address, hidden as dots so nothing of it shows on a shared screen.
        let address = {
            let (eye, link) = (id.clone(), url.clone());
            let copy_id = id.clone();
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .p(px(4.0))
                        .pl(px(12.0))
                        .rounded(corner(12.0))
                        .border_1()
                        .border_color(p.border)
                        .bg(alpha(p.muted, 0.4))
                        .child(motion::fade_in(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_family("monospace")
                                .text_xs()
                                .line_height(px(16.0))
                                .when(!shown, |el| el.truncate())
                                .child(if shown { url.clone() } else { "•".repeat(28) }),
                            SharedString::from(format!("hook-addr-{id}-{shown}")),
                            Duration::from_millis(180),
                        ))
                        .child(
                            icon_button(
                                SharedString::from(format!("hook-eye-{id}")),
                                if shown { "eye-off" } else { "eye" },
                                p,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.hooks.shown = if this.hooks.shown.as_deref() == Some(eye.as_str()) {
                                    None
                                } else {
                                    Some(eye.clone())
                                };
                                cx.notify();
                            })),
                        )
                        .child(
                            primary_button(
                                SharedString::from(format!("hook-copy-{id}")),
                                if copied {
                                    t("serversettings.webhooks.copied")
                                } else {
                                    t("serversettings.webhooks.copy")
                                },
                                p,
                            )
                            .h(px(32.0))
                            .px(px(12.0))
                            .text_sm()
                            .line_height(px(20.0))
                            .child(motion::rise(
                                icon(if copied { "check" } else { "copy" }).size(px(14.0)),
                                SharedString::from(format!("hook-copy-icon-{id}-{copied}")),
                                Duration::ZERO,
                                if copied { 6.0 } else { 0.0 },
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(link.clone()));
                                this.copied = Some((copy_id.clone(), Instant::now()));
                                cx.spawn(async move |this, cx| {
                                    cx.background_executor().timer(Duration::from_millis(2100)).await;
                                    let _ = this.update(cx, |_, cx| cx.notify());
                                })
                                .detach();
                                cx.notify();
                            })),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .line_height(px(16.0))
                        .text_color(p.muted_foreground)
                        .child(t("serversettings.webhooks.secret")),
                )
        };

        let test = {
            let w2 = w.clone();
            let label = if sent {
                t("serversettings.webhooks.posted")
            } else if busy == Some(Busy::Test) {
                t("desktop.server.webhooks.sending")
            } else {
                t("serversettings.webhooks.test")
            };
            soft_button(SharedString::from(format!("hook-test-{id}")), label, p)
                .when(busy.is_some() || channels.iter().all(|c| c.id != w.channel_id), |el| el.opacity(0.6))
                .child(if busy == Some(Busy::Test) {
                    spinner(format!("hook-test-spin-{id}"), 14.0, window)
                } else if sent {
                    motion::rise(
                        icon("check").size(px(14.0)).text_color(p.success),
                        SharedString::from(format!("hook-sent-{id}")),
                        Duration::ZERO,
                        6.0,
                    )
                    .into_any_element()
                } else {
                    icon("send").size(px(14.0)).into_any_element()
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.hooks.busy.is_none() {
                        this.test_hook(w2.clone(), cx)
                    }
                }))
        };

        let actions = match asking {
            Some(ask) => {
                let w2 = w.clone();
                motion::slide_in(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(div().px(px(4.0)).text_xs().line_height(px(16.0)).font_weight(FontWeight::BOLD).child(
                            match ask {
                                Ask::Reset => t("serversettings.webhooks.resetAsk"),
                                Ask::Delete => t("serversettings.webhooks.deleteAsk"),
                            },
                        ))
                        .child(
                            danger_button(
                                SharedString::from(format!("hook-yes-{id}")),
                                if ask == Ask::Reset {
                                    t("serversettings.webhooks.newAddress")
                                } else {
                                    t("serversettings.shared.delete")
                                },
                                p,
                            )
                            .h(px(32.0))
                            .px(px(12.0))
                            .text_xs()
                            .line_height(px(16.0))
                            .on_click(cx.listener(move |this, _, _, cx| this.answer_hook(w2.clone(), ask, cx))),
                        )
                        .child(icon_button(SharedString::from(format!("hook-no-{id}")), "x", p).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.hooks.asking = None;
                                cx.notify();
                            },
                        ))),
                    SharedString::from(format!("hook-ask-{id}")),
                    8.0,
                )
                .into_any_element()
            }
            None => {
                let (reset_id, delete_id) = (id.clone(), id.clone());
                let red = alpha(p.destructive, 0.1);
                let resetting = busy == Some(Busy::Reset);
                div()
                    .flex()
                    .gap(px(4.0))
                    .child(
                        soft_button(
                            SharedString::from(format!("hook-reset-{id}")),
                            t("serversettings.webhooks.newAddress"),
                            p,
                        )
                        .bg(gpui_kit::transparent_black())
                        .child(if resetting {
                            spinner(format!("hook-reset-spin-{id}"), 14.0, window)
                        } else {
                            icon("refresh-cw").size(px(14.0)).into_any_element()
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.hooks.busy.is_none() {
                                this.hooks.asking = Some((reset_id.clone(), Ask::Reset));
                                cx.notify();
                            }
                        })),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("hook-delete-{id}")))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .h(px(36.0))
                            .px(px(12.0))
                            .rounded(corner(12.0))
                            .cursor_pointer()
                            .text_sm()
                            .line_height(px(20.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.destructive)
                            .hover(move |s| s.bg(red))
                            .child(icon("trash").size(px(14.0)))
                            .child(t("serversettings.shared.delete"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.hooks.asking = Some((delete_id.clone(), Ask::Delete));
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            }
        };

        motion::rise(
            div()
                .flex()
                .flex_col()
                .gap(px(16.0))
                .p(px(16.0))
                .border_t_1()
                .border_color(p.border)
                .child(
                    div().flex().items_start().gap(px(16.0)).child(picture).child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(12.0))
                            .child(labeled(&t("serversettings.overview.name"), Input::new(&self.hooks.name), p))
                            .child(labeled(&t("serversettings.webhooks.postsIn"), picks, p)),
                    ),
                )
                .child(labeled(&t("serversettings.webhooks.address"), address, p))
                .child(
                    div().flex().flex_wrap().items_center().gap(px(8.0)).child(test).child(actions).child(
                        div()
                            .ml_auto()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .child(avatar(creator, 16.0, p))
                            .child({
                                let when = w.created_at.as_ref().map(|at| stamp(at.seconds * 1000)).unwrap_or_default();
                                match creator {
                                    Some(u) => t_with(
                                        "serversettings.webhooks.madeBy",
                                        &[("name", Arg::Str(&user_name(u))), ("when", Arg::Str(&when))],
                                    ),
                                    None => {
                                        t_with("serversettings.webhooks.madeBySomeone", &[("when", Arg::Str(&when))])
                                    }
                                }
                            }),
                    ),
                ),
            SharedString::from(format!("hook-body-{id}")),
            Duration::ZERO,
            6.0,
        )
        .into_any_element()
    }

    fn how_apps_post(&self, example: &str, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let open = self.hooks.how_to;
        let code = format!(
            "curl -X POST '{example}' \\\n  -H 'content-type: application/json' \\\n  -d '{{\"content\": \"Build **passed** ✨\"}}'"
        );
        div()
            .rounded(corner(16.0))
            .border_1()
            .border_color(p.border)
            .child(
                div()
                    .id("hook-how")
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .p(px(12.0))
                    .cursor_pointer()
                    .text_sm()
                    .line_height(px(20.0))
                    .font_weight(FontWeight::BOLD)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.hooks.how_to = !this.hooks.how_to;
                        cx.notify();
                    }))
                    .child(icon("terminal").size(px(16.0)).text_color(p.primary))
                    .child(div().flex_1().child(t("serversettings.webhooks.howTo")))
                    .child(
                        icon(if open { "chevron-up" } else { "chevron-down" })
                            .size(px(16.0))
                            .text_color(p.muted_foreground),
                    ),
            )
            .when(open, |el| {
                el.child(motion::rise(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .px(px(12.0))
                        .pb(px(12.0))
                        .text_sm()
                        .line_height(px(20.0))
                        .text_color(p.muted_foreground)
                        .child(t_with(
                            "serversettings.webhooks.howToPost",
                            &[
                                ("post", Arg::Str("POST")),
                                ("content", Arg::Str("content")),
                                ("username", Arg::Str("username")),
                                ("avatarUrl", Arg::Str("avatar_url")),
                                ("embeds", Arg::Str("embeds")),
                                ("wait", Arg::Str("?wait=true")),
                            ],
                        ))
                        .child(
                            div()
                                .p(px(12.0))
                                .rounded(corner(12.0))
                                .bg(p.muted)
                                .font_family("monospace")
                                .text_xs()
                                .line_height(px(16.0))
                                .text_color(p.foreground)
                                .child(code),
                        )
                        .child(t_with("serversettings.webhooks.limits", &[("count", Arg::Num(30))])),
                    "hook-how-body",
                    Duration::ZERO,
                    6.0,
                ))
            })
            .into_any_element()
    }
}
