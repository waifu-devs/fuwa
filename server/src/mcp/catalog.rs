//! Resources (read-only views an app can attach to a conversation) and
//! prompts (common agent tasks, written out for the model).

use serde_json::{Value, json};
use tonic::Status;

use super::tools::{list_channels, list_messages, list_roles, may_use};
use super::{Cx, RpcError, call, view};
use crate::pb;

const INSTANCE: &str = "fuwa://instance";

pub fn templates() -> Vec<Value> {
    vec![
        json!({
            "uriTemplate": "fuwa://servers/{server_id}",
            "name": "server",
            "title": "A server",
            "description": "A server with its channels and roles.",
            "mimeType": "application/json"
        }),
        json!({
            "uriTemplate": "fuwa://servers/{server_id}/channels/{channel_id}",
            "name": "channel",
            "title": "A channel's latest messages",
            "description": "The latest 50 messages in a channel, oldest first.",
            "mimeType": "application/json"
        }),
    ]
}

/// The instance, and each server the agent is in.
pub async fn list_resources(cx: &Cx) -> Result<Value, RpcError> {
    let mut resources = vec![json!({
        "uri": INSTANCE,
        "name": "instance",
        "title": cx.instance,
        "description": "This instance and the agent's account on it.",
        "mimeType": "application/json"
    })];
    if let Ok(r) = call!(cx, server_service_client::ServerServiceClient.list_servers(pb::ListServersRequest {})) {
        resources.extend(r.servers.iter().map(|s| {
            json!({
                "uri": format!("fuwa://servers/{}", s.id),
                "name": format!("server-{}", s.id),
                "title": s.name,
                "description": "A server with its channels and roles.",
                "mimeType": "application/json"
            })
        }));
    }
    Ok(json!({ "resources": resources }))
}

pub async fn read_resource(cx: &Cx, params: &Value) -> Result<Value, RpcError> {
    let uri = params.get("uri").and_then(Value::as_str).unwrap_or_default();
    let read = match parse(uri) {
        Some(Uri::Instance) => {
            Ok(json!({ "instance": cx.instance, "version": crate::VERSION, "me": view::user(&cx.me) }))
        }
        Some(Uri::Server(server_id)) => server(cx, server_id).await,
        Some(Uri::Channel(server_id, channel_id)) => channel(cx, server_id, channel_id).await,
        None => return Err(RpcError { code: -32002, message: format!("there's no resource {uri:?}") }),
    };
    match read {
        Ok(value) => Ok(json!({
            "contents": [{ "uri": uri, "mimeType": "application/json", "text": value.to_string() }]
        })),
        Err(status) => Err(RpcError { code: -32603, message: status.message().to_string() }),
    }
}

enum Uri<'a> {
    Instance,
    Server(&'a str),
    Channel(&'a str, &'a str),
}

fn parse(uri: &str) -> Option<Uri<'_>> {
    if uri == INSTANCE {
        return Some(Uri::Instance);
    }
    let rest = uri.strip_prefix("fuwa://servers/")?;
    let id = |s: &str| !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric());
    match rest.split('/').collect::<Vec<_>>()[..] {
        [server] if id(server) => Some(Uri::Server(server)),
        [server, "channels", channel] if id(server) && id(channel) => Some(Uri::Channel(server, channel)),
        _ => None,
    }
}

async fn server(cx: &Cx, server_id: &str) -> Result<Value, Status> {
    may_use(cx, server_id).await?;
    let server = call!(
        cx,
        server_service_client::ServerServiceClient.get_server(pb::GetServerRequest { server_id: server_id.into() })
    )?;
    let channels = list_channels(cx, server_id.into()).await?;
    let roles = list_roles(cx, server_id.into()).await?;
    Ok(json!({
        "server": server.server.as_ref().map(view::server),
        "channels": channels["channels"],
        "roles": roles["roles"],
    }))
}

async fn channel(cx: &Cx, server_id: &str, channel_id: &str) -> Result<Value, Status> {
    may_use(cx, server_id).await?;
    let req = pb::ListMessagesRequest {
        server_id: server_id.into(),
        channel_id: channel_id.into(),
        limit: 50,
        ..Default::default()
    };
    list_messages(cx, req).await
}

struct Prompt {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    arguments: &'static [(&'static str, &'static str)],
    text: &'static str,
}

const PROMPTS: &[Prompt] = &[
    Prompt {
        name: "catch_up",
        title: "Catch up on a channel",
        description: "Read a channel's recent messages and sum up what was said.",
        arguments: &[("server_id", "The server's id."), ("channel_id", "The channel's id.")],
        text: "Read the latest messages in channel {channel_id} of server {server_id} with list_messages (page back \
               with before_id if the conversation started earlier). Then sum up what was discussed, what was \
               decided and any questions still open, naming who said what by display name. Don't post anything.",
    },
    Prompt {
        name: "answer_mentions",
        title: "Answer mentions",
        description: "Find recent messages that mention this agent and answer them.",
        arguments: &[("server_id", "The server's id.")],
        text: "Call get_me to learn your user id, then get_events for server {server_id} (once without \
               after_sequence for the cursor if you don't have one, then with it). For each message_created that \
               mentions you as <@your id> or replies to one of your messages, read the conversation around it \
               with list_messages and answer with send_message, using reply_to_id. Keep answers short and stay \
               on what was asked.",
    },
    Prompt {
        name: "moderate_channel",
        title: "Review a channel",
        description: "Look over recent messages for anything against the server's rules.",
        arguments: &[("server_id", "The server's id."), ("channel_id", "The channel's id.")],
        text: "Read the latest messages in channel {channel_id} of server {server_id}. List any that look like \
               spam, scams, harassment or other things a moderator should see, with their ids and why. Don't \
               delete, time out, kick or ban anyone unless you were asked to: suggest it instead.",
    },
    Prompt {
        name: "welcome_members",
        title: "Welcome new members",
        description: "Greet people who joined since a cursor.",
        arguments: &[("server_id", "The server's id."), ("channel_id", "Where to post the welcome.")],
        text: "Use get_events on server {server_id} to find member_joined events since your last cursor. Post one \
               friendly welcome in channel {channel_id} that mentions each new member as <@their id>, and point \
               them to the server's channels from list_channels if that helps.",
    },
];

pub fn prompts() -> Vec<Value> {
    PROMPTS
        .iter()
        .map(|p| {
            let arguments: Vec<Value> = p
                .arguments
                .iter()
                .map(|(name, description)| json!({ "name": name, "description": description, "required": true }))
                .collect();
            json!({ "name": p.name, "title": p.title, "description": p.description, "arguments": arguments })
        })
        .collect()
}

pub fn get_prompt(_cx: &Cx, params: &Value) -> Result<Value, RpcError> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
    let prompt = PROMPTS
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| RpcError::invalid(format!("there's no prompt {name:?}")))?;
    let mut text = prompt.text.to_string();
    for (argument, _) in prompt.arguments {
        let value = params
            .get("arguments")
            .and_then(|a| a.get(argument))
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty() && v.len() <= 64 && v.chars().all(|c| c.is_ascii_alphanumeric()))
            .ok_or_else(|| RpcError::invalid(format!("{argument} is needed, as an id")))?;
        text = text.replace(&format!("{{{argument}}}"), value);
    }
    Ok(json!({
        "description": prompt.description,
        "messages": [{ "role": "user", "content": { "type": "text", "text": text } }]
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_resource_addresses() {
        assert!(matches!(parse("fuwa://instance"), Some(Uri::Instance)));
        assert!(matches!(parse("fuwa://servers/abc"), Some(Uri::Server("abc"))));
        assert!(matches!(parse("fuwa://servers/abc/channels/def"), Some(Uri::Channel("abc", "def"))));
        assert!(parse("fuwa://servers/../x").is_none());
        assert!(parse("fuwa://servers/a/channels/b/c").is_none());
        assert!(parse("https://example.com").is_none());
    }
}
