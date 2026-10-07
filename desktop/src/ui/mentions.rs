//! Mentions: finding `@username`, `@everyone`, `@here` and roles (`<@&id>`)
//! in messages so they show as chips (coloured like the role, brighter when
//! they're about you; people's open their card), and the @ list in the
//! composer. Like the web app's `chat/mentions.tsx` and `MentionPicker.tsx`:
//! a role goes in as `@Name` and is sent as `<@&id>`.

use std::collections::HashMap;

use crate::core::store::{InstanceState, user_name};
use crate::pb;

/// Links that mean a mention, not an address.
pub const SCHEME: &str = "fuwa-mention:";

/// What a server's messages need to draw mentions.
#[derive(Default, Clone)]
pub struct Look {
    /// Usernames (lowercase) to the names they show as.
    pub people: HashMap<String, String>,
    /// Role ids to names.
    pub roles: HashMap<String, String>,
    /// Role ids to their colours (0xRRGGBB), for those that have one.
    pub role_colors: HashMap<String, u32>,
    /// Your username (lowercase) and roles, to light up mentions of you.
    pub me: String,
    pub my_roles: Vec<String>,
    /// Emoji ids to pictures: the server's own, and the other servers' on
    /// the instance (a message may write those too).
    pub emojis: HashMap<String, String>,
    /// Changes whenever any of the above does, so what was worked out with
    /// one look can be kept until the next.
    pub digest: u64,
}

impl Look {
    pub fn of(i: &InstanceState, server_id: &str) -> Self {
        use std::hash::{Hash as _, Hasher as _};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let people = i
            .members
            .get(server_id)
            .into_iter()
            .flatten()
            .filter_map(|m| {
                let user = m.user.as_ref()?;
                let name = if m.nickname.is_empty() { user_name(user) } else { m.nickname.clone() };
                (&user.username, &name).hash(&mut h);
                Some((user.username.to_lowercase(), name))
            })
            .collect();
        let roles = i
            .roles
            .get(server_id)
            .into_iter()
            .flatten()
            .map(|r| {
                (&r.id, &r.name, r.color).hash(&mut h);
                (r.id.clone(), r.name.clone())
            })
            .collect();
        let role_colors = i
            .roles
            .get(server_id)
            .into_iter()
            .flatten()
            .filter_map(|r| Some((r.id.clone(), r.color? as u32)))
            .collect();
        let me = i.me.as_ref().map(|u| u.username.to_lowercase()).unwrap_or_default();
        let my_roles = i.my_member(server_id).map(|m| m.role_ids.clone()).unwrap_or_default();
        (&me, &my_roles).hash(&mut h);
        // The server's own last, so they win should two ever share an id.
        let mut emojis = HashMap::new();
        let others = i.emojis.iter().filter(|(id, _)| *id != server_id).flat_map(|(_, list)| list);
        for e in others.chain(i.emojis.get(server_id).into_iter().flatten()) {
            (&e.id, &e.url).hash(&mut h);
            emojis.insert(e.id.clone(), e.url.clone());
        }
        Self { people, roles, role_colors, me, my_roles, emojis, digest: h.finish() }
    }

    /// With the emoji a message brought along from other servers, for the
    /// ones not here already.
    pub fn with(&self, brought: &[pb::Emoji]) -> std::borrow::Cow<'_, Self> {
        if brought.iter().all(|e| self.emojis.contains_key(&e.id)) {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut look = self.clone();
        for e in brought {
            look.emojis.entry(e.id.clone()).or_insert_with(|| e.url.clone());
        }
        std::borrow::Cow::Owned(look)
    }
}

fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A username after an @: letters, digits, `_` and `.`, not ending in a dot.
fn username_at(rest: &str) -> Option<&str> {
    let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.')).unwrap_or(rest.len());
    let name = rest[..end].trim_end_matches('.');
    (!name.is_empty() && name.len() <= 32).then_some(name)
}

/// Turns mentions into chips (`![@Mika](fuwa-mention:user/mika/-/0)`: the
/// kind, who or what, a colour and whether it's you), leaving code and links
/// alone. People who aren't in the server stay plain text.
pub fn mention_links(source: &str, look: &Look) -> String {
    let mut out = String::with_capacity(source.len() + 16);
    let mut fenced = false;
    for (n, line) in source.split('\n').enumerate() {
        if n > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
        }
        if fenced || trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            out.push_str(line);
            continue;
        }
        let mut in_code = false;
        // Inside a link's text or its address.
        let mut bracket = 0usize;
        let mut paren = 0usize;
        let mut prev: Option<char> = None;
        let mut i = 0;
        while i < line.len() {
            let rest = &line[i..];
            let c = rest.chars().next().unwrap_or_default();
            let plain = !in_code && bracket == 0 && paren == 0;
            if plain
                && rest.starts_with("<@&")
                && let Some(end) = rest.find('>')
                && rest[3..end].len() == 26
                && rest[3..end].chars().all(|c| c.is_ascii_alphanumeric())
            {
                let id = rest[3..end].to_uppercase();
                match look.roles.get(&id) {
                    Some(name) => {
                        let color = look.role_colors.get(&id).map_or("-".to_owned(), |c| format!("{c:06x}"));
                        let mine = u8::from(look.my_roles.iter().any(|r| r.eq_ignore_ascii_case(&id)));
                        out.push_str(&format!("![@{}]({SCHEME}role/{id}/{color}/{mine})", escape(name)));
                    }
                    None => out.push_str(&format!("![@deleted-role]({SCHEME}role/{id}/deleted/0)")),
                }
                i += end + 1;
                prev = Some('>');
                continue;
            }
            // A server emoji, `<:name:id>` (or `<a:name:id>` when it moves):
            // its picture, or its :name: when it's gone or never came along.
            if plain && let Some((name, id, len)) = crate::ui::emoji::token_at(rest) {
                let url = look.emojis.get(id).or_else(|| look.emojis.get(&id.to_uppercase()));
                match url.filter(|url| url.starts_with("http")) {
                    Some(url) => out.push_str(&format!(
                        "![:{}:]({}{})",
                        escape(name),
                        crate::ui::emoji::SCHEME,
                        url.replace(' ', "%20")
                    )),
                    None => out.push_str(&format!(":{}:", escape(name))),
                }
                i += len;
                prev = Some('>');
                continue;
            }
            if plain && c == '@' && !prev.is_some_and(|p| word(p) || p == '@' || p == '<' || p == '.') {
                let after = &rest[1..];
                let loud = ["everyone", "here"].into_iter().find(|loud| {
                    after.get(..loud.len()).is_some_and(|w| w.eq_ignore_ascii_case(loud))
                        && !after[loud.len()..].chars().next().is_some_and(word)
                });
                if let Some(loud) = loud {
                    out.push_str(&format!("![@{loud}]({SCHEME}everyone/{loud}/-/0)"));
                    i += 1 + loud.len();
                    prev = Some('x');
                    continue;
                }
                if let Some(name) = username_at(after)
                    && !after[name.len()..].chars().next().is_some_and(word)
                    && let Some(shown) = look.people.get(&name.to_lowercase())
                {
                    let mine = u8::from(name.eq_ignore_ascii_case(&look.me));
                    out.push_str(&format!("![@{}]({SCHEME}user/{}/-/{mine})", escape(shown), name.to_lowercase()));
                    i += 1 + name.len();
                    prev = Some('x');
                    continue;
                }
            }
            match c {
                '`' => in_code = !in_code,
                '\\' if !in_code => {
                    out.push(c);
                    i += 1;
                    if let Some(next) = line[i..].chars().next() {
                        out.push(next);
                        i += next.len_utf8();
                        prev = Some(next);
                    }
                    continue;
                }
                '[' if !in_code => bracket += 1,
                ']' if !in_code && bracket > 0 => {
                    bracket -= 1;
                    if line[i + 1..].starts_with('(') {
                        paren += 1;
                        out.push_str("](");
                        i += 2;
                        prev = Some('(');
                        continue;
                    }
                }
                ')' if !in_code && paren > 0 => paren -= 1,
                _ => {}
            }
            out.push(c);
            i += c.len_utf8();
            prev = Some(c);
        }
    }
    out
}

/// Whether a message is only emoji (up to 27), which draw big (the web's `onlyEmoji`).
pub fn only_emoji(content: &str) -> bool {
    let mut tokens = 0;
    let mut rest = String::with_capacity(content.len());
    let mut at = 0;
    while at < content.len() {
        if let Some((_, _, len)) = crate::ui::emoji::token_at(&content[at..]) {
            tokens += 1;
            rest.push(' ');
            at += len;
            continue;
        }
        let c = content[at..].chars().next().unwrap_or(' ');
        rest.push(c);
        at += c.len_utf8();
    }
    let pictograph = |c: char| {
        matches!(c as u32,
            0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2300..=0x23FF | 0x2B00..=0x2BFF | 0x2190..=0x21FF
            | 0x25A0..=0x25FF | 0x2900..=0x297F | 0x3030 | 0x303D | 0x3297 | 0x3299 | 0xA9 | 0xAE | 0x203C | 0x2049
            | 0x2122 | 0x2139 | 0x24C2)
    };
    // Joiners, variation selectors, keycaps and tags ride along with the emoji before them.
    let joining = |c: char| matches!(c as u32, 0x200D | 0xFE0E | 0xFE0F | 0x20E3 | 0xE0020..=0xE007F);
    let (mut count, mut joined, mut flags) = (tokens, false, 0);
    for c in rest.chars().filter(|c| !c.is_whitespace()) {
        if (0x1F1E6..=0x1F1FF).contains(&(c as u32)) {
            flags += 1;
            if flags % 2 == 1 {
                count += 1;
            }
        } else if joining(c) {
            joined = c == '\u{200D}';
            continue;
        } else if pictograph(c) {
            if !joined {
                count += 1;
            }
        } else {
            return false;
        }
        joined = false;
    }
    count > 0 && count <= 27
}

/// A mention as a chip in Markdown (`Mention`): `@Name` on its colour, or
/// the primary's; brighter, with a ring, when it's about you.
pub struct Plugin;

#[derive(Clone)]
struct Chip {
    kind: String,
    target: String,
    color: Option<u32>,
    deleted: bool,
    mine: bool,
    label: String,
}

impl gpui_kit::component::text::MarkdownPlugin for Plugin {
    fn name(&self) -> &str {
        "fuwa-mention"
    }

    fn parse(
        &self,
        node: &gpui_kit::component::text::markdown_ast::Node,
        _: &gpui_kit::component::text::MarkdownParseContext<'_>,
    ) -> Option<gpui_kit::component::text::MarkdownNode> {
        let gpui_kit::component::text::markdown_ast::Node::Image(image) = node else { return None };
        let mut parts = image.url.strip_prefix(SCHEME)?.split('/');
        let kind = parts.next()?.to_owned();
        let target = parts.next()?.to_owned();
        let color = parts.next().unwrap_or("-");
        let mine = parts.next() == Some("1");
        let chip = Chip {
            kind,
            target,
            color: u32::from_str_radix(color, 16).ok(),
            deleted: color == "deleted",
            mine,
            label: if color == "deleted" {
                crate::core::i18n::t("chat.mentions.deletedRole")
            } else {
                image.alt.clone()
            },
        };
        let text = chip.label.clone();
        Some(gpui_kit::component::text::MarkdownNode::new("fuwa-mention", chip).text(text.clone()).markdown(text))
    }

    fn render_inline(
        &self,
        node: &gpui_kit::component::text::MarkdownNode,
        _: &gpui_kit::component::text::InlineRenderContext,
        _: &mut gpui_kit::Window,
        cx: &mut gpui_kit::App,
    ) -> Option<gpui_kit::component::text::InlineElement> {
        use gpui_kit::prelude::FluentBuilder as _;
        use gpui_kit::{InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _, Styled as _};
        let chip = node.data::<Chip>()?;
        let p = crate::ui::widgets::pal(cx);
        let alpha = crate::ui::theme::alpha;
        let (fg, bg, ring): (gpui_kit::Hsla, gpui_kit::Hsla, Option<gpui_kit::Hsla>) =
            match (&chip.kind[..], chip.color) {
                ("role", _) if chip.deleted => (p.muted_foreground.into(), p.muted.into(), None),
                ("role", Some(c)) => {
                    let c = gpui_kit::rgb(c);
                    (c.into(), alpha(c, if chip.mine { 0.24 } else { 0.15 }), chip.mine.then(|| alpha(c, 0.4)))
                }
                ("user", _) if chip.mine => (p.primary.into(), alpha(p.primary, 0.25), Some(alpha(p.primary, 0.4))),
                ("user", _) => (p.primary.into(), alpha(p.primary, 0.12), None),
                _ => (p.primary.into(), alpha(p.primary, 0.15), chip.mine.then(|| alpha(p.primary, 0.4))),
            };
        let hover = alpha(p.primary, 0.2);
        let user = (chip.kind == "user").then(|| chip.target.clone());
        Some(gpui_kit::component::text::InlineElement::new(
            gpui_kit::div()
                .id(gpui_kit::SharedString::from(format!("mention|{}|{}", chip.kind, chip.target)))
                .px(gpui_kit::px(4.0))
                .rounded(crate::ui::theme::radius_md())
                .bg(bg)
                .text_color(fg)
                .font_weight(gpui_kit::FontWeight::BOLD)
                .when_some(ring, |el, ring| el.border_1().border_color(ring).px(gpui_kit::px(3.0)))
                .when(chip.kind == "role" && !chip.deleted, |el| {
                    let tip = crate::core::i18n::t_with(
                        "chat.mentions.role",
                        &[("name", crate::core::i18n::Arg::Str(chip.label.trim_start_matches('@')))],
                    );
                    el.tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                    })
                })
                .when_some(user, |el, username| {
                    el.cursor_pointer().when(!chip.mine, |el| el.hover(move |s| s.bg(hover))).on_click(
                        move |_, window, cx| {
                            open_mention(&username, window, cx);
                        },
                    )
                })
                .child(chip.label.clone()),
        ))
    }
}

thread_local! {
    /// The window's app, so a person's chip can open their card.
    static APP: std::cell::RefCell<Option<gpui_kit::WeakEntity<crate::ui::app::FuwaApp>>> = const { std::cell::RefCell::new(None) };
}

/// Remembers the window's app for chips to reach (set as the message list draws).
pub fn set_app(app: gpui_kit::WeakEntity<crate::ui::app::FuwaApp>) {
    APP.with(|a| *a.borrow_mut() = Some(app));
}

/// Opens the card of the person a chip names, in the server on screen.
fn open_mention(username: &str, window: &mut gpui_kit::Window, cx: &mut gpui_kit::App) {
    let Some(app) = APP.with(|a| a.borrow().clone()) else { return };
    let _ = app.update(cx, |this, cx| {
        let Some(crate::ui::app::Target::Channel { key, server, .. }) = this.target() else { return };
        let found = this.core.shared.read(|s| {
            s.instance(&key)?
                .members
                .get(&server)?
                .iter()
                .filter_map(|m| m.user.as_ref())
                .find(|u| u.username.eq_ignore_ascii_case(username))
                .map(|u| u.id.clone())
        });
        if let Some(user_id) = found {
            this.open_dialog(crate::ui::app::Dialog::Profile { key, user_id, server: Some(server) }, window, cx);
        }
    });
}

/// Markdown's special characters in a name, escaped.
fn escape(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if matches!(c, '[' | ']' | '(' | ')' | '*' | '_' | '`' | '\\' | '<' | '>' | '~') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The `@query` being typed just before the caret: where it starts, and what it says.
pub fn token(text: &str, caret: usize) -> Option<(usize, String)> {
    let before = text.get(..caret)?;
    let at = before.rfind('@')?;
    let query = &before[at + 1..];
    let starts_word = before[..at].chars().next_back().is_none_or(char::is_whitespace);
    (starts_word && query.chars().count() <= 32 && !query.chars().any(|c| c.is_whitespace() || c == '@'))
        .then(|| (at, query.to_lowercase()))
}

/// One choice in the @ list.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    Member {
        user: Box<pb::User>,
        name: String,
    },
    Role {
        id: String,
        name: String,
        color: Option<u32>,
    },
    Everyone(&'static str),
    /// After a colon: an emoji.
    Emoji(crate::ui::emoji::Choice),
}

impl Pick {
    /// What goes in the box.
    pub fn insert(&self) -> String {
        match self {
            Pick::Member { user, .. } => format!("@{}", user.username),
            Pick::Role { name, .. } => format!("@{name}"),
            Pick::Everyone(name) => format!("@{name}"),
            Pick::Emoji(choice) => choice.insert.clone(),
        }
    }

    pub fn id(&self) -> String {
        match self {
            Pick::Member { user, .. } => format!("u-{}", user.id),
            Pick::Role { id, .. } => format!("r-{id}"),
            Pick::Everyone(name) => (*name).to_owned(),
            Pick::Emoji(choice) => format!("e-{}", choice.key),
        }
    }
}

const MAX: usize = 8;

/// People, roles you can ping, and @everyone and @here when you may.
pub fn options(i: &InstanceState, server_id: &str, query: &str, everyone: bool) -> Vec<Pick> {
    let hit = |names: &[&str]| names.iter().any(|n| n.to_lowercase().contains(query));
    let mut people: Vec<Pick> = i
        .members
        .get(server_id)
        .into_iter()
        .flatten()
        .filter(|m| !m.pending)
        .filter_map(|m| {
            let user = m.user.as_ref()?;
            hit(&[&user.username, &user.display_name, &m.nickname]).then(|| Pick::Member {
                user: Box::new(user.clone()),
                name: if m.nickname.is_empty() { user_name(user) } else { m.nickname.clone() },
            })
        })
        .collect();
    people.sort_by_key(|p| match p {
        Pick::Member { user, .. } => !user.username.starts_with(query),
        _ => true,
    });
    let roles: Vec<Pick> = i
        .roles
        .get(server_id)
        .into_iter()
        .flatten()
        .filter(|r| r.id != server_id && (r.mentionable || everyone) && hit(&[&r.name]))
        .map(|r| Pick::Role { id: r.id.clone(), name: r.name.clone(), color: r.color.map(|c| c as u32) })
        .collect();
    let loud: Vec<Pick> = if everyone {
        ["everyone", "here"].into_iter().filter(|n| n.starts_with(query)).map(Pick::Everyone).collect()
    } else {
        Vec::new()
    };
    let room = MAX - (roles.len() + loud.len()).min(4);
    people.truncate(room);
    people.into_iter().chain(roles).chain(loud).take(MAX).collect()
}

/// The text to send: roles picked by name become their tokens.
pub fn encode(content: &str, picked: &[(String, String)]) -> String {
    let mut names: Vec<&(String, String)> = picked.iter().collect();
    names.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
    let mut out = content.to_owned();
    for (name, id) in names {
        let needle = format!("@{name}");
        let mut next = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(at) = rest.find(&needle) {
            let before_ok = rest[..at].chars().next_back().is_none_or(char::is_whitespace)
                && (at > 0 || next.is_empty() || next.ends_with(char::is_whitespace));
            let after = &rest[at + needle.len()..];
            let after_ok = !after.chars().next().is_some_and(word);
            next.push_str(&rest[..at]);
            if before_ok && after_ok {
                next.push_str(&format!("<@&{id}>"));
            } else {
                next.push_str(&needle);
            }
            rest = after;
        }
        next.push_str(rest);
        out = next;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROLE: &str = "01J9ZZZZZZZZZZZZZZZZZZZZZZ";

    fn look() -> Look {
        Look {
            people: [("mika".to_owned(), "Mika Sato".to_owned())].into_iter().collect(),
            roles: [(ROLE.to_owned(), "Mods".to_owned())].into_iter().collect(),
            role_colors: [(ROLE.to_owned(), 0x3b82f6)].into_iter().collect(),
            me: "mika".to_owned(),
            my_roles: Vec::new(),
            emojis: [(ROLE.to_owned(), "https://x/e.webp".to_owned())].into_iter().collect(),
            digest: 0,
        }
    }

    #[test]
    fn mentions_become_links_but_code_and_links_stay() {
        let l = look();
        assert_eq!(mention_links("hi @mika!", &l), format!("hi ![@Mika Sato]({SCHEME}user/mika/-/1)!"));
        assert_eq!(mention_links("@Everyone look", &l), format!("![@everyone]({SCHEME}everyone/everyone/-/0) look"));
        assert_eq!(
            mention_links(&format!("<@&{ROLE}> pls"), &l),
            format!("![@Mods]({SCHEME}role/{ROLE}/3b82f6/0) pls")
        );
        assert_eq!(mention_links("@stranger", &l), "@stranger");
        assert_eq!(mention_links("`@mika`", &l), "`@mika`");
        assert_eq!(mention_links("```\n@mika\n```", &l), "```\n@mika\n```");
        assert_eq!(mention_links("[@mika](https://x)", &l), "[@mika](https://x)");
        assert_eq!(mention_links("a@mika.dev", &l), "a@mika.dev");
        assert_eq!(mention_links("@mikasa", &l), "@mikasa");
        assert_eq!(mention_links("@here.", &l), format!("![@here]({SCHEME}everyone/here/-/0)."));
        assert_eq!(
            mention_links(&format!("nice <:blob_cat:{ROLE}>!"), &l),
            "nice ![:blob\\_cat:](fuwa-emoji:https://x/e.webp)!"
        );
        assert_eq!(mention_links("<a:party:01J9AAAAAAAAAAAAAAAAAAAAAA>", &l), ":party:");
    }

    #[test]
    fn only_emoji_messages_are_jumbo() {
        assert!(only_emoji("🌸"));
        assert!(only_emoji("👋 🎉"));
        assert!(only_emoji("<:blob_cat:01J9ZZZZZZZZZZZZZZZZZZZZZZ>"));
        assert!(!only_emoji("hi 👋"));
        assert!(!only_emoji("123"));
        assert!(!only_emoji(""));
    }

    #[test]
    fn the_token_before_the_caret() {
        assert_eq!(token("hi @mi", 6), Some((3, "mi".into())));
        assert_eq!(token("@", 1), Some((0, String::new())));
        assert_eq!(token("mail@x", 6), None);
        assert_eq!(token("@mika hi", 8), None);
    }

    #[test]
    fn picked_roles_go_out_as_tokens() {
        let picked = vec![("Mods".to_owned(), ROLE.to_owned())];
        assert_eq!(encode("hey @Mods and @Modsy", &picked), format!("hey <@&{ROLE}> and @Modsy"));
        assert_eq!(encode("@Mods", &picked), format!("<@&{ROLE}>"));
    }
}
