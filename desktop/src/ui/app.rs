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
use crate::ui::instance_settings::{InstanceSettingsEvent, InstanceSettingsView};
use crate::ui::server_settings::{ServerSettingsEvent, ServerSettingsView};
use crate::ui::settings::{SettingsEvent, SettingsView};
use crate::ui::theme::{self, FONT};
use crate::ui::widgets::pal;

actions!(fuwa, [CloseOverlay, OpenSettings, AddInstance, ComposerEmoji, ComposerTimestamp]);

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
    /// Your friends on an instance, under Home.
    Friends { key: String },
    /// An instance's own page: its connection, and making or joining servers there.
    Instance { key: String },
    /// A server, with the channel last opened in it.
    Server { key: String, server: String },
}

/// Where the composer sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Channel {
        key: String,
        server: String,
        channel: String,
    },
    /// A server's end-to-end encrypted channel: read and written through this device's encryption.
    Secure {
        key: String,
        server: String,
        channel: String,
    },
    Dm {
        key: String,
        conversation: String,
    },
}

impl Target {
    pub fn id(&self) -> String {
        match self {
            Target::Channel { key, channel, .. } | Target::Secure { key, channel, .. } => format!("c|{key}|{channel}"),
            Target::Dm { key, conversation } => format!("d|{key}|{conversation}"),
        }
    }

    pub fn key(&self) -> &str {
        match self {
            Target::Channel { key, .. } | Target::Secure { key, .. } | Target::Dm { key, .. } => key,
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
    /// A new channel (or category) of `kind`, under `parent` (or at the top).
    CreateChannel {
        key: String,
        server: String,
        parent: String,
        kind: crate::pb::ChannelType,
    },
    /// How a secure channel is kept private, and who can read it.
    Secure {
        key: String,
        server: String,
        channel: String,
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
    /// A server's first steps for new members (ui/onboarding.rs).
    Onboarding {
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
    /// Making a poll in a channel (the editor's state is in `polls`).
    Poll {
        key: String,
        server: String,
        channel: String,
    },
    /// A picture from a message, opened large.
    Picture {
        key: String,
        url: String,
        name: String,
        width: i32,
        height: i32,
        /// The file's size in bytes, as its message says.
        bytes: i64,
    },
    /// Who voted for each answer of a public poll.
    PollVoters {
        key: String,
        server: String,
        channel: String,
        message: String,
    },
    /// Time out, kick or ban someone, with a reason for the audit log.
    Moderate {
        key: String,
        server: String,
        user_id: String,
        action: crate::core::moderation::Action,
    },
    /// A game or app reporting to Discord's local RPC asks, once, to show
    /// what you're doing (`core::presence`).
    AllowGame {
        key: String,
        name: String,
    },
    /// Applying to a server that lets people in by hand (`ui/join.rs`, the form in `home.apply`).
    Apply {
        key: String,
        server: String,
    },
    /// Where your application to a server stands (`ui/join.rs`).
    Application {
        key: String,
        server: String,
    },
}

/// A small menu hanging under a bell, or over your name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Menu {
    Channel {
        key: String,
        server: String,
        channel: String,
    },
    Server {
        key: String,
        server: String,
    },
    /// Your status on an instance, from your name at the bottom of the sidebar.
    Status {
        key: String,
    },
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
    /// A thread reply's: the thread to open in that channel.
    pub thread: Option<String>,
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
    /// Starting a secure channel's encryption over, from its dialog.
    pub secure_reset: crate::ui::secure::Reset,
    /// Turning a secure channel's history sharing on or off.
    pub secure_saving: bool,
    pub hovered: Option<String>,
    /// A redraw is coming for live tiles' clocks (`live_tiles.rs`).
    pub(crate) tiles_ticking: bool,
    /// The rail's drag and folder dialog (`rail.rs`).
    pub(crate) rail: crate::ui::rail::RailState,
    /// "Streamer mode is on" hidden for this run (`banners.rs`).
    pub(crate) streamer_banner_hidden: bool,
    /// Categories folded away in the sidebar, by id (for this run, as on the web).
    pub collapsed: std::collections::HashSet<String>,
    pub members_open: bool,
    /// The open server's member list, a view of its own.
    pub members_view: Option<Entity<crate::ui::members::MembersView>>,
    pub connect: Option<Entity<ConnectView>>,
    pub settings: Option<Entity<SettingsView>>,
    pub server_settings: Option<Entity<ServerSettingsView>>,
    pub instance_settings: Option<Entity<InstanceSettingsView>>,
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
    /// The message being edited is in the open thread's panel, not the channel (a reply
    /// also sent to the channel shows in both).
    pub edit_in_thread: bool,
    /// The message whose author (from another server) we're asking whether to keep out.
    pub keeping_out: Option<String>,
    /// What the message list keeps between frames: a delete being asked, a copy, waves.
    pub msg_ui: crate::ui::chat::MsgUi,
    /// Votes on their way, peeks, polls being ended, and the poll editor.
    pub polls: crate::ui::polls::PollState,
    /// Voice messages being recorded, sent and played.
    pub voice: crate::ui::voice_notes::VoiceState,
    /// Files picked to go with the next message.
    pub files: crate::ui::attachments::Files,
    /// The composer's countdowns, shakes and voice problems.
    pub composing: crate::ui::composer::Composing,
    /// Searching the server on screen: the header's field and the results beside the chat.
    pub search: crate::ui::search::Search,
    pub threads: crate::ui::threads::Threads,
    pub friends: crate::ui::friends::Friends,
    /// Profile cards and the moderation dialog.
    pub people: crate::ui::profile_card::People,
    /// The instance page: Browse, invites, applying and making servers (`ui::instance_home`, `ui::join`).
    pub home: crate::ui::instance_home::Home,
    pub onboarding: crate::ui::onboarding::Onboarding,
    /// The timestamp picker, while it's open, and the style picked last.
    pub time_picker: Option<crate::ui::timestamps::TimePicker>,
    pub time_style: crate::core::timestamps::Style,
    /// A redraw is coming for relative timestamps.
    pub time_ticking: bool,
    pub edit_box: Entity<TextareaState>,
    pub picker: Option<Picker>,
    /// Agents' commands: the "/" list, a picked command's options, buttons being pressed.
    pub commands: crate::ui::commands::Commands,
    /// Where the @ list was closed with Escape, so it stays closed for that mention.
    pub picker_dismissed: Option<usize>,
    /// Roles picked from the @ list by name, sent as their tokens.
    pub picked_roles: Vec<(String, String)>,
    pub menu: Option<Menu>,
    /// The open right-click menu.
    pub(crate) context: Option<crate::ui::context_menu::ContextMenu>,
    /// What the pointer is over that has a right-click menu, for Shift+F10 and the Menu key.
    pub(crate) hover_target: Option<crate::ui::context_menu::MenuOf>,
    /// A picture right-clicked in a message, as (message, which of its files), for its menu.
    pub right_picture: Option<(String, usize)>,
    /// The emoji picker over the composer, and its search box.
    pub emoji_open: bool,
    pub emoji_query: Entity<InputState>,
    pub emoji: crate::ui::emoji_picker::EmojiPicker,
    /// When the picker opened, so its emoji ripple in only then.
    pub emoji_born: Option<std::time::Instant>,
    /// The GIF picker over the composer, and what it knows of each instance's GIFs.
    pub gifs: crate::ui::gifs::Gifs,
    /// The open channel's pinned messages, beside its messages (`ui::pins`).
    pub pins: Option<crate::ui::pins::PinsPanel>,
    /// The profile the open card shows, once it arrives.
    pub profile: Option<crate::pb::Profile>,
    /// The activity link the open card asks about following (`ui::presence`).
    pub profile_leaving: Option<String>,
    /// Draws the open card again in a second, while it shows a running timer.
    pub profile_tick: Option<gpui_kit::Task<()>>,
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
    /// Calls: the open voice channel, the card open over one, calls turned down (`ui::call_parts`).
    pub(crate) calls: crate::ui::call_parts::CallsUi,
    /// How many things that take focus were open last frame.
    covers: usize,
    /// The page's color before the theme changed, fading out over the new one.
    theme_fade: Option<(gpui_kit::Rgba, Instant)>,
    _subscriptions: Vec<Subscription>,
}

impl FuwaApp {
    pub fn new(core: Arc<Core>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prefs = core.prefs();
        // The box's right-click menu is the app's own (`compose.rs`), as on the web.
        let composer =
            cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 10).submit_on_enter(true).context_menu(false));
        let edit_box = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 10).submit_on_enter(true));
        let dialog_input = cx.new(|cx| InputState::new(window, cx));
        let emoji_query = cx.new(|cx| InputState::new(window, cx).placeholder("Find an emoji"));
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let search = crate::ui::search::Search::new(window, cx);
        let (threads, thread_subs) = crate::ui::threads::Threads::new(window, cx);
        let (friends, friend_subs) = crate::ui::friends::Friends::new(window, cx);
        let people = crate::ui::profile_card::People::new(window, cx);
        let (home, home_subs) = crate::ui::instance_home::Home::new(window, cx);
        let (gifs, gif_subs) = crate::ui::gifs::Gifs::new(window, cx);
        let (onboarding, onboarding_subs) = crate::ui::onboarding::Onboarding::new(window, cx);
        let mut subscriptions = vec![
            cx.subscribe_in(&composer, window, |this: &mut Self, _, event: &InputEvent, window, cx| {
                match event {
                    // Enter sends unless the Chat setting says Ctrl+Enter (taken in `intercept`).
                    InputEvent::PressEnter { shift: false, secondary: false }
                        if this.core.prefs().send_with == crate::core::config::SendWith::Enter =>
                    {
                        this.send_now(window, cx)
                    }
                    // The send button lights up once there's something to send.
                    InputEvent::Change => {
                        this.update_picker(cx);
                        this.update_commands(cx);
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
            cx.subscribe_in(&search.field, window, |this: &mut Self, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => {
                    this.search.active = None;
                    this.search.hushed = false;
                    cx.notify();
                }
                InputEvent::Focus | InputEvent::Blur => cx.notify(),
                _ => {}
            }),
            cx.subscribe_in(&emoji_query, window, |this: &mut Self, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    // A new search starts at the top, on its best match.
                    this.emoji.active = None;
                    this.emoji.to_top();
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
        subscriptions.extend(thread_subs);
        subscriptions.extend(friend_subs);
        subscriptions.extend(home_subs);
        subscriptions.extend(gif_subs);
        subscriptions.extend(onboarding_subs);
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
                        Some(server) => {
                            this.open_channel(&click.instance, &server, &click.channel, window, cx);
                            if let Some(thread) = click.thread {
                                this.open_thread(thread, window, cx);
                            }
                        }
                        None if click.channel.is_empty() => {
                            this.navigate(Nav::Friends { key: click.instance }, window, cx)
                        }
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
            secure_reset: Default::default(),
            secure_saving: false,
            hovered: None,
            tiles_ticking: false,
            rail: Default::default(),
            streamer_banner_hidden: false,
            collapsed: Default::default(),
            members_open: true,
            members_view: None,
            connect: None,
            settings: None,
            server_settings: None,
            instance_settings: None,
            dialog: None,
            dialog_input,
            dialog_busy: false,
            dialog_error: None,
            toasts: Vec::new(),
            next_toast: 1,
            copied: None,
            focus: cx.focus_handle(),
            editing: None,
            edit_in_thread: false,
            keeping_out: None,
            msg_ui: Default::default(),
            polls: Default::default(),
            voice: Default::default(),
            files: Default::default(),
            composing: Default::default(),
            search,
            threads,
            friends,
            people,
            home,
            onboarding,
            time_picker: None,
            time_style: crate::core::timestamps::Style::Relative,
            time_ticking: false,
            edit_box,
            picker: None,
            commands: Default::default(),
            picker_dismissed: None,
            picked_roles: Vec::new(),
            menu: None,
            context: None,
            hover_target: None,
            right_picture: None,
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
            calls: Default::default(),
            emoji_open: false,
            emoji_query,
            emoji: Default::default(),
            emoji_born: None,
            gifs,
            pins: None,
            profile: None,
            profile_leaving: None,
            profile_tick: None,
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
            Nav::Instance { key } | Nav::Friends { key } => self.core.shared.read(|s| s.instance(key).is_none()),
            // A conversation with someone just blocked goes out of sight too.
            Nav::Home { dm: Some((key, id)) } => self.core.shared.read(|s| {
                s.instance(key).is_none_or(|i| {
                    i.dms.conversations.iter().find(|c| &c.id == id).is_none_or(|c| crate::core::friends::hidden(i, c))
                })
            }),
            Nav::Home { dm: None } => false,
        };
        if gone {
            self.navigate(Nav::Home { dm: None }, window, cx);
        }
        // Home with nothing open is an instance's page, as the web's `/` goes to one.
        if matches!(self.nav, Nav::Home { dm: None })
            && let Some(first) = self.core.shared.read(|s| s.order.first().cloned())
        {
            self.navigate(Nav::Instance { key: first }, window, cx);
        }
        self.maybe_welcome(cx);
        // A game asking to show what you're doing, once nothing else is open.
        if self.dialog.is_none()
            && let Some(program) = self.core.games.asking()
        {
            self.dialog_error = None;
            self.dialog = Some(Dialog::AllowGame { key: program.key, name: program.name });
        }
        // A server's channels arrived after it was opened: open the first.
        if self.target().map(|t| t.id()) != self.draft_for {
            self.after_move(window, cx);
        }
        self.ensure_loaded(cx);
        self.sync_list(cx);
        self.sync_thread(cx);
        cx.notify();
    }

    fn on_notice(&mut self, notice: Notice, window: &mut Window, cx: &mut Context<Self>) {
        match notice {
            Notice::Message { instance, server_id, channel_id, title, body, mention, thread } => {
                // Do not disturb is quiet everywhere, on every app.
                let busy = self.core.shared.read(|s| {
                    s.instance(&instance).is_some_and(|i| i.status() == crate::pb::PresenceStatus::DoNotDisturb)
                });
                // fuwa's little sounds, made on the spot (`core::sounds`).
                if !busy && self.prefs.sounds_on() {
                    let sound =
                        if mention { crate::core::sounds::Sound::Mention } else { crate::core::sounds::Sound::Message };
                    let on = if mention { self.prefs.sounds.mention } else { self.prefs.sounds.message };
                    if on {
                        crate::core::sounds::play(sound, self.prefs.volume, &self.prefs.output_device);
                    }
                }
                if !self.prefs.notifies() || busy {
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
                        crate::ui::notify::Clicked { instance, server: server_id, channel: channel_id, thread },
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
                if let Some(toast) = self.toasts.last_mut() {
                    toast.thread = thread;
                }
            }
            Notice::Removed { server } => {
                self.toast("door-open", "You're no longer in a server".into(), server, None, None, cx);
            }
            Notice::Friend { instance, title } => {
                if !self.prefs.notifies() {
                    return;
                }
                let body = if self.prefs.streamer_mode {
                    "Friends".to_owned()
                } else {
                    self.core.shared.read(|s| s.instance(&instance).map(|i| i.name())).unwrap_or_default()
                };
                if !window.is_window_active() {
                    // An empty channel opens Friends.
                    crate::ui::notify::show(
                        title,
                        body,
                        crate::ui::notify::Clicked { instance, server: None, channel: String::new(), thread: None },
                    );
                    return;
                }
                self.toast("user-plus", title, body, Some(Nav::Friends { key: instance }), None, cx);
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
        self.toasts.push(Toast { id, icon, title, body, open, channel, thread: None, leaving: false });
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
                // A voice channel's page has no messages of its own.
                if self.stage_in(key, server).is_some() {
                    return None;
                }
                let channel = self.channel_in(key, server)?;
                let secure = self.core.shared.read(|s| {
                    s.instance(key)
                        .and_then(|i| i.channel(server, &channel))
                        .is_some_and(|c| c.r#type == crate::pb::ChannelType::Secure as i32)
                });
                let (key, server) = (key.clone(), server.clone());
                Some(if secure {
                    Target::Secure { key, server, channel }
                } else {
                    Target::Channel { key, server, channel }
                })
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
                    Ok(crate::pb::ChannelType::Text
                        | crate::pb::ChannelType::Announcement
                        | crate::pb::ChannelType::Secure)
                )
            };
            last.and_then(|id| channels.iter().filter(text).find(|c| &c.id == id))
                .or_else(|| channels.iter().find(text))
                .map(|c| c.id.clone())
        })
    }

    pub fn navigate(&mut self, nav: Nav, window: &mut Window, cx: &mut Context<Self>) {
        self.home.moved(&nav);
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
        self.calls.stage = None;
        self.nav = Nav::Server { key: key.to_owned(), server: server.to_owned() };
        self.after_move(window, cx);
    }

    /// Keeps the draft, the focus and the message list in step with where you are.
    fn after_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.emoji_open = false;
        self.close_gifs(cx);
        // A recording belongs to where it was started; what's playing stops with the conversation.
        self.discard_recording(cx);
        self.stop_voice();
        self.time_picker = None;
        self.forget_files_elsewhere();
        self.search_after_move(window, cx);
        self.pins_after_move();
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
            // A new place puts the caret in its box, as the web's composer does.
            if id.is_some() {
                self.composer.update(cx, |state, cx| state.focus(window, cx));
            }
            self.draft_for = id;
            self.load_dm_pins_here(cx);
            self.editing = None;
            self.picker = None;
            self.picker_dismissed = None;
            self.picked_roles.clear();
            self.menu = None;
        }
        self.threads_after_move(cx);
        let focus = target.as_ref().map(|t| match t {
            Target::Channel { key, channel, .. } | Target::Secure { key, channel, .. } => Focus {
                instance: key.clone(),
                channel: channel.clone(),
                thread: self.threads.open.as_ref().filter(|o| &o.channel == channel).map(|o| o.id.clone()),
            },
            Target::Dm { key, conversation } => {
                Focus { instance: key.clone(), channel: conversation.clone(), thread: None }
            }
        });
        self.core.set_focus(focus);
        self.ensure_loaded(cx);
        self.sync_list(cx);
        cx.notify();
    }

    fn placeholder(&self) -> String {
        match self.target() {
            Some(Target::Channel { key, server, channel } | Target::Secure { key, server, channel }) => {
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
                    c.users.iter().find(|u| u.id != me).map(|u| u.username.clone())
                });
                let name = name.unwrap_or_else(|| crate::core::i18n::t("dms-calls.dm.view.them"));
                crate::core::i18n::t_with(
                    "dms-calls.dm.view.placeholder",
                    &[("name", crate::core::i18n::Arg::Str(&name))],
                )
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
                // The threads you follow here, so their replies reach you.
                let shared = self.core.shared.read(|s| {
                    s.instance(&key).and_then(|i| i.channel(&server, &channel)).is_some_and(|c| c.shared.is_some())
                });
                if !shared && self.requested.insert(format!("{key}|{server}|followed")) {
                    let (core, key, server) = (self.core.clone(), key.clone(), server.clone());
                    let id = format!("{key}|{server}|followed");
                    self.run(cx, async move { core.load_followed(&key, &server).await }, move |this, result, _| {
                        if result.is_err() {
                            this.requested.remove(&id);
                        }
                    });
                }
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
            Some(Target::Secure { key, server, channel }) => {
                let (ready, write) = self.core.shared.read(|s| {
                    s.instance(&key).map_or((false, false), |i| {
                        (
                            i.dms.status.is_ready(),
                            i.access(&server).has_in(&channel, crate::pb::Permission::SendMessages),
                        )
                    })
                });
                let id = format!("{key}|{channel}|{write}");
                if ready && self.prepared.insert(id) {
                    let core = self.core.clone();
                    self.run(
                        cx,
                        async move { core.prepare_secure_channel(&key, &server, &channel, write).await },
                        |_, _, cx| cx.notify(),
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
        if self.commands.form.is_some() {
            return self.run_picked_command(window, cx);
        }
        let Some(target) = self.target() else { return };
        let text = self.composer.read(cx).value().trim().to_owned();
        // Files picked in a conversation or secure channel go sealed, with what's typed.
        if let Target::Dm { conversation: id, .. } | Target::Secure { channel: id, .. } = &target
            && !crate::ui::sealed_files::picked(id).is_empty()
        {
            if self.send_picked(text, cx) {
                self.composer.update(cx, |state, cx| state.set_value("", window, cx));
            }
            return;
        }
        if text.is_empty() && !self.has_files() {
            return;
        }
        if self.send_held(&text, cx) {
            return;
        }
        let files = match self.take_files() {
            Ok(files) => files,
            Err(why) => {
                self.toast("paperclip", "Not sent yet".into(), why.into(), None, None, cx);
                return;
            }
        };
        if let Target::Dm { key, conversation: id } | Target::Secure { key, channel: id, .. } = &target {
            let conversation = id;
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
                self.run(
                    cx,
                    async move { core.send_message_with(&key, &server, &channel, &text, files).await },
                    |_, _, cx| cx.notify(),
                );
            }
            Target::Dm { key, conversation } | Target::Secure { key, channel: conversation, .. } => {
                self.run(
                    cx,
                    {
                        let conversation = conversation.clone();
                        async move { core.send_dm(&key, &conversation, Content::Text { text, reply_to: 0 }).await }
                    },
                    // What went wrong is said under the box, and on the message, which can be sent again.
                    move |_, result, cx| {
                        crate::ui::dm_view::set_problem(&conversation, result.err().map(|e| e.0));
                        cx.notify();
                    },
                );
            }
        }
    }

    /// Sends again what didn't go, in the channel or with `thread` in that thread.
    pub fn retry(&mut self, nonce: u64, thread: Option<String>, cx: &mut Context<Self>) {
        if matches!(self.target(), Some(Target::Dm { .. } | Target::Secure { .. })) {
            return self.retry_dm(nonce, cx);
        }
        let Some(Target::Channel { key, server, channel }) = self.target() else { return };
        let at = thread.as_deref().map_or_else(|| channel.clone(), crate::core::threads::thread_key);
        let Some((content, files)) = self.core.shared.read(|s| {
            let p = s.instance(&key)?.pending.get(&at)?.iter().find(|p| p.nonce == nonce)?;
            Some((p.content.clone(), p.attachments.clone()))
        }) else {
            return;
        };
        self.core.dismiss_pending(&key, &at, nonce);
        let core = self.core.clone();
        let target = thread.map(|thread_id| crate::core::threads::ThreadTarget { thread_id, also_to_channel: false });
        self.run(
            cx,
            async move { core.send_to(&key, &server, &channel, &content, files, target.as_ref()).await },
            |this, _, cx| {
                this.sync_thread(cx);
                cx.notify()
            },
        );
    }

    /// At a shared channel's home: keeps someone from another server out of it.
    pub fn keep_out(&mut self, user_id: String, name: String, cx: &mut Context<Self>) {
        let Some(Target::Channel { key, server, channel }) = self.target() else { return };
        let channel_name = self
            .core
            .shared
            .read(|s| s.instance(&key).and_then(|i| i.channel(&server, &channel)).map(|c| c.name.clone()))
            .unwrap_or_default();
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.block_from_channel(&key, &server, &channel, &user_id, true).await },
            move |this, result, cx| {
                this.keeping_out = None;
                this.sync_list(cx);
                match result {
                    Ok(()) => this.toast(
                        "user-x",
                        format!("{name} can't see #{channel_name} anymore"),
                        String::new(),
                        None,
                        None,
                        cx,
                    ),
                    Err(err) => {
                        this.toast("circle-alert", "Couldn't keep them out".into(), err.message, None, None, cx)
                    }
                }
            },
        );
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
            Some(Target::Dm { key, conversation } | Target::Secure { key, channel: conversation, .. }) => {
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

    /// Settings, on Friends and privacy for one instance.
    pub fn open_friend_settings(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings(window, cx);
        if let Some(view) = &self.settings {
            let key = key.to_owned();
            view.update(cx, |view, cx| {
                view.page = crate::ui::settings::Page::Friends;
                view.account.key = Some(key);
                cx.notify();
            });
        }
    }

    pub fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        crate::core::reports::used("settings.open");
        let core = self.core.clone();
        let view = cx.new(|cx| SettingsView::new(core, window, cx));
        // The account pages are about the instance on screen.
        let on_screen = match &self.nav {
            Nav::Friends { key } | Nav::Instance { key } | Nav::Server { key, .. } => Some(key.clone()),
            Nav::Home { dm } => dm.as_ref().map(|(key, _)| key.clone()),
        };
        view.update(cx, |v, _| v.account.key = on_screen);
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
                    SettingsEvent::Toast { icon, title } => {
                        this.toast(icon, title.clone(), String::new(), None, None, cx)
                    }
                }
                cx.notify();
            },
        ));
        self.settings = Some(view);
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// An instance's settings, for its admins, over everything but dialogs.
    pub fn open_instance_settings(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        let core = self.core.clone();
        let view = cx.new(|cx| InstanceSettingsView::new(core, key.to_owned(), window, cx));
        let key = key.to_owned();
        self._subscriptions.push(cx.subscribe_in(&view, window, move |this: &mut Self, _, event, window, cx| {
            match event {
                InstanceSettingsEvent::Close => this.instance_settings = None,
                InstanceSettingsEvent::OpenServer(server) => {
                    this.instance_settings = None;
                    this.navigate(Nav::Server { key: key.clone(), server: server.clone() }, window, cx);
                }
                InstanceSettingsEvent::Toast { icon, title } => {
                    this.toast(icon, title.clone(), String::new(), None, None, cx)
                }
            }
            cx.notify();
        }));
        self.instance_settings = Some(view);
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
                    ServerSettingsEvent::Toast { icon, title } => {
                        this.toast(icon, title.clone(), String::new(), None, None, cx)
                    }
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
                        let dialog = Dialog::CreateChannel {
                            key,
                            server,
                            parent: parent.clone(),
                            kind: crate::pb::ChannelType::Text,
                        };
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
            Dialog::CreateChannel { kind, .. } => crate::ui::secure::name_hint(*kind),
            Dialog::Moderate { .. } => "Why? It goes in the audit log",
            _ => "",
        };
        self.menu = None;
        self.profile = None;
        self.profile_leaving = None;
        self.rules = None;
        match &dialog {
            Dialog::Profile { user_id, .. } if !self.card_opening(user_id, window) => return,
            Dialog::Profile { key, user_id, .. } => {
                self.friends.relation = None;
                self.load_relation(key, user_id, cx);
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
        if let Dialog::CreateServer { key } = &dialog {
            self.reset_create_form(key, window, cx);
        }
        self.dialog = Some(dialog);
        if matches!(
            self.dialog,
            Some(Dialog::JoinInvite { .. } | Dialog::CreateChannel { .. } | Dialog::Moderate { .. })
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
            let srv = i.server(&server)?;
            let me = i.my_member(&server)?;
            let joined = me.joined_at.as_ref().map(|t| t.seconds * 1000).unwrap_or_default();
            // Someone who went through the onboarding has seen where to start already.
            let onboarded = srv.has_onboarding && me.onboarded_at.is_some();
            Some((
                crate::core::onboarding::due(i, &server, now_ms())?,
                srv.has_welcome_screen
                    && !onboarded
                    && !me.pending
                    && !i.access(&server).has(crate::pb::Permission::ManageServer)
                    && now_ms() - joined < NEW_FOR,
            ))
        });
        // Not loaded yet: look again on the next change.
        let Some((onboard, newcomer)) = ready else { return };
        self.welcome_checked.insert(seen.clone());
        if self.prefs.welcomed.contains(&seen) {
            return;
        }
        if onboard {
            self.open_onboarding(&key, &server, cx);
            return;
        }
        if !newcomer {
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
        // A game's question closed unanswered waits for the next start.
        if let Some(Dialog::AllowGame { key, .. }) = self.dialog.take() {
            self.core.answer_game(&key, None);
        }
        self.polls.editor = None;
        self.polls.voters = None;
        self.profile_leaving = None;
        cx.notify();
    }

    /// "Don't allow" on a game's question: remembered, like "Allow".
    pub fn refuse_game(&mut self, cx: &mut Context<Self>) {
        if let Some(Dialog::AllowGame { key, .. }) = self.dialog.take() {
            self.core.answer_game(&key, Some(false));
        }
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
            Dialog::CreateServer { .. } => self.create_server_now(window, cx),
            Dialog::Apply { .. } | Dialog::Application { .. } => {}
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
            Dialog::AllowGame { key, .. } => {
                self.dialog = None;
                core.answer_game(&key, Some(true));
                cx.notify();
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
            Dialog::Welcome { .. } | Dialog::Secure { .. } | Dialog::PollVoters { .. } | Dialog::Picture { .. } => {
                self.close_dialog(cx)
            }
            Dialog::Onboarding { .. } => self.step_do(false, window, cx),
            Dialog::Poll { .. } => self.send_poll(cx),
            Dialog::Moderate { key, server, user_id, action } => {
                let reason: String = value.chars().take(512).collect();
                self.moderate(key, server, user_id, action, reason, cx);
            }
            Dialog::Profile { key, user_id, .. } => {
                self.dialog = None;
                self.message_person(key, user_id, window, cx);
            }
            Dialog::CreateChannel { key, server, parent, kind } => {
                let category = kind == crate::pb::ChannelType::Category;
                if value.is_empty() {
                    self.dialog_error = Some("Give it a name.".into());
                    cx.notify();
                    return;
                }
                self.dialog_busy = true;
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
                    if let Some((glyph, title)) = crate::ui::moderate::done_toast(action, &name, deleted) {
                        this.toast(glyph, title, String::new(), None, None, cx);
                    }
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
        } else if self.context.is_some() {
            self.context = None;
        } else if self.menu.is_some() {
            self.menu = None;
        } else if self.calls.pop.is_some() {
            self.calls.pop = None;
        } else if self.calls.recordings.is_some() {
            self.calls.recordings = None;
        } else if self.rail.editing.is_some() {
            self.rail.editing = None;
        } else if self.dialog.is_some() {
            self.dialog = None;
        } else if self.server_settings.is_some() {
            self.server_settings = None;
        } else if let Some(view) = self.instance_settings.clone() {
            if !view.update(cx, |v, cx| v.escape(cx)) {
                self.instance_settings = None;
            }
        } else if let Some(view) = self.settings.clone() {
            view.update(cx, |v, cx| v.escape(window, cx));
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
        window.set_rem_size(px(16.0 * self.prefs.text_scale.clamp(0.8, 1.5)));
        // When something that takes focus closes (a dialog, a page, the
        // switcher), the window takes focus back, or no shortcut would reach it.
        let covers = [
            self.dialog.is_some(),
            self.connect.is_some(),
            self.settings.is_some(),
            self.server_settings.is_some(),
            self.instance_settings.is_some(),
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
        crate::ui::effects::hold(self.server_settings.is_some() || self.instance_settings.is_some());
        let behind = crate::ui::backdrop::layers(&theme::backdrop(cx), &p, window, cx);

        let base = div()
            .id("fuwa")
            .key_context("Fuwa")
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::on_key))
            .capture_key_up(cx.listener(Self::on_key_up))
            // Any input means you're here (`core::presence::people`).
            .capture_any_mouse_down(cx.listener(|this, _, _, _| this.core.idle.seen()))
            .on_mouse_move(cx.listener(|this, _, _, _| this.core.idle.seen()))
            .on_scroll_wheel(cx.listener(|this, _, _, _| this.core.idle.seen()))
            .on_action(cx.listener(Self::close_overlay))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)))
            .on_action(cx.listener(|this, _: &ComposerEmoji, window, cx| this.open_emoji(window, cx)))
            .on_action(cx.listener(|this, _: &ComposerTimestamp, window, cx| this.open_time_picker(window, cx)))
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

        // Under a full-window page the banner can't be seen, so it doesn't move there either.
        let covered = self.server_settings.is_some() || self.instance_settings.is_some();
        let announcement = if covered { None } else { self.render_announcement(window, cx) };
        let update_note = if covered { None } else { self.render_update_note(window, cx) };
        // Streamer mode's bar is over everything; the sign-in notice under the announcement (banners.rs).
        let streamer = self.render_streamer_banner(window, cx);
        let sign_in = if covered { None } else { self.render_sign_in_notice(window, cx) };
        base.child(
            div()
                .size_full()
                .flex()
                .flex_col()
                .when_some(streamer, |el, banner| el.child(banner))
                .when_some(announcement, |el, banner| el.child(banner))
                .when_some(sign_in, |el, banner| el.child(banner))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .child(self.render_rail(window, cx))
                        .child(self.render_sidebar(window, cx))
                        .child(self.render_main(window, cx)),
                ),
        )
        .when_some(self.render_call_pop_layer(cx), |el, layer| el.child(layer))
        .when_some(update_note, |el, note| el.child(note))
        // The connect view draws its own frame: the welcome page, or a dialog over a dimmed app.
        .when_some(self.connect.clone(), |el, connect| el.child(connect))
        .when_some(self.render_folder_dialog(window, cx), |el, dialog| el.child(dialog))
        .when_some(
            match self.menu.clone() {
                Some(Menu::Server { key, server }) => Some(self.server_bell_menu(&key, &server, cx)),
                Some(Menu::Status { key }) => Some(self.status_menu(&key, window, cx)),
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
        .when_some(self.instance_settings.clone(), |el, settings| {
            el.child(
                gpui_kit::AnyView::from(settings)
                    .cached(gpui_kit::StyleRefinement::default().absolute().top_0().left_0().size_full()),
            )
        })
        .when_some(self.render_dialog(window, cx), |el, d| el.child(d))
        .when_some(self.render_recordings(window, cx), |el, d| el.child(d))
        .when_some(self.render_context_menu(window, cx), |el, menu| el.child(menu))
        .when_some(self.render_sheet(cx), |el, sheet| el.child(sheet))
        .when_some(self.render_switcher(cx), |el, switcher| el.child(switcher))
        .when_some(self.render_incoming_calls(window, cx), |el, calls| el.child(calls))
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
