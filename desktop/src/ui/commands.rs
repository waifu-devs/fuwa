//! Agents' slash commands and buttons (docs/commands.md), as the web app's
//! `Commands.tsx`. Typing "/" at the start of the composer lists the
//! commands of the agents in this server; picking one swaps the box for its
//! options, and Enter (or the send button) runs it. Escape goes back to
//! typing. The agent answers with an ordinary message headed "used /name",
//! and its messages can carry buttons: pressing one tells the agent, a link
//! button just opens its https link.
//!
//! Only in a server's plain channels, never threads, secure or shared ones,
//! and only on an instance that has `agent-commands`.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash as _, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, AppContext as _, Context, Entity, Focusable as _, FontWeight,
    InteractiveElement as _, IntoElement, Keystroke, ParentElement as _, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, WeakEntity, Window, div, px,
};

use crate::core::commands::{self, Choice};
use crate::core::i18n::t;
use crate::core::store::InstanceState;
use crate::pb;
use crate::ui::app::{FuwaApp, Target};
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{avatar, icon};

/// What the window keeps about commands between frames.
#[derive(Default)]
pub struct Commands {
    /// Each server's commands ("key\nserver"), and when they came.
    lists: HashMap<String, (Instant, Rc<Vec<Choice>>)>,
    loading: HashSet<String>,
    /// The lit row of the "/" list.
    pub active: usize,
    /// Escape closed the list; it stays closed until the box stops being "/name".
    dismissed: bool,
    /// The command picked, its options being filled in.
    pub form: Option<Form>,
    /// Buttons whose press is on its way, and how many times each was pressed (for its bounce).
    pressing: HashSet<(String, String)>,
    pressed: HashMap<(String, String), u32>,
}

impl Commands {
    /// The picked command's name, while its options are being filled in.
    pub fn picked_name(&self) -> Option<String> {
        self.form.as_ref().map(|f| f.choice.command.name.clone())
    }
}

/// A picked command, in place of the composer's box.
pub struct Form {
    key: String,
    server: String,
    channel: String,
    choice: Choice,
    fields: Vec<Field>,
    running: bool,
    /// The lit row of the focused field's list.
    lit: usize,
    _subs: Vec<Subscription>,
}

/// One option: typed into its box, or picked from a list the box filters.
struct Field {
    option: pb::CommandOption,
    input: Entity<InputState>,
    /// What was picked: the value sent and the name shown.
    picked: Option<(String, String)>,
}

impl Field {
    fn value(&self, cx: &gpui_kit::App) -> String {
        if commands::picked(&self.option) {
            self.picked.as_ref().map(|(value, _)| value.clone()).unwrap_or_default()
        } else {
            self.input.read(cx).value().to_string()
        }
    }
}

/// One line of a field's list: the value sent, the name shown, and a hint.
#[derive(Clone)]
struct Pick {
    value: String,
    label: String,
    hint: String,
    user: Option<pb::User>,
}

/// The most rows a field's list shows.
const PICKS: usize = 8;

/// What an option offers to pick, filtered by what's typed in its box.
fn picks(i: &InstanceState, server: &str, option: &pb::CommandOption, typed: &str) -> Vec<Pick> {
    let plain =
        |value: &str, label: &str| Pick { value: value.into(), label: label.into(), hint: String::new(), user: None };
    let all: Vec<Pick> = match commands::kind(option) {
        pb::CommandOptionType::Boolean => vec![plain("true", "Yes"), plain("false", "No")],
        pb::CommandOptionType::User => i
            .members
            .get(server)
            .into_iter()
            .flatten()
            .filter_map(|m| m.user.as_ref())
            .map(|u| Pick {
                value: u.id.clone(),
                label: i.display_name(Some(server), &u.id),
                hint: format!("@{}", u.username),
                user: Some(u.clone()),
            })
            .collect(),
        pb::CommandOptionType::Channel => i
            .channels
            .get(server)
            .into_iter()
            .flatten()
            .filter(|c| c.r#type == pb::ChannelType::Text as i32 || c.r#type == pb::ChannelType::Voice as i32)
            .map(|c| plain(&c.id, &format!("#{}", c.name)))
            .collect(),
        pb::CommandOptionType::Role => i
            .roles
            .get(server)
            .into_iter()
            .flatten()
            .map(|r| plain(&r.id, if r.id == server { "@everyone" } else { &r.name }))
            .collect(),
        _ => option.choices.iter().map(|c| plain(c, c)).collect(),
    };
    let typed = typed.trim().to_lowercase();
    all.into_iter()
        .filter(|p| {
            typed.is_empty() || p.label.to_lowercase().contains(&typed) || p.hint.to_lowercase().contains(&typed)
        })
        .take(PICKS)
        .collect()
}

/// What the box of an option says while it's empty.
fn placeholder(option: &pb::CommandOption) -> &'static str {
    match commands::kind(option) {
        pb::CommandOptionType::Integer => "A number",
        pb::CommandOptionType::User => "Pick someone",
        pb::CommandOptionType::Channel => "Pick a channel",
        pb::CommandOptionType::Role => "Pick a role",
        _ if commands::picked(option) => "Pick one",
        _ => "",
    }
}

/// An agent's buttons and "used /name", as a message draws them.
#[derive(Clone, PartialEq)]
pub struct AgentBits {
    /// Over an agent's answer: who used what (no command for a button).
    pub used: Option<(String, Option<pb::User>, Option<String>)>,
    pub rows: Vec<pb::ComponentRow>,
    /// Whether you may press them here.
    pub can_press: bool,
    /// The buttons whose press is on its way, and how often each was pressed.
    pub pressing: Vec<String>,
    pub pressed: Vec<(String, u32)>,
}

impl AgentBits {
    pub fn of(i: &InstanceState, server: &str, m: &pb::Message, can_press: bool, state: &Commands) -> Option<Self> {
        if m.interaction.is_none() && m.components.is_empty() {
            return None;
        }
        let used = m.interaction.as_ref().map(|used| {
            let member = i
                .members
                .get(server)
                .is_some_and(|ms| ms.iter().any(|x| x.user.as_ref().is_some_and(|u| u.id == used.user_id)));
            let name = if member { i.display_name(Some(server), &used.user_id) } else { "Someone".to_owned() };
            let command = (used.kind() != pb::InteractionKind::Button).then(|| used.command.clone());
            (name, member.then(|| i.users.get(&used.user_id).cloned()).flatten(), command)
        });
        let mine = |(mid, _): &&(String, String)| *mid == m.id;
        Some(Self {
            used,
            rows: m.components.clone(),
            can_press,
            pressing: state.pressing.iter().filter(mine).map(|(_, c)| c.clone()).collect(),
            pressed: state.pressed.iter().filter(|(k, _)| k.0 == m.id).map(|(k, n)| (k.1.clone(), *n)).collect(),
        })
    }

    pub fn digest(&self, h: &mut impl Hasher) {
        self.used.as_ref().map(|(n, u, c)| (n, u.as_ref().map(|u| (&u.id, &u.avatar_url)), c)).hash(h);
        for row in &self.rows {
            for b in &row.buttons {
                (&b.custom_id, &b.label, b.style, &b.url, b.disabled).hash(h);
            }
            0u8.hash(h);
        }
        (self.can_press, &self.pressing, &self.pressed).hash(h);
    }
}

/// "*Juan* used **/roll**" over an agent's answer.
pub(crate) fn used_line(mid: &str, bits: &AgentBits, p: &Palette) -> Option<AnyElement> {
    let (name, user, command) = bits.used.clone()?;
    Some(
        div()
            .id(SharedString::from(format!("used|{mid}")))
            .flex()
            .items_center()
            .gap(px(6.0))
            .min_w_0()
            .text_xs()
            .text_color(p.muted_foreground)
            .child(avatar(user.as_ref(), 16.0, p))
            .child(div().font_weight(FontWeight::BOLD).text_color(alpha(p.foreground, 0.8)).child(name))
            .map(|el| match command {
                Some(command) => el
                    .child("used")
                    .child(div().font_weight(FontWeight::BOLD).text_color(p.primary).child(format!("/{command}"))),
                None => el.child("pressed a button"),
            })
            .into_any_element(),
    )
}

/// An agent's buttons under its message.
pub(crate) fn buttons_view(
    mid: &str,
    bits: &AgentBits,
    p: &Palette,
    this: &WeakEntity<FuwaApp>,
    key: &str,
    server: Option<&str>,
) -> Option<AnyElement> {
    if bits.rows.is_empty() {
        return None;
    }
    let mut rows = div().mt(px(6.0)).flex().flex_col().gap(px(6.0));
    for (r, row) in bits.rows.iter().enumerate() {
        let mut line = div().flex().flex_wrap().gap(px(6.0));
        for (b, button) in row.buttons.iter().enumerate() {
            line = line.child(button_el(mid, r, b, button, bits, p, this, key, server));
        }
        rows = rows.child(line);
    }
    Some(rows.into_any_element())
}

#[allow(clippy::too_many_arguments)]
fn button_el(
    mid: &str,
    r: usize,
    b: usize,
    button: &pb::Button,
    bits: &AgentBits,
    p: &Palette,
    this: &WeakEntity<FuwaApp>,
    key: &str,
    server: Option<&str>,
) -> AnyElement {
    // Each style and its hover (the web's `hover:bg-primary/90`, `hover:bg-muted/70`).
    let (bg, hover, fg) = match button.style() {
        pb::ButtonStyle::Primary => (p.primary.into(), alpha(p.primary, 0.9), p.primary_foreground.into()),
        pb::ButtonStyle::Success => (p.success.into(), alpha(p.success, 0.9), gpui_kit::white()),
        pb::ButtonStyle::Danger => (p.destructive.into(), alpha(p.destructive, 0.9), gpui_kit::white()),
        _ => (alpha(p.muted_foreground, 0.16), alpha(p.muted_foreground, 0.11), p.foreground.into()),
    };
    let id = SharedString::from(format!("btn|{mid}|{r}|{b}"));
    let base = div()
        .id(id)
        .h(px(32.0))
        .max_w_full()
        .px(px(12.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .rounded(corner(10.0))
        .bg(bg)
        .text_color(fg)
        .text_sm()
        .font_weight(FontWeight::BOLD)
        .child(div().min_w_0().truncate().child(button.label.clone()));
    if button.style() == pb::ButtonStyle::Link {
        let Some(url) = commands::opens(button).map(str::to_owned) else {
            return base.opacity(0.5).into_any_element();
        };
        return base
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .active(|s| s.scale(0.95))
            .on_click(move |_, _, cx| cx.open_url(&url))
            .child(icon("external-link").size(px(14.0)))
            .into_any_element();
    }
    let pressing = bits.pressing.contains(&button.custom_id);
    let can = bits.can_press && !button.disabled && !pressing && server.is_some();
    let times = bits.pressed.iter().find(|(c, _)| *c == button.custom_id).map_or(0, |(_, n)| *n);
    let el = base
        .when(!can, |el| el.opacity(0.5))
        .when(can, |el| {
            let (this, key, server) = (this.clone(), key.to_owned(), server.unwrap_or_default().to_owned());
            let (mid, custom) = (mid.to_owned(), button.custom_id.clone());
            el.cursor_pointer().hover(move |s| s.bg(hover)).active(|s| s.scale(0.95)).on_click(move |_, _, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.press_button(key.clone(), server.clone(), mid.clone(), custom.clone(), cx)
                });
            })
        })
        .when(pressing, |el| {
            el.child(icon("loader-circle").size(px(14.0)).with_animation(
                SharedString::from(format!("btn-spin|{mid}|{r}|{b}")),
                Animation::new(Duration::from_millis(900)).repeat(),
                |el, t| el.rotate(gpui_kit::percentage(t)),
            ))
        });
    if times == 0 {
        return el.into_any_element();
    }
    // A press that went through swells once (the web's `scale: [1, 1.06, 1]`).
    motion::once(
        el,
        SharedString::from(format!("btn-pop|{mid}|{r}|{b}|{times}")),
        Duration::from_millis(320),
        |el, t| el.scale(1.0 + 0.06 * (t * std::f32::consts::PI).sin()),
    )
}

impl FuwaApp {
    /// Whether the open channel takes commands: a server's plain channel on an instance that
    /// has them, where you may send.
    pub(crate) fn can_command(&self) -> Option<(String, String, String)> {
        let Some(Target::Channel { key, server, channel }) = self.target() else { return None };
        let ok = self.core.shared.read(|s| {
            s.instance(&key).is_some_and(|i| {
                i.has("agent-commands")
                    && i.access(&server).has_in(&channel, pb::Permission::SendMessages)
                    && i.channel(&server, &channel).is_some_and(commands::takes_commands)
            })
        });
        ok.then_some((key, server, channel))
    }

    /// Asks for this server's commands as "/" is typed, when they're not fresh.
    pub(crate) fn update_commands(&mut self, cx: &mut Context<Self>) {
        let typed = self.composer.read(cx).value().to_string();
        if commands::query(&typed).is_none() {
            self.commands.dismissed = false;
            return;
        }
        self.commands.active = 0;
        let Some((key, server, _)) = self.can_command() else { return };
        let at = format!("{key}\n{server}");
        let fresh = self.commands.lists.get(&at).is_some_and(|(when, _)| when.elapsed() < commands::FRESH);
        if fresh || !self.commands.loading.insert(at.clone()) {
            return;
        }
        let core = self.core.clone();
        self.run(cx, async move { core.list_commands(&key, &server).await }, move |this, result, cx| {
            this.commands.loading.remove(&at);
            // A list that didn't come is treated as none, and asked for again next time.
            let (when, list) = match result {
                Ok(list) => (Instant::now(), list),
                Err(_) => (Instant::now() - commands::FRESH, Vec::new()),
            };
            this.commands.lists.insert(at, (when, Rc::new(list)));
            cx.notify();
        });
    }

    /// The "/" list while it's open: the matching commands, and whether they're still coming.
    fn command_list(&self, cx: &gpui_kit::App) -> Option<(Vec<Choice>, bool, bool)> {
        if self.commands.dismissed || self.commands.form.is_some() {
            return None;
        }
        let query = commands::query(self.composer.read(cx).value().as_ref())?;
        let (key, server, _) = self.can_command()?;
        let at = format!("{key}\n{server}");
        let all = self.commands.lists.get(&at).map(|(_, l)| l.clone());
        let loading = all.is_none();
        let empty = all.as_ref().is_some_and(|l| l.is_empty());
        let options = all.map(|l| commands::matching(&l, &query)).unwrap_or_default();
        (loading || empty || !options.is_empty()).then_some((options, loading, empty))
    }

    /// Picks a command: the box makes way for its options.
    fn pick_command(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, server, channel)) = self.can_command() else { return };
        let mut subs = Vec::new();
        let fields: Vec<Field> = choice
            .command
            .options
            .iter()
            .map(|option| {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder(option)));
                subs.push(cx.subscribe_in(&input, window, |this: &mut Self, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change | InputEvent::Focus) {
                        if let Some(form) = this.commands.form.as_mut() {
                            form.lit = 0;
                        }
                        cx.notify();
                    }
                }));
                Field { option: option.clone(), input, picked: None }
            })
            .collect();
        let first = fields.first().map(|f| f.input.clone());
        self.commands.form = Some(Form { key, server, channel, choice, fields, running: false, lit: 0, _subs: subs });
        self.composer.update(cx, |state, cx| state.set_value("", window, cx));
        match first {
            Some(input) => input.update(cx, |s, cx| s.focus(window, cx)),
            // With nothing to fill in, the box keeps Enter and Escape.
            None => self.composer.update(cx, |s, cx| s.focus(window, cx)),
        }
        cx.notify();
    }

    /// Back to typing.
    pub(crate) fn cancel_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.commands.form.take().is_some() {
            self.composer.update(cx, |s, cx| s.focus(window, cx));
            cx.notify();
        }
    }

    /// The options still missing something, by name.
    fn form_missing(&self, cx: &gpui_kit::App) -> Vec<String> {
        let Some(form) = &self.commands.form else { return Vec::new() };
        commands::missing(&form.choice.command, &form_values(form, cx))
    }

    /// Runs the picked command, once everything it needs is there.
    pub(crate) fn run_picked_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let missing = self.form_missing(cx);
        let Some(form) = self.commands.form.as_mut() else { return };
        if form.running || !missing.is_empty() {
            // Straight to the first thing still needed.
            if let Some(field) = form.fields.iter().find(|f| missing.contains(&f.option.name)) {
                field.input.update(cx, |s, cx| s.focus(window, cx));
            }
            return;
        }
        let args = commands::arguments(&form.choice.command, &form_values(form, cx));
        form.running = true;
        let (key, server, channel, choice) =
            (form.key.clone(), form.server.clone(), form.channel.clone(), form.choice.clone());
        let core = self.core.clone();
        let ran = choice.key();
        cx.notify();
        let window_handle = window.window_handle();
        self.run(
            cx,
            async move { core.run_command(&key, &server, &channel, &choice, args).await },
            move |this, result, cx| {
                let same = this.commands.form.as_ref().is_some_and(|f| f.choice.key() == ran);
                if !same {
                    return;
                }
                match result {
                    Ok(()) => {
                        this.commands.form = None;
                        let composer = this.composer.clone();
                        let _ =
                            window_handle.update(cx, |_, window, cx| composer.update(cx, |s, cx| s.focus(window, cx)));
                    }
                    Err(err) => {
                        if let Some(form) = this.commands.form.as_mut() {
                            form.running = false;
                        }
                        this.toast("circle-alert", "Couldn't run that command".into(), err.message, None, None, cx);
                    }
                }
                cx.notify();
            },
        );
    }

    /// Presses an agent's button.
    pub(crate) fn press_button(
        &mut self,
        key: String,
        server: String,
        message_id: String,
        custom_id: String,
        cx: &mut Context<Self>,
    ) {
        let at = (message_id.clone(), custom_id.clone());
        if !self.commands.pressing.insert(at.clone()) {
            return;
        }
        self.sync_list(cx);
        cx.notify();
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.press_button(&key, &server, &message_id, &custom_id).await },
            move |this, result, cx| {
                this.commands.pressing.remove(&at);
                match result {
                    Ok(()) => *this.commands.pressed.entry(at).or_default() += 1,
                    Err(err) => this.toast("circle-alert", "Couldn't press that".into(), err.message, None, None, cx),
                }
                this.sync_list(cx);
                cx.notify();
            },
        );
    }

    /// The field whose box has focus, by its place in the form.
    fn focused_field(&self, window: &Window, cx: &gpui_kit::App) -> Option<usize> {
        let form = self.commands.form.as_ref()?;
        form.fields.iter().position(|f| f.input.read(cx).focus_handle(cx).is_focused(window))
    }

    /// What the focused field offers to pick, when it's one you pick from.
    fn open_picks(&self, window: &Window, cx: &gpui_kit::App) -> Option<(usize, Vec<Pick>)> {
        let ix = self.focused_field(window, cx)?;
        let form = self.commands.form.as_ref()?;
        let field = &form.fields[ix];
        if !commands::picked(&field.option) || field.picked.is_some() {
            return None;
        }
        let typed = field.input.read(cx).value().to_string();
        let list = self
            .core
            .shared
            .read(|s| s.instance(&form.key).map(|i| picks(i, &form.server, &field.option, &typed)).unwrap_or_default());
        Some((ix, list))
    }

    fn choose(&mut self, ix: usize, pick: Pick, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.commands.form.as_mut() else { return };
        let Some(field) = form.fields.get_mut(ix) else { return };
        field.picked = Some((pick.value, pick.label));
        field.input.update(cx, |s, cx| s.set_value("", window, cx));
        // On to the next option, or the box takes Enter to run it.
        match form.fields.get(ix + 1) {
            Some(next) => next.input.update(cx, |s, cx| s.focus(window, cx)),
            None => self.composer.update(cx, |s, cx| s.focus(window, cx)),
        }
        cx.notify();
    }

    fn unpick(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.commands.form.as_mut().and_then(|f| f.fields.get_mut(ix)) else { return };
        field.picked = None;
        field.input.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    /// Takes the keys the "/" list and a command's options use. True when it took the key.
    pub(crate) fn command_keys(&mut self, key: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let m = &key.modifiers;
        let bare = !(m.shift || m.control || m.alt || m.platform || m.function);
        let in_composer = self.composer.read(cx).focus_handle(cx).is_focused(window);
        if self.commands.form.is_some() {
            if !in_composer && self.focused_field(window, cx).is_none() {
                return false;
            }
            if key.key == "escape" {
                self.cancel_command(window, cx);
                return true;
            }
            if let Some((ix, list)) = self.open_picks(window, cx) {
                let lit = self.commands.form.as_ref().map_or(0, |f| f.lit);
                let count = list.len();
                match key.key.as_str() {
                    "down" if bare && count > 0 => self.set_lit((lit + 1) % count, cx),
                    "up" if bare && count > 0 => self.set_lit((lit + count - 1) % count, cx),
                    "enter" | "tab" if bare && count > 0 => {
                        let pick = list.get(lit).cloned().unwrap_or_else(|| list[0].clone());
                        self.choose(ix, pick, window, cx);
                    }
                    "enter" if bare => self.run_picked_command(window, cx),
                    _ => return false,
                }
                return true;
            }
            if key.key == "enter" && bare {
                self.run_picked_command(window, cx);
                return true;
            }
            return false;
        }
        if !in_composer {
            return false;
        }
        let Some((options, _, _)) = self.command_list(cx) else { return false };
        if key.key == "escape" {
            self.commands.dismissed = true;
            cx.notify();
            return true;
        }
        let count = options.len();
        if count == 0 {
            return false;
        }
        match key.key.as_str() {
            "down" if bare => self.commands.active = (self.commands.active + 1) % count,
            "up" if bare => self.commands.active = (self.commands.active + count - 1) % count,
            "enter" | "tab" if bare => {
                let pick = options.get(self.commands.active).cloned().unwrap_or_else(|| options[0].clone());
                self.pick_command(pick, window, cx);
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    fn set_lit(&mut self, lit: usize, cx: &mut Context<Self>) {
        if let Some(form) = self.commands.form.as_mut() {
            form.lit = lit;
        }
        cx.notify();
    }

    /// Whether the open form belongs to the open channel; one for another place is dropped.
    pub(crate) fn command_form_here(&mut self) -> bool {
        let here = self.can_command();
        let same =
            self.commands.form.as_ref().is_some_and(|f| {
                here.as_ref().is_some_and(|(k, s, c)| *k == f.key && *s == f.server && *c == f.channel)
            });
        if !same {
            self.commands.form = None;
        }
        same
    }

    /// The "/" list floating over the composer.
    /// Once closed, it's drawn a moment more on its way out. The @ list goes first.
    pub(crate) fn command_list_view(
        &self,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let now = if self.picker.is_none() { self.command_list(cx) } else { None };
        let going = motion::kept("commands-list", now.as_ref(), window, cx);
        let ((options, loading, empty), going) = match (now, going) {
            (Some(list), _) => (list, None),
            (None, Some((list, t))) => (list, Some(t)),
            (None, None) => return None,
        };
        let mut list = div().relative().flex().flex_col();
        if !options.is_empty() {
            // The lit row's fill glides (the web's `layoutId="command-active"`).
            list = list.child(Self::above_glide("command-lit".into(), self.commands.active, p, cx));
        }
        list = list.child(Self::above_title("slash", &t("chattools.commands.title"), p));
        if loading {
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .py(px(8.0))
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(icon("loader-circle").size(px(16.0)).with_animation(
                        "commands-loading",
                        Animation::new(Duration::from_millis(900)).repeat(),
                        |el, t| el.rotate(gpui_kit::percentage(t)),
                    ))
                    .child(t("chattools.commands.looking")),
            );
        } else if empty {
            list = list.child(
                div()
                    .px(px(8.0))
                    .py(px(8.0))
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(t("chattools.commands.none")),
            );
        }
        for (n, choice) in options.into_iter().enumerate() {
            let agent = choice.agent.as_ref().map(crate::core::store::user_name).unwrap_or_else(|| "An agent".into());
            let id = SharedString::from(format!("cmd|{}", choice.key()));
            let (name, about, face) =
                (format!("/{}", choice.command.name), choice.command.description.clone(), choice.agent.clone());
            list = list.child(
                div()
                    .id(id)
                    .h(px(36.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(crate::ui::theme::radius_xl())
                    .text_sm()
                    .cursor_pointer()
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.commands.active != n {
                            this.commands.active = n;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| this.pick_command(choice.clone(), window, cx)))
                    .child(avatar(face.as_ref(), 24.0, p))
                    .child(div().flex_none().font_weight(FontWeight::BOLD).child(name))
                    .child(div().flex_1().min_w_0().truncate().text_color(p.muted_foreground).child(about))
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .text_xs()
                            .text_color(p.muted_foreground)
                            .child(icon("bot").size(px(12.0)))
                            .child(agent),
                    ),
            );
        }
        Some(Self::above_composer(list, "commands-list", going, p))
    }

    /// The focused option's list, floating over the composer.
    pub(crate) fn command_picks_view(
        &self,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // Once closed, it's drawn a moment more on its way out.
        let now = self.open_picks(window, cx).and_then(|(ix, list)| {
            let form = self.commands.form.as_ref()?;
            let lit = form.lit.min(list.len().saturating_sub(1));
            Some((ix, list, lit, form.fields[ix].option.name.clone()))
        });
        let going = motion::kept("command-picks", now.as_ref(), window, cx);
        let ((ix, list, lit, title), going) = match (now, going) {
            (Some(open), _) => (open, None),
            (None, Some((open, t))) => (open, Some(t)),
            (None, None) => return None,
        };
        let hover = alpha(p.primary, 0.08);
        let mut rows = div().relative().flex().flex_col();
        if !list.is_empty() {
            rows = rows.child(Self::above_glide(format!("command-pick-lit|{ix}"), lit, p, cx));
        }
        rows = rows.child(Self::above_title("", &title, p));
        if list.is_empty() {
            rows = rows.child(
                div().px(px(8.0)).py(px(8.0)).text_sm().text_color(p.muted_foreground).child("Nothing matches."),
            );
        }
        for (n, pick) in list.into_iter().enumerate() {
            let id = SharedString::from(format!("cmd-pick|{ix}|{}", pick.value));
            let (label, hint, user) = (pick.label.clone(), pick.hint.clone(), pick.user.clone());
            rows = rows.child(
                div()
                    .id(id)
                    .h(px(36.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(crate::ui::theme::radius_xl())
                    .cursor_pointer()
                    .when(n != lit, |el| el.hover(move |s| s.bg(hover)))
                    .on_click(cx.listener(move |this, _, window, cx| this.choose(ix, pick.clone(), window, cx)))
                    .when_some(user, |el, user| el.child(avatar(Some(&user), 22.0, p)))
                    .child(div().min_w_0().truncate().font_weight(FontWeight::BOLD).text_sm().child(label))
                    .child(div().flex_1())
                    .child(div().text_xs().text_color(p.muted_foreground).child(hint)),
            );
        }
        Some(Self::above_composer(rows, format!("command-picks-{ix}"), going, p))
    }

    /// In place of the box once a command is picked: its options as fields.
    pub(crate) fn command_form_view(&self, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let form = self.commands.form.as_ref()?;
        let missing = commands::missing(&form.choice.command, &form_values(form, cx));
        let agent = form.choice.agent.as_ref().map(crate::core::store::user_name).unwrap_or_else(|| "An agent".into());
        let head = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .min_w_0()
            .child(
                div()
                    .flex_none()
                    .px(px(8.0))
                    .py(px(2.0))
                    .rounded(corner(8.0))
                    .bg(alpha(p.primary, 0.12))
                    .text_color(p.primary)
                    .text_sm()
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(format!("/{}", form.choice.command.name)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(p.muted_foreground)
                    .child(format!("{} · {agent}", form.choice.command.description)),
            )
            .child(
                // `size-7 rounded-lg`, the muted fill and the text's color on hover.
                div()
                    .id("command-cancel")
                    .size(px(28.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(crate::ui::theme::radius_lg())
                    .cursor_pointer()
                    .text_color(p.muted_foreground)
                    .hover({
                        let (bg, fg) = (p.muted, p.foreground);
                        move |s| s.bg(bg).text_color(fg)
                    })
                    .child(icon("x").size(px(16.0)))
                    .tooltip(|window, cx| crate::ui::overlay::Tip::new("Back to typing (Escape)").build(window, cx))
                    .on_click(cx.listener(|this, _, window, cx| this.cancel_command(window, cx))),
            );
        let mut fields = div().flex().flex_wrap().gap(px(8.0));
        for (ix, field) in form.fields.iter().enumerate() {
            let option = &field.option;
            let wanting = missing.contains(&option.name);
            let label = format!("{}{}", option.name, if option.required { "" } else { " (optional)" });
            let width = if commands::kind(option) == pb::CommandOptionType::Integer { 112.0 } else { 192.0 };
            let control: AnyElement = match &field.picked {
                Some((_, shown)) => div()
                    .id(SharedString::from(format!("cmd-chip|{ix}")))
                    .h(px(32.0))
                    .max_w(px(width))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .rounded(corner(8.0))
                    .bg(alpha(p.primary, 0.12))
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.85))
                    .on_click(cx.listener(move |this, _, window, cx| this.unpick(ix, window, cx)))
                    .child(div().min_w_0().truncate().child(shown.clone()))
                    .child(icon("x").size(px(12.0)).text_color(p.muted_foreground))
                    .into_any_element(),
                // The web's `h-8 rounded-lg border bg-background px-2 text-sm` field.
                None => div()
                    .w(px(width))
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .rounded(crate::ui::theme::radius_lg())
                    .border_1()
                    .border_color(p.border)
                    .bg(p.background)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .ml(px(-4.0))
                            .child(Input::new(&field.input).appearance(false).text_sm()),
                    )
                    .into_any_element(),
            };
            fields = fields.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(if wanting { p.foreground } else { p.muted_foreground })
                            .child(label),
                    )
                    .child(control),
            );
        }
        Some(
            motion::rise(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .py(px(4.0))
                    .child(head)
                    .when(!form.fields.is_empty(), |el| el.child(fields)),
                SharedString::from(format!("command-form-{}", form.choice.key())),
                Duration::ZERO,
                6.0,
            )
            .into_any_element(),
        )
    }

    /// Whether the picked command can run: everything it needs is there and it isn't on its way.
    pub(crate) fn command_ready(&self, cx: &gpui_kit::App) -> bool {
        self.commands.form.as_ref().is_some_and(|f| !f.running) && self.form_missing(cx).is_empty()
    }
}

fn form_values(form: &Form, cx: &gpui_kit::App) -> HashMap<String, String> {
    form.fields.iter().map(|f| (f.option.name.clone(), f.value(cx))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> InstanceState {
        let mut i = InstanceState::new("k", "https://k");
        let user = |id: &str, name: &str| pb::User { id: id.into(), username: name.into(), ..Default::default() };
        i.members.insert(
            "s".into(),
            vec![
                pb::Member { user: Some(user("u1", "juan")), ..Default::default() },
                pb::Member { user: Some(user("u2", "mika")), nickname: "Mika".into(), ..Default::default() },
            ],
        );
        i.users.insert("u1".into(), user("u1", "juan"));
        i.users.insert("u2".into(), user("u2", "mika"));
        i.channels.insert(
            "s".into(),
            vec![
                pb::Channel {
                    id: "c1".into(),
                    name: "general".into(),
                    r#type: pb::ChannelType::Text as i32,
                    ..Default::default()
                },
                pb::Channel {
                    id: "k1".into(),
                    name: "Stuff".into(),
                    r#type: pb::ChannelType::Category as i32,
                    ..Default::default()
                },
            ],
        );
        i.roles.insert(
            "s".into(),
            vec![
                pb::Role { id: "s".into(), name: "everyone".into(), ..Default::default() },
                pb::Role { id: "r1".into(), name: "Mods".into(), ..Default::default() },
            ],
        );
        i
    }

    fn option(kind: pb::CommandOptionType) -> pb::CommandOption {
        pb::CommandOption { name: "x".into(), r#type: kind as i32, ..Default::default() }
    }

    #[test]
    fn options_offer_what_the_server_has() {
        let i = state();
        let labels = |kind, typed: &str| -> Vec<String> {
            picks(&i, "s", &option(kind), typed).into_iter().map(|p| p.label).collect()
        };
        assert_eq!(labels(pb::CommandOptionType::Boolean, ""), ["Yes", "No"]);
        assert_eq!(labels(pb::CommandOptionType::User, ""), ["juan", "Mika"]);
        assert_eq!(labels(pb::CommandOptionType::User, "mik"), ["Mika"]);
        assert_eq!(labels(pb::CommandOptionType::Channel, ""), ["#general"]);
        assert_eq!(labels(pb::CommandOptionType::Role, ""), ["@everyone", "Mods"]);
        let mut choices = option(pb::CommandOptionType::String);
        choices.choices = vec!["red".into(), "blue".into()];
        assert_eq!(picks(&i, "s", &choices, "bl").into_iter().map(|p| p.value).collect::<Vec<_>>(), ["blue"]);
    }

    #[test]
    fn an_answer_says_who_used_what() {
        let i = state();
        let m = pb::Message {
            id: "m".into(),
            interaction: Some(pb::MessageInteraction {
                kind: pb::InteractionKind::Command as i32,
                command: "roll".into(),
                user_id: "u2".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let bits = AgentBits::of(&i, "s", &m, true, &Commands::default()).unwrap();
        assert_eq!(bits.used.as_ref().map(|u| (u.0.as_str(), u.2.as_deref())), Some(("Mika", Some("roll"))));
        let gone = pb::Message {
            interaction: Some(pb::MessageInteraction {
                kind: pb::InteractionKind::Button as i32,
                user_id: "left".into(),
                ..Default::default()
            }),
            ..m.clone()
        };
        let bits = AgentBits::of(&i, "s", &gone, true, &Commands::default()).unwrap();
        assert_eq!(bits.used.map(|u| (u.0, u.1.is_none(), u.2)), Some(("Someone".into(), true, None)));
        assert!(AgentBits::of(&i, "s", &pb::Message::default(), true, &Commands::default()).is_none());
    }
}
