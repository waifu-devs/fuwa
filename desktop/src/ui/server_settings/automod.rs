//! The AutoMod page: rules that read every message as it's sent or edited,
//! and block it, tell the mods or time its author out. Blocked words (up to
//! six lists), mention spam, links, and a smart filter that hands the text
//! to a moderation service the instance turned on and acts on what kind it
//! is. Each rule is tried out right under it before it's saved. People who
//! manage the server are never caught. The web's `settings/server/AutoMod.tsx`.

use gpui_kit::AnimationExt as _;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};

use super::roles::switch;
use super::*;
use crate::core::server_admin::add_words;
use crate::pb::{AutoModActionKind as K, AutoModLevel as L, AutoModTrigger as T};

const MAX_KEYWORD_RULES: usize = 6;
const MAX_WORDS: usize = 1000;
const MAX_ALLOWED: usize = 100;
const NAME_MAX: usize = 100;
const MESSAGE_MAX: usize = 150;

/// The three kinds of rule, as the page lists them: trigger, name, what it's
/// for, its icon, its hue, and how many a server may have.
const KINDS: [(T, &str, &str, &str, f32, usize); 4] = [
    (
        T::Keywords,
        "Blocked words",
        "Words and phrases you don't want said. Up to six lists, each with its own actions.",
        "type",
        0.97,
        MAX_KEYWORD_RULES,
    ),
    (
        T::MentionSpam,
        "Mention spam",
        "Messages that ping too many people or roles at once, the usual sign of a raid.",
        "at-sign",
        0.11,
        1,
    ),
    (
        T::Links,
        "Links",
        "Links to sites you haven't allowed. Allowing a site allows its subdomains too.",
        "link",
        0.55,
        1,
    ),
    (
        T::Provider,
        "Smart filter",
        "A moderation service reads each message and says what kind it is: hate, harassment, scams, spam and more. You pick what happens for each.",
        "sparkles",
        0.76,
        1,
    ),
];

/// What a smart filter can do about one kind of message, mildest first.
const LEVELS: [(L, &str); 4] = [(L::Off, "Off"), (L::Flag, "Flag"), (L::Block, "Block"), (L::TimeOut, "Time out")];

/// How sure a provider must be, when a label doesn't say.
const SURE: i32 = 80;

/// The level a label is at; unset reads as nothing.
fn level_of(level: i32) -> L {
    match L::try_from(level).unwrap_or(L::Off) {
        L::Unspecified => L::Off,
        l => l,
    }
}

const TIME_OUTS: [i64; 6] = [60, 300, 600, 3_600, 86_400, 604_800];

/// A rule on the page: as saved (none yet for a new one) and as edited.
struct Rule {
    key: String,
    saved: Option<pb::AutoModRule>,
    draft: pb::AutoModRule,
}

impl Rule {
    fn dirty(&self) -> bool {
        self.saved.as_ref() != Some(&self.draft)
    }
}

/// What a test message made of the open rule.
enum Tried {
    Checking,
    /// What set it off, and for a smart filter how long its provider took.
    Caught(Vec<String>, i32),
    Through(i32),
    /// A smart filter's provider didn't answer.
    Failed(String, i32),
}

pub(super) struct AutoMod {
    rules: Option<Vec<Rule>>,
    /// The moderation services the instance turned on, for smart filters.
    providers: Vec<pb::AutoModProvider>,
    /// "How sure" for each of the open smart filter's labels.
    sure: HashMap<String, Entity<SliderState>>,
    sure_subscriptions: Vec<Subscription>,
    loading: bool,
    open: Option<String>,
    /// Which rule the boxes were filled for.
    filled_for: Option<String>,
    name: Entity<InputState>,
    words: Entity<InputState>,
    allowed: Entity<InputState>,
    message: Entity<InputState>,
    tester: Entity<InputState>,
    limit: Entity<SliderState>,
    tried: Option<Tried>,
    /// Bumped on each keystroke in the tester, so only the last one asks.
    test_seq: u64,
    busy: Option<String>,
    confirming: Option<String>,
    /// A rule's own problem, under it.
    problem: Option<(String, String)>,
    /// When a rule didn't save, so its card shakes.
    shook: Option<(String, Instant)>,
    next: u64,
}

impl AutoMod {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ServerSettingsView>) -> (Self, Vec<Subscription>) {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Give it a name"));
        let words = cx.new(|cx| InputState::new(window, cx).placeholder("Type a word, then Enter"));
        let allowed = cx.new(|cx| InputState::new(window, cx).placeholder("Type a word, then Enter"));
        let message = cx.new(|cx| {
            InputState::new(window, cx).placeholder("What they're told (optional): “Keep it friendly, please.”")
        });
        let tester = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Write something the rule should catch, or let through.")
        });
        let limit = cx.new(|_| SliderState::new().min(1.0).max(50.0).step(1.0).default_value(5.0));
        let subscriptions = vec![
            cx.subscribe(&name, |this: &mut ServerSettingsView, s, e: &InputEvent, cx| {
                if let InputEvent::Change = e {
                    let v: String = s.read(cx).value().chars().take(NAME_MAX).collect();
                    this.edit_rule(cx, |r| r.name = v);
                }
            }),
            cx.subscribe(&message, |this: &mut ServerSettingsView, s, e: &InputEvent, cx| {
                if let InputEvent::Change = e {
                    let v: String = s.read(cx).value().chars().take(MESSAGE_MAX).collect();
                    this.edit_rule(cx, |r| {
                        if r.trigger == T::Provider as i32 {
                            // A smart filter blocks by its labels; the action only carries what people are told.
                            let keep = !v.trim().is_empty();
                            set_action(r, K::Block, keep, |a| a.message = v);
                        } else if let Some(a) = r.actions.iter_mut().find(|a| a.kind == K::Block as i32) {
                            a.message = v;
                        }
                    });
                }
            }),
            cx.subscribe_in(&words, window, |this: &mut ServerSettingsView, s, e: &InputEvent, window, cx| {
                this.take_words(s.clone(), false, e, window, cx)
            }),
            cx.subscribe_in(&allowed, window, |this: &mut ServerSettingsView, s, e: &InputEvent, window, cx| {
                this.take_words(s.clone(), true, e, window, cx)
            }),
            cx.subscribe(&tester, |this: &mut ServerSettingsView, _, e: &InputEvent, cx| {
                if let InputEvent::Change = e {
                    this.try_rule(cx)
                }
            }),
            cx.subscribe(&limit, |this: &mut ServerSettingsView, _, e: &SliderEvent, cx| {
                let SliderEvent::Change(v) = e else { return };
                let v = v.start().round().clamp(1.0, 50.0) as i32;
                this.edit_rule(cx, |r| r.mention_limit = v);
            }),
        ];
        let automod = Self {
            rules: None,
            providers: Vec::new(),
            sure: HashMap::new(),
            sure_subscriptions: Vec::new(),
            loading: false,
            open: None,
            filled_for: None,
            name,
            words,
            allowed,
            message,
            tester,
            limit,
            tried: None,
            test_seq: 0,
            busy: None,
            confirming: None,
            problem: None,
            shook: None,
            next: 0,
        };
        (automod, subscriptions)
    }
}

fn action(r: &pb::AutoModRule, kind: K) -> Option<&pb::AutoModAction> {
    r.actions.iter().find(|a| a.kind == kind as i32)
}

/// Turns an action on (keeping what it had) or off, in the order the server keeps them.
fn set_action(r: &mut pb::AutoModRule, kind: K, on: bool, fill: impl FnOnce(&mut pb::AutoModAction)) {
    if !on {
        r.actions.retain(|a| a.kind != kind as i32);
        return;
    }
    if !r.actions.iter().any(|a| a.kind == kind as i32) {
        r.actions.push(pb::AutoModAction { kind: kind as i32, ..Default::default() });
    }
    if let Some(a) = r.actions.iter_mut().find(|a| a.kind == kind as i32) {
        fill(a);
    }
    r.actions.sort_by_key(|a| a.kind);
}

fn fresh(trigger: T, provider: Option<&pb::AutoModProvider>, alert_channel: &str) -> pb::AutoModRule {
    if let (T::Provider, Some(provider)) = (trigger, provider) {
        return pb::AutoModRule {
            enabled: true,
            trigger: trigger as i32,
            provider: provider.id.clone(),
            pictures: provider.pictures,
            labels: defaults(provider),
            actions: vec![pb::AutoModAction {
                kind: K::Alert as i32,
                channel_id: alert_channel.into(),
                ..Default::default()
            }],
            ..Default::default()
        };
    }
    pb::AutoModRule {
        enabled: true,
        trigger: trigger as i32,
        mention_limit: if trigger == T::MentionSpam { 5 } else { 0 },
        actions: vec![pb::AutoModAction { kind: K::Block as i32, ..Default::default() }],
        ..Default::default()
    }
}

/// Each of a provider's labels at the level it suggests.
fn defaults(provider: &pb::AutoModProvider) -> Vec<pb::AutoModLabelRule> {
    provider
        .labels
        .iter()
        .map(|l| pb::AutoModLabelRule { label: l.id.clone(), level: l.default_level, threshold: SURE })
        .collect()
}

/// Sets one label's level or how sure the provider must be, adding the label if it's new.
fn set_label(r: &mut pb::AutoModRule, label: &str, level: Option<L>, threshold: Option<i32>) {
    if !r.labels.iter().any(|l| l.label == label) {
        r.labels.push(pb::AutoModLabelRule { label: label.into(), level: L::Off as i32, threshold: SURE });
    }
    if let Some(l) = r.labels.iter_mut().find(|l| l.label == label) {
        if let Some(level) = level {
            l.level = level as i32;
        }
        if let Some(t) = threshold {
            l.threshold = t.clamp(50, 99);
        }
        if l.threshold == 0 {
            l.threshold = SURE;
        }
    }
}

/// "12 words · blocks, alerts", under a rule's name.
fn summary(r: &pb::AutoModRule, providers: &[pb::AutoModProvider]) -> String {
    if r.trigger == T::Provider as i32 {
        let chosen = providers.iter().find(|p| p.id == r.provider);
        let name = chosen.map_or("Provider turned off on this instance", |p| p.name.as_str());
        let n = r.labels.iter().filter(|l| level_of(l.level) != L::Off).count();
        let pictures = if chosen.is_some_and(|p| p.pictures) && r.pictures { " · with pictures" } else { "" };
        return format!("{name} · watching {n} {}{pictures}", if n == 1 { "kind" } else { "kinds" });
    }
    let what = match T::try_from(r.trigger).unwrap_or(T::Keywords) {
        T::MentionSpam => format!("More than {} pings", r.mention_limit),
        T::Links => format!("{} allowed {}", r.allowed.len(), if r.allowed.len() == 1 { "site" } else { "sites" }),
        _ => format!("{} {}", r.keywords.len(), if r.keywords.len() == 1 { "word" } else { "words" }),
    };
    let doing: Vec<&str> = [(K::Block, "blocks"), (K::Alert, "alerts"), (K::TimeOut, "times out")]
        .into_iter()
        .filter(|(k, _)| action(r, *k).is_some())
        .map(|(_, s)| s)
        .collect();
    if doing.is_empty() { what } else { format!("{what} · {}", doing.join(", ")) }
}

fn hue(h: f32, p: &Palette) -> Hsla {
    hsla(h, 0.75, if p.dark { 0.65 } else { 0.5 }, 1.0)
}

impl ServerSettingsView {
    fn load_automod(&mut self, cx: &mut Context<Self>) {
        if self.automod.loading || self.automod.rules.is_some() {
            return;
        }
        self.automod.loading = true;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.automod_rules(&key, &sid).await }, |this, result, cx| {
            this.automod.loading = false;
            match result {
                Ok((list, providers)) => {
                    this.automod.providers = providers;
                    this.automod.rules = Some(
                        list.into_iter()
                            .map(|r| Rule { key: r.id.clone(), saved: Some(r.clone()), draft: r })
                            .collect(),
                    )
                }
                Err(err) => {
                    this.automod.rules = Some(Vec::new());
                    this.error = Some(err.message);
                }
            }
            cx.notify();
        });
    }

    fn rule_mut(&mut self, key: &str) -> Option<&mut Rule> {
        self.automod.rules.as_mut()?.iter_mut().find(|r| r.key == key)
    }

    /// Changes the open rule's draft.
    fn edit_rule(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut pb::AutoModRule)) {
        let Some(key) = self.automod.filled_for.clone() else { return };
        let Some(rule) = self.rule_mut(&key) else { return };
        let before = rule.draft.clone();
        change(&mut rule.draft);
        if rule.draft != before {
            self.automod.problem = None;
            cx.notify();
        }
    }

    /// Words typed into a list box: Enter or a comma adds them, and so does leaving the box.
    fn take_words(
        &mut self,
        state: Entity<InputState>,
        allowed: bool,
        e: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = state.read(cx).value().to_string();
        let (typed, rest) = match e {
            InputEvent::PressEnter { .. } | InputEvent::Blur => (text.clone(), String::new()),
            InputEvent::Change => match text.rfind([',', '\n']) {
                Some(at) => (text[..at].to_owned(), text[at + 1..].to_owned()),
                None => return cx.notify(),
            },
            _ => return,
        };
        if typed.trim().is_empty() && rest == text {
            return;
        }
        let words = |r: &pb::AutoModRule| if allowed { &r.allowed } else { &r.keywords }.clone();
        self.edit_rule(cx, |r| {
            let max = if allowed { MAX_ALLOWED } else { MAX_WORDS };
            let next = add_words(&words(r), &typed, max);
            if allowed { r.allowed = next } else { r.keywords = next }
        });
        state.update(cx, |s, cx| s.set_value(rest, window, cx));
        self.try_rule(cx);
    }

    /// Tries what's in the tester against the open rule as it stands, a
    /// moment after the last keystroke.
    fn try_rule(&mut self, cx: &mut Context<Self>) {
        let text = self.automod.tester.read(cx).value().trim().to_owned();
        self.automod.test_seq += 1;
        let seq = self.automod.test_seq;
        if text.is_empty() {
            self.automod.tried = None;
            cx.notify();
            return;
        }
        let Some(rule) = self.automod.filled_for.clone().and_then(|k| self.rule_mut(&k).map(|r| r.draft.clone()))
        else {
            return;
        };
        self.automod.tried = Some(Tried::Checking);
        cx.notify();
        // A smart filter asks its provider each time, so it waits for a pause in typing.
        let wait = if rule.trigger == T::Provider as i32 { 700 } else { 250 };
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(wait)).await;
            let Ok(true) = this.update(cx, |this, _| this.automod.test_seq == seq) else { return };
            let rx = core.spawn({
                let core = core.clone();
                async move { core.test_automod_rule(&key, &sid, rule, &text).await }
            });
            let result = rx.await;
            let _ = this.update(cx, |this, cx| {
                if this.automod.test_seq != seq {
                    return;
                }
                this.automod.tried = match result {
                    Ok(Ok(r)) if !r.error.is_empty() => Some(Tried::Failed(r.error, r.elapsed_ms)),
                    Ok(Ok(r)) if r.matched => Some(Tried::Caught(r.matches, r.elapsed_ms)),
                    Ok(Ok(r)) => Some(Tried::Through(r.elapsed_ms)),
                    _ => None,
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn add_rule(&mut self, trigger: T, cx: &mut Context<Self>) {
        if self.automod.rules.is_none() {
            return;
        }
        self.automod.next += 1;
        let key = format!("new-{}", self.automod.next);
        let alert = self.text_channels().first().map(|c| c.id.clone()).unwrap_or_default();
        let Some(rules) = self.automod.rules.as_mut() else { return };
        rules.push(Rule {
            key: key.clone(),
            saved: None,
            draft: fresh(trigger, self.automod.providers.first(), &alert),
        });
        self.automod.open = Some(key);
        self.automod.confirming = None;
        cx.notify();
    }

    fn save_rule(&mut self, key: String, draft: pb::AutoModRule, cx: &mut Context<Self>) {
        if self.automod.busy.is_some() {
            return;
        }
        self.automod.busy = Some(key.clone());
        self.automod.problem = None;
        let (core, k, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.save_automod_rule(&k, &sid, draft).await }, move |this, result, cx| {
            this.automod.busy = None;
            match result {
                Ok(saved) => {
                    let id = saved.id.clone();
                    let follow = this.automod.open.as_deref() == Some(key.as_str());
                    if let Some(rule) = this.rule_mut(&key) {
                        // Edits made while it saved stay edits.
                        let kept = rule.draft.clone();
                        rule.key = id.clone();
                        rule.saved = Some(saved.clone());
                        rule.draft = if kept.id.is_empty() || kept == saved { saved } else { kept };
                        rule.draft.id = id.clone();
                    }
                    if follow {
                        this.automod.open = Some(id.clone());
                    }
                    if this.automod.filled_for.as_deref() == Some(key.as_str()) {
                        this.automod.filled_for = Some(id);
                    }
                    this.flash_saved(cx);
                }
                Err(err) => {
                    this.automod.problem = Some((key.clone(), err.message));
                    this.automod.shook = Some((key, Instant::now()));
                }
            }
            cx.notify();
        });
        cx.notify();
    }

    /// The switch on a rule's head: saved at once, unless it has edits of its own.
    fn switch_rule(&mut self, key: String, on: bool, cx: &mut Context<Self>) {
        let Some(rule) = self.rule_mut(&key) else { return };
        if rule.saved.is_none() || rule.dirty() {
            rule.draft.enabled = on;
            cx.notify();
            return;
        }
        rule.draft.enabled = on;
        let draft = rule.draft.clone();
        self.save_rule(key, draft, cx);
    }

    fn delete_rule(&mut self, key: String, cx: &mut Context<Self>) {
        self.automod.confirming = None;
        let saved = self.rule_mut(&key).and_then(|r| r.saved.clone());
        let Some(saved) = saved else {
            if let Some(rules) = self.automod.rules.as_mut() {
                rules.retain(|r| r.key != key);
            }
            cx.notify();
            return;
        };
        self.automod.busy = Some(key.clone());
        let (core, k, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.delete_automod_rule(&k, &sid, &saved.id).await }, move |this, result, cx| {
            this.automod.busy = None;
            match result {
                Ok(()) => {
                    if let Some(rules) = this.automod.rules.as_mut() {
                        rules.retain(|r| r.key != key);
                    }
                }
                Err(err) => this.automod.problem = Some((key, err.message)),
            }
            cx.notify();
        });
        cx.notify();
    }

    pub(super) fn automod_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.load_automod(cx);
        // Keep the boxes filled for the open rule.
        if self.automod.open != self.automod.filled_for {
            self.automod.filled_for = self.automod.open.clone();
            self.automod.tried = None;
            let draft = self.automod.open.clone().and_then(|k| self.rule_mut(&k).map(|r| r.draft.clone()));
            if let Some(d) = draft {
                let message = action(&d, K::Block).map(|a| a.message.clone()).unwrap_or_default();
                let limit = d.mention_limit.clamp(1, 50) as f32;
                self.automod.name.update(cx, |s, cx| s.set_value(d.name.clone(), window, cx));
                self.automod.message.update(cx, |s, cx| s.set_value(message, window, cx));
                for input in [&self.automod.words, &self.automod.allowed, &self.automod.tester] {
                    input.update(cx, |s, cx| s.set_value("", window, cx));
                }
                self.automod.limit.update(cx, |s, cx| s.set_value(limit, window, cx));
                for l in &d.labels {
                    if let Some(slider) = self.automod.sure.get(&l.label) {
                        let t = if l.threshold == 0 { SURE } else { l.threshold } as f32;
                        slider.update(cx, |s, cx| s.set_value(t, window, cx));
                    }
                }
            }
        }

        let intro = div()
            .flex()
            .items_start()
            .gap(px(12.0))
            .p(px(12.0))
            .rounded(corner(16.0))
            .border_1()
            .border_color(alpha(p.success, 0.3))
            .bg(alpha(p.success, 0.06))
            .child(
                div()
                    .size(px(34.0))
                    .flex_none()
                    .rounded(corner(12.0))
                    .bg(alpha(p.success, 0.15))
                    .text_color(p.success)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(motion::once(
                        icon("shield-check").size(px(17.0)),
                        "automod-shield",
                        Duration::from_millis(1200),
                        |el, t| {
                            el.rotate(gpui_kit::radians((t * std::f32::consts::TAU * 1.5).sin() * 0.16 * (1.0 - t)))
                        },
                    )),
            )
            .child(div().flex_1().min_w_0().text_sm().text_color(p.muted_foreground).child(
                "AutoMod reads each message as it's sent or edited, before anyone sees it. People who can manage \
                 the server are never caught, so try a rule with the box under it rather than in chat.",
            ));
        let mut page =
            div().flex().flex_col().gap(px(24.0)).child(motion::rise(intro, "automod-intro", Duration::ZERO, 8.0));

        let Some(rules) = self.automod.rules.as_ref() else {
            return page.child(shimmer_rows(3, p)).into_any_element();
        };
        let keys: Vec<(String, i32)> = rules.iter().map(|r| (r.key.clone(), r.draft.trigger)).collect();
        for (n, (trigger, label, blurb, glyph, h, max)) in KINDS.into_iter().enumerate() {
            let mine: Vec<&String> = keys.iter().filter(|(_, t)| *t == trigger as i32).map(|(k, _)| k).collect();
            let color = hue(h, p);
            // A smart filter needs a provider the instance turned on.
            let unavailable = trigger == T::Provider && self.automod.providers.is_empty();
            let room = mine.len() < max && !unavailable;
            let mut section = div().flex().flex_col().gap(px(10.0)).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        div()
                            .size(px(40.0))
                            .flex_none()
                            .rounded(corner(14.0))
                            .bg(gpui_kit::linear_gradient(
                                135.0,
                                gpui_kit::linear_color_stop(color.opacity(0.22), 0.0),
                                gpui_kit::linear_color_stop(color.opacity(0.0), 1.0),
                            ))
                            .text_color(color)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(glyph).size(px(20.0))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(label))
                            .child(div().text_sm().text_color(p.muted_foreground).child(blurb)),
                    )
                    .when(room, |el| {
                        let first = mine.is_empty();
                        let id = SharedString::from(format!("automod-add-{n}"));
                        let label = if first { "Set up" } else { "Another list" };
                        let button = if first { primary_button(id, label, p) } else { soft_button(id, label, p) };
                        el.child(
                            button
                                .flex_none()
                                .group("automod-add")
                                .child(motion::once(
                                    icon("plus").size(px(15.0)),
                                    SharedString::from(format!("automod-add-plus-{n}")),
                                    Duration::from_millis(500),
                                    |el, t| el.rotate(gpui_kit::radians(t * std::f32::consts::FRAC_PI_2)),
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| this.add_rule(trigger, cx))),
                        )
                    }),
            );
            if unavailable && mine.is_empty() {
                section = section.child(motion::rise(
                    div()
                        .p(px(12.0))
                        .rounded(corner(16.0))
                        .border_1()
                        .border_dashed()
                        .border_color(p.border)
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child(
                            "This instance hasn't turned on a moderation service yet. Its admins can, under Instance \
                             settings, Moderation: TypeSafe Jev and Cloudflare Clef take a key and one switch.",
                        ),
                    "automod-no-provider",
                    Duration::ZERO,
                    6.0,
                ));
            }
            for (k, key) in mine.into_iter().enumerate() {
                section = section.child(self.rule_card(key, k, p, window, cx));
            }
            page = page.child(motion::rise(
                section,
                SharedString::from(format!("automod-kind-{n}")),
                Duration::from_millis(60 * n as u64),
                12.0,
            ));
        }
        page.into_any_element()
    }

    fn rule_card(
        &mut self,
        key: &str,
        n: usize,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(rule) = self.automod.rules.as_ref().and_then(|l| l.iter().find(|r| r.key == key)) else {
            return div().into_any_element();
        };
        let (draft, is_new, dirty) = (rule.draft.clone(), rule.saved.is_none(), rule.dirty());
        let label = KINDS.iter().find(|k| k.0 as i32 == draft.trigger).map_or("Rule", |k| k.1);
        let open = self.automod.open.as_deref() == Some(key);
        let id = key.to_owned();
        let chevron =
            motion::follow(SharedString::from(format!("automod-chev-{id}")), if open { 1.0 } else { 0.0 }, window, cx);
        let toggle = id.clone();
        let flip = id.clone();
        let head = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .p(px(12.0))
            .pl(px(16.0))
            .child(
                div()
                    .id(SharedString::from(format!("automod-head-{id}")))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.automod.open = if this.automod.open.as_deref() == Some(toggle.as_str()) {
                            None
                        } else {
                            Some(toggle.clone())
                        };
                        this.automod.confirming = None;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().font_weight(FontWeight::BOLD).child(if draft.name.is_empty() {
                                label.to_owned()
                            } else {
                                draft.name.clone()
                            }))
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .child(summary(&draft, &self.automod.providers)),
                            ),
                    )
                    .when(dirty && !is_new, |el| {
                        el.child(motion::once(
                            pill("UNSAVED", p.primary.into()),
                            SharedString::from(format!("automod-unsaved-{id}")),
                            Duration::from_millis(260),
                            |el, t| el.opacity(t).relative().top(px(4.0 * (1.0 - t))),
                        ))
                    })
                    .child(
                        icon("chevron-down")
                            .size(px(16.0))
                            .text_color(p.muted_foreground)
                            .rotate(gpui_kit::radians(chevron * std::f32::consts::PI)),
                    ),
            )
            .child(switch(
                SharedString::from(format!("automod-on-{id}")),
                draft.enabled,
                self.automod.busy.is_some(),
                cx,
                move |this, on, cx| this.switch_rule(flip.clone(), on, cx),
            ));

        let hover_border = alpha(p.primary, 0.3);
        let card = div()
            .id(SharedString::from(format!("automod-{id}")))
            .rounded(corner(20.0))
            .border_1()
            .bg(p.card)
            .map(|el| {
                if open {
                    el.border_color(alpha(p.primary, 0.35)).shadow_lg()
                } else {
                    el.border_color(p.border).hover(move |s| s.border_color(hover_border))
                }
            })
            .when(!draft.enabled && !open, |el| el.opacity(0.7))
            .child(head)
            .when(open, |el| el.child(self.rule_body(&id, &draft, is_new, dirty, label, p, window, cx)));
        let card =
            match self.automod.shook.as_ref().filter(|(k, at)| k == key && at.elapsed() < Duration::from_millis(500)) {
                Some((_, at)) => card
                    .with_animation(
                        SharedString::from(format!("automod-shake-{at:?}")),
                        gpui_kit::Animation::new(Duration::from_millis(400)),
                        |el, t| el.relative().left(px((t * std::f32::consts::TAU * 2.5).sin() * 8.0 * (1.0 - t))),
                    )
                    .into_any_element(),
                None => card.into_any_element(),
            };
        motion::rise(
            div().child(card),
            SharedString::from(format!("automod-in-{id}")),
            Duration::from_millis((n.min(6) * 40) as u64),
            8.0,
        )
        .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn rule_body(
        &mut self,
        id: &str,
        draft: &pb::AutoModRule,
        is_new: bool,
        dirty: bool,
        label: &str,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let trigger = T::try_from(draft.trigger).unwrap_or(T::Keywords);
        let busy = self.automod.busy.as_deref() == Some(id);
        let problem = self.automod.problem.as_ref().filter(|(k, _)| k == id).map(|(_, m)| m.clone());

        let what: AnyElement = match trigger {
            T::MentionSpam => field(
                "Ping limit",
                "Each person or role counts once; @everyone and @here count as one together.",
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.0))
                    .child(div().flex_1().child(Slider::new(&self.automod.limit)))
                    .child(motion::once(
                        div()
                            .w(px(110.0))
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child(format!("More than {}", draft.mention_limit)),
                        SharedString::from(format!("automod-limit-{}", draft.mention_limit)),
                        Duration::from_millis(220),
                        |el, t| el.opacity(0.5 + 0.5 * t).relative().top(px(-3.0 * (1.0 - t))),
                    )),
                p,
            ),
            T::Links => field(
                "Allowed sites",
                "Every other link is caught. Leave it empty to catch them all.",
                self.word_list(id, &draft.allowed, true, MAX_ALLOWED, p, cx),
                p,
            ),
            _ => div()
                .flex()
                .flex_col()
                .gap(px(18.0))
                .child(field(
                    "Words and phrases",
                    "Matched whole, ignoring case. A * lets a word run on: *cat catches “bobcat”, cat* catches “catapult”.",
                    self.word_list(id, &draft.keywords, false, MAX_WORDS, p, cx),
                    p,
                ))
                .child(field(
                    "Allowed anyway",
                    "Words the list would catch that are fine, like “class” under *ass*.",
                    self.word_list(id, &draft.allowed, true, MAX_ALLOWED, p, cx),
                    p,
                ))
                .into_any_element(),
        };

        let footer = {
            let confirming = self.automod.confirming.as_deref() == Some(id);
            let (k1, k2, k3, k4) = (id.to_owned(), id.to_owned(), id.to_owned(), id.to_owned());
            let named = if draft.name.is_empty() { label.to_owned() } else { draft.name.clone() };
            let red = alpha(p.destructive, 0.1);
            let left: AnyElement = if confirming {
                motion::slide_in(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            danger_button(
                                SharedString::from(format!("automod-del-yes-{id}")),
                                format!("Delete {named}"),
                                p,
                            )
                            .h(px(32.0))
                            .px(px(12.0))
                            .text_xs()
                            .on_click(cx.listener(move |this, _, _, cx| this.delete_rule(k1.clone(), cx))),
                        )
                        .child(soft_button(SharedString::from(format!("automod-del-no-{id}")), "Keep it", p).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.automod.confirming = None;
                                cx.notify();
                            }),
                        )),
                    SharedString::from(format!("automod-ask-{id}")),
                    -8.0,
                )
                .into_any_element()
            } else {
                div()
                    .id(SharedString::from(format!("automod-del-{id}")))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .h(px(36.0))
                    .px(px(12.0))
                    .rounded(corner(12.0))
                    .cursor_pointer()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .hover({
                        let c = p.destructive;
                        move |s| s.bg(red).text_color(c)
                    })
                    .child(icon(if is_new { "x" } else { "trash" }).size(px(14.0)))
                    .child(if is_new { "Cancel" } else { "Delete" })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if is_new {
                            this.delete_rule(k2.clone(), cx)
                        } else {
                            this.automod.confirming = Some(k2.clone());
                            cx.notify();
                        }
                    }))
                    .into_any_element()
            };
            let saving = busy && self.automod.confirming.is_none();
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .pt(px(16.0))
                .border_t_1()
                .border_color(p.border)
                .child(left)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_right()
                        .text_sm()
                        .text_color(p.destructive)
                        .when_some(problem, |el, m| el.child(m)),
                )
                .when(dirty && !is_new, |el| {
                    el.child(soft_button(SharedString::from(format!("automod-discard-{id}")), "Discard", p).on_click(
                        cx.listener(move |this, _, _, cx| {
                            if let Some(rule) = this.rule_mut(&k3)
                                && let Some(saved) = rule.saved.clone()
                            {
                                rule.draft = saved;
                            }
                            // Fill the boxes again from what's saved.
                            this.automod.filled_for = None;
                            this.automod.problem = None;
                            cx.notify();
                        }),
                    ))
                })
                .child(
                    primary_button(
                        SharedString::from(format!("automod-save-{id}")),
                        if is_new { "Create rule" } else { "Save" },
                        p,
                    )
                    .when(!dirty || saving, |el| el.opacity(0.55))
                    .child(if saving {
                        spinner(format!("automod-save-spin-{id}"), 15.0, window)
                    } else {
                        icon("check").size(px(15.0)).into_any_element()
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let Some(rule) = this.rule_mut(&k4) else { return };
                        if rule.dirty() {
                            let draft = rule.draft.clone();
                            this.save_rule(k4.clone(), draft, cx)
                        }
                    })),
                )
        };

        motion::rise(
            div()
                .flex()
                .flex_col()
                .gap(px(20.0))
                .p(px(16.0))
                .border_t_1()
                .border_color(p.border)
                .child(field("Name", "", Input::new(&self.automod.name), p))
                .map(|el| {
                    if trigger == T::Provider {
                        el.child(self.provider_picker(draft, p, cx))
                            .children(self.labels(draft, p, window, cx))
                            .child(self.tester(true, p, window))
                            .child(self.smart_actions(draft, p, cx))
                    } else {
                        el.child(what).child(self.tester(false, p, window)).child(self.rule_actions(draft, p, cx))
                    }
                })
                .child(self.exemptions(draft, p, cx))
                .child(footer),
            SharedString::from(format!("automod-body-{id}")),
            Duration::ZERO,
            6.0,
        )
        .into_any_element()
    }

    /// Words as chips (crossed out by the ×), and a box to type more.
    #[allow(clippy::too_many_arguments)]
    fn word_list(
        &self,
        id: &str,
        words: &[String],
        allowed: bool,
        max: usize,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let color = if allowed { p.success.into() } else { hue(0.97, p) };
        let mut chips = div().flex().flex_wrap().items_center().gap(px(6.0));
        for (n, w) in words.iter().enumerate() {
            let gone = w.clone();
            chips = chips.child(motion::once(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .h(px(28.0))
                    .pl(px(9.0))
                    .pr(px(4.0))
                    .rounded(corner(8.0))
                    .bg(color.opacity(0.13))
                    .text_color(color)
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child(w.clone())
                    .child(
                        div()
                            .id(SharedString::from(format!("automod-word-x-{allowed}-{n}-{w}")))
                            .size(px(18.0))
                            .rounded(px(5.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .opacity(0.6)
                            .hover(|s| s.opacity(1.0))
                            .child(icon("x").size(px(12.0)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.edit_rule(cx, |r| {
                                    if allowed { &mut r.allowed } else { &mut r.keywords }.retain(|x| *x != gone)
                                });
                                this.try_rule(cx);
                            })),
                    ),
                SharedString::from(format!("automod-word-{id}-{allowed}-{w}")),
                Duration::from_millis(260),
                |el, t| {
                    let s = 0.6 + 0.4 * t;
                    el.opacity(t).relative().top(px(4.0 * (1.0 - s) * 5.0))
                },
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .when(!words.is_empty(), |el| el.child(chips))
            .child(Input::new(if allowed { &self.automod.allowed } else { &self.automod.words }))
            .when(!words.is_empty(), |el| {
                el.child(
                    div().text_xs().text_right().text_color(p.muted_foreground).child(format!("{}/{max}", words.len())),
                )
            })
            .into_any_element()
    }

    /// Try a message against the rule as it stands, saved or not.
    fn tester(&self, smart: bool, p: &Palette, window: &mut Window) -> AnyElement {
        // How long the provider took, at the end of a smart filter's answer.
        let took = |ms: i32| {
            div().ml_auto().text_xs().text_color(p.muted_foreground).when(smart, |el| el.child(format!("{ms} ms")))
        };
        let result: Option<AnyElement> = self.automod.tried.as_ref().map(|t| match t {
            Tried::Checking => div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_sm()
                .text_color(p.muted_foreground)
                .child(spinner("automod-try-spin", 14.0, window))
                .child(if smart { "Asking the provider…" } else { "Checking…" })
                .into_any_element(),
            Tried::Failed(error, ms) => div()
                .flex()
                .items_start()
                .gap(px(8.0))
                .child(div().flex_1().min_w_0().text_xs().text_color(amber(p)).child(format!(
                    "The provider didn't answer: {error}. Messages go through this rule unchecked until it \
                         does; your other rules still apply."
                )))
                .child(took(*ms))
                .into_any_element(),
            Tried::Through(ms) => div()
                .flex()
                .items_center()
                .child(motion::rise(pill("✓ GETS THROUGH", p.success.into()), "automod-through", Duration::ZERO, 6.0))
                .child(took(*ms))
                .into_any_element(),
            Tried::Caught(matches, ms) => {
                let mut row = div().flex().flex_wrap().items_center().gap(px(6.0)).child(motion::rise(
                    pill("CAUGHT", p.destructive.into()),
                    "automod-caught",
                    Duration::ZERO,
                    6.0,
                ));
                for (n, m) in matches.iter().enumerate() {
                    row = row.child(motion::rise(
                        div()
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded(px(6.0))
                            .bg(alpha(p.destructive, 0.1))
                            .text_color(p.destructive)
                            .font_family("monospace")
                            .text_xs()
                            .child(m.clone()),
                        SharedString::from(format!("automod-match-{n}-{m}")),
                        Duration::from_millis(50 * n.min(8) as u64),
                        6.0,
                    ));
                }
                row.child(took(*ms)).into_any_element()
            }
        });
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(12.0))
            .rounded(corner(16.0))
            .bg(alpha(p.muted, 0.5))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_sm()
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(icon("flask-conical").size(px(15.0)).text_color(p.primary))
                    .child("Try a message"),
            )
            .child(Input::new(&self.automod.tester))
            .when(smart, |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .child("What you write here goes to the provider, the same way members' messages will."),
                )
            })
            .when_some(result, |el, r| el.child(r))
            .into_any_element()
    }

    /// What happens to a message the rule catches: any of block, alert and time out.
    fn rule_actions(&self, draft: &pb::AutoModRule, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let block = action(draft, K::Block).is_some();
        let alert = action(draft, K::Alert).map(|a| a.channel_id.clone());
        let time_out = action(draft, K::TimeOut).map(|a| i64::from(a.duration_seconds));
        let channels = self.text_channels();
        let first = channels.first().map(|c| c.id.clone()).unwrap_or_default();

        let mut picks = div().flex().flex_wrap().gap(px(6.0));
        for c in &channels {
            let on = alert.as_deref() == Some(c.id.as_str());
            let cid = c.id.clone();
            picks = picks.child(
                chip(SharedString::from(format!("automod-alert-{}", c.id)), &format!("# {}", c.name), on, p).on_click(
                    cx.listener(move |this, _, _, cx| {
                        let cid = cid.clone();
                        this.edit_rule(cx, |r| set_action(r, K::Alert, true, |a| a.channel_id = cid))
                    }),
                ),
            );
        }
        let picked = time_out.unwrap_or(60);
        let mut lengths: Vec<i64> = TIME_OUTS.to_vec();
        if !lengths.contains(&picked) {
            lengths.push(picked);
            lengths.sort();
        }
        let mut times = div().flex().flex_wrap().gap(px(6.0));
        for s in lengths {
            times = times.child(
                chip(SharedString::from(format!("automod-time-{s}")), &duration(s), s == picked, p).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.edit_rule(cx, |r| set_action(r, K::TimeOut, true, |a| a.duration_seconds = s as i32))
                    }),
                ),
            );
        }

        let cards = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(action_card(
                "block",
                block,
                "ban",
                "Block the message",
                "It never reaches the channel. Its author sees why.",
                Input::new(&self.automod.message).into_any_element(),
                p,
                cx,
                |this, on, cx| this.edit_rule(cx, |r| set_action(r, K::Block, on, |_| {})),
            ))
            .child(action_card(
                "alert",
                alert.is_some(),
                "bell-ring",
                "Alert a channel",
                "Posts what was caught, who said it and where, for your mods.",
                picks.into_any_element(),
                p,
                cx,
                move |this, on, cx| {
                    let first = first.clone();
                    this.edit_rule(cx, |r| {
                        set_action(r, K::Alert, on, |a| {
                            if a.channel_id.is_empty() {
                                a.channel_id = first
                            }
                        })
                    })
                },
            ))
            .child(action_card(
                "timeout",
                time_out.is_some(),
                "timer",
                "Time them out",
                "They can read but not talk for a while.",
                times.into_any_element(),
                p,
                cx,
                |this, on, cx| {
                    this.edit_rule(cx, |r| {
                        set_action(r, K::TimeOut, on, |a| {
                            if a.duration_seconds == 0 {
                                a.duration_seconds = 60
                            }
                        })
                    })
                },
            ));
        field("When it catches one", "Pick any. Without blocking, the message is still sent.", cards, p)
    }

    /// Which of the instance's moderation services reads messages, and what it gets to see.
    fn provider_picker(&self, draft: &pb::AutoModRule, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let providers = &self.automod.providers;
        let chosen = providers.iter().find(|x| x.id == draft.provider);
        let mut out = div().flex().flex_col().gap(px(8.0));
        if providers.len() > 1 {
            let options: Vec<(String, String)> = providers.iter().map(|x| (x.id.clone(), x.name.clone())).collect();
            out = out.child(text_chips("automod-provider", &options, &draft.provider, p, cx, |this, id, cx| {
                let pictures = this.automod.providers.iter().any(|x| x.id == id && x.pictures);
                this.edit_rule(cx, |r| {
                    r.provider = id;
                    r.pictures = pictures;
                });
                this.try_rule(cx);
            }));
        }
        let violet = hue(0.76, p);
        let warn = amber(p);
        let note = match chosen {
            Some(x) => {
                let gets = format!(
                    ". It gets the text{} alone: never who wrote it, where, or this server's name, and mentions and \
                     custom emoji are swapped for placeholders first. Your instance sends it, never anyone's app.",
                    if x.pictures && draft.pictures { " and pictures" } else { "" }
                );
                div().flex_1().min_w_0().text_sm().text_color(p.muted_foreground).child(
                    crate::ui::instance_settings::emphasized(
                        &[
                            (&x.name, true),
                            (" reads the messages this rule checks, at ", false),
                            (&x.host, true),
                            (&gets, false),
                        ],
                        p,
                    ),
                )
            }
            None => div().flex_1().min_w_0().text_sm().text_color(warn).child(
                "This instance turned that provider off, so the rule lets every message through. Pick another, or \
                 ask the instance's admins.",
            ),
        };
        let tint = if chosen.is_some() { violet } else { warn };
        out = out.child(motion::rise(
            div()
                .flex()
                .items_start()
                .gap(px(10.0))
                .p(px(12.0))
                .rounded(corner(16.0))
                .border_1()
                .border_color(tint.opacity(0.35))
                .bg(tint.opacity(0.06))
                .child(icon("globe-lock").size(px(16.0)).text_color(tint))
                .child(note),
            SharedString::from(format!("automod-provider-note-{}", draft.provider)),
            Duration::ZERO,
            6.0,
        ));
        if chosen.is_some_and(|x| x.pictures) {
            out = out.child(self.pictures(draft, p, cx));
        }
        field("Provider", "The services this instance's admins turned on.", out, p)
    }

    /// Whether the provider sees messages' pictures too, for providers that read them.
    fn pictures(&self, draft: &pb::AutoModRule, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let on = draft.pictures;
        let set = |this: &mut ServerSettingsView, on: bool, cx: &mut Context<ServerSettingsView>| {
            this.edit_rule(cx, |r| r.pictures = on);
        };
        motion::rise(
            div()
                .id("automod-pictures")
                .flex()
                .items_center()
                .gap(px(12.0))
                .p(px(12.0))
                .rounded(corner(16.0))
                .border_1()
                .cursor_pointer()
                .map(|el| {
                    if on {
                        el.border_color(alpha(p.primary, 0.4)).bg(alpha(p.primary, 0.05))
                    } else {
                        el.border_color(p.border).hover(|s| s.border_color(alpha(p.primary, 0.2)))
                    }
                })
                .on_click(cx.listener(move |this, _, _, cx| set(this, !on, cx)))
                .child(motion::once(
                    div()
                        .flex_none()
                        .size(px(32.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(corner(11.0))
                        .map(|el| {
                            if on {
                                el.bg(p.primary).text_color(p.primary_foreground)
                            } else {
                                el.bg(p.muted).text_color(p.muted_foreground)
                            }
                        })
                        .child(icon("image").size(px(16.0))),
                    SharedString::from(format!("automod-pictures-{on}")),
                    Duration::from_millis(350),
                    // A little hop when it turns on, as on the web.
                    move |el, t| {
                        if on {
                            let bump = (t * std::f32::consts::PI).sin();
                            el.relative().top(px(-4.0 * bump))
                        } else {
                            el
                        }
                    },
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().font_weight(FontWeight::BOLD).child("Check pictures too"))
                        .child(div().text_xs().text_color(p.muted_foreground).child(
                            "Up to 4 per message, attached or in embeds (PNG, JPEG or WebP). Your instance fetches \
                             them and sends them along; GIFs and very large photos are skipped.",
                        )),
                )
                .child(switch("automod-pictures-switch".into(), on, false, cx, set)),
            "automod-pictures-in",
            Duration::ZERO,
            -6.0,
        )
        .into_any_element()
    }

    /// "How sure" for one label, made the first time it's shown.
    fn sure_slider(
        &mut self,
        label: &str,
        value: i32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<SliderState> {
        if let Some(slider) = self.automod.sure.get(label) {
            return slider.clone();
        }
        let slider = cx.new(|_| SliderState::new().min(50.0).max(99.0).step(1.0).default_value(value as f32));
        let id = label.to_owned();
        let sub = cx.subscribe(&slider, move |this: &mut ServerSettingsView, _, e: &SliderEvent, cx| {
            let SliderEvent::Change(v) = e else { return };
            let t = v.start().round() as i32;
            let id = id.clone();
            this.edit_rule(cx, |r| {
                if r.labels.iter().any(|l| l.label == id) {
                    set_label(r, &id, None, Some(t))
                }
            });
            this.try_rule(cx);
        });
        self.automod.sure_subscriptions.push(sub);
        self.automod.sure.insert(label.to_owned(), slider.clone());
        let _ = window;
        slider
    }

    /// Each kind of message the provider tells apart: what happens to it, and how sure it must be.
    fn labels(
        &mut self,
        draft: &pb::AutoModRule,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let provider = self.automod.providers.iter().find(|x| x.id == draft.provider).cloned()?;
        if provider.labels.is_empty() {
            return None;
        }
        let mut list = div().flex().flex_col().gap(px(8.0));
        for (n, label) in provider.labels.iter().enumerate() {
            let rule = draft.labels.iter().find(|l| l.label == label.id);
            let level = level_of(rule.map_or(label.default_level, |r| r.level));
            let threshold = rule.map(|r| r.threshold).filter(|t| *t > 0).unwrap_or(SURE);
            let off = level == L::Off;
            // The choice slides between the four, as on the web.
            const CELL: f32 = 70.0;
            let at = LEVELS.iter().position(|(l, _)| *l == level).unwrap_or(0) as f32;
            let x = motion::follow(
                SharedString::from(format!("automod-level-{}-{}", draft.id, label.id)),
                at * CELL,
                window,
                cx,
            );
            let fill: Hsla = match level {
                L::Block | L::TimeOut => p.destructive.into(),
                L::Flag => amber(p).opacity(0.85),
                _ => p.card.into(),
            };
            let mut seg =
                div().relative().flex_none().flex().p(px(2.0)).rounded(corner(11.0)).bg(alpha(p.muted, 0.7)).child(
                    div()
                        .absolute()
                        .top(px(2.0))
                        .left(px(2.0 + x))
                        .w(px(CELL))
                        .h(px(28.0))
                        .rounded(corner(9.0))
                        .bg(fill),
                );
            for (value, short) in LEVELS {
                let on = value == level;
                let id = label.id.clone();
                let fg = p.foreground;
                seg = seg.child(
                    div()
                        .id(SharedString::from(format!("automod-lv-{}-{}", label.id, value as i32)))
                        .relative()
                        .w(px(CELL))
                        .h(px(28.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(if on && matches!(value, L::Block | L::TimeOut) {
                            gpui_kit::white()
                        } else if on {
                            fg.into()
                        } else {
                            p.muted_foreground.into()
                        })
                        .when(!on, |el| {
                            el.cursor_pointer().hover(move |s| s.text_color(fg)).on_click(cx.listener(
                                move |this, _, _, cx| {
                                    let id = id.clone();
                                    this.edit_rule(cx, |r| set_label(r, &id, Some(value), None));
                                    this.try_rule(cx);
                                },
                            ))
                        })
                        .child(short),
                );
            }
            let mut card = div()
                .p(px(12.0))
                .rounded(corner(16.0))
                .border_1()
                .map(|el| {
                    if off {
                        el.border_color(p.border)
                    } else {
                        el.border_color(alpha(p.primary, 0.25)).bg(alpha(p.primary, 0.03))
                    }
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .when(off, |el| el.opacity(0.6))
                                .child(div().text_sm().font_weight(FontWeight::BOLD).child(label.name.clone()))
                                .child(div().text_xs().text_color(p.muted_foreground).child(label.description.clone())),
                        )
                        .child(seg),
                );
            if !off {
                let slider = self.sure_slider(&label.id, threshold, window, cx);
                card = card.child(motion::rise(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .pt(px(10.0))
                        .child(div().flex_none().text_xs().text_color(p.muted_foreground).child("How sure"))
                        .child(div().flex_1().child(Slider::new(&slider)))
                        .child(motion::once(
                            div()
                                .w(px(40.0))
                                .text_right()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .child(format!("{threshold}%")),
                            SharedString::from(format!("automod-sure-{}-{threshold}", label.id)),
                            Duration::from_millis(220),
                            |el, t| el.opacity(0.5 + 0.5 * t).relative().top(px(-3.0 * (1.0 - t))),
                        )),
                    SharedString::from(format!("automod-sure-in-{}", label.id)),
                    Duration::ZERO,
                    -4.0,
                ));
            }
            list = list.child(motion::rise(
                card,
                SharedString::from(format!("automod-label-{}", label.id)),
                Duration::from_millis(30 * n.min(10) as u64),
                8.0,
            ));
        }
        let reset = provider.clone();
        let hint = div()
            .flex()
            .flex_wrap()
            .gap_x(px(4.0))
            .text_xs()
            .text_color(p.muted_foreground)
            .child("Flags post to your alert channel and let the message through. The bar is how sure the provider must be.")
            .child(
                div()
                    .id("automod-labels-reset")
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.primary)
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let labels = defaults(&reset);
                        this.edit_rule(cx, |r| r.labels = labels);
                        for slider in this.automod.sure.values() {
                            slider.update(cx, |s, cx| s.set_value(SURE as f32, window, cx));
                        }
                        this.try_rule(cx);
                    }))
                    .child("Back to the defaults"),
            );
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(div().text_sm().font_weight(FontWeight::EXTRA_BOLD).child("What to do about each kind"))
                .child(hint)
                .child(list)
                .into_any_element(),
        )
    }

    /// Where flags go, what blocked people are told and how long a time-out lasts, shown as the labels need them.
    fn smart_actions(&self, draft: &pb::AutoModRule, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let levels: Vec<L> = draft.labels.iter().map(|l| level_of(l.level)).collect();
        let blocks = levels.iter().any(|l| matches!(l, L::Block | L::TimeOut));
        let times_out = levels.contains(&L::TimeOut);
        let flags = levels.contains(&L::Flag);
        let alert = action(draft, K::Alert).map(|a| a.channel_id.clone()).unwrap_or_default();
        let mut options: Vec<(String, String)> = Vec::new();
        if !flags {
            options.push((String::new(), "No alerts".into()));
        }
        options.extend(self.text_channels().into_iter().map(|c| (c.id, format!("# {}", c.name))));
        let mut out = div().flex().flex_col().gap(px(18.0)).child(field(
            "Alert channel",
            if flags {
                "Flagged messages are posted here for your mods; blocked ones too."
            } else {
                "Blocked messages are posted here too, if you pick one."
            },
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(text_chips("automod-smart-alert", &options, &alert, p, cx, |this, id, cx| {
                    this.edit_rule(cx, |r| set_action(r, K::Alert, !id.is_empty(), |a| a.channel_id = id));
                }))
                .when(flags && alert.is_empty(), |el| {
                    el.child(div().text_xs().text_color(amber(p)).child("Pick a channel for flags."))
                }),
            p,
        ));
        if blocks {
            out = out.child(motion::rise(
                div().child(field("What blocked people are told", "Optional.", Input::new(&self.automod.message), p)),
                "automod-smart-block",
                Duration::ZERO,
                6.0,
            ));
        }
        if times_out {
            let picked =
                action(draft, K::TimeOut).map(|a| i64::from(a.duration_seconds)).filter(|s| *s > 0).unwrap_or(600);
            let mut times = div().flex().flex_wrap().gap(px(6.0));
            for s in TIME_OUTS {
                times = times.child(
                    chip(SharedString::from(format!("automod-smart-time-{s}")), &duration(s), s == picked, p).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.edit_rule(cx, |r| set_action(r, K::TimeOut, true, |a| a.duration_seconds = s as i32))
                        }),
                    ),
                );
            }
            out = out.child(motion::rise(
                div().child(field("Time-out length", "", times, p)),
                "automod-smart-time",
                Duration::ZERO,
                6.0,
            ));
        }
        out.into_any_element()
    }

    /// Roles and channels the rule leaves alone, picked as chips.
    fn exemptions(&self, draft: &pb::AutoModRule, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let (mut roles, channels) = self.core.shared.read(|s| {
            let i = s.instance(&self.key);
            let roles: Vec<pb::Role> = i
                .and_then(|i| i.roles.get(&self.server))
                .into_iter()
                .flatten()
                .filter(|r| r.id != self.server)
                .cloned()
                .collect();
            let mut channels: Vec<pb::Channel> = i
                .and_then(|i| i.channels.get(&self.server))
                .into_iter()
                .flatten()
                .filter(|c| {
                    !matches!(
                        pb::ChannelType::try_from(c.r#type).unwrap_or(pb::ChannelType::Text),
                        pb::ChannelType::Category | pb::ChannelType::Voice
                    )
                })
                .cloned()
                .collect();
            channels.sort_by_key(|c| c.position);
            (roles, channels)
        });
        roles.sort_by_key(|r| std::cmp::Reverse(r.position));
        let mut role_chips = div().flex().flex_wrap().gap(px(6.0));
        for r in &roles {
            let on = draft.exempt_role_ids.contains(&r.id);
            let rid = r.id.clone();
            role_chips =
                role_chips.child(chip(SharedString::from(format!("automod-xrole-{}", r.id)), &r.name, on, p).on_click(
                    cx.listener(move |this, _, _, cx| {
                        let rid = rid.clone();
                        this.edit_rule(cx, |d| flip(&mut d.exempt_role_ids, rid))
                    }),
                ));
        }
        let mut channel_chips = div().flex().flex_wrap().gap(px(6.0));
        for c in &channels {
            let on = draft.exempt_channel_ids.contains(&c.id);
            let cid = c.id.clone();
            channel_chips = channel_chips.child(
                chip(SharedString::from(format!("automod-xch-{}", c.id)), &format!("# {}", c.name), on, p).on_click(
                    cx.listener(move |this, _, _, cx| {
                        let cid = cid.clone();
                        this.edit_rule(cx, |d| flip(&mut d.exempt_channel_ids, cid))
                    }),
                ),
            );
        }
        let sub = |title: &str| {
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_xs()
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(p.muted_foreground)
                .child(icon(if title == "ROLES" { "shield" } else { "hash" }).size(px(12.0)))
                .child(title.to_owned())
        };
        field(
            "Leave out",
            "Roles and channels this rule never looks at.",
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .when(!roles.is_empty(), |el| el.child(sub("ROLES")).child(role_chips))
                .child(sub("CHANNELS"))
                .child(channel_chips),
            p,
        )
    }
}

fn flip(list: &mut Vec<String>, id: String) {
    if list.contains(&id) {
        list.retain(|x| *x != id);
    } else {
        list.push(id);
    }
}

/// A label, a line on what it's for, and the control under them.
fn field(label: &str, hint: &str, control: impl IntoElement, p: &Palette) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .child(div().text_sm().font_weight(FontWeight::EXTRA_BOLD).child(label.to_owned()))
                .when(!hint.is_empty(), |el| {
                    el.child(div().text_xs().text_color(p.muted_foreground).child(hint.to_owned()))
                }),
        )
        .child(control)
        .into_any_element()
}

/// One of the things a rule does: a switch, and what it needs once it's on.
#[allow(clippy::too_many_arguments)]
fn action_card(
    id: &'static str,
    on: bool,
    glyph: &str,
    title: &str,
    hint: &str,
    body: AnyElement,
    p: &Palette,
    cx: &mut Context<ServerSettingsView>,
    set: impl Fn(&mut ServerSettingsView, bool, &mut Context<ServerSettingsView>) + 'static,
) -> AnyElement {
    let tile = div()
        .size(px(32.0))
        .flex_none()
        .rounded(corner(11.0))
        .flex()
        .items_center()
        .justify_center()
        .map(|el| {
            if on {
                el.bg(p.primary).text_color(p.primary_foreground)
            } else {
                el.bg(p.muted).text_color(p.muted_foreground)
            }
        })
        .child(icon(glyph).size(px(16.0)));
    // A little hop each time it's turned on.
    let tile = if on {
        motion::once(tile, SharedString::from(format!("automod-act-on-{id}")), Duration::from_millis(350), |el, t| {
            let k = (t * std::f32::consts::PI).sin();
            el.relative().top(px(-4.0 * k))
        })
    } else {
        tile.into_any_element()
    };
    let hover = alpha(p.primary, 0.25);
    div()
        .id(SharedString::from(format!("automod-act-{id}")))
        .p(px(12.0))
        .rounded(corner(16.0))
        .border_1()
        .map(|el| {
            if on {
                el.border_color(alpha(p.primary, 0.4)).bg(alpha(p.primary, 0.05))
            } else {
                el.border_color(p.border).hover(move |s| s.border_color(hover))
            }
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(tile)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().font_weight(FontWeight::BOLD).child(title.to_owned()))
                        .child(div().text_xs().text_color(p.muted_foreground).child(hint.to_owned())),
                )
                .child(switch(SharedString::from(format!("automod-act-sw-{id}")), on, false, cx, set)),
        )
        .when(on, |el| {
            el.child(motion::rise(
                div().pt(px(12.0)).child(body),
                SharedString::from(format!("automod-act-body-{id}")),
                Duration::ZERO,
                6.0,
            ))
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_say_what_they_do() {
        let mut r = fresh(T::Keywords, None, "");
        r.keywords = vec!["cat*".into(), "dog".into()];
        assert_eq!(summary(&r, &[]), "2 words · blocks");
        set_action(&mut r, K::TimeOut, true, |a| a.duration_seconds = 60);
        set_action(&mut r, K::Alert, true, |a| a.channel_id = "c".into());
        assert_eq!(summary(&r, &[]), "2 words · blocks, alerts, times out");
        assert_eq!(r.actions.iter().map(|a| a.kind).collect::<Vec<_>>(), [1, 2, 3]);
        set_action(&mut r, K::Block, false, |_| {});
        set_action(&mut r, K::Alert, true, |a| a.duration_seconds = 9);
        assert_eq!(action(&r, K::Alert).map(|a| a.channel_id.as_str()), Some("c"), "turning on again keeps it");
        assert_eq!(summary(&fresh(T::MentionSpam, None, ""), &[]), "More than 5 pings · blocks");
        assert_eq!(summary(&fresh(T::Links, None, ""), &[]), "0 allowed sites · blocks");
    }

    #[test]
    fn smart_filters_start_from_the_providers_suggestions() {
        let label = |id: &str, level: L| pb::AutoModLabel {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            default_level: level as i32,
        };
        let jev = pb::AutoModProvider {
            id: "typesafe-jev".into(),
            name: "TypeSafe Jev".into(),
            host: "api.typesafe.ai".into(),
            labels: vec![label("hate", L::Block), label("spam", L::Flag), label("sexual", L::Off)],
            pictures: false,
        };
        let mut r = fresh(T::Provider, Some(&jev), "mods");
        assert_eq!(r.provider, "typesafe-jev");
        assert_eq!(
            r.labels.iter().map(|l| (l.label.as_str(), l.threshold)).collect::<Vec<_>>(),
            [("hate", 80), ("spam", 80), ("sexual", 80)]
        );
        assert_eq!(action(&r, K::Alert).map(|a| a.channel_id.as_str()), Some("mods"));
        assert_eq!(summary(&r, std::slice::from_ref(&jev)), "TypeSafe Jev · watching 2 kinds");
        set_label(&mut r, "spam", Some(L::Off), None);
        set_label(&mut r, "hate", None, Some(120));
        assert_eq!(r.labels[0].threshold, 99, "held to 50 to 99");
        set_label(&mut r, "scam", Some(L::TimeOut), None);
        assert_eq!(r.labels[3].threshold, 80);
        assert_eq!(summary(&r, &[]), "Provider turned off on this instance · watching 2 kinds");
        assert_eq!(level_of(0), L::Off);
        let clef =
            pb::AutoModProvider { id: "cloudflare-clef".into(), name: "Cloudflare Clef".into(), pictures: true, ..jev };
        let mut r = fresh(T::Provider, Some(&clef), "");
        assert!(r.pictures, "pictures start on where the provider reads them");
        assert_eq!(summary(&r, std::slice::from_ref(&clef)), "Cloudflare Clef · watching 2 kinds · with pictures");
        r.pictures = false;
        assert_eq!(summary(&r, std::slice::from_ref(&clef)), "Cloudflare Clef · watching 2 kinds");
    }
}
