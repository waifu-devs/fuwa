//! Server emoji as messages write them: `<:name:id>`, or `<a:name:id>`
//! when the picture moves.

use crate::core::store::InstanceState;
use crate::pb;

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
}
