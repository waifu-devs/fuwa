//! Discord's local RPC, as games speak it: frames of a little-endian opcode,
//! a little-endian length and that much JSON. Only what discord-rpc, the
//! Game SDK's activity manager and pypresence need is understood; the
//! answers say nothing about the person (the user in READY is a placeholder).

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use crate::pb;

pub const HANDSHAKE: u32 = 0;
pub const FRAME: u32 = 1;
pub const CLOSE: u32 = 2;
pub const PING: u32 = 3;
pub const PONG: u32 = 4;

/// Larger frames end the connection: an activity is a few hundred bytes.
const MAX_FRAME: usize = 64 * 1024;

const MAX_TEXT: usize = 128;
const MAX_LABEL: usize = 32;
const MAX_BUTTONS: usize = 2;
const MAX_BUTTON_URL: usize = 512;
const MAX_PICTURE: usize = 2048;
const MAX_APPLICATION_ID: usize = 64;
const MAX_PARTY: u64 = 1_000_000;

pub async fn read_frame(r: &mut (impl AsyncRead + Unpin)) -> std::io::Result<(u32, Value)> {
    let mut head = [0u8; 8];
    r.read_exact(&mut head).await?;
    let op = u32::from_le_bytes(head[..4].try_into().expect("4 bytes"));
    let len = u32::from_le_bytes(head[4..].try_into().expect("4 bytes")) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    Ok((op, value))
}

pub async fn write_frame(w: &mut (impl AsyncWrite + Unpin), op: u32, value: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(value).unwrap_or_default();
    let mut frame = Vec::with_capacity(8 + body.len());
    frame.extend_from_slice(&op.to_le_bytes());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    w.write_all(&frame).await?;
    w.flush().await
}

/// The application id a handshake carries, when it's one Discord's libraries send.
pub fn handshake(value: &Value) -> Option<String> {
    if value.get("v").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let id = match value.get("client_id")? {
        Value::String(s) => s.trim().to_owned(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let fits = !id.is_empty() && id.len() <= MAX_APPLICATION_ID && id.chars().all(|c| c.is_ascii_alphanumeric());
    fits.then_some(id)
}

/// READY, with a placeholder user: nothing about whoever is signed in.
pub fn ready() -> Value {
    json!({
        "cmd": "DISPATCH",
        "evt": "READY",
        "nonce": null,
        "data": {
            "v": 1,
            "config": {
                "cdn_host": "",
                "api_endpoint": "",
                "environment": "production",
            },
            "user": {
                "id": "0",
                "username": "fuwa",
                "discriminator": "0",
                "global_name": null,
                "avatar": null,
                "bot": false,
                "flags": 0,
                "premium_type": 0,
            },
        },
    })
}

/// What a FRAME asked for.
#[derive(Debug, PartialEq)]
pub enum Command {
    /// Show this activity, or none.
    SetActivity(Option<Box<pb::Activity>>),
    /// Anything else; answered without doing anything.
    Other,
}

/// Reads a FRAME's command and the answer to send back.
pub fn command(value: &Value, name: &str, application_id: &str) -> (Command, Value) {
    let cmd = value.get("cmd").and_then(Value::as_str).unwrap_or_default();
    let nonce = value.get("nonce").cloned().unwrap_or(Value::Null);
    let args = value.get("args").cloned().unwrap_or(Value::Null);
    match cmd {
        "SET_ACTIVITY" => {
            let activity = args.get("activity").filter(|a| !a.is_null());
            let parsed = activity.map(|a| Box::new(activity_of(a, name, application_id)));
            let answer = json!({ "cmd": cmd, "evt": null, "nonce": nonce, "data": activity.cloned() });
            (Command::SetActivity(parsed), answer)
        }
        // The Game SDK subscribes to invites and joins; nothing is ever sent to them.
        "SUBSCRIBE" | "UNSUBSCRIBE" => {
            let evt = value.get("evt").cloned().unwrap_or(Value::Null);
            (Command::Other, json!({ "cmd": cmd, "evt": null, "nonce": nonce, "data": { "evt": evt } }))
        }
        _ => (Command::Other, error(cmd, nonce)),
    }
}

/// The one error answer, with a fixed message.
pub fn error(cmd: &str, nonce: Value) -> Value {
    json!({
        "cmd": cmd,
        "evt": "ERROR",
        "nonce": nonce,
        "data": { "code": 1000, "message": "fuwa doesn't do that." },
    })
}

/// A Discord activity as fuwa's, trimmed to what an instance takes, so a
/// long state line never gets the whole update refused. `name` is what the
/// person allowed (the program's name), since Discord takes it from the
/// application's page instead.
pub fn activity_of(a: &Value, name: &str, application_id: &str) -> pb::Activity {
    fn s(v: Option<&Value>) -> &str {
        v.and_then(Value::as_str).unwrap_or_default()
    }
    let assets = a.get("assets");
    let asset = |key: &str| assets.map(|x| s(x.get(key))).unwrap_or_default();
    let timestamps = a.get("timestamps");
    let at = |key: &str| timestamps.and_then(|t| t.get(key)).and_then(Value::as_u64).filter(|&n| n > 0).map(time);
    let size = a.get("party").and_then(|p| p.get("size")).and_then(Value::as_array);
    let part = |i: usize| size.and_then(|s| s.get(i)).and_then(Value::as_u64).unwrap_or(0).min(MAX_PARTY) as u32;
    let (party_size, party_max) = match (part(0), part(1)) {
        (_, 0) => (0, 0),
        (size, max) => (size.min(max), max),
    };
    let kind = match a.get("type").and_then(Value::as_u64) {
        Some(1) => pb::ActivityKind::Streaming,
        Some(2) => pb::ActivityKind::Listening,
        Some(3) => pb::ActivityKind::Watching,
        Some(5) => pb::ActivityKind::Competing,
        _ => pb::ActivityKind::Playing,
    };
    let buttons = a
        .get("buttons")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|b| {
            let label = clean(s(b.get("label")), MAX_LABEL);
            let url = https(s(b.get("url")), MAX_BUTTON_URL)?;
            (!label.is_empty()).then_some(pb::ActivityButton { label, url })
        })
        .take(MAX_BUTTONS)
        .collect();
    let name = clean(name, MAX_TEXT);
    pb::Activity {
        kind: kind as i32,
        name: if name.is_empty() { "A game".into() } else { name },
        details: clean(s(a.get("details")), MAX_TEXT),
        state: clean(s(a.get("state")), MAX_TEXT),
        started_at: at("start"),
        ends_at: at("end"),
        party_size,
        party_max,
        large_image_url: picture(asset("large_image")),
        large_text: clean(asset("large_text"), MAX_TEXT),
        small_image_url: picture(asset("small_image")),
        small_text: clean(asset("small_text"), MAX_TEXT),
        buttons,
        application_id: application_id.to_owned(),
    }
}

/// Seconds or milliseconds since 1970: the libraries send either.
fn time(n: u64) -> prost_types::Timestamp {
    let ms = if n < 100_000_000_000 { n.saturating_mul(1000) } else { n };
    let ms = ms.min(i64::MAX as u64) as i64;
    prost_types::Timestamp { seconds: ms / 1000, nanos: ((ms % 1000) * 1_000_000) as i32 }
}

/// One line of at most `max` characters, without the characters instances refuse.
fn clean(value: &str, max: usize) -> String {
    let hidden = |c: char| {
        matches!(c, '\u{061C}' | '\u{180E}' | '\u{200B}' | '\u{200C}' | '\u{200E}' | '\u{200F}' | '\u{FEFF}')
            || ('\u{202A}'..='\u{202E}').contains(&c)
            || ('\u{2060}'..='\u{2069}').contains(&c)
    };
    let line: String = value.chars().filter(|c| !hidden(*c)).map(|c| if c.is_control() { ' ' } else { c }).collect();
    line.trim().chars().take(max).collect::<String>().trim_end().to_owned()
}

/// An https link without a user name or password, or nothing.
fn https(value: &str, max: usize) -> Option<String> {
    let value = value.trim();
    let parsed = url::Url::parse(value).ok()?;
    let fine = parsed.scheme() == "https"
        && parsed.host_str().is_some_and(|h| !h.is_empty())
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && value.len() <= max;
    fine.then(|| parsed.to_string()).filter(|s| s.len() <= max)
}

/// A picture link the instance can fetch; Discord's own picture keys mean
/// nothing to it, so they're left out.
fn picture(value: &str) -> String {
    https(value, MAX_PICTURE).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_round_trip() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        write_frame(&mut a, PING, &json!({ "x": 1 })).await.unwrap();
        let (op, value) = read_frame(&mut b).await.unwrap();
        assert_eq!(op, PING);
        assert_eq!(value, json!({ "x": 1 }));
    }

    #[tokio::test]
    async fn oversized_frames_end_the_connection() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let mut head = PING.to_le_bytes().to_vec();
        head.extend_from_slice(&(MAX_FRAME as u32 + 1).to_le_bytes());
        a.write_all(&head).await.unwrap();
        assert!(read_frame(&mut b).await.is_err());
    }

    #[test]
    fn handshakes() {
        assert_eq!(handshake(&json!({ "v": 1, "client_id": "383226320970055681" })), Some("383226320970055681".into()));
        assert_eq!(handshake(&json!({ "v": 1, "client_id": 42 })), Some("42".into()));
        assert_eq!(handshake(&json!({ "v": 2, "client_id": "1" })), None);
        assert_eq!(handshake(&json!({ "v": 1, "client_id": "../x" })), None);
        assert_eq!(handshake(&json!({ "v": 1 })), None);
    }

    #[test]
    fn ready_says_nothing_about_the_person() {
        let user = &ready()["data"]["user"];
        assert_eq!(user["id"], "0");
        assert_eq!(user["username"], "fuwa");
        assert!(user["avatar"].is_null());
    }

    #[test]
    fn set_activity_from_discord_rpc() {
        let frame = json!({
            "cmd": "SET_ACTIVITY",
            "nonce": "1",
            "args": {
                "pid": 4242,
                "activity": {
                    "state": "In a party\n\u{202E}x",
                    "details": "Chapter 3",
                    "timestamps": { "start": 1_700_000_000u64, "end": 1_700_000_600_000u64 },
                    "assets": {
                        "large_image": "summit",
                        "large_text": "The summit",
                        "small_image": "https://example.com/s.png",
                    },
                    "party": { "id": "p", "size": [7, 4] },
                    "buttons": [
                        { "label": "Watch", "url": "https://example.com/w" },
                        { "label": "Bad", "url": "javascript:alert(1)" },
                        { "label": "", "url": "https://example.com/e" },
                        { "label": "Third", "url": "https://example.com/3" },
                        { "label": "Fourth", "url": "https://example.com/4" },
                    ],
                    "type": 3,
                },
            },
        });
        let (cmd, answer) = command(&frame, "celeste", "123");
        let Command::SetActivity(Some(a)) = cmd else { panic!("expected an activity") };
        assert_eq!(a.name, "celeste");
        assert_eq!(a.kind, pb::ActivityKind::Watching as i32);
        assert_eq!(a.state, "In a party x");
        assert_eq!(a.started_at.unwrap().seconds, 1_700_000_000);
        assert_eq!(a.ends_at.unwrap().seconds, 1_700_000_600);
        assert_eq!((a.party_size, a.party_max), (4, 4));
        assert_eq!(a.large_image_url, "");
        assert_eq!(a.small_image_url, "https://example.com/s.png");
        assert_eq!(a.buttons.iter().map(|b| b.label.as_str()).collect::<Vec<_>>(), ["Watch", "Third"]);
        assert_eq!(a.application_id, "123");
        assert_eq!(answer["cmd"], "SET_ACTIVITY");
        assert_eq!(answer["nonce"], "1");
        assert!(answer["evt"].is_null());
    }

    #[test]
    fn long_text_is_cut_to_fit() {
        let long = "é".repeat(500);
        let a =
            activity_of(&json!({ "details": long, "buttons": [{ "label": long, "url": "https://x.dev" }] }), "", "");
        assert_eq!(a.details.chars().count(), MAX_TEXT);
        assert_eq!(a.buttons[0].label.chars().count(), MAX_LABEL);
        assert_eq!(a.name, "A game");
    }

    #[test]
    fn clearing_and_other_commands() {
        let (cmd, answer) = command(&json!({ "cmd": "SET_ACTIVITY", "args": { "activity": null } }), "x", "");
        assert_eq!(cmd, Command::SetActivity(None));
        assert!(answer["data"].is_null());
        let (cmd, answer) = command(&json!({ "cmd": "CONNECTIONS_CALLBACK", "nonce": 9 }), "x", "");
        assert_eq!(cmd, Command::Other);
        assert_eq!(answer["evt"], "ERROR");
        assert_eq!(answer["nonce"], 9);
        let (_, answer) = command(&json!({ "cmd": "SUBSCRIBE", "evt": "ACTIVITY_JOIN", "nonce": "n" }), "x", "");
        assert_eq!(answer["data"]["evt"], "ACTIVITY_JOIN");
        assert!(answer["evt"].is_null());
    }
}
