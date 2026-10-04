//! Emoji: a server's own (`<:name:id>`, `<a:name:id>` when it moves), the
//! person's other servers' on the same instance, and the standard Unicode
//! set. Like the web app's `lib/emoji-search.ts` and `lib/emoji-catalog.ts`.
//!
//! Another server's emoji goes into a message as its token, and the app
//! sends it along (`SendMessageRequest.emojis`): the instance checks the
//! sender is in that server and keeps its picture link with the message, so
//! everyone in the channel sees it. Emoji from other instances aren't
//! offered: their pictures would load from someone else's server.
//!
//! In the composer server emoji are written `:name:`. When two servers have
//! the same name the later one is `:name~2:` (then `~3`), so each name finds
//! one.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::pb;

// ───────────────────────── The standard set ─────────────────────────

/// One standard emoji: how it's written, its names (the first is the one
/// typed after a colon), words to find it by, and its five skin tones.
#[derive(Debug)]
pub struct Standard {
    pub char: String,
    pub names: Vec<String>,
    pub words: String,
    pub skins: Option<Vec<String>>,
}

#[derive(Debug)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub emojis: Vec<Standard>,
}

/// The standard emoji, the same file the web app bundles, read the first time
/// something needs them.
pub fn standard() -> &'static [Group] {
    static GROUPS: OnceLock<Vec<Group>> = OnceLock::new();
    GROUPS.get_or_init(|| {
        let raw: serde_json::Value =
            serde_json::from_str(include_str!("../../../web/src/lib/emoji-data.json")).unwrap_or_default();
        let text = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_owned();
        let mut groups = Vec::new();
        for group in raw.as_array().into_iter().flatten() {
            let Some([id, name, entries]) = group.as_array().map(Vec::as_slice) else { continue };
            let emojis = entries
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| {
                    let e = e.as_array()?;
                    Some(Standard {
                        char: text(e.first()?),
                        names: text(e.get(1)?).split(' ').map(str::to_owned).collect(),
                        words: e.get(2).map(text).unwrap_or_default(),
                        skins: e.get(3).and_then(|s| s.as_array()).map(|s| s.iter().map(text).collect()),
                    })
                })
                .collect();
            groups.push(Group { id: text(id), name: text(name), emojis });
        }
        groups
    })
}

impl Standard {
    /// In a skin tone: 0 is the default yellow, 1 to 5 light to dark.
    pub fn toned(&self, tone: u8) -> &str {
        match (tone, &self.skins) {
            (1..=5, Some(skins)) => skins.get(tone as usize - 1).unwrap_or(&self.char),
            _ => &self.char,
        }
    }
}

/// A standard emoji by its character.
pub fn standard_by_char(char: &str) -> Option<&'static Standard> {
    static BY_CHAR: OnceLock<HashMap<&'static str, &'static Standard>> = OnceLock::new();
    BY_CHAR
        .get_or_init(|| standard().iter().flat_map(|g| &g.emojis).map(|e| (e.char.as_str(), e)).collect())
        .get(char)
        .copied()
}

// ───────────────────────── Server emoji ─────────────────────────

pub use crate::core::emoji::{token, token_at};

fn word(s: &str, min: usize, under: bool) -> bool {
    (min..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || (under && b == b'_'))
}

/// The server a section of emoji comes from.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerRef {
    pub id: String,
    pub name: String,
    pub icon_url: String,
}

/// A server's emoji with the name the composer writes it by.
#[derive(Debug, Clone, PartialEq)]
pub struct Custom {
    pub emoji: pb::Emoji,
    pub alias: String,
    pub server: usize,
    /// From the server being written in.
    pub here: bool,
}

/// Every server emoji someone can use in a server: its own, then their other
/// servers' on the same instance (in the order they were joined).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Catalog {
    pub servers: Vec<ServerRef>,
    pub emojis: Vec<Custom>,
    /// Indexes into `emojis`, by section, each sorted by name.
    pub sections: Vec<(usize, Vec<usize>)>,
    by_alias: HashMap<String, usize>,
    by_id: HashMap<String, usize>,
}

impl Catalog {
    /// The catalog for writing in `server_id`, from what the instance has loaded.
    pub fn of(servers: &[pb::Server], emojis: &HashMap<String, Vec<pb::Emoji>>, server_id: &str) -> Self {
        let here = servers.iter().filter(|s| s.id == server_id);
        let mut out = Catalog::default();
        for server in here.chain(servers.iter().filter(|s| s.id != server_id)) {
            let mut list: Vec<&pb::Emoji> = emojis.get(&server.id).into_iter().flatten().collect();
            if list.is_empty() {
                continue;
            }
            list.sort_by(|a, b| a.name.cmp(&b.name));
            let at = out.servers.len();
            out.servers.push(ServerRef {
                id: server.id.clone(),
                name: server.name.clone(),
                icon_url: server.icon_url.clone(),
            });
            let mut section = Vec::with_capacity(list.len());
            for emoji in list {
                let mut alias = emoji.name.clone();
                let mut n = 2;
                while out.by_alias.contains_key(&alias.to_lowercase()) {
                    alias = format!("{}~{n}", emoji.name);
                    n += 1;
                }
                let index = out.emojis.len();
                out.by_alias.insert(alias.to_lowercase(), index);
                out.by_id.insert(emoji.id.clone(), index);
                out.emojis.push(Custom { emoji: emoji.clone(), alias, server: at, here: server.id == server_id });
                section.push(index);
            }
            out.sections.push((at, section));
        }
        out
    }

    /// Only one server's own (the welcome screen).
    pub fn own(server: &pb::Server, emojis: &[pb::Emoji]) -> Self {
        let map = HashMap::from([(server.id.clone(), emojis.to_vec())]);
        Self::of(std::slice::from_ref(server), &map, &server.id)
    }

    pub fn by_id(&self, id: &str) -> Option<&Custom> {
        self.by_id.get(id).map(|&n| &self.emojis[n])
    }

    pub fn by_alias(&self, alias: &str) -> Option<&Custom> {
        self.by_alias.get(&alias.to_lowercase()).map(|&n| &self.emojis[n])
    }

    pub fn server_of(&self, custom: &Custom) -> &ServerRef {
        &self.servers[custom.server]
    }

    /// The text with each `:name:` (or `:name~2:`) of a catalog emoji made into its token.
    pub fn encode(&self, content: &str) -> String {
        if self.emojis.is_empty() {
            return content.to_owned();
        }
        let mut out = String::with_capacity(content.len());
        let mut rest = content;
        while let Some(start) = rest.find(':') {
            let after = &rest[start + 1..];
            let alias = after.find(':').map(|e| &after[..e]).filter(|a| {
                let (name, n) = a.split_once('~').unwrap_or((a, "1"));
                word(name, 2, true) && (1..=3).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit())
            });
            // Not inside a token someone typed (`<:name:id>`).
            let in_token = rest[..start].ends_with('<') || rest[..start].ends_with("<a");
            match alias.filter(|_| !in_token).and_then(|a| Some((a, self.by_alias(a)?))) {
                Some((alias, custom)) => {
                    out.push_str(&rest[..start]);
                    out.push_str(&token(&custom.emoji));
                    rest = &after[alias.len() + 1..];
                }
                None => {
                    out.push_str(&rest[..=start]);
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// Ids to pictures of the other servers' emoji, for drawing tokens in messages.
    pub fn other_pictures(&self) -> impl Iterator<Item = (&str, &str)> {
        self.emojis.iter().filter(|c| !c.here).map(|c| (c.emoji.id.as_str(), c.emoji.url.as_str()))
    }
}

// ───────────────────────── Picking ─────────────────────────

/// One emoji to pick.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    /// `c:` and a server emoji's id, or `u:` and the character (untoned), for remembering it.
    pub key: String,
    /// What it's written as after a colon.
    pub name: String,
    /// What goes in the box: the character, or `:alias:` for a server's own.
    pub insert: String,
    /// A server emoji's picture.
    pub url: Option<String>,
    /// The server it's from, when it isn't the one being written in.
    pub from: Option<String>,
}

impl Choice {
    pub fn custom(catalog: &Catalog, c: &Custom) -> Self {
        Self {
            key: format!("c:{}", c.emoji.id),
            name: c.alias.clone(),
            insert: format!(":{}:", c.alias),
            url: Some(c.emoji.url.clone()),
            from: (!c.here).then(|| catalog.server_of(c).name.clone()),
        }
    }

    pub fn standard(e: &Standard, tone: u8) -> Self {
        Self {
            key: format!("u:{}", e.char),
            name: e.names.first().cloned().unwrap_or_default(),
            insert: e.toned(tone).to_owned(),
            url: None,
            from: None,
        }
    }

    /// A remembered one, if it's still around.
    pub fn recalled(key: &str, catalog: &Catalog, tone: u8) -> Option<Self> {
        if let Some(id) = key.strip_prefix("c:") {
            return catalog.by_id(id).map(|c| Self::custom(catalog, c));
        }
        standard_by_char(key.strip_prefix("u:")?).map(|e| Self::standard(e, tone))
    }
}

/// Emoji whose names (or, for standard ones, words) match `query`: whole
/// names first, then names that start with it, then ones that have it;
/// within each, this server's, then other servers', then standard ones.
pub fn search(query: &str, catalog: &Catalog, tone: u8, limit: usize) -> Vec<Choice> {
    let q = query.trim().trim_matches(':').to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let name_rank = |name: &str| {
        let name = name.to_lowercase();
        if name == q {
            Some(0)
        } else if name.starts_with(&q) {
            Some(1)
        } else if name.contains(&q) {
            Some(2)
        } else {
            None
        }
    };
    let mut ranked: Vec<(u8, usize, Choice)> = Vec::new();
    for (_, section) in &catalog.sections {
        for &n in section {
            let c = &catalog.emojis[n];
            let best = [name_rank(&c.alias), name_rank(&c.emoji.name)].into_iter().flatten().min();
            if let Some(best) = best {
                let rank = best * 3 + u8::from(!c.here);
                ranked.push((rank, ranked.len(), Choice::custom(catalog, c)));
            }
        }
    }
    for group in standard() {
        for e in &group.emojis {
            let best = e.names.iter().filter_map(|n| name_rank(n)).min();
            let rank = match best {
                Some(best) => best * 3 + 2,
                None if q.len() >= 2 && e.words.split(' ').any(|w| w.starts_with(&q)) => 9,
                None => continue,
            };
            ranked.push((rank, ranked.len(), Choice::standard(e, tone)));
        }
    }
    ranked.sort_by_key(|(rank, order, _)| (*rank, *order));
    ranked.into_iter().take(limit).map(|(_, _, c)| c).collect()
}

/// The `:query` being typed just before the caret, two letters or more.
pub fn typing(text: &str, caret: usize) -> Option<(usize, String)> {
    let before = text.get(..caret)?;
    let colon = before.rfind(':')?;
    let query = &before[colon + 1..];
    let starts_word = before[..colon].chars().next_back().is_none_or(char::is_whitespace);
    (starts_word
        && query.len() >= 2
        && query.len() <= 32
        && query.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    .then(|| (colon, query.to_lowercase()))
}

/// Draws emoji in the system's color emoji font where asking for it by name
/// works. Not on Linux: the text system there drops any font without an "m"
/// (every color emoji font) when it's asked for by name, which also takes it
/// out of the fallback that finds emoji for ordinary text.
pub trait InColor: gpui_kit::Styled + Sized {
    fn in_color(self) -> Self {
        if cfg!(target_os = "macos") {
            self.font_family("Apple Color Emoji")
        } else if cfg!(target_os = "windows") {
            self.font_family("Segoe UI Emoji")
        } else {
            // Don't name "Noto Color Emoji" here: see above.
            self
        }
    }
}

impl<E: gpui_kit::Styled> InColor for E {}

/// Images whose address starts with this are server emoji, drawn in the line.
pub const SCHEME: &str = "fuwa-emoji:";

/// Draws `![:name:](fuwa-emoji:<url>)` images as the picture, the size of the text.
pub struct Plugin;

struct Picture {
    url: String,
}

impl gpui_kit::component::text::MarkdownPlugin for Plugin {
    fn name(&self) -> &str {
        "fuwa-emoji"
    }

    fn parse(
        &self,
        node: &gpui_kit::component::text::markdown_ast::Node,
        _: &gpui_kit::component::text::MarkdownParseContext<'_>,
    ) -> Option<gpui_kit::component::text::MarkdownNode> {
        let gpui_kit::component::text::markdown_ast::Node::Image(image) = node else { return None };
        let url = image.url.strip_prefix(SCHEME)?;
        Some(
            gpui_kit::component::text::MarkdownNode::new("fuwa-emoji", Picture { url: url.to_owned() })
                .text(image.alt.clone())
                .markdown(image.alt.clone()),
        )
    }

    fn render_inline(
        &self,
        node: &gpui_kit::component::text::MarkdownNode,
        context: &gpui_kit::component::text::InlineRenderContext,
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::App,
    ) -> Option<gpui_kit::component::text::InlineElement> {
        use gpui_kit::{IntoElement as _, ObjectFit, Styled as _, StyledImage as _, img, px};
        let picture = node.data::<Picture>()?;
        let size = context.font_size() * 1.375;
        let alt = node.as_text().to_owned();
        Some(gpui_kit::component::text::InlineElement::new(
            img(gpui_kit::SharedString::from(picture.url.clone()))
                .size(size)
                .mx(px(1.0))
                .object_fit(ObjectFit::Contain)
                .with_fallback(move || alt.clone().into_any_element()),
        ))
    }
}

/// Markdown with the emoji plugin, kept between frames.
pub fn markdown_extensions() -> gpui_kit::component::text::MarkdownExtensions {
    gpui_kit::component::text::MarkdownExtensions::default()
        .plugin(Plugin)
        .plugin(crate::ui::timestamps::Plugin)
        .parser_revision(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "01J9ZZZZZZZZZZZZZZZZZZZZZZ";

    fn emoji(id: &str, name: &str) -> pb::Emoji {
        pb::Emoji { id: id.into(), name: name.into(), url: format!("https://x/{id}"), ..Default::default() }
    }

    fn server(id: &str) -> pb::Server {
        pb::Server { id: id.into(), name: format!("Server {id}"), ..Default::default() }
    }

    fn catalog() -> Catalog {
        let servers = [server("a"), server("b")];
        let emojis = HashMap::from([
            ("a".to_owned(), vec![emoji(ID, "blob_cat")]),
            (
                "b".to_owned(),
                vec![emoji("01J9YYYYYYYYYYYYYYYYYYYYYY", "blob_cat"), emoji("01J9XXXXXXXXXXXXXXXXXXXXXX", "wave")],
            ),
        ]);
        Catalog::of(&servers, &emojis, "b")
    }

    #[test]
    fn the_server_written_in_comes_first_and_names_stay_apart() {
        let c = catalog();
        assert_eq!(c.servers[c.sections[0].0].id, "b");
        assert_eq!(c.by_alias("blob_cat").unwrap().emoji.id, "01J9YYYYYYYYYYYYYYYYYYYYYY");
        assert_eq!(c.by_alias("BLOB_CAT~2").unwrap().emoji.id, ID);
        assert!(!c.by_id(ID).unwrap().here);
    }

    #[test]
    fn names_become_tokens_and_other_servers_go_along() {
        let c = catalog();
        let sent = c.encode("hi :blob_cat~2: :wave: :nope: 12:30");
        assert_eq!(sent, format!("hi <:blob_cat:{ID}> <:wave:01J9XXXXXXXXXXXXXXXXXXXXXX> :nope: 12:30"));
        assert_eq!(c.encode(&format!("<:blob_cat:{ID}>")), format!("<:blob_cat:{ID}>"));
    }

    #[test]
    fn searching() {
        let c = catalog();
        let found = search("blob", &c, 0, 3);
        assert_eq!(found[0].insert, ":blob_cat:");
        assert_eq!(found[1].from.as_deref(), Some("Server a"));
        assert!(search("smile", &Catalog::default(), 0, 5).iter().any(|c| c.insert == "😄"));
        let wave = search("wave", &Catalog::default(), 3, 1);
        assert_ne!(wave[0].insert, "👋");
        assert_eq!(wave[0].key, "u:👋");
    }

    #[test]
    fn the_standard_set_loads() {
        let groups = standard();
        assert_eq!(groups.len(), 8);
        assert!(groups.iter().map(|g| g.emojis.len()).sum::<usize>() > 1500);
        assert!(standard_by_char("\u{1F44D}\u{FE0F}").is_some_and(|e| e.skins.is_some()));
    }

    #[test]
    fn typing_after_a_colon() {
        assert_eq!(typing("so :sm", 6), Some((3, "sm".into())));
        assert_eq!(typing("so :s", 5), None);
        assert_eq!(typing("12:30", 5), None);
    }
}
