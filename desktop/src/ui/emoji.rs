//! Emoji: a server's own (`<:name:id>`, `<a:name:id>` when it moves) and the
//! everyday Unicode ones the pickers offer. The same list as the web app's
//! `lib/emoji.ts`.

use crate::pb;

/// Everyday Unicode emoji, with the names people type after a colon.
pub const UNICODE: &[(&str, &str)] = &[
    ("😀", "grinning smile happy"),
    ("😄", "smile happy joy"),
    ("😂", "joy laugh lol tears"),
    ("🤣", "rofl rolling laugh"),
    ("😊", "blush smile"),
    ("😇", "innocent halo angel"),
    ("🙂", "slight_smile"),
    ("😉", "wink"),
    ("😍", "heart_eyes love"),
    ("🥰", "smiling_hearts love"),
    ("😘", "kiss"),
    ("😋", "yum tasty"),
    ("😛", "tongue"),
    ("😜", "wink_tongue crazy"),
    ("🤪", "zany crazy"),
    ("🤗", "hug hugs"),
    ("🤔", "thinking think hmm"),
    ("🤨", "raised_eyebrow sus"),
    ("😐", "neutral meh"),
    ("😑", "expressionless"),
    ("🙄", "eye_roll rolling_eyes"),
    ("😏", "smirk"),
    ("😴", "sleeping zzz"),
    ("😌", "relieved"),
    ("🥺", "pleading puppy please"),
    ("😢", "cry sad"),
    ("😭", "sob crying"),
    ("😤", "triumph huff"),
    ("😠", "angry mad"),
    ("🤯", "mind_blown exploding"),
    ("😳", "flushed blush"),
    ("🥵", "hot"),
    ("🥶", "cold freezing"),
    ("😱", "scream shock"),
    ("😅", "sweat_smile phew"),
    ("😬", "grimace yikes"),
    ("🫠", "melting"),
    ("🫡", "salute"),
    ("🤫", "shush quiet"),
    ("🤭", "giggle oops"),
    ("😎", "sunglasses cool"),
    ("🤓", "nerd"),
    ("🥳", "partying_face celebrate"),
    ("😈", "smiling_imp devil"),
    ("💀", "skull dead"),
    ("👻", "ghost boo"),
    ("🤖", "robot bot"),
    ("👀", "eyes look"),
    ("👍", "thumbsup +1 yes like"),
    ("👎", "thumbsdown -1 no"),
    ("👏", "clap applause"),
    ("🙌", "raised_hands hooray"),
    ("🙏", "pray please thanks"),
    ("👋", "wave hello hi bye"),
    ("🤝", "handshake deal"),
    ("✌️", "v peace"),
    ("🤞", "crossed_fingers luck"),
    ("👌", "ok_hand perfect"),
    ("🤙", "call_me shaka"),
    ("💪", "muscle strong flex"),
    ("🫶", "heart_hands"),
    ("❤️", "heart love red"),
    ("🩷", "pink_heart"),
    ("🧡", "orange_heart"),
    ("💛", "yellow_heart"),
    ("💚", "green_heart"),
    ("💙", "blue_heart"),
    ("💜", "purple_heart"),
    ("🖤", "black_heart"),
    ("🤍", "white_heart"),
    ("💔", "broken_heart"),
    ("💖", "sparkling_heart"),
    ("💕", "two_hearts"),
    ("✨", "sparkles shiny"),
    ("⭐", "star"),
    ("🌟", "glowing_star"),
    ("🔥", "fire lit hot"),
    ("💯", "100 hundred"),
    ("🎉", "tada party celebrate"),
    ("🎊", "confetti"),
    ("🎁", "gift present"),
    ("🎂", "cake birthday"),
    ("🏆", "trophy win"),
    ("🥇", "first_place gold"),
    ("🎮", "video_game gaming controller"),
    ("🎧", "headphones music"),
    ("🎵", "music note"),
    ("🎨", "art paint"),
    ("📚", "books study"),
    ("💻", "computer laptop code"),
    ("🛠️", "tools build"),
    ("🐛", "bug"),
    ("🚀", "rocket ship launch"),
    ("💡", "bulb idea"),
    ("📌", "pin pushpin"),
    ("📣", "mega announcement"),
    ("📝", "memo note"),
    ("✅", "white_check_mark check done yes"),
    ("❌", "x no cross"),
    ("⚠️", "warning"),
    ("❓", "question"),
    ("❗", "exclamation"),
    ("💤", "zzz sleep"),
    ("💬", "speech chat"),
    ("🔔", "bell"),
    ("🔒", "lock"),
    ("🌸", "cherry_blossom sakura flower"),
    ("🌈", "rainbow"),
    ("☀️", "sun sunny"),
    ("🌙", "moon night"),
    ("⚡", "zap lightning"),
    ("☕", "coffee"),
    ("🍵", "tea"),
    ("🍕", "pizza"),
    ("🍜", "ramen noodles"),
    ("🍣", "sushi"),
    ("🍙", "rice_ball onigiri"),
    ("🍰", "shortcake"),
    ("🍓", "strawberry"),
    ("🐱", "cat"),
    ("🐶", "dog"),
    ("🦊", "fox"),
    ("🐰", "rabbit bunny"),
    ("🐻", "bear"),
    ("🐼", "panda"),
    ("🐸", "frog"),
    ("🐧", "penguin"),
    ("🦄", "unicorn"),
    ("🐉", "dragon"),
    ("🍀", "four_leaf_clover luck"),
    ("🌊", "wave ocean"),
    ("🏠", "house home"),
    ("👑", "crown king queen"),
    ("💎", "gem diamond"),
];

/// How a server emoji is written in a message.
pub fn token(e: &pb::Emoji) -> String {
    format!("<{}:{}:{}>", if e.animated { "a" } else { "" }, e.name, e.id)
}

/// Images whose address starts with this are server emoji, drawn in the line.
pub const SCHEME: &str = "fuwa-emoji:";

/// One emoji to pick: a server's own (with its picture) or a Unicode one.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub name: String,
    /// What goes in the box: the character, or `:name:` for a server's own.
    pub insert: String,
    /// A server emoji's picture.
    pub url: Option<String>,
}

impl Choice {
    fn own(e: &pb::Emoji) -> Self {
        Self { name: e.name.clone(), insert: format!(":{}:", e.name), url: Some(e.url.clone()) }
    }

    fn plain(char: &str, name: &str) -> Self {
        Self { name: name.to_owned(), insert: char.to_owned(), url: None }
    }
}

/// Server emoji and Unicode ones whose names start with (then contain) `query`.
pub fn search(query: &str, server: &[pb::Emoji], limit: usize) -> Vec<Choice> {
    let q = query.to_lowercase();
    let mut ranked: Vec<(u8, Choice)> = server
        .iter()
        .filter(|e| e.name.to_lowercase().contains(&q))
        .map(|e| (if e.name.to_lowercase().starts_with(&q) { 0 } else { 2 }, Choice::own(e)))
        .collect();
    for (char, names) in UNICODE {
        let mut names = names.split(' ');
        let name = names.clone().find(|n| n.starts_with(&q)).or_else(|| names.find(|n| n.contains(&q)));
        if let Some(name) = name {
            ranked.push((if name.starts_with(&q) { 1 } else { 3 }, Choice::plain(char, name)));
        }
    }
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().take(limit).map(|(_, c)| c).collect()
}

/// Everything for the picker: the server's own (by name), then the
/// everyday ones, each narrowed by `query`.
pub fn all(query: &str, server: &[pb::Emoji]) -> (Vec<Choice>, Vec<Choice>) {
    let q = query.trim().trim_matches(':').to_lowercase();
    let mut own: Vec<Choice> = server.iter().filter(|e| e.name.to_lowercase().contains(&q)).map(Choice::own).collect();
    own.sort_by(|a, b| a.name.cmp(&b.name));
    let plain = UNICODE
        .iter()
        .filter(|(_, names)| q.is_empty() || names.split(' ').any(|n| n.contains(&q)))
        .map(|(char, names)| Choice::plain(char, names.split(' ').next().unwrap_or_default()))
        .collect();
    (own, plain)
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

/// The text to send: each `:name:` of one of the server's emoji becomes its token.
pub fn encode(content: &str, server: &[pb::Emoji]) -> String {
    if server.is_empty() {
        return content.to_owned();
    }
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(start) = rest.find(':') {
        let after = &rest[start + 1..];
        let end = after.find(':');
        let name = end
            .map(|e| &after[..e])
            .filter(|n| (2..=32).contains(&n.len()) && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        // Not inside a token someone typed (`<:name:id>`).
        let in_token = rest[..start].ends_with('<') || rest[..start].ends_with("<a");
        let emoji = name.filter(|_| !in_token).and_then(|n| {
            server.iter().find(|e| e.name == n).or_else(|| server.iter().find(|e| e.name.eq_ignore_ascii_case(n)))
        });
        match (emoji, name) {
            (Some(e), Some(n)) => {
                out.push_str(&rest[..start]);
                out.push_str(&token(e));
                rest = &after[n.len() + 1..];
            }
            _ => {
                out.push_str(&rest[..=start]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

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
    gpui_kit::component::text::MarkdownExtensions::default().plugin(Plugin).parser_revision(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emoji(name: &str) -> pb::Emoji {
        pb::Emoji { id: "01J9ZZZZZZZZZZZZZZZZZZZZZZ".into(), name: name.into(), url: "u".into(), ..Default::default() }
    }

    #[test]
    fn names_become_tokens() {
        let own = [emoji("blob_cat")];
        assert_eq!(encode("hi :blob_cat: :nope:", &own), "hi <:blob_cat:01J9ZZZZZZZZZZZZZZZZZZZZZZ> :nope:");
        assert_eq!(encode("<:blob_cat:01J9ZZZZZZZZZZZZZZZZZZZZZZ>", &own), "<:blob_cat:01J9ZZZZZZZZZZZZZZZZZZZZZZ>");
        assert_eq!(encode("12:30 and :BLOB_CAT:", &own), "12:30 and <:blob_cat:01J9ZZZZZZZZZZZZZZZZZZZZZZ>");
    }

    #[test]
    fn typing_after_a_colon() {
        assert_eq!(typing("so :sm", 6), Some((3, "sm".into())));
        assert_eq!(typing("so :s", 5), None);
        assert_eq!(typing("12:30", 5), None);
        assert!(search("smile", &[], 3).iter().any(|c| c.insert == "😄"));
        assert_eq!(search("blob", &[emoji("blob_cat")], 3)[0].insert, ":blob_cat:");
    }
}
