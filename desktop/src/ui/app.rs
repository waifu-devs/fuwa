//! The window: what's open, what's being typed, and everything floating over
//! it (dialogs, settings, toasts). Each part draws itself in its own module.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::AnimationExt as _;
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::message_scroller::MessageScrollerState;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _,
    Render, Styled as _, Subscription, Window, actions, div, px,
};

use crate::core::config::Prefs;
use crate::core::dms::{Content, now_ms};
use crate::core::store::Focus;
use crate::core::{Core, Notice};
use crate::ui::connect::{ConnectEvent, ConnectView};
use crate::ui::server_settings::{ServerSettingsEvent, ServerSettingsView};
use crate::ui::settings::{SettingsEvent, SettingsView};
use crate::ui::theme::{self, FONT};
use crate::ui::widgets::pal;

actions!(fuwa, [CloseOverlay, OpenSettings, AddInstance]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", CloseOverlay, Some("Fuwa")),
        KeyBinding::new("secondary-shift-n", AddInstance, Some("Fuwa")),
    ]);
}

/// What the middle of the window shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Nav {
    /// Direct messages from every instance; one open, or none.
    Home { dm: Option<(String, String)> },
    /// An instance's own page: its connection, and making or joining servers there.
    Instance { key: String },
    /// A server, with the channel last opened in it.
    Server { key: String, server: String },
}

/// Where the composer sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Channel { key: String, server: String, channel: String },
    Dm { key: String, conversation: String },
}

impl Target {
    pub fn id(&self) -> String {
        match self {
            Target::Channel { key, channel, .. } => format!("c|{key}|{channel}"),
            Target::Dm { key, conversation } => format!("d|{key}|{conversation}"),
        }
    }

    pub fn key(&self) -> &str {
        match self {
            Target::Channel { key, .. } | Target::Dm { key, .. } => key,
        }
    }
}

/// Something floating in the middle of the window, waiting for an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    CreateServer {
        key: String,
    },
    JoinInvite {
        key: String,
    },
    Invite {
        link: Option<String>,
        server: String,
    },
    Safety {
        key: String,
        conversation: String,
    },
    LeaveServer {
        key: String,
        server: String,
    },
    /// Someone's card: their picture, names, pronouns and bio.
    Profile {
        key: String,
        user_id: String,
        server: Option<String>,
    },
    /// A new channel, or a category with `category`, under `parent` (or at the top).
    CreateChannel {
        key: String,
        server: String,
        parent: String,
        category: bool,
    },
    /// A server's rules, to agree to before talking.
    Rules {
        key: String,
        server: String,
    },
    /// A server's welcome screen: a few words and where to start.
    Welcome {
        key: String,
        server: String,
    },
    /// Joining a server that asks people to sign in through its identity
    /// provider first, found from an invite.
    SsoJoin {
        key: String,
        server: String,
        name: String,
        provider: String,
        host: String,
        code: String,
    },
    /// Time out, kick or ban someone, with a reason for the audit log.
    Moderate {
        key: String,
        server: String,
        user_id: String,
        action: crate::core::moderation::Action,
    },
}

/// A small menu hanging under a bell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Menu {
    Channel { key: String, server: String, channel: String },
    Server { key: String, server: String },
}

/// The @ list while someone types a mention.
#[derive(Debug, Clone, Default)]
pub struct Picker {
    /// Where the `@` is in the composer, in bytes.
    pub start: usize,
    pub options: Vec<crate::ui::mentions::Pick>,
    pub active: usize,
}

pub struct Toast {
    pub id: u64,
    pub icon: &'static str,
    pub title: String,
    pub body: String,
    pub open: Option<Nav>,
    pub channel: Option<String>,
    pub leaving: bool,
}

/// What the message list shows, so it knows when to grow, shrink or start over.
#[derive(Default)]
pub struct ListSync {
    pub target: Option<String>,
    pub len: usize,
    pub first: String,
    pub digest: u64,
    /// Each row's digest, so only the rows that changed are measured again.
    pub rows: Vec<u64>,
}

pub struct FuwaApp {
    pub core: Arc<Core>,
    pub prefs: Prefs,
    pub nav: Nav,
    /// The channel last opened in each server ("key|server").
    pub channel_of: HashMap<String, String>,
    pub composer: Entity<TextareaState>,
    pub drafts: HashMap<String, String>,
    pub draft_for: Option<String>,
    pub scroller: Entity<MessageScrollerState>,
    pub list: ListSync,
    pub rows: std::rc::Rc<Vec<crate::ui::chat::Row>>,
    /// The open channel's messages as built, kept between changes.
    pub built: crate::ui::chat::Built,
    /// Messages that arrived while their list was open, and when: they rise in.
    pub fresh: HashMap<String, Instant>,
    pub requested: HashSet<String>,
    pub prepared: HashSet<String>,
    pub hovered: Option<String>,
    pub members_open: bool,
    /// The open server's member list, a view of its own.
    pub members_view: Option<Entity<crate::ui::members::MembersView>>,
    pub connect: Option<Entity<ConnectView>>,
    pub settings: Option<Entity<SettingsView>>,
    pub server_settings: Option<Entity<ServerSettingsView>>,
    pub dialog: Option<Dialog>,
    pub dialog_input: Entity<InputState>,
    pub dialog_busy: bool,
    pub dialog_error: Option<String>,
    pub toasts: Vec<Toast>,
    pub next_toast: u64,
    pub copied: Option<Instant>,
    /// The window's own focus, so its shortcuts work when no field has it.
    pub focus: gpui_kit::FocusHandle,
    /// The message being edited in the open list (an id, or a private message's sequence).
    pub editing: Option<String>,
    pub edit_box: Entity<TextareaState>,
    pub picker: Option<Picker>,
    /// Where the @ list was closed with Escape, so it stays closed for that mention.
    pub picker_dismissed: Option<usize>,
    /// Roles picked from the @ list by name, sent as their tokens.
    pub picked_roles: Vec<(String, String)>,
    pub menu: Option<Menu>,
    /// The emoji picker over the composer, and its search box.
    pub emoji_open: bool,
    pub emoji_query: Entity<InputState>,
    /// The profile the open card shows, once it arrives.
    pub profile: Option<crate::pb::Profile>,
    /// The rules the rules dialog shows, once they arrive.
    pub rules: Option<Vec<String>>,
    /// The welcome screen the welcome dialog shows, once it arrives.
    pub welcome: Option<crate::pb::WelcomeScreen>,
    /// Servers checked for a welcome screen to greet you with, this run.
    pub welcome_checked: HashSet<String>,
    /// The server whose sign-in page is open in the browser, and which try
    /// that is: clicking again starts over, and the old one's answer is dropped.
    pub sso_waiting: Option<String>,
    pub sso_try: u64,
    /// Dragging channels into order: where it would land, where each row of
    /// the open server's list sits, what's being dragged, and the row that
    /// just landed (it flashes).
    pub arrange: Option<crate::ui::arrange::Mark>,
    pub arrange_slots: Vec<crate::ui::arrange::Slot>,
    pub dragging: Option<String>,
    pub landed: Option<(String, Instant)>,
    /// The quick switcher, and the shortcut sheet, when open.
    pub switcher: Option<crate::ui::keys::Switcher>,
    pub sheet_open: bool,
    /// How many things that take focus were open last frame.
    covers: usize,
    /// The page's color before the theme changed, fading out over the new one.
    theme_fade: Option<(gpui_kit::Rgba, Instant)>,
    _subscriptions: Vec<Subscription>,
}

impl FuwaApp {
    pub fn new(core: Arc<Core>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prefs = core.prefs();
        let composer = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 10).submit_on_enter(true));
        let edit_box = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 10).submit_on_enter(true));
        let dialog_input = cx.new(|cx| InputState::new(window, cx));
        let emoji_query = cx.new(|cx| InputState::new(window, cx).placeholder("Find an emoji"));
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let mut subscriptions = vec![
            cx.subscribe_in(&composer, window, |this: &mut Self, _, event: &InputEvent, window, cx| {
                match event {
                    InputEvent::PressEnter { shift: false, .. } => this.send_now(window, cx),
                    // The send button lights up once there's something to send.
                    InputEvent::Change => {
                        this.update_picker(cx);
                        cx.notify()
                    }
                    InputEvent::Focus | InputEvent::Blur => cx.notify(),
                    _ => {}
                }
            }),
            cx.subscribe_in(&edit_box, window, |this: &mut Self, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.save_edit(window, cx);
                }
            }),
            cx.subscribe_in(&emoji_query, window, |_: &mut Self, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }),
            cx.subscribe_in(&dialog_input, window, |this: &mut Self, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.confirm_dialog(window, cx);
                }
            }),
            cx.observe_window_appearance(window, |this, window, cx| {
                theme::apply(&this.prefs, window.appearance(), cx);
                cx.notify();
            }),
            // Ambient loops run only while the window is in front.
            cx.observe_window_activation(window, |_, _, cx| cx.notify()),
        ];
        // Keys the text fields would otherwise take: the @ list's arrows, Enter
        // and Escape, Up to edit your last message, Escape to stop editing.
        let weak = cx.entity().downgrade();
        subscriptions.push(cx.intercept_keystrokes(move |event, window, cx| {
            let _ = weak.update(cx, |this, cx| {
                if this.intercept(&event.keystroke, window, cx) {
                    cx.stop_propagation();
                }
            });
        }));
        subscriptions.shrink_to_fit();

        // Redraw whenever the store changes.
        let mut changes = core.changes();
        cx.spawn_in(window, async move |this, cx| {
            while changes.changed().await.is_ok() {
                if this.update_in(cx, |this, window, cx| this.on_change(window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        // Tell the person about what happens elsewhere.
        if let Some(mut notices) = core.take_notices() {
            cx.spawn_in(window, async move |this, cx| {
                while let Some(notice) = notices.recv().await {
                    if this.update_in(cx, |this, window, cx| this.on_notice(notice, window, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }

        // A notification clicked while the window was in the background.
        let mut clicks = crate::ui::notify::clicks();
        cx.spawn_in(window, async move |this, cx| {
            use futures::StreamExt as _;
            while let Some(click) = clicks.next().await {
                let done = this.update_in(cx, |this, window, cx| {
                    window.activate_window();
                    if click.instance.is_empty() {
                        return;
                    }
                    match click.server {
                        Some(server) => this.open_channel(&click.instance, &server, &click.channel, window, cx),
                        None => this.navigate(Nav::Home { dm: Some((click.instance, click.channel)) }, window, cx),
                    }
                });
                if done.is_err() {
                    break;
                }
            }
        })
        .detach();

        let first = core.shared.read(|s| s.order.first().cloned());
        let mut app = Self {
            core,
            prefs,
            nav: Nav::Home { dm: None },
            channel_of: HashMap::new(),
            composer,
            drafts: HashMap::new(),
            draft_for: None,
            scroller,
            list: ListSync::default(),
            rows: std::rc::Rc::new(Vec::new()),
            built: Default::default(),
            fresh: HashMap::new(),
            requested: HashSet::new(),
            prepared: HashSet::new(),
            hovered: None,
            members_open: true,
            members_view: None,
            connect: None,
            settings: None,
            server_settings: None,
            dialog: None,
            dialog_input,
            dialog_busy: false,
            dialog_error: None,
            toasts: Vec::new(),
            next_toast: 1,
            copied: None,
            focus: cx.focus_handle(),
            editing: None,
            edit_box,
            picker: None,
            picker_dismissed: None,
            picked_roles: Vec::new(),
            menu: None,
            welcome: None,
            welcome_checked: HashSet::new(),
            sso_waiting: None,
            sso_try: 0,
            arrange: None,
            arrange_slots: Vec::new(),
            dragging: None,
            landed: None,
            theme_fade: None,
            switcher: None,
            sheet_open: false,
            covers: 0,
            emoji_open: false,
            emoji_query,
            profile: None,
            rules: None,
            _subscriptions: subscriptions,
        };
        // Shortcuts are read on the way to whatever has focus, so something always has it.
        app.focus.focus(window, cx);
        if first.is_none() {
            app.open_connect(false, window, cx);
        }
        app
    }

    // ───────────────────────── Reacting ─────────────────────────

    fn on_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _timed = crate::ui::perf::time("store change");
        // A server that went away (left, removed) takes you home.
        let gone = match &self.nav {
            Nav::Server { key, server } => {
                self.core.shared.read(|s| s.instance(key).and_then(|i| i.server(server)).is_none())
            }
            Nav::Instance { key } => self.core.shared.read(|s| s.instance(key).is_none()),
            Nav::Home { dm: Some((key, id)) } => self
                .core
                .shared
                .read(|s| s.instance(key).is_none_or(|i| !i.dms.conversations.iter().any(|c| &c.id == id))),
            Nav::Home { dm: None } => false,
        };
        if gone {
            self.navigate(Nav::Home { dm: None }, window, cx);
        }
        self.maybe_welcome(cx);
        // A server's channels arrived after it was opened: open the first.
        if self.target().map(|t| t.id()) != self.draft_for {
            self.after_move(window, cx);
        }
        self.ensure_loaded(cx);
        self.sync_list(cx);
        cx.notify();
    }

    fn on_notice(&mut self, notice: Notice, window: &mut Window, cx: &mut Context<Self>) {
        match notice {
            Notice::Message { instance, server_id, channel_id, title, body, mention } => {
                if !self.prefs.notifications {
                    return;
                }
                let streamer = self.prefs.streamer_mode;
                let open = match &server_id {
                    Some(server) => Nav::Server { key: instance.clone(), server: server.clone() },
                    None => Nav::Home { dm: Some((instance.clone(), channel_id.clone())) },
                };
                let body = if streamer && server_id.is_none() {
                    "New private message".to_owned()
                } else if mention && server_id.is_some() {
                    format!("Mentioned you: {}", crate::ui::notify::plain(&body))
                } else {
                    crate::ui::notify::plain(&body)
                };
                // In the background, the system's own; in front, a card in the corner.
                if !window.is_window_active() {
                    crate::ui::notify::show(
                        title,
                        body,
                        crate::ui::notify::Clicked { instance, server: server_id, channel: channel_id },
                    );
                    return;
                }
                self.toast(
                    if server_id.is_none() {
                        "lock"
                    } else if mention {
                        "at-sign"
                    } else {
                        "message-circle"
                    },
                    title,
                    body,
                    Some(open),
                    server_id.map(|_| channel_id),
                    cx,
                );
            }
            Notice::Removed { server } => {
                self.toast("door-open", "You're no longer in a server".into(), server, None, None, cx);
            }
            Notice::SignedOut { instance } => {
                let name =
                    self.core.shared.read(|s| s.instance(&instance).map(|i| i.name())).unwrap_or(instance.clone());
                self.toast(
                    "log-out",
                    format!("Signed out of {name}"),
                    "Your session ended. Sign in again to keep chatting.".into(),
                    Some(Nav::Instance { key: instance }),
                    None,
                    cx,
                );
            }
        }
    }

    pub fn toast(
        &mut self,
        icon: &'static str,
        title: String,
        body: String,
        open: Option<Nav>,
        channel: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_toast;
        self.next_toast += 1;
        self.toasts.push(Toast { id, icon, title, body, open, channel, leaving: false });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(5)).await;
            let _ = this.update(cx, |this, cx| this.dismiss_toast(id, cx));
        })
        .detach();
        cx.notify();
    }

    /// Lets a toast slide away, then drops it.
    pub fn dismiss_toast(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(toast) = self.toasts.iter_mut().find(|t| t.id == id) else { return };
        if toast.leaving {
            return;
        }
        toast.leaving = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(260)).await;
            let _ = this.update(cx, |this, cx| {
                this.toasts.retain(|t| t.id != id);
                cx.notify();
            });
        })
        .detach();
    }

    // ───────────────────────── Going places ─────────────────────────

    /// The channel or conversation open now, if any.
    pub fn target(&self) -> Option<Target> {
        match &self.nav {
            Nav::Server { key, server } => {
                let channel = self.channel_in(key, server)?;
                Some(Target::Channel { key: key.clone(), server: server.clone(), channel })
            }
            Nav::Home { dm: Some((key, conversation)) } => {
                Some(Target::Dm { key: key.clone(), conversation: conversation.clone() })
            }
            _ => None,
        }
    }

    /// The channel open in a server: the last one you opened, or its first text channel.
    pub fn channel_in(&self, key: &str, server: &str) -> Option<String> {
        self.core.shared.read(|s| {
            let channels = s.instance(key)?.channels.get(server)?;
            let last = self.channel_of.get(&format!("{key}|{server}"));
            let text = |c: &&crate::pb::Channel| {
                matches!(
                    crate::pb::ChannelType::try_from(c.r#type),
                    Ok(crate::pb::ChannelType::Text | crate::pb::ChannelType::Announcement)
                )
            };
            last.and_then(|id| channels.iter().filter(text).find(|c| &c.id == id))
                .or_else(|| channels.iter().find(text))
                .map(|c| c.id.clone())
        })
    }

    pub fn navigate(&mut self, nav: Nav, window: &mut Window, cx: &mut Context<Self>) {
        if self.nav == nav {
            return;
        }
        self.nav = nav;
        self.after_move(window, cx);
    }

    pub fn open_channel(
        &mut self,
        key: &str,
        server: &str,
        channel: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.channel_of.insert(format!("{key}|{server}"), channel.to_owned());
        self.nav = Nav::Server { key: key.to_owned(), server: server.to_owned() };
        self.after_move(window, cx);
    }

    /// Keeps the draft, the focus and the message list in step with where you are.
    fn after_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.emoji_open = false;
        self.maybe_welcome(cx);
        let target = self.target();
        let id = target.as_ref().map(Target::id);
        if self.draft_for != id {
            let text = self.composer.read(cx).value().to_string();
            if let Some(old) = self.draft_for.take() {
                if text.trim().is_empty() {
                    self.drafts.remove(&old);
                } else {
                    self.drafts.insert(old, text);
                }
            }
            let draft = id.as_ref().and_then(|id| self.drafts.get(id)).cloned().unwrap_or_default();
            let placeholder = self.placeholder();
            self.composer.update(cx, |state, cx| {
                state.set_value(draft, window, cx);
                state.set_placeholder(placeholder, window, cx);
            });
            self.draft_for = id;
            self.editing = None;
            self.picker = None;
            self.picker_dismissed = None;
            self.picked_roles.clear();
            self.menu = None;
        }
        let focus = target.as_ref().map(|t| match t {
            Target::Channel { key, channel, .. } => Focus { instance: key.clone(), channel: channel.clone() },
            Target::Dm { key, conversation } => Focus { instance: key.clone(), channel: conversation.clone() },
        });
        self.core.set_focus(focus);
        self.ensure_loaded(cx);
        self.sync_list(cx);
        cx.notify();
    }

    fn placeholder(&self) -> String {
        match self.target() {
            Some(Target::Channel { key, server, channel }) => {
                let name = self
                    .core
                    .shared
                    .read(|s| s.instance(&key).and_then(|i| i.channel(&server, &channel)).map(|c| c.name.clone()))
                    .unwrap_or_default();
                format!("Message #{name}")
            }
            Some(Target::Dm { key, conversation }) => {
                let name = self.core.shared.read(|s| {
                    let i = s.instance(&key)?;
                    let me = i.me.as_ref()?.id.clone();
                    let c = i.dms.conversations.iter().find(|c| c.id == conversation)?;
                    c.users.iter().find(|u| u.id != me).map(crate::core::store::user_name)
                });
                format!("Message {} privately", name.unwrap_or_else(|| "them".into()))
            }
            None => String::new(),
        }
    }

    /// Fetches what the open channel needs, once.
    fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        match self.target() {
            Some(Target::Channel { key, server, channel }) => {
                let loaded =
                    self.core.shared.read(|s| s.instance(&key).is_some_and(|i| i.messages.contains_key(&channel)));
                let id = format!("{key}|{channel}");
                if !loaded && self.requested.insert(id.clone()) {
                    let core = self.core.clone();
                    self.run(
                        cx,
                        async move { core.load_messages(&key, &server, &channel, false).await },
                        move |this, result, cx| {
                            if result.is_err() {
                                this.requested.remove(&id);
                            }
                            cx.notify();
                        },
                    );
                }
            }
            Some(Target::Dm { key, conversation }) => {
                let ready = self.core.shared.read(|s| s.instance(&key).is_some_and(|i| i.dms.status.is_ready()));
                let id = format!("{key}|{conversation}");
                if ready && self.prepared.insert(id) {
                    let core = self.core.clone();
                    self.run(cx, async move { core.prepare_conversation(&key, &conversation).await }, |_, _, cx| {
                        cx.notify()
                    });
                }
            }
            None => {}
        }
    }

    /// Runs `future` on the core and hands its answer back to the window.
    pub fn run<T: Send + 'static>(
        &self,
        cx: &mut Context<Self>,
        future: impl Future<Output = T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        let rx = self.core.spawn(future);
        cx.spawn(async move |this, cx| {
            if let Ok(value) = rx.await {
                let _ = this.update(cx, |this, cx| done(this, value, cx));
            }
        })
        .detach();
    }

    // ───────────────────────── Sending ─────────────────────────

    pub(crate) fn send_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.target() else { return };
        let text = self.composer.read(cx).value().trim().to_owned();
        if text.is_empty() {
            return;
        }
        if let Target::Dm { key, conversation } = &target {
            let blocked =
                self.core.shared.read(|s| s.instance(key).and_then(|i| i.dms.blocked.get(conversation).cloned()));
            if blocked.is_some() {
                return;
            }
        }
        self.composer.update(cx, |state, cx| state.set_value("", window, cx));
        if let Some(id) = &self.draft_for {
            self.drafts.remove(id);
        }
        self.picker = None;
        let core = self.core.clone();
        match target {
            Target::Channel { key, server, channel } => {
                let text = self.encode_mentions(&text);
                self.run(cx, async move { core.send_message(&key, &server, &channel, &text).await }, |_, _, cx| {
                    cx.notify()
                });
            }
            Target::Dm { key, conversation } => {
                self.run(
                    cx,
                    async move { core.send_dm(&key, &conversation, Content::Text { text, reply_to: 0 }).await },
                    |this, result, cx| {
                        if let Err(err) = result {
                            this.toast("circle-alert", "Couldn't send that".into(), err.0, None, None, cx);
                        }
                    },
                );
            }
        }
    }

    pub fn retry(&mut self, nonce: u64, cx: &mut Context<Self>) {
        let Some(Target::Channel { key, server, channel }) = self.target() else { return };
        let Some(content) = self.core.shared.read(|s| {
            s.instance(&key)?.pending.get(&channel)?.iter().find(|p| p.nonce == nonce).map(|p| p.content.clone())
        }) else {
            return;
        };
        self.core.dismiss_pending(&key, &channel, nonce);
        let core = self.core.clone();
        self.run(cx, async move { core.send_message(&key, &server, &channel, &content).await }, |_, _, cx| cx.notify());
    }

    pub fn delete(&mut self, id: String, cx: &mut Context<Self>) {
        let core = self.core.clone();
        match self.target() {
            Some(Target::Channel { key, server, channel }) => {
                self.run(
                    cx,
                    async move { core.delete_message(&key, &server, &channel, &id).await },
                    |this, result, cx| {
                        if let Err(err) = result {
                            this.toast("circle-alert", "Couldn't delete that".into(), err.message, None, None, cx);
                        }
                    },
                );
            }
            Some(Target::Dm { key, conversation }) => {
                let Ok(seq) = id.parse::<i64>() else { return };
                self.run(cx, async move { core.delete_dm(&key, &conversation, seq).await }, |this, result, cx| {
                    if let Err(err) = result {
                        this.toast("circle-alert", "Couldn't delete that".into(), err.0, None, None, cx);
                    }
                });
            }
            None => {}
        }
    }

    pub fn load_older(&mut self, cx: &mut Context<Self>) {
        let Some(Target::Channel { key, server, channel }) = self.target() else { return };
        let core = self.core.clone();
        self.run(cx, async move { core.load_messages(&key, &server, &channel, true).await }, |_, _, cx| cx.notify());
    }

    pub fn message_person(&mut self, key: String, user_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let core = self.core.clone();
        let rx = core.spawn({
            let core = core.clone();
            let key = key.clone();
            async move { core.open_conversation(&key, &user_id).await }
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(id) => this.navigate(Nav::Home { dm: Some((key, id)) }, window, cx),
                Err(err) => this.toast("circle-alert", "Couldn't open a conversation".into(), err.0, None, None, cx),
            });
        })
        .detach();
    }

    // ───────────────────────── Overlays ─────────────────────────

    pub fn open_connect(&mut self, can_cancel: bool, window: &mut Window, cx: &mut Context<Self>) {
        let core = self.core.clone();
        let view = cx.new(|cx| ConnectView::new(core, can_cancel, window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &view,
            window,
            |this: &mut Self, _, event: &ConnectEvent, window, cx| {
                match event {
                    ConnectEvent::Done { key } => {
                        this.connect = None;
                        this.navigate(Nav::Instance { key: key.clone() }, window, cx);
                    }
                    ConnectEvent::Cancel => this.connect = None,
                }
                cx.notify();
            },
        ));
        self.connect = Some(view);
        cx.notify();
    }

    /// Signing in again to an instance whose session ended.
    pub fn reconnect(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.core.shared.read(|s| s.instance(key).map(|i| i.url.clone()));
        self.open_connect(true, window, cx);
        if let (Some(view), Some(url)) = (&self.connect, url) {
            view.update(cx, |view, cx| view.start_at(&url, window, cx));
        }
    }

    pub fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        crate::core::reports::used("settings.open");
        let core = self.core.clone();
        let view = cx.new(|cx| SettingsView::new(core, window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &view,
            window,
            |this: &mut Self, _, event: &SettingsEvent, window, cx| {
                match event {
                    SettingsEvent::Close => this.settings = None,
                    SettingsEvent::Prefs => {
                        let before = pal(cx).background;
                        this.prefs = this.core.prefs();
                        theme::apply(&this.prefs, window.appearance(), cx);
                        if pal(cx).background != before && !cx.reduce_motion() {
                            this.theme_fade = Some((before, Instant::now()));
                        }
                        window.refresh();
                    }
                    SettingsEvent::SignIn { key } => {
                        this.settings = None;
                        this.reconnect(key, window, cx);
                    }
                    SettingsEvent::AddInstance => {
                        this.settings = None;
                        this.open_connect(true, window, cx);
                    }
                }
                cx.notify();
            },
        ));
        self.settings = Some(view);
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// A server's settings, over everything but dialogs.
    pub fn open_server_settings(&mut self, key: &str, server: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        let core = self.core.clone();
        let view = cx.new(|cx| ServerSettingsView::new(core, key.to_owned(), server.to_owned(), window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &view,
            window,
            |this: &mut Self, view, event: &ServerSettingsEvent, window, cx| {
                match event {
                    ServerSettingsEvent::Close => this.server_settings = None,
                    ServerSettingsEvent::Moderate { user_id, action } => {
                        let (key, server) = {
                            let v = view.read(cx);
                            (v.key.clone(), v.server.clone())
                        };
                        let dialog = Dialog::Moderate { key, server, user_id: user_id.clone(), action: *action };
                        this.open_dialog(dialog, window, cx);
                    }
                    ServerSettingsEvent::CreateChannel { parent } => {
                        let (key, server) = {
                            let v = view.read(cx);
                            (v.key.clone(), v.server.clone())
                        };
                        let dialog = Dialog::CreateChannel { key, server, parent: parent.clone(), category: false };
                        this.open_dialog(dialog, window, cx);
                    }
                }
                cx.notify();
            },
        ));
        self.server_settings = Some(view);
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub fn open_dialog(&mut self, dialog: Dialog, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = match &dialog {
            Dialog::CreateServer { .. } => "My cozy server",
            Dialog::JoinInvite { .. } => "https://fuwa.chat/invite/hTKzmak",
            Dialog::CreateChannel { category: false, .. } => "new-channel",
            Dialog::CreateChannel { category: true, .. } => "Cozy corner",
            Dialog::Moderate { .. } => "Why? It goes in the audit log",
            _ => "",
        };
        self.menu = None;
        self.profile = None;
        self.rules = None;
        match &dialog {
            Dialog::Profile { key, user_id, .. } => {
                let (core, key, user) = (self.core.clone(), key.clone(), user_id.clone());
                self.run(cx, async move { core.profile(&key, &user).await }, |this, result, cx| {
                    if let Ok(profile) = result {
                        this.profile = Some(profile);
                    }
                    cx.notify();
                });
            }
            Dialog::Welcome { key, server } => {
                self.welcome = None;
                let (core, key, server) = (self.core.clone(), key.clone(), server.clone());
                self.run(cx, async move { core.welcome_screen(&key, &server).await }, |this, result, cx| {
                    match result {
                        Ok(screen) => this.welcome = Some(screen),
                        Err(err) => this.dialog_error = Some(err.message),
                    }
                    cx.notify();
                });
            }
            Dialog::Rules { key, server } => {
                let (core, key, server) = (self.core.clone(), key.clone(), server.clone());
                self.run(cx, async move { core.server_rules(&key, &server).await }, |this, result, cx| {
                    match result {
                        Ok(rules) => this.rules = Some(rules),
                        Err(err) => this.dialog_error = Some(err.message),
                    }
                    cx.notify();
                });
            }
            _ => {}
        }
        self.dialog_input.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.set_placeholder(placeholder, window, cx);
        });
        self.dialog_busy = false;
        self.dialog_error = None;
        self.copied = None;
        if let Dialog::Invite { link: None, server } = &dialog
            && let Nav::Server { key, .. } = &self.nav
        {
            let (core, key, server) = (self.core.clone(), key.clone(), server.clone());
            self.run(cx, async move { core.create_invite(&key, &server).await }, |this, result, cx| {
                if let Some(Dialog::Invite { link, .. }) = &mut this.dialog {
                    match result {
                        Ok(made) => *link = Some(made),
                        Err(err) => this.dialog_error = Some(err.message),
                    }
                }
                cx.notify();
            });
        }
        self.dialog = Some(dialog);
        if matches!(
            self.dialog,
            Some(
                Dialog::CreateServer { .. }
                    | Dialog::JoinInvite { .. }
                    | Dialog::CreateChannel { .. }
                    | Dialog::Moderate { .. }
            )
        ) {
            self.dialog_input.update(cx, |s, cx| s.focus(window, cx));
        } else {
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    /// Greets a new member of the open server with its welcome screen, once,
    /// after any rules. People who can change it never get it unasked.
    fn maybe_welcome(&mut self, cx: &mut Context<Self>) {
        let Nav::Server { key, server } = self.nav.clone() else { return };
        let seen = format!("{key}/{server}");
        if self.dialog.is_some() || self.welcome_checked.contains(&seen) {
            return;
        }
        const NEW_FOR: i64 = 7 * 86_400_000;
        let ready = self.core.shared.read(|s| {
            let i = s.instance(&key)?;
            let has = i.server(&server)?.has_welcome_screen;
            let me = i.my_member(&server)?;
            let joined = me.joined_at.as_ref().map(|t| t.seconds * 1000).unwrap_or_default();
            Some(
                has && !me.pending
                    && !i.access(&server).has(crate::pb::Permission::ManageServer)
                    && now_ms() - joined < NEW_FOR,
            )
        });
        // Not loaded yet: look again on the next change.
        let Some(newcomer) = ready else { return };
        self.welcome_checked.insert(seen.clone());
        if !newcomer || self.prefs.welcomed.contains(&seen) {
            return;
        }
        let core = self.core.clone();
        let (k, sid) = (key.clone(), server.clone());
        self.run(cx, async move { core.welcome_screen(&k, &sid).await }, move |this, result, cx| {
            let Ok(screen) = result else { return };
            this.core.set_prefs(|p| {
                p.welcomed.insert(seen.clone());
            });
            this.prefs = this.core.prefs();
            if screen.enabled
                && this.dialog.is_none()
                && this.nav == (Nav::Server { key: key.clone(), server: server.clone() })
            {
                this.dialog = Some(Dialog::Welcome { key: key.clone(), server: server.clone() });
                this.dialog_error = None;
                this.welcome = Some(screen);
                cx.notify();
            }
        });
    }

    pub fn close_dialog(&mut self, cx: &mut Context<Self>) {
        self.dialog = None;
        cx.notify();
    }

    pub fn confirm_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.clone() else { return };
        if self.dialog_busy {
            return;
        }
        let value = self.dialog_input.read(cx).value().trim().to_owned();
        let core = self.core.clone();
        match dialog {
            Dialog::CreateServer { key } => {
                if value.is_empty() {
                    self.dialog_error = Some("Give it a name.".into());
                    cx.notify();
                    return;
                }
                self.dialog_busy = true;
                let rx = core.spawn({
                    let (core, key) = (core.clone(), key.clone());
                    async move { core.create_server(&key, &value).await }
                });
                self.after_dialog(rx, key, window, cx);
            }
            Dialog::JoinInvite { key } => {
                self.dialog_busy = true;
                cx.notify();
                // A server that asks for its provider's sign-in says so before
                // anyone is sent there; any other joins straight away.
                let found = core.spawn({
                    let (core, key) = (core.clone(), key.clone());
                    async move {
                        let (code, server) = core.open_invite(&key, &value).await?;
                        if server.sso_required {
                            return Ok(Err((code, server)));
                        }
                        core.join_with_invite(&key, &server.id, &code).await.map(Ok)
                    }
                });
                cx.spawn_in(window, async move |this, cx| {
                    let Ok(result) = found.await else { return };
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.dialog_busy = false;
                        match result {
                            Ok(Ok(server)) => {
                                this.dialog = None;
                                this.navigate(Nav::Server { key, server: server.id }, window, cx);
                            }
                            Ok(Err((code, server))) => {
                                this.dialog_error = None;
                                this.dialog = Some(Dialog::SsoJoin {
                                    key,
                                    server: server.id,
                                    name: server.name,
                                    provider: server.sso_name,
                                    host: server.sso_host,
                                    code,
                                });
                            }
                            Err(err) => this.dialog_error = Some(err.message),
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            Dialog::SsoJoin { key, server, code, .. } => {
                self.dialog_busy = true;
                let rx = core.spawn({
                    let (core, key) = (core.clone(), key.clone());
                    async move {
                        // Members signing in again come back with their
                        // membership; anyone else joins once they have.
                        if core
                            .server_sso(&key, &server, Some(code.clone()), crate::ui::open_in_browser)
                            .await?
                            .is_none()
                        {
                            return core.join_with_invite(&key, &server, &code).await;
                        }
                        core.shared.read(|s| s.instance(&key).and_then(|i| i.server(&server).cloned())).ok_or_else(
                            || {
                                crate::core::api::Problem::new(
                                    tonic::Code::NotFound,
                                    "That server isn't here any more.",
                                )
                            },
                        )
                    }
                });
                self.after_dialog(rx, key, window, cx);
            }
            Dialog::LeaveServer { key, server } => {
                self.dialog_busy = true;
                self.run(cx, async move { core.leave_server(&key, &server).await }, |this, result, cx| {
                    this.dialog_busy = false;
                    match result {
                        Ok(()) => this.dialog = None,
                        Err(err) => this.dialog_error = Some(err.message),
                    }
                    cx.notify();
                });
            }
            Dialog::Safety { key, conversation } => {
                let safety =
                    core.shared.read(|s| s.instance(&key).and_then(|i| i.dms.safety.get(&conversation).cloned()));
                if let Some(safety) = safety {
                    self.run(
                        cx,
                        async move { core.verify_conversation(&key, &conversation, &safety).await },
                        |this, _, cx| {
                            this.dialog = None;
                            this.toast(
                                "shield-check",
                                "Marked as verified".into(),
                                "If their safety number ever changes, you'll see it here.".into(),
                                None,
                                None,
                                cx,
                            );
                        },
                    );
                }
            }
            Dialog::Invite { link, .. } => {
                if let Some(link) = link {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(link));
                    self.copied = Some(Instant::now());
                    cx.notify();
                }
            }
            Dialog::Welcome { .. } => self.close_dialog(cx),
            Dialog::Moderate { key, server, user_id, action } => {
                let reason: String = value.chars().take(512).collect();
                self.moderate(key, server, user_id, action, reason, cx);
            }
            Dialog::Profile { key, user_id, .. } => {
                self.dialog = None;
                self.message_person(key, user_id, window, cx);
            }
            Dialog::CreateChannel { key, server, parent, category } => {
                if value.is_empty() {
                    self.dialog_error = Some("Give it a name.".into());
                    cx.notify();
                    return;
                }
                self.dialog_busy = true;
                let kind = if category { crate::pb::ChannelType::Category } else { crate::pb::ChannelType::Text };
                let rx = core.spawn({
                    let (core, key, server) = (core.clone(), key.clone(), server.clone());
                    async move { core.create_channel(&key, &server, &value, kind, &parent).await }
                });
                cx.spawn_in(window, async move |this, cx| {
                    let Ok(result) = rx.await else { return };
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.dialog_busy = false;
                        match result {
                            Ok(channel) => {
                                this.dialog = None;
                                // From server settings, stay there: the new channel shows in its list.
                                if !category && this.server_settings.is_none() {
                                    this.open_channel(&key, &server, &channel.id, window, cx);
                                }
                            }
                            Err(err) => this.dialog_error = Some(err.message),
                        }
                        cx.notify();
                    });
                })
                .detach();
                cx.notify();
            }
            Dialog::Rules { key, server } => {
                self.dialog_busy = true;
                self.run(cx, async move { core.agree_to_rules(&key, &server).await }, |this, result, cx| {
                    this.dialog_busy = false;
                    match result {
                        Ok(()) => {
                            this.dialog = None;
                            this.toast(
                                "party-popper",
                                "Welcome in!".into(),
                                "You agreed to the rules. Say hi!".into(),
                                None,
                                None,
                                cx,
                            );
                        }
                        Err(err) => this.dialog_error = Some(err.message),
                    }
                    cx.notify();
                });
            }
        }
    }

    /// Does what the moderation dialog asks, then says how it went.
    pub fn moderate(
        &mut self,
        key: String,
        server: String,
        user_id: String,
        action: crate::core::moderation::Action,
        reason: String,
        cx: &mut Context<Self>,
    ) {
        use crate::core::moderation::Action;
        let name = self.core.shared.read(|s| {
            s.instance(&key).map(|i| i.display_name(Some(&server), &user_id)).unwrap_or_else(|| "Them".into())
        });
        self.dialog_busy = true;
        self.dialog_error = None;
        let core = self.core.clone();
        let (k, sid, uid) = (key.clone(), server.clone(), user_id.clone());
        self.run(cx, async move { core.moderate(&k, &sid, &uid, action, &reason).await }, move |this, result, cx| {
            this.dialog_busy = false;
            match result {
                Ok(deleted) => {
                    this.dialog = None;
                    let (glyph, title) = match action {
                        Action::TimeOut(0) => ("message-circle", format!("{name} can talk again")),
                        Action::TimeOut(s) => {
                            ("hourglass", format!("{name} is timed out for {}", crate::ui::moderate::duration(s)))
                        }
                        Action::Kick => ("door-open", format!("Kicked {name}")),
                        Action::Ban(_) if deleted > 0 => (
                            "gavel",
                            format!(
                                "Banned {name} and deleted {deleted} {}",
                                if deleted == 1 { "message" } else { "messages" }
                            ),
                        ),
                        Action::Ban(_) => ("gavel", format!("Banned {name}")),
                    };
                    this.toast(glyph, title, "It's in the server's audit log.".into(), None, None, cx);
                }
                Err(err) => this.dialog_error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    // ───────────────────────── Single sign-on ─────────────────────────

    /// The server, while you're kept out of its channels until you sign in
    /// through its identity provider.
    pub(crate) fn sso_locked(&self, key: &str, server: &str) -> Option<crate::pb::Server> {
        self.core.shared.read(|s| {
            let i = s.instance(key)?;
            let found = i.server(server)?;
            crate::core::sso::locked(found, i.my_member(server), crate::core::dms::now_ms()).then(|| found.clone())
        })
    }

    /// Signs in through a server's provider in the browser, so its channels
    /// come back; they arrive through the event stream.
    pub(crate) fn sign_in_server(&mut self, key: String, server: String, cx: &mut Context<Self>) {
        self.sso_try += 1;
        let attempt = self.sso_try;
        self.sso_waiting = Some(server.clone());
        cx.notify();
        let (name, provider) = self
            .core
            .shared
            .read(|s| s.instance(&key).and_then(|i| i.server(&server)).map(|s| (s.name.clone(), s.sso_name.clone())))
            .unwrap_or_default();
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.server_sso(&key, &server, None, crate::ui::open_in_browser).await },
            move |this, result, cx| {
                if this.sso_try != attempt {
                    return;
                }
                this.sso_waiting = None;
                match result {
                    Ok(_) => this.toast(
                        "shield-check",
                        format!("Signed in with {}", crate::ui::overlay::provider_name(&provider)),
                        format!("Welcome back to {name}."),
                        None,
                        None,
                        cx,
                    ),
                    Err(err) => this.toast("lock-keyhole", "Couldn't sign you in".into(), err.message, None, None, cx),
                }
                cx.notify();
            },
        );
    }

    fn after_dialog(
        &mut self,
        rx: futures::channel::oneshot::Receiver<Result<crate::pb::Server, crate::core::api::Problem>>,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.dialog_busy = false;
                match result {
                    Ok(server) => {
                        this.dialog = None;
                        this.navigate(Nav::Server { key, server: server.id }, window, cx);
                    }
                    Err(err) => this.dialog_error = Some(err.message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn close_overlay(&mut self, _: &CloseOverlay, window: &mut Window, cx: &mut Context<Self>) {
        // Bindings run before the window's key handler, so a shortcut being
        // changed gets its Escape (stop listening) from here.
        if self.forward_to_recording("escape", cx) {
            return;
        }
        if self.switcher.is_some() {
            self.close_switcher(window, cx);
        } else if self.sheet_open {
            self.sheet_open = false;
        } else if self.menu.is_some() {
            self.menu = None;
        } else if self.dialog.is_some() {
            self.dialog = None;
        } else if self.server_settings.is_some() {
            self.server_settings = None;
        } else if self.settings.is_some() {
            self.settings = None;
        } else if let Some(connect) = &self.connect {
            if connect.read(cx).can_cancel {
                self.connect = None;
            }
        } else {
            self.focus.focus(window, cx);
        }
        cx.notify();
    }
}

/// How long a new theme takes to wash in.
const THEME_FADE: Duration = Duration::from_millis(420);

impl Render for FuwaApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let began = crate::ui::perf::frame();
        let root = self.render_root(window, cx);
        crate::ui::perf::measured(root, began)
    }
}

impl FuwaApp {
    fn render_root(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let p = pal(cx);
        window.set_rem_size(px(16.0 * self.prefs.text_scale.clamp(0.8, 1.4)));
        // When something that takes focus closes (a dialog, a page, the
        // switcher), the window takes focus back, or no shortcut would reach it.
        let covers = [
            self.dialog.is_some(),
            self.connect.is_some(),
            self.settings.is_some(),
            self.server_settings.is_some(),
            self.switcher.is_some(),
        ]
        .into_iter()
        .filter(|c| *c)
        .count();
        if covers < self.covers || window.focused(cx).is_none() {
            self.focus.focus(window, cx);
        }
        self.covers = covers;
        let empty = self.core.shared.read(|s| s.order.is_empty());
        // Server settings cover the whole window with a solid page.
        crate::ui::effects::hold(self.server_settings.is_some());
        let behind = crate::ui::backdrop::layers(&theme::backdrop(cx), &p, window, cx);

        let base = div()
            .id("fuwa")
            .key_context("Fuwa")
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::on_key))
            .on_action(cx.listener(Self::close_overlay))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)))
            .on_action(cx.listener(|this, _: &AddInstance, window, cx| {
                if !this.forward_to_recording("secondary-shift-n", cx) {
                    this.open_connect(true, window, cx)
                }
            }))
            .size_full()
            .relative()
            .overflow_hidden()
            .font_family(FONT)
            .bg(p.background)
            .text_color(p.foreground)
            .when_some(behind, |el, behind| el.child(behind));

        if empty && let Some(connect) = &self.connect {
            return base.child(connect.clone()).into_any_element();
        }

        base.child(
            div()
                .size_full()
                .flex()
                .child(self.render_rail(window, cx))
                .child(self.render_sidebar(window, cx))
                .child(self.render_main(window, cx)),
        )
        .when_some(self.connect.clone(), |el, connect| {
            el.child(crate::ui::overlay::scrim("connect-scrim", &p).child(connect))
        })
        .when_some(
            match self.menu.clone() {
                Some(Menu::Server { key, server }) => Some(self.server_bell_menu(&key, &server, cx)),
                _ => None,
            },
            |el, menu| el.child(menu),
        )
        .when_some(self.settings.clone(), |el, settings| el.child(settings))
        .when_some(self.server_settings.clone(), |el, settings| {
            // Drawn again only when it changes, not on every frame of the window.
            el.child(
                gpui_kit::AnyView::from(settings)
                    .cached(gpui_kit::StyleRefinement::default().absolute().top_0().left_0().size_full()),
            )
        })
        .when_some(self.render_dialog(window, cx), |el, d| el.child(d))
        .when_some(self.render_sheet(cx), |el, sheet| el.child(sheet))
        .when_some(self.render_switcher(cx), |el, switcher| el.child(switcher))
        .child(self.render_toasts(window, cx))
        .when_some(self.theme_fade.filter(|(_, at)| at.elapsed() < THEME_FADE), |el, (color, at)| {
            // A new theme washes in: the old page color fades away over it.
            el.child(div().absolute().inset_0().bg(color).with_animation(
                gpui_kit::SharedString::from(format!("theme-fade|{at:?}")),
                gpui_kit::Animation::new(THEME_FADE).with_easing(gpui_kit::ease_out_quint()),
                |el, t| el.opacity(0.85 * (1.0 - t)),
            ))
        })
        .into_any_element()
    }
}
