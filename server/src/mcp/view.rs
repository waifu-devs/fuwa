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
        .map(|a| trim(json!({ "filename": a.filename, "content_type": a.content_type, "size": a.size, "url": a.url })))
        .collect();
    let embeds: Vec<Value> = m
        .embeds
        .iter()
        .map(|e| trim(json!({ "title": e.title, "description": e.description, "url": e.url })))
        .collect();
    trim(json!({
        "id": m.id,
        "channel_id": m.channel_id,
        "author": author,
        "content": m.content,
        "kind": if kind == pb::MessageKind::Unspecified { String::new() } else { name(kind.as_str_name(), "MESSAGE_KIND_") },
        "reply_to_id": m.reply_to_id,
        "attachments": attachments,
        "embeds": embeds,
        "mentions_everyone": m.mentions_everyone,
        "mention_role_ids": m.mention_role_ids,
        "poll": m.poll.as_ref().map(poll),
        "created_at": time(&m.created_at),
        "edited_at": time(&m.edited_at),
    }))
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
        Some(Payload::ApplicationUpdated(_)) => ("application_updated", json!({})),
        Some(Payload::SecureRecordAdded(_)) | Some(Payload::SecureRecordDeleted(_)) => ("secure_channel", json!({})),
        Some(Payload::SharedChannelsUpdated(_)) => ("shared_channels_updated", json!({})),
        Some(Payload::PollUpdated(p)) => (
            "poll_updated",
            json!({ "channel_id": p.channel_id, "message_id": p.message_id, "poll": p.poll.as_ref().map(poll) }),
        ),
        Some(Payload::VoiceStateUpdated(_)) | Some(Payload::VoiceStateRemoved(_)) => ("voice", json!({})),
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
}
