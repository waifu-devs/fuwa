//! Mentions: finding `@username`, `@everyone`, `@here` and roles (`<@&id>`)
//! in messages so they show as links you can click, and the @ list in the
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
                (&r.id, &r.name).hash(&mut h);
                (r.id.clone(), r.name.clone())
            })
            .collect();
        // The server's own last, so they win should two ever share an id.
        let mut emojis = HashMap::new();
        let others = i.emojis.iter().filter(|(id, _)| *id != server_id).flat_map(|(_, list)| list);
        for e in others.chain(i.emojis.get(server_id).into_iter().flatten()) {
            (&e.id, &e.url).hash(&mut h);
            emojis.insert(e.id.clone(), e.url.clone());
        }
        Self { people, roles, emojis, digest: h.finish() }
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

/// Turns mentions into links (`[@Mika](fuwa-mention:user/mika)`), leaving
/// code and links alone. People who aren't in the server stay plain text.
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
                let name = look.roles.get(&id).map(String::as_str).unwrap_or("deleted-role");
                out.push_str(&format!("[@{}]({SCHEME}role/{id})", escape(name)));
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
                    out.push_str(&format!("[@{loud}]({SCHEME}everyone/{loud})"));
                    i += 1 + loud.len();
                    prev = Some('x');
                    continue;
                }
                if let Some(name) = username_at(after)
                    && !after[name.len()..].chars().next().is_some_and(word)
                    && let Some(shown) = look.people.get(&name.to_lowercase())
                {
                    out.push_str(&format!("[@{}]({SCHEME}user/{})", escape(shown), name.to_lowercase()));
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
            emojis: [(ROLE.to_owned(), "https://x/e.webp".to_owned())].into_iter().collect(),
            digest: 0,
        }
    }

    #[test]
    fn mentions_become_links_but_code_and_links_stay() {
        let l = look();
        assert_eq!(mention_links("hi @mika!", &l), format!("hi [@Mika Sato]({SCHEME}user/mika)!"));
        assert_eq!(mention_links("@Everyone look", &l), format!("[@everyone]({SCHEME}everyone/everyone) look"));
        assert_eq!(mention_links(&format!("<@&{ROLE}> pls"), &l), format!("[@Mods]({SCHEME}role/{ROLE}) pls"));
        assert_eq!(mention_links("@stranger", &l), "@stranger");
        assert_eq!(mention_links("`@mika`", &l), "`@mika`");
        assert_eq!(mention_links("```\n@mika\n```", &l), "```\n@mika\n```");
        assert_eq!(mention_links("[@mika](https://x)", &l), "[@mika](https://x)");
        assert_eq!(mention_links("a@mika.dev", &l), "a@mika.dev");
        assert_eq!(mention_links("@mikasa", &l), "@mikasa");
        assert_eq!(mention_links("@here.", &l), format!("[@here]({SCHEME}everyone/here)."));
        assert_eq!(
            mention_links(&format!("nice <:blob_cat:{ROLE}>!"), &l),
            "nice ![:blob\\_cat:](fuwa-emoji:https://x/e.webp)!"
        );
        assert_eq!(mention_links("<a:party:01J9AAAAAAAAAAAAAAAAAAAAAA>", &l), ":party:");
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
