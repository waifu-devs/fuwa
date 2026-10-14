//! Server emoji as messages write them: `<:name:id>`, or `<a:name:id>`
//! when the picture moves.

use crate::core::store::InstanceState;
use crate::core::vault::EmojiRef;
use crate::pb;

/// The most server emoji one encrypted message carries.
pub const CARRIED: usize = 50;

/// How a server emoji is written in a message.
pub fn token(e: &pb::Emoji) -> String {
    format!("<{}:{}:{}>", if e.animated { "a" } else { "" }, e.name, e.id)
}

fn word(s: &str, min: usize, under: bool) -> bool {
    (min..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || (under && b == b'_'))
}

/// A token at the start of `rest`: its name, its id and how long it is.
pub fn token_at(rest: &str) -> Option<(&str, &str, usize)> {
    let inner = rest.strip_prefix("<a:").or_else(|| rest.strip_prefix("<:"))?;
    let end = inner.as_bytes()[..inner.len().min(66)].iter().position(|&b| b == b'>')?;
    let (name, id) = inner[..end].split_once(':')?;
    (word(name, 2, true) && word(id, 10, false)).then(|| (name, id, rest.len() - inner.len() + end + 1))
}

/// The ids of the server emoji `content` writes, once each, in order.
pub fn token_ids(content: &str) -> Vec<&str> {
    let mut ids: Vec<&str> = Vec::new();
    for (start, _) in content.match_indices('<') {
        if let Some((_, id, _)) = token_at(&content[start..])
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    ids
}

/// Emoji from your other servers on the instance that `content` writes, to
/// send along with a message in `server_id` so everyone there can see them.
/// The instance checks each one; any it won't keep show as their names.
pub fn outside(i: &InstanceState, server_id: &str, content: &str) -> Vec<pb::Emoji> {
    if !content.contains('<') {
        return Vec::new();
    }
    let others: Vec<&pb::Emoji> =
        i.emojis.iter().filter(|(id, _)| *id != server_id).flat_map(|(_, list)| list).collect();
    token_ids(content)
        .into_iter()
        .filter(|id| !i.emojis.get(server_id).is_some_and(|own| own.iter().any(|e| e.id == *id)))
        .filter_map(|id| others.iter().find(|e| e.id == id).map(|e| (*e).clone()))
        .collect()
}

/// Emoji from any of your servers on the instance that `content` writes,
/// to carry inside an encrypted message (`DirectMessageText.emojis`), so
/// every device in it can draw them: once each, at most [`CARRIED`].
pub fn carried(i: &InstanceState, content: &str) -> Vec<pb::Emoji> {
    if !content.contains('<') {
        return Vec::new();
    }
    let ids = token_ids(content);
    let mut out: Vec<pb::Emoji> = Vec::new();
    for server in &i.servers {
        for e in i.emojis.get(&server.id).into_iter().flatten() {
            if out.len() < CARRIED && ids.contains(&e.id.as_str()) && !out.iter().any(|o| o.id == e.id) {
                out.push(e.clone());
            }
        }
    }
    out
}

/// The addresses an instance's emoji pictures may be at: the one it was
/// added by, and the public one it says it has.
pub fn origins(i: &InstanceState) -> Vec<String> {
    let mut out = vec![i.url.clone()];
    if let Some(node) = i.node.as_ref().filter(|n| !n.public_url.is_empty()) {
        out.push(node.public_url.clone());
    }
    out
}

/// Whether a server emoji someone else's device carried could be one from
/// this instance: a sound id and name, and a picture under `/media/` at
/// one of the instance's `origins`.
pub fn valid_carried(e: &pb::Emoji, origins: &[String]) -> bool {
    let id_ok = (10..=32).contains(&e.id.len()) && e.id.bytes().all(|b| b.is_ascii_alphanumeric());
    if !id_ok || !word(&e.name, 2, true) {
        return false;
    }
    let Ok(url) = url::Url::parse(&e.url) else { return false };
    matches!(url.scheme(), "http" | "https")
        && url.path().starts_with("/media/")
        && origins.iter().filter_map(|o| url::Url::parse(o).ok()).any(|o| o.origin() == url.origin())
}

/// The carried emoji worth keeping: sound ones (`valid_carried`) that
/// `content` writes, once each, at most [`CARRIED`]. Only their sender
/// vouches for them; any left out show as their names.
pub fn checked(emojis: &[pb::Emoji], content: &str, origins: &[String]) -> Vec<EmojiRef> {
    let ids = token_ids(content);
    let mut out: Vec<EmojiRef> = Vec::new();
    for e in emojis {
        if out.len() == CARRIED {
            break;
        }
        if ids.contains(&e.id.as_str()) && !out.iter().any(|o| o.id == e.id) && valid_carried(e, origins) {
            out.push(EmojiRef::of(e));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "01J9ZZZZZZZZZZZZZZZZZZZZZZ";
    const OTHER: &str = "01J9XXXXXXXXXXXXXXXXXXXXXX";

    #[test]
    fn tokens_are_found_once() {
        let text = format!("<:a:{ID}> <a:bb:{ID}> <:bb:short> <:bb:{OTHER}>");
        assert_eq!(token_ids(&text), [ID, OTHER]);
        assert_eq!(token_at(&format!("<a:party:{ID}>!")).map(|t| t.2), Some(36));
        assert_eq!(token_at("<:é:0123456789>"), None);
    }

    #[test]
    fn other_servers_emoji_go_along() {
        let mut i = InstanceState::new("k", "https://x");
        let emoji = |id: &str| pb::Emoji { id: id.into(), name: "blob".into(), ..Default::default() };
        i.emojis.insert("here".into(), vec![emoji(ID)]);
        i.emojis.insert("there".into(), vec![emoji(OTHER)]);
        let sent = outside(&i, "here", &format!("<:blob:{ID}> <:blob:{OTHER}> <:blob:{OTHER}>"));
        assert_eq!(sent.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), [OTHER]);
    }

    fn carried_emoji(id: &str, url: &str) -> pb::Emoji {
        pb::Emoji { id: id.into(), server_id: "s".into(), name: "blob_2".into(), url: url.into(), ..Default::default() }
    }

    #[test]
    fn carried_emoji_are_checked() {
        let origins = vec!["https://fuwa.example".to_owned(), "http://10.0.0.2:8080".to_owned()];
        let good = carried_emoji(ID, "https://fuwa.example/media/e1.png");
        let lan = carried_emoji(OTHER, "http://10.0.0.2:8080/media/e2.gif");
        let text = format!("hi <:blob_2:{ID}> <a:blob_2:{OTHER}>");
        let kept = checked(&[good.clone(), lan.clone(), good.clone()], &text, &origins);
        assert_eq!(kept.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), [ID, OTHER]);
        assert_eq!(kept[0], EmojiRef::of(&good));
        // Somewhere else, or not a picture under /media/.
        for url in [
            "https://evil.example/media/e1.png",
            "http://fuwa.example/media/e1.png",
            "https://fuwa.example:8443/media/e1.png",
            "https://fuwa.example/api/e1.png",
            "https://fuwa.example/media/../api/x",
            "javascript:alert(1)",
            "/media/e1.png",
        ] {
            assert!(checked(&[carried_emoji(ID, url)], &text, &origins).is_empty(), "{url}");
        }
        // A bad id or name.
        let mut bad = good.clone();
        bad.name = "a".into();
        assert!(checked(&[bad], &text, &origins).is_empty());
        let mut bad = good.clone();
        bad.name = "no-dash".into();
        assert!(checked(&[bad], &text, &origins).is_empty());
        for id in ["short", "0123456789-abcdef", &"A".repeat(33)] {
            let bad = carried_emoji(id, "https://fuwa.example/media/e1.png");
            assert!(checked(&[bad], &format!("<:blob_2:{id}>"), &origins).is_empty(), "{id}");
        }
        // One the text doesn't write.
        assert!(checked(std::slice::from_ref(&good), "no tokens here", &origins).is_empty());
    }

    #[test]
    fn at_most_fifty_are_carried() {
        let origins = vec!["https://fuwa.example".to_owned()];
        let list: Vec<pb::Emoji> =
            (0..60).map(|n| carried_emoji(&format!("EMOJI{n:08}"), "https://fuwa.example/media/x.png")).collect();
        let text: String = list.iter().map(token).collect();
        assert_eq!(checked(&list, &text, &origins).len(), CARRIED);
        let mut i = InstanceState::new("k", "https://fuwa.example");
        i.servers = vec![pb::Server { id: "s".into(), ..Default::default() }];
        i.emojis.insert("s".into(), list);
        assert_eq!(carried(&i, &text).len(), CARRIED);
    }

    #[test]
    fn every_servers_emoji_are_carried_once() {
        let mut i = InstanceState::new("k", "https://fuwa.example");
        i.servers = ["a", "b"].map(|id| pb::Server { id: id.into(), ..Default::default() }).to_vec();
        i.emojis.insert("a".into(), vec![carried_emoji(ID, "https://fuwa.example/media/a.png")]);
        i.emojis.insert("b".into(), vec![carried_emoji(OTHER, "https://fuwa.example/media/b.png")]);
        let sent = carried(&i, &format!("<:blob_2:{OTHER}> <:blob_2:{ID}> <:blob_2:{OTHER}>"));
        assert_eq!(sent.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), [ID, OTHER]);
        assert!(carried(&i, "plain").is_empty());
    }
}
