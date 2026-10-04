//! The Welcome screen page: what new members see first, a few words and up
//! to five channels to start in, each with an emoji and a note, beside a
//! preview drawn like the real thing. The web's
//! `settings/server/WelcomeScreenEditor.tsx`.

use crate::ui::emoji::InColor as _;
use gpui_kit::component::input::Textarea;

use super::roles::switch;
use super::*;
use crate::ui::overlay::welcome_emoji;

const MAX_CHANNELS: usize = 5;
const DESCRIPTION_MAX: usize = 300;
const NOTE_MAX: usize = 60;
/// How many everyday emoji the picker offers under the server's own.
const EVERYDAY: usize = 48;

/// A suggested channel being edited, with a key that survives moving it.
struct Row {
    key: u64,
    channel_id: String,
    emoji: String,
    note: Entity<InputState>,
    _changes: Subscription,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pick {
    Emoji,
    Channel,
}

pub(super) struct Welcome {
    saved: Option<pb::WelcomeScreen>,
    loading: bool,
    enabled: bool,
    description: Entity<TextareaState>,
    rows: Vec<Row>,
    next: u64,
    /// A row whose emoji or channel is being picked, the choices open under it.
    picking: Option<(u64, Pick)>,
    /// A row just moved, and how far it came, so it glides into place.
    moved: Option<(u64, f32, Instant)>,
    saving: bool,
}

impl Welcome {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ServerSettingsView>) -> (Self, Vec<Subscription>) {
        let description = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(3, 6).placeholder("What this place is about, and where to begin.")
        });
        let subscriptions = vec![cx.subscribe(&description, |_: &mut ServerSettingsView, _, e: &InputEvent, cx| {
            if let InputEvent::Change = e {
                cx.notify()
            }
        })];
        let welcome = Self {
            saved: None,
            loading: false,
            enabled: false,
            description,
            rows: Vec::new(),
            next: 0,
            picking: None,
            moved: None,
            saving: false,
        };
        (welcome, subscriptions)
    }
}

/// The screen as it's being edited, the way it would be saved.
fn draft(enabled: bool, description: &str, rows: &[(String, String, String)]) -> pb::WelcomeScreen {
    pb::WelcomeScreen {
        enabled,
        description: description.trim().to_owned(),
        channels: rows
            .iter()
            .filter(|(c, _, _)| !c.is_empty())
            .map(|(c, note, emoji)| pb::WelcomeChannel {
                channel_id: c.clone(),
                description: note.trim().chars().take(NOTE_MAX).collect(),
                emoji: emoji.clone(),
            })
            .collect(),
    }
}

/// How many of the three parts differ from what's saved.
fn changes(saved: &pb::WelcomeScreen, now: &pb::WelcomeScreen) -> usize {
    [saved.enabled != now.enabled, saved.description != now.description, saved.channels != now.channels]
        .into_iter()
        .filter(|c| *c)
        .count()
}

impl ServerSettingsView {
    fn load_welcome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.welcome.loading || self.welcome.saved.is_some() {
            return;
        }
        self.welcome.loading = true;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let rx = core.spawn({
            let core = core.clone();
            async move { core.welcome_screen(&key, &sid).await }
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.welcome.loading = false;
                match result {
                    Ok(screen) => this.fill_welcome(screen, window, cx),
                    Err(err) => {
                        this.welcome.saved = Some(pb::WelcomeScreen::default());
                        this.error = Some(err.message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Fills the page from the screen as saved.
    fn fill_welcome(&mut self, screen: pb::WelcomeScreen, window: &mut Window, cx: &mut Context<Self>) {
        self.welcome.enabled = screen.enabled;
        let text = screen.description.clone();
        self.welcome.description.update(cx, |s, cx| s.set_value(text, window, cx));
        self.welcome.rows.clear();
        for c in &screen.channels {
            self.add_welcome_row(c.channel_id.clone(), c.emoji.clone(), &c.description, window, cx);
        }
        self.welcome.picking = None;
        self.welcome.saved = Some(screen);
    }

    fn add_welcome_row(
        &mut self,
        channel_id: String,
        emoji: String,
        note: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.welcome.next += 1;
        let key = self.welcome.next;
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Why go there (optional)"));
        let note = note.to_owned();
        input.update(cx, |s, cx| s.set_value(note, window, cx));
        let changes = cx.subscribe(&input, |_: &mut Self, _, e: &InputEvent, cx| {
            if let InputEvent::Change = e {
                cx.notify()
            }
        });
        self.welcome.rows.push(Row { key, channel_id, emoji, note: input, _changes: changes });
    }

    fn welcome_draft(&self, cx: &Context<Self>) -> pb::WelcomeScreen {
        let rows: Vec<(String, String, String)> = self
            .welcome
            .rows
            .iter()
            .map(|r| (r.channel_id.clone(), r.note.read(cx).value().to_string(), r.emoji.clone()))
            .collect();
        draft(self.welcome.enabled, &self.welcome.description.read(cx).value(), &rows)
    }

    fn save_welcome(&mut self, cx: &mut Context<Self>) {
        let screen = self.welcome_draft(cx);
        if screen.description.chars().count() > DESCRIPTION_MAX {
            self.error = Some(format!("The few words are {DESCRIPTION_MAX} characters at most."));
            cx.notify();
            return;
        }
        if screen.enabled && screen.description.is_empty() && screen.channels.is_empty() {
            self.error = Some("Add a few words or a channel first.".into());
            cx.notify();
            return;
        }
        self.welcome.saving = true;
        self.error = None;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let rx = core.spawn({
            let core = core.clone();
            async move { core.set_welcome_screen(&key, &sid, screen).await }
        });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.welcome.saving = false;
                match result {
                    // The rows stay as they are: they're what was saved.
                    Ok(screen) => {
                        this.welcome.saved = Some(screen);
                        this.flash_saved(cx);
                    }
                    Err(err) => this.error = Some(err.message),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn move_welcome_row(&mut self, key: u64, by: isize, cx: &mut Context<Self>) {
        let rows = &mut self.welcome.rows;
        let Some(at) = rows.iter().position(|r| r.key == key) else { return };
        let to = at as isize + by;
        if to < 0 || to as usize >= rows.len() {
            return;
        }
        rows.swap(at, to as usize);
        self.welcome.moved = Some((key, -60.0 * by as f32, Instant::now()));
        self.welcome.picking = None;
        cx.notify();
    }

    pub(super) fn welcome_page(
        &mut self,
        server: &pb::Server,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.load_welcome(window, cx);
        let Some(saved) = self.welcome.saved.clone() else { return shimmer_rows(3, p).into_any_element() };
        let (channels, emojis, look) = self.core.shared.read(|s| {
            let i = s.instance(&self.key);
            let mut channels: Vec<pb::Channel> = i
                .and_then(|i| i.channels.get(&self.server))
                .into_iter()
                .flatten()
                .filter(|c| {
                    pb::ChannelType::try_from(c.r#type).unwrap_or(pb::ChannelType::Text) != pb::ChannelType::Category
                })
                .cloned()
                .collect();
            channels.sort_by_key(|c| c.position);
            (
                channels,
                i.and_then(|i| i.emojis.get(&self.server)).cloned().unwrap_or_default(),
                i.map(|i| crate::ui::mentions::Look::of(i, &self.server)).unwrap_or_default(),
            )
        });
        let now = self.welcome_draft(cx);
        let n = changes(&saved, &now);
        if n > 0 {
            self.bar = Some(save_bar(
                "welcome-save-bar",
                n,
                self.welcome.saving,
                p,
                cx,
                |this, window, cx| {
                    if let Some(saved) = this.welcome.saved.clone() {
                        this.fill_welcome(saved, window, cx);
                    }
                    this.error = None;
                    cx.notify();
                },
                |this, _, cx| this.save_welcome(cx),
            ));
        }

        let enabled = self.welcome.enabled;
        let length = self.welcome.description.read(cx).value().chars().count();
        let toggle = div()
            .flex()
            .items_center()
            .gap(px(16.0))
            .pb(px(18.0))
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_weight(FontWeight::EXTRA_BOLD).child("Show a welcome screen"))
                    .child(div().text_sm().text_color(p.muted_foreground).child(
                        "New members see it once, after agreeing to any rules. Anyone can open it again from the \
                         server menu.",
                    )),
            )
            .child(switch("welcome-on".into(), enabled, false, cx, |this, on, cx| {
                this.welcome.enabled = on;
                cx.notify();
            }));

        let words = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .py(px(18.0))
            .border_b_1()
            .border_color(p.border)
            .child(div().font_weight(FontWeight::EXTRA_BOLD).child("A few words"))
            .child(Textarea::new(&self.welcome.description))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap(px(12.0))
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child("Markdown works on one line: **bold**, *italics*, links.")
                    .child(
                        div()
                            .when(length > DESCRIPTION_MAX - 30, |el| el.text_color(amber(p)))
                            .when(length > DESCRIPTION_MAX, |el| el.text_color(p.destructive))
                            .child(format!("{length}/{DESCRIPTION_MAX}")),
                    ),
            );

        let used: Vec<String> = self.welcome.rows.iter().map(|r| r.channel_id.clone()).collect();
        let unused: Vec<pb::Channel> = channels.iter().filter(|c| !used.contains(&c.id)).cloned().collect();
        let mut list = div().flex().flex_col().gap(px(8.0));
        let count = self.welcome.rows.len();
        for at in 0..count {
            list = list.child(self.welcome_row(at, count, &channels, &unused, &emojis, &look, p, cx));
        }
        let mut suggested = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .py(px(18.0))
            .child(div().child(div().font_weight(FontWeight::EXTRA_BOLD).child("Channels to start in")).child(
                div().text_sm().text_color(p.muted_foreground).child(format!(
                    "Up to {MAX_CHANNELS}, in this order. People only see the ones they're allowed into."
                )),
            ))
            .child(list);
        if count < MAX_CHANNELS && !unused.is_empty() {
            let first = unused[0].id.clone();
            let hover = alpha(p.primary, 0.4);
            suggested = suggested.child(
                div()
                    .id("welcome-add")
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(8.0))
                    .h(px(40.0))
                    .rounded(corner(12.0))
                    .border_1()
                    .border_dashed()
                    .border_color(p.border)
                    .cursor_pointer()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .hover(move |s| s.border_color(hover))
                    .active(|s| s.top(px(1.0)))
                    .child(icon("plus").size(px(15.0)))
                    .child("Add a channel")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.add_welcome_row(first.clone(), String::new(), "", window, cx);
                        cx.notify();
                    })),
            );
        }

        let editor = div().flex_1().min_w_0().flex().flex_col().child(toggle).child(words).child(suggested);
        div()
            .flex()
            .items_start()
            .gap(px(28.0))
            .pb(px(80.0))
            .child(editor)
            .child(self.welcome_preview(server, &now, &channels, &look, p))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn welcome_row(
        &self,
        at: usize,
        count: usize,
        channels: &[pb::Channel],
        unused: &[pb::Channel],
        emojis: &[pb::Emoji],
        look: &crate::ui::mentions::Look,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = &self.welcome.rows[at];
        let key = row.key;
        let channel = channels.iter().find(|c| c.id == row.channel_id);
        let picking = self.welcome.picking.filter(|(k, _)| *k == key).map(|(_, pick)| pick);
        let hover = alpha(p.primary, 0.4);

        let arrows = div()
            .flex()
            .flex_col()
            .flex_none()
            .child(
                icon_button(SharedString::from(format!("welcome-up-{key}")), "chevron-up", p)
                    .size(px(18.0))
                    .when(at == 0, |el| el.opacity(0.3))
                    .on_click(cx.listener(move |this, _, _, cx| this.move_welcome_row(key, -1, cx))),
            )
            .child(
                icon_button(SharedString::from(format!("welcome-down-{key}")), "chevron-down", p)
                    .size(px(18.0))
                    .when(at + 1 == count, |el| el.opacity(0.3))
                    .on_click(cx.listener(move |this, _, _, cx| this.move_welcome_row(key, 1, cx))),
            );
        let pick = move |what: Pick| {
            move |this: &mut Self, _: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.welcome.picking = if this.welcome.picking == Some((key, what)) { None } else { Some((key, what)) };
                cx.notify();
            }
        };
        let emoji_button = div()
            .id(SharedString::from(format!("welcome-emoji-{key}")))
            .flex_none()
            .rounded(corner(12.0))
            .border_1()
            .border_color(if picking == Some(Pick::Emoji) { alpha(p.primary, 0.6) } else { p.border.into() })
            .cursor_pointer()
            .hover(move |s| s.border_color(hover))
            .active(|s| s.top(px(1.0)))
            .on_click(cx.listener(pick(Pick::Emoji)))
            .child(if row.emoji.is_empty() {
                div()
                    .size(px(36.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(p.muted_foreground)
                    .child(icon("face-slightly-smiling-plus").size(px(17.0)))
                    .into_any_element()
            } else {
                welcome_emoji(&row.emoji, look, p)
            });
        let channel_button = div()
            .id(SharedString::from(format!("welcome-channel-{key}")))
            .flex_none()
            .w(px(170.0))
            .h(px(36.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .rounded(corner(12.0))
            .border_1()
            .border_color(if picking == Some(Pick::Channel) { alpha(p.primary, 0.6) } else { p.border.into() })
            .cursor_pointer()
            .hover(move |s| s.border_color(hover))
            .on_click(cx.listener(pick(Pick::Channel)))
            .child(icon("hash").size(px(15.0)).text_color(p.muted_foreground))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child(channel.map(|c| c.name.clone()).unwrap_or_else(|| "Pick a channel".into())),
            )
            .child(icon("chevron-down").size(px(14.0)).text_color(p.muted_foreground));

        let line = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(arrows)
            .child(emoji_button)
            .child(channel_button)
            .child(div().flex_1().min_w_0().child(Input::new(&row.note)))
            .child(icon_button(SharedString::from(format!("welcome-remove-{key}")), "x", p).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.welcome.rows.retain(|r| r.key != key);
                    this.welcome.picking = None;
                    cx.notify();
                },
            )));

        let choices: Option<AnyElement> = picking.map(|what| match what {
            Pick::Channel => {
                let mut chips = div().flex().flex_wrap().gap(px(6.0));
                for c in channel.into_iter().chain(unused.iter()) {
                    let on = c.id == row.channel_id;
                    let cid = c.id.clone();
                    chips = chips.child(
                        chip(
                            SharedString::from(format!("welcome-pick-{key}-{}", c.id)),
                            &format!("# {}", c.name),
                            on,
                            p,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(r) = this.welcome.rows.iter_mut().find(|r| r.key == key) {
                                r.channel_id = cid.clone();
                            }
                            this.welcome.picking = None;
                            cx.notify();
                        })),
                    );
                }
                chips.into_any_element()
            }
            Pick::Emoji => self.emoji_choices(key, emojis, p, cx),
        });

        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .p(px(8.0))
            .rounded(corner(16.0))
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.background, 0.6))
            .child(line)
            .when_some(choices, |el, c| {
                el.child(motion::rise(
                    div().px(px(6.0)).pb(px(4.0)).child(c),
                    SharedString::from(format!("welcome-choices-{key}-{}", what_id(picking))),
                    Duration::ZERO,
                    6.0,
                ))
            });
        // A row that just moved glides from where it was; a new one rises in.
        match self.welcome.moved.filter(|(k, _, at)| *k == key && at.elapsed() < Duration::from_millis(400)) {
            Some((_, from, at)) => motion::once(
                body,
                SharedString::from(format!("welcome-moved-{key}-{at:?}")),
                Duration::from_millis(280),
                move |el, t| {
                    let eased = 1.0 - (1.0 - t).powi(3);
                    el.relative().top(px(from * (1.0 - eased)))
                },
            ),
            None => motion::rise(body, SharedString::from(format!("welcome-row-{key}")), Duration::ZERO, 8.0)
                .into_any_element(),
        }
    }

    /// The server's own emoji, then everyday ones, and a way back to none.
    fn emoji_choices(&self, key: u64, emojis: &[pb::Emoji], p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let hover = alpha(p.primary, 0.12);
        let cell = |id: String, value: String, face: AnyElement, cx: &mut Context<Self>| {
            div()
                .id(SharedString::from(id))
                .size(px(34.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(corner(10.0))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .active(|s| s.top(px(1.0)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(r) = this.welcome.rows.iter_mut().find(|r| r.key == key) {
                        r.emoji = value.clone();
                    }
                    this.welcome.picking = None;
                    cx.notify();
                }))
                .child(face)
        };
        let mut grid = div().flex().flex_wrap();
        grid = grid.child(cell(
            format!("welcome-e-{key}-none"),
            String::new(),
            icon("hash").size(px(16.0)).text_color(p.muted_foreground).into_any_element(),
            cx,
        ));
        for e in emojis {
            use gpui_kit::StyledImage as _;
            grid = grid.child(cell(
                format!("welcome-e-{key}-{}", e.id),
                crate::ui::emoji::token(e),
                gpui_kit::img(SharedString::from(e.url.clone()))
                    .size(px(22.0))
                    .object_fit(gpui_kit::ObjectFit::Contain)
                    .into_any_element(),
                cx,
            ));
        }
        let everyday = crate::ui::emoji::standard().iter().flat_map(|g| &g.emojis).take(EVERYDAY);
        for (n, e) in everyday.enumerate() {
            grid = grid.child(cell(
                format!("welcome-e-{key}-u{n}"),
                e.char.clone(),
                div().in_color().text_size(px(19.0)).child(e.char.clone()).into_any_element(),
                cx,
            ));
        }
        div()
            .id(SharedString::from(format!("welcome-grid-{key}")))
            .max_h(px(168.0))
            .overflow_y_scroll()
            .child(grid)
            .into_any_element()
    }

    /// The welcome screen as new members will see it.
    fn welcome_preview(
        &self,
        server: &pb::Server,
        screen: &pb::WelcomeScreen,
        channels: &[pb::Channel],
        look: &crate::ui::mentions::Look,
        p: &Palette,
    ) -> AnyElement {
        let mut body = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .relative()
                    .child(server_icon(server, 56.0, 18.0, p))
                    .child(div().absolute().top(px(-10.0)).right(px(-14.0)).text_size(px(20.0)).child("👋")),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .text_color(p.muted_foreground)
                            .child("WELCOME TO"),
                    )
                    .child(div().text_lg().font_weight(FontWeight::EXTRA_BOLD).child(server.name.clone())),
            );
        if !screen.description.is_empty() {
            let shown =
                crate::ui::mentions::mention_links(&crate::ui::text::images_as_links(&screen.description), look);
            body = body.child(
                div().w_full().min_w_0().text_sm().text_color(p.muted_foreground).child(
                    crate::ui::text::markdown("welcome-preview-words", shown)
                        .markdown_extensions(crate::ui::emoji::markdown_extensions()),
                ),
            );
        }
        let mut list = div().w_full().flex().flex_col().gap(px(6.0));
        for (n, w) in screen.channels.iter().enumerate() {
            let Some(c) = channels.iter().find(|c| c.id == w.channel_id) else { continue };
            list = list.child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .p(px(10.0))
                    .rounded(corner(12.0))
                    .border_1()
                    .border_color(p.border)
                    .bg(p.secondary)
                    .child(welcome_emoji(&w.emoji, look, p))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div().truncate().text_sm().font_weight(FontWeight::BOLD).child(format!("#{}", c.name)),
                            )
                            .when(!w.description.is_empty(), |el| {
                                el.child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(p.muted_foreground)
                                        .child(w.description.clone()),
                                )
                            }),
                    )
                    .child(icon("arrow-right").size(px(14.0)).text_color(p.muted_foreground)),
                SharedString::from(format!("welcome-preview-{n}-{}", w.channel_id)),
                Duration::from_millis(40 * n as u64),
                8.0,
            ));
        }
        if !screen.channels.is_empty() {
            body = body.child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .text_color(p.muted_foreground)
                            .child("START HERE"),
                    )
                    .child(list),
            );
        }
        if screen.description.is_empty() && screen.channels.is_empty() {
            body = body.child(
                div().text_xs().text_color(p.muted_foreground).child("Add a few words or a channel to see it here."),
            );
        }
        let on = screen.enabled;
        let glow = alpha(p.primary, 0.2);
        let card = div()
            .relative()
            .overflow_hidden()
            .rounded(corner(24.0))
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .shadow_lg()
            .child(div().absolute().top_0().left_0().right_0().h(px(110.0)).bg(gpui_kit::linear_gradient(
                180.0,
                gpui_kit::linear_color_stop(glow, 0.0),
                gpui_kit::linear_color_stop(alpha(p.primary, 0.0), 1.0),
            )))
            .child(div().relative().p(px(20.0)).opacity(if on { 1.0 } else { 0.35 }).child(body))
            .when(!on, |el| {
                el.child(
                    div().absolute().inset_0().flex().items_center().justify_center().p(px(24.0)).child(motion::rise(
                        div()
                            .p(px(12.0))
                            .rounded(corner(16.0))
                            .bg(p.card)
                            .border_1()
                            .border_color(p.border)
                            .shadow_lg()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_center()
                            .child("Off: new members go straight to the server."),
                        "welcome-off",
                        Duration::ZERO,
                        8.0,
                    )),
                )
            });
        div()
            .flex_none()
            .w(px(320.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(p.muted_foreground)
                    .child("PREVIEW"),
            )
            .child(card)
            .into_any_element()
    }
}

fn what_id(pick: Option<Pick>) -> &'static str {
    match pick {
        Some(Pick::Emoji) => "emoji",
        Some(Pick::Channel) => "channel",
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drafts_count_what_changed() {
        let rows = vec![
            ("c1".to_owned(), "  say hi ".to_owned(), "👋".to_owned()),
            (String::new(), "lost".to_owned(), String::new()),
        ];
        let d = draft(true, " Hello! ", &rows);
        assert_eq!(d.description, "Hello!");
        assert_eq!(d.channels.len(), 1, "a row without a channel isn't kept");
        assert_eq!(d.channels[0].description, "say hi");
        assert_eq!(changes(&pb::WelcomeScreen::default(), &d), 3);
        assert_eq!(changes(&d, &d), 0);
        let long = draft(false, "", &[("c".into(), "x".repeat(90), String::new())]);
        assert_eq!(long.channels[0].description.len(), NOTE_MAX);
    }
}
