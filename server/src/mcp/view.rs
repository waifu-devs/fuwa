//! What tools and resources return: the API's messages as compact JSON,
//! with enums as short names and times as RFC 3339.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use crate::pb;

/// An enum's protobuf name without its prefix, lower case: CHANNEL_TYPE_TEXT is "text".
fn name(full: &str, prefix: &str) -> String {
    full.strip_prefix(prefix).unwrap_or(full).to_ascii_lowercase()
}

fn time(t: &Option<prost_types::Timestamp>) -> Value {
    t.as_ref().map_or(Value::Null, |t| Value::from(t.to_string()))
}

/// Drops empty strings, nulls, false and empty lists, so answers stay short.
fn trim(value: Value) -> Value {
    let Value::Object(map) = value else { return value };
    Value::Object(
        map.into_iter()
            .filter(|(_, v)| match v {
                Value::Null => false,
                Value::Bool(b) => *b,
                Value::String(s) => !s.is_empty(),
                Value::Array(a) => !a.is_empty(),
                _ => true,
            })
            .collect::<Map<_, _>>(),
    )
}

pub fn user(u: &pb::User) -> Value {
    let kind = pb::AccountKind::try_from(u.kind).unwrap_or_default();
    trim(json!({
        "id": u.id,
        "username": u.username,
        "display_name": u.display_name,
        "agent": kind == pb::AccountKind::Agent,
        "status": u.status,
    }))
}

pub fn server(s: &pb::Server) -> Value {
    trim(json!({
        "id": s.id,
        "name": s.name,
        "description": s.description,
        "owner_id": s.owner_id,
        "member_count": s.member_count,
        "system_channel_id": s.system_channel_id,
        "has_rules": s.has_rules,
        "region": s.region,
        "created_at": time(&s.created_at),
    }))
}

pub fn channel(c: &pb::Channel) -> Value {
    let kind = pb::ChannelType::try_from(c.r#type).unwrap_or_default();
    trim(json!({
        "id": c.id,
        "name": c.name,
        "type": name(kind.as_str_name(), "CHANNEL_TYPE_"),
        "parent_id": c.parent_id,
        "topic": c.topic,
        "position": c.position,
        "slowmode_seconds": c.slowmode_seconds,
        "shared": c.shared.as_ref().map(|s| json!({ "home": s.home })),
    }))
}

pub fn role(r: &pb::Role) -> Value {
    let permissions: Vec<String> = r
        .permissions
        .iter()
        .filter_map(|p| pb::Permission::try_from(*p).ok())
        .map(|p| name(p.as_str_name(), "PERMISSION_"))
        .collect();
    trim(json!({
        "id": r.id,
        "name": r.name,
        "position": r.position,
        "color": r.color.map(|c| format!("#{c:06x}")),
        "permissions": permissions,
        "hoist": r.hoist,
        "mentionable": r.mentionable,
    }))
}

pub fn member(m: &pb::Member) -> Value {
    let mut value = trim(json!({
        "user": m.user.as_ref().map(user),
        "nickname": m.nickname,
        "role_ids": m.role_ids,
        "joined_at": time(&m.joined_at),
        "pending": m.pending,
    }));
    if let Some(until) = &m.timed_out_until
        && crate::id::millis(until) > crate::id::now_ms()
    {
        value["timed_out_until"] = Value::from(until.to_string());
    }
    value
}

pub fn profile(p: &pb::Profile) -> Value {
    trim(json!({
        "user": p.user.as_ref().map(user),
        "pronouns": p.pronouns,
        "bio": p.bio,
        "created_at": time(&p.created_at),
    }))
}

pub fn emoji(e: &pb::Emoji) -> Value {
    trim(json!({ "id": e.id, "name": e.name, "animated": e.animated, "write_as": emoji_tag(e) }))
}

fn emoji_tag(e: &pb::Emoji) -> String {
    format!("<{}:{}:{}>", if e.animated { "a" } else { "" }, e.name, e.id)
}

/// A message, with its author's name when `authors` has them.
pub fn message(m: &pb::Message, authors: &HashMap<&str, &pb::User>) -> Value {
    let kind = pb::MessageKind::try_from(m.kind).unwrap_or_default();
    let author = match (&m.webhook, &m.shared, authors.get(m.author_id.as_str())) {
        (Some(hook), _, _) => json!({ "id": m.author_id, "display_name": hook.name, "webhook": true }),
        (_, Some(shared), _) => {
            let mut author = shared.user.as_ref().map_or_else(|| json!({ "id": m.author_id }), user);
            author["from_server"] = Value::from(shared.server.as_ref().map(|s| s.name.clone()).unwrap_or_default());
            author
        }
        (_, _, Some(u)) => user(u),
        _ => json!({ "id": m.author_id }),
    };
    let attachments: Vec<Value> = m
        .attachments
        .iter()
        .map(|a| {
            // A voice message's length, never its waveform.
            let voice_ms = a.voice.as_ref().map(|v| v.duration_ms);
            trim(json!({
                "filename": a.filename,
                "content_type": a.content_type,
                "size": a.size,
                "url": a.url,
                // 0 is unknown.
                "width": (a.width > 0).then_some(a.width),
                "height": (a.height > 0).then_some(a.height),
                "voice_duration_ms": voice_ms,
            }))
        })
        .collect();
    let embeds: Vec<Value> = m
        .embeds
        .iter()
        .map(|e| trim(json!({ "title": e.title, "description": e.description, "url": e.url })))
        .collect();
    // As members see it: the file on this instance and its credit, never the seal.
    let gif = m.gif.as_ref().map(|g| {
        let provider = pb::GifProvider::try_from(g.provider).unwrap_or_default();
        let provider = match provider {
            pb::GifProvider::Unspecified => String::new(),
            provider => name(provider.as_str_name(), "GIF_PROVIDER_"),
        };
        trim(json!({ "url": g.url, "width": g.width, "height": g.height, "title": g.title, "provider": provider }))
    });
    trim(json!({
        "id": m.id,
        "channel_id": m.channel_id,
        "author": author,
        "content": m.content,
        "kind": if kind == pb::MessageKind::Unspecified { String::new() } else { name(kind.as_str_name(), "MESSAGE_KIND_") },
        "reply_to_id": m.reply_to_id,
        "attachments": attachments,
        "embeds": embeds,
        "gif": gif,
        "mentions_everyone": m.mentions_everyone,
        "mention_role_ids": m.mention_role_ids,
        "mention_user_ids": m.mention_user_ids,
        "buttons": buttons(&m.components),
        // Who used what; never what they typed.
        "interaction": m.interaction.as_ref().map(|i| {
            let kind = pb::InteractionKind::try_from(i.kind).unwrap_or_default();
            trim(json!({
                "id": i.id,
                "kind": name(kind.as_str_name(), "INTERACTION_KIND_"),
                "command": i.command,
                "user_id": i.user_id,
            }))
        }),
        "poll": m.poll.as_ref().map(poll),
        "created_at": time(&m.created_at),
        "edited_at": time(&m.edited_at),
        "pinned_at": time(&m.pinned_at),
        "reactions": m.reactions.iter().map(reaction).collect::<Vec<_>>(),
    }))
}

/// A reaction: the emoji as it's written in a message (`<:name:id>` for a
/// custom one), how many, and whether the agent is one of them.
pub fn reaction(r: &pb::Reaction) -> Value {
    let emoji = if r.emoji_id.is_empty() {
        r.emoji.clone()
    } else {
        format!("<{}:{}:{}>", if r.animated { "a" } else { "" }, r.emoji_name, r.emoji_id)
    };
    trim(json!({ "emoji": emoji, "count": r.count, "me": r.me }))
}

pub fn messages(list: &[pb::Message], authors: &[pb::User]) -> Vec<Value> {
    let authors: HashMap<&str, &pb::User> = authors.iter().map(|u| (u.id.as_str(), u)).collect();
    list.iter().map(|m| message(m, &authors)).collect()
}

/// A poll: its answers with their counts (0 each while an anonymous one
/// runs) and the reader's own picks.
pub fn poll(p: &pb::Poll) -> Value {
    let answers: Vec<Value> = p
        .answers
        .iter()
        .map(|a| trim(json!({ "id": a.id, "text": a.text, "emoji": a.emoji, "votes": a.votes })))
        .collect();
    trim(json!({
        "question": p.question,
        "answers": answers,
        "multiple": p.multiple,
        "anonymous": p.anonymous,
        "voters": p.voters,
        "my_answer_ids": p.my_answer_ids,
        "ends_at": time(&p.ends_at),
        "ended_at": time(&p.ended_at),
    }))
}

/// A message's buttons, row by row, as members see them.
fn buttons(rows: &[pb::ComponentRow]) -> Vec<Value> {
    rows.iter()
        .map(|row| {
            let buttons: Vec<Value> = row
                .buttons
                .iter()
                .map(|b| {
                    let style = pb::ButtonStyle::try_from(b.style).unwrap_or_default();
                    trim(json!({
                        "label": b.label,
                        "custom_id": b.custom_id,
                        "style": name(style.as_str_name(), "BUTTON_STYLE_"),
                        "url": b.url,
                        "disabled": b.disabled,
                    }))
                })
                .collect();
            Value::from(buttons)
        })
        .collect()
}

/// A slash command, as its agent set it.
pub fn command(c: &pb::Command) -> Value {
    let options: Vec<Value> = c
        .options
        .iter()
        .map(|o| {
            let kind = pb::CommandOptionType::try_from(o.r#type).unwrap_or_default();
            trim(json!({
                "name": o.name,
                "description": o.description,
                "type": name(kind.as_str_name(), "COMMAND_OPTION_TYPE_"),
                "required": o.required,
                "choices": o.choices,
            }))
        })
        .collect();
    trim(json!({ "name": c.name, "description": c.description, "options": options }))
}

/// An interaction, for the agent it's for.
pub fn interaction(i: &pb::Interaction) -> Value {
    let kind = pb::InteractionKind::try_from(i.kind).unwrap_or_default();
    let arguments: Map<String, Value> =
        i.arguments.iter().map(|a| (a.name.clone(), Value::from(a.value.clone()))).collect();
    trim(json!({
        "id": i.id,
        "kind": name(kind.as_str_name(), "INTERACTION_KIND_"),
        "channel_id": i.channel_id,
        "user_id": i.user_id,
        "command": i.command,
        "arguments": if arguments.is_empty() { Value::Null } else { Value::Object(arguments) },
        "message_id": i.message_id,
        "custom_id": i.custom_id,
        "created_at": time(&i.created_at),
    }))
}

/// An app's live tile: what it says and whose it is.
pub fn live_tile(t: &pb::LiveTile) -> Value {
    let content = t.content.clone().unwrap_or_default();
    trim(json!({
        "id": t.id,
        "channel_id": t.channel_id,
        "source_id": t.source_id,
        "source_name": t.source_name,
        "title": content.title,
        "status": content.status,
        "live": content.live,
        "rows": content.rows.iter().map(|r| json!({ "label": r.label, "value": r.value })).collect::<Vec<_>>(),
        "progress": content.progress,
        "action": content.action,
        "expires_at": time(&t.expires_at),
    }))
}

/// An event from a server's log: its type and what it carries.
pub fn event(e: &pb::Event) -> Value {
    use pb::event::Payload;
    let none = HashMap::new();
    let (kind, data) = match &e.payload {
        Some(Payload::ServerUpdated(p)) => ("server_updated", json!({ "server": p.server.as_ref().map(server) })),
        Some(Payload::ServerDeleted(_)) => ("server_deleted", json!({})),
        Some(Payload::ChannelCreated(p)) => ("channel_created", json!({ "channel": p.channel.as_ref().map(channel) })),
        Some(Payload::ChannelUpdated(p)) => ("channel_updated", json!({ "channel": p.channel.as_ref().map(channel) })),
        Some(Payload::ChannelDeleted(p)) => ("channel_deleted", json!({ "channel_id": p.channel_id })),
        Some(Payload::MessageCreated(p)) => {
            ("message_created", json!({ "message": p.message.as_ref().map(|m| message(m, &none)) }))
        }
        Some(Payload::MessageUpdated(p)) => {
            ("message_updated", json!({ "message": p.message.as_ref().map(|m| message(m, &none)) }))
        }
        Some(Payload::MessageDeleted(p)) => {
            ("message_deleted", json!({ "channel_id": p.channel_id, "message_id": p.message_id }))
        }
        Some(Payload::MemberJoined(p)) => ("member_joined", json!({ "member": p.member.as_ref().map(member) })),
        Some(Payload::MemberUpdated(p)) => ("member_updated", json!({ "member": p.member.as_ref().map(member) })),
        Some(Payload::MemberLeft(p)) => {
            let reason = pb::LeaveReason::try_from(p.reason).unwrap_or_default();
            ("member_left", json!({ "user_id": p.user_id, "reason": name(reason.as_str_name(), "LEAVE_REASON_") }))
        }
        Some(Payload::UserUpdated(p)) => ("user_updated", json!({ "user": p.user.as_ref().map(user) })),
        Some(Payload::RoleCreated(p)) => ("role_created", json!({ "role": p.role.as_ref().map(role) })),
        Some(Payload::RoleUpdated(p)) => ("role_updated", json!({ "role": p.role.as_ref().map(role) })),
        Some(Payload::RoleDeleted(p)) => ("role_deleted", json!({ "role_id": p.role_id })),
        Some(Payload::EmojisUpdated(p)) => {
            ("emojis_updated", json!({ "emojis": p.emojis.iter().map(emoji).collect::<Vec<_>>() }))
        }
        Some(Payload::ProfileItemsUpdated(p)) => (
            "profile_items_updated",
            json!({ "items": p.items.iter().map(|i| json!({ "id": i.id, "name": i.name })).collect::<Vec<_>>() }),
        ),
        Some(Payload::ApplicationUpdated(_)) => ("application_updated", json!({})),
        Some(Payload::SecureRecordAdded(_)) | Some(Payload::SecureRecordDeleted(_)) => ("secure_channel", json!({})),
        Some(Payload::SharedChannelsUpdated(_)) => ("shared_channels_updated", json!({})),
        Some(Payload::PollUpdated(p)) => (
            "poll_updated",
            json!({ "channel_id": p.channel_id, "message_id": p.message_id, "poll": p.poll.as_ref().map(poll) }),
        ),
        Some(Payload::VoiceStateUpdated(_)) | Some(Payload::VoiceStateRemoved(_)) => ("voice", json!({})),
        Some(Payload::LiveTileUpdated(p)) => ("live_tile_updated", json!({ "tile": p.tile.as_ref().map(live_tile) })),
        Some(Payload::LiveTileEnded(p)) => {
            ("live_tile_ended", json!({ "channel_id": p.channel_id, "tile_id": p.tile_id, "source_id": p.source_id }))
        }
        Some(Payload::ThreadUpdated(p)) => (
            "thread_updated",
            json!({
                "channel_id": p.channel_id,
                "thread_id": p.thread_id,
                "reply_count": p.thread.as_ref().map_or(0, |t| t.reply_count),
                "locked": p.thread.as_ref().is_some_and(|t| t.locked),
            }),
        ),
        Some(Payload::ReactionUpdated(p)) => (
            "reaction_updated",
            json!({
                "channel_id": p.channel_id,
                "message_id": p.message_id,
                "thread_id": p.thread_id,
                "reaction": p.reaction.as_ref().map(reaction),
                "user_id": p.user_id,
                "added": p.added,
            }),
        ),
        Some(Payload::ReactionsCleared(p)) => (
            "reactions_cleared",
            json!({
                "channel_id": p.channel_id,
                "message_id": p.message_id,
                "thread_id": p.thread_id,
                "emoji": p.emoji,
                "emoji_id": p.emoji_id,
            }),
        ),
        Some(Payload::MessagePinned(p)) => (
            "message_pinned",
            json!({
                "channel_id": p.channel_id,
                "message_id": p.message_id,
                "thread_id": p.thread_id,
                "pinned": p.pinned_at.is_some(),
            }),
        ),
        // Only the agent it's for ever gets this (events.rs), so it shows
        // whole: what was used, by whom, with what.
        Some(Payload::InteractionCreated(p)) => {
            ("interaction_created", json!({ "interaction": p.interaction.as_ref().map(interaction) }))
        }
        None => ("unknown", json!({})),
    };
    let mut value = trim(json!({
        "sequence": e.sequence,
        "type": kind,
        "actor_id": e.actor_id,
        "created_at": time(&e.created_at),
    }));
    if let (Value::Object(out), Value::Object(data)) = (&mut value, data) {
        out.extend(trim(Value::Object(data)).as_object().cloned().unwrap_or_default());
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_enum_names_and_no_empty_fields() {
        let c = pb::Channel {
            id: "c".into(),
            name: "general".into(),
            r#type: pb::ChannelType::Text as i32,
            ..Default::default()
        };
        assert_eq!(
            channel(&c),
            json!({ "id": "c", "name": "general", "type": "text", "position": 0, "slowmode_seconds": 0 })
        );
        let r = pb::Role {
            permissions: vec![pb::Permission::SendMessages as i32],
            color: Some(0xff00aa),
            ..Default::default()
        };
        assert_eq!(role(&r)["permissions"], json!(["send_messages"]));
        assert_eq!(role(&r)["color"], json!("#ff00aa"));
    }

    #[test]
    fn events_carry_their_type() {
        let e = pb::Event {
            sequence: 7,
            payload: Some(pb::event::Payload::MessageDeleted(pb::MessageDeleted {
                channel_id: "c".into(),
                message_id: "m".into(),
            })),
            ..Default::default()
        };
        assert_eq!(
            event(&e),
            json!({ "sequence": 7, "type": "message_deleted", "channel_id": "c", "message_id": "m" })
        );
    }

    #[test]
    fn a_gif_shows_as_members_see_it() {
        let m = pb::Message {
            id: "m".into(),
            gif: Some(pb::MessageGif {
                url: "/media/g".into(),
                width: 320,
                height: 240,
                provider: pb::GifProvider::Giphy as i32,
                seal: "secret".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let shown = message(&m, &HashMap::new());
        assert_eq!(shown["gif"], json!({ "url": "/media/g", "width": 320, "height": 240, "provider": "giphy" }));
        assert!(!shown.to_string().contains("secret"), "never the seal");
    }

    #[test]
    fn attachments_show_as_members_see_them() {
        let file = |name: &str, width: i32| pb::Attachment {
            id: "f".into(),
            filename: name.into(),
            content_type: "image/png".into(),
            size: 2048,
            url: "https://fuwa.example/media/f".into(),
            width,
            height: width / 2,
            voice: None,
        };
        let m = pb::Message {
            id: "m".into(),
            attachments: vec![file("cat.png", 640), file("dog.png", 0)],
            ..Default::default()
        };
        let shown = message(&m, &HashMap::new());
        assert_eq!(
            shown["attachments"],
            json!([
                {
                    "filename": "cat.png",
                    "content_type": "image/png",
                    "size": 2048,
                    "url": "https://fuwa.example/media/f",
                    "width": 640,
                    "height": 320,
                },
                { "filename": "dog.png", "content_type": "image/png", "size": 2048, "url": "https://fuwa.example/media/f" },
            ])
        );
        // A voice message: its length, never its waveform.
        let voice = pb::Attachment {
            filename: "voice-message.ogg".into(),
            content_type: "audio/ogg; codecs=opus".into(),
            size: 4096,
            voice: Some(pb::VoiceNote { duration_ms: 3000, waveform: vec![9, 200, 40] }),
            ..Default::default()
        };
        let m = pb::Message { id: "v".into(), attachments: vec![voice], ..Default::default() };
        let shown = message(&m, &HashMap::new());
        assert_eq!(
            shown["attachments"],
            json!([{ "filename": "voice-message.ogg", "content_type": "audio/ogg; codecs=opus", "size": 4096, "voice_duration_ms": 3000 }])
        );
    }
}
