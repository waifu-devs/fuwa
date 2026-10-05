//! The tools: each is the gRPC call (or two) an agent could make itself, so
//! it can do through MCP exactly what its roles let it do anywhere else.

use base64::Engine;
use serde_json::{Map, Value, json};
use tonic::Status;

use super::{Cx, RpcError, call, view};
use crate::pb;

/// One tool as `tools/list` shows it.
struct Tool {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    /// Its input's properties, as JSON Schema, and which are required.
    properties: fn() -> Value,
    required: &'static [&'static str],
    read_only: bool,
    destructive: bool,
}

fn server_id() -> Value {
    json!({ "type": "string", "description": "The server's id (list_servers)." })
}
fn channel_id() -> Value {
    json!({ "type": "string", "description": "The channel's id (list_channels)." })
}
fn user_id() -> Value {
    json!({ "type": "string", "description": "The person's or agent's user id." })
}
fn message_id() -> Value {
    json!({ "type": "string", "description": "The message's id." })
}
fn reason() -> Value {
    json!({ "type": "string", "description": "Why, kept in the server's audit log." })
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "get_me",
        title: "Who am I",
        description: "This agent's own account, and the instance's name and version.",
        properties: || json!({}),
        required: &[],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "list_servers",
        title: "List servers",
        description: "The servers (communities) this agent was added to.",
        properties: || json!({}),
        required: &[],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "get_server",
        title: "Get a server",
        description: "One server: its name, description, owner and member count.",
        properties: || json!({ "server_id": server_id() }),
        required: &["server_id"],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "list_channels",
        title: "List channels",
        description: "The channels in a server this agent can see, in display order. Messages are in text and \
                      announcement channels; categories group the others. Secure channels are end-to-end \
                      encrypted and can't be read here.",
        properties: || json!({ "server_id": server_id() }),
        required: &["server_id"],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "list_messages",
        title: "Read messages",
        description: "A page of a channel's messages, oldest first, with their authors. Without a cursor, the \
                      latest page. has_more says whether there are more in the direction paged.",
        properties: || {
            json!({
                "server_id": server_id(),
                "channel_id": channel_id(),
                "limit": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Defaults to 50." },
                "before_id": { "type": "string", "description": "The page right before this message." },
                "after_id": { "type": "string", "description": "The page right after this message." }
            })
        },
        required: &["server_id", "channel_id"],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "get_message",
        title: "Read a message",
        description: "One message and its author.",
        properties: || {
            json!({
                "server_id": server_id(),
                "message_id": message_id(),
                "channel_id": { "type": "string", "description": "Its channel; needed in a channel shared from another server." }
            })
        },
        required: &["server_id", "message_id"],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "send_message",
        title: "Send a message",
        description: "Posts a message in a channel as this agent. Markdown works; mention someone as <@user id>, \
                      a role as <@&role id>, a custom emoji as written by list_emojis.",
        properties: || {
            json!({
                "server_id": server_id(),
                "channel_id": channel_id(),
                "content": { "type": "string", "description": "The text, up to 4000 characters." },
                "reply_to_id": { "type": "string", "description": "A message in the same channel this one answers." },
                "interaction_id": {
                    "type": "string",
                    "description": "Answers an interaction_created event (someone ran this agent's command or \
                                    pressed its button): send in that interaction's channel, within 15 minutes, \
                                    at most 5 times. The message shows who used what."
                },
                "buttons": {
                    "type": "array",
                    "maxItems": 5,
                    "description": "Rows of buttons under the message (up to 5 rows of 5). Pressing one sends this \
                                    agent an interaction_created event with its custom_id.",
                    "items": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 5,
                        "items": {
                            "type": "object",
                            "properties": {
                                "label": { "type": "string", "description": "1 to 80 characters." },
                                "custom_id": { "type": "string", "description": "What comes back when pressed; unique on the message." },
                                "style": { "type": "string", "enum": ["primary", "secondary", "success", "danger", "link"] },
                                "url": { "type": "string", "description": "link buttons only: an https link they open." },
                                "disabled": { "type": "boolean" }
                            },
                            "required": ["label"]
                        }
                    }
                }
            })
        },
        required: &["server_id", "channel_id", "content"],
        read_only: false,
        destructive: false,
    },
    Tool {
        name: "set_commands",
        title: "Set slash commands",
        description: "Replaces this agent's slash commands in a server. People see them when they type \"/\" and \
                      run them; each run arrives as an interaction_created event in list_events, and this agent \
                      answers with send_message and its interaction_id. An empty list removes them all.",
        properties: || {
            json!({
                "server_id": server_id(),
                "commands": {
                    "type": "array",
                    "maxItems": 50,
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "1 to 32 of a-z, 0-9, _ and -." },
                            "description": { "type": "string", "description": "1 to 100 characters." },
                            "options": {
                                "type": "array",
                                "maxItems": 10,
                                "description": "What people fill in; required ones first.",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "name": { "type": "string" },
                                        "description": { "type": "string" },
                                        "type": { "type": "string", "enum": ["string", "integer", "boolean", "user", "channel", "role"] },
                                        "required": { "type": "boolean" },
                                        "choices": { "type": "array", "items": { "type": "string" }, "maxItems": 25 }
                                    },
                                    "required": ["name", "description"]
                                }
                            }
                        },
                        "required": ["name", "description"]
                    }
                }
            })
        },
        required: &["server_id", "commands"],
        read_only: false,
        destructive: false,
    },
    Tool {
        name: "list_commands",
        title: "List slash commands",
        description: "The slash commands of every agent in a server, this one's included.",
        properties: || json!({ "server_id": server_id() }),
        required: &["server_id"],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "update_message",
        title: "Edit a message",
        description: "Changes the text of a message this agent sent.",
        properties: || {
            json!({
                "server_id": server_id(),
                "message_id": message_id(),
                "content": { "type": "string" },
                "channel_id": { "type": "string", "description": "Its channel; needed in a channel shared from another server." }
            })
        },
        required: &["server_id", "message_id", "content"],
        read_only: false,
        destructive: false,
    },
    Tool {
        name: "delete_message",
        title: "Delete a message",
        description: "Deletes a message this agent sent, or someone else's where its roles can manage messages.",
        properties: || {
            json!({
                "server_id": server_id(),
                "message_id": message_id(),
                "channel_id": { "type": "string", "description": "Its channel; needed in a channel shared from another server." }
            })
        },
        required: &["server_id", "message_id"],
        read_only: false,
        destructive: true,
    },
    Tool {
        name: "list_events",
        title: "Follow what happens",
        description: "What happened in a server since a cursor: messages sent, edited and deleted, members \
                      joining and leaving, channels and roles changing, and interaction_created when someone \
                      runs this agent's slash command or presses its button. Call it once without \
                      after_sequence to get the current cursor, then again with the cursor each answer \
                      returns. Only events about channels this agent can see.",
        properties: || {
            json!({
                "server_id": server_id(),
                "after_sequence": { "type": "integer", "minimum": 0, "description": "The cursor from the last answer." },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "Defaults to 50." }
            })
        },
        required: &["server_id"],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "list_members",
        title: "List members",
        description: "A server's members, highest ranked first, a page at a time.",
        properties: || {
            json!({
                "server_id": server_id(),
                "offset": { "type": "integer", "minimum": 0, "description": "Members to skip. Defaults to 0." },
                "limit": { "type": "integer", "minimum": 1, "maximum": 500, "description": "Defaults to 100." }
            })
        },
        required: &["server_id"],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "list_roles",
        title: "List roles",
        description: "A server's roles, with what each lets its members do. The server's id is @everyone's.",
        properties: || json!({ "server_id": server_id() }),
        required: &["server_id"],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "list_emojis",
        title: "List emoji",
        description: "A server's custom emoji, with how to write each in a message.",
        properties: || json!({ "server_id": server_id() }),
        required: &["server_id"],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "get_profile",
        title: "Get a profile",
        description: "Someone's profile: name, pronouns and bio. Anyone who shares a server with this agent.",
        properties: || json!({ "user_id": user_id() }),
        required: &["user_id"],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "time_out_member",
        title: "Time someone out",
        description: "Stops a member from talking for a while. Needs Time Out Members, and to rank above them.",
        properties: || {
            json!({
                "server_id": server_id(),
                "user_id": user_id(),
                "seconds": { "type": "integer", "minimum": 0, "maximum": 2419200, "description": "Up to 28 days; 0 ends a time-out." },
                "reason": reason()
            })
        },
        required: &["server_id", "user_id", "seconds"],
        read_only: false,
        destructive: false,
    },
    Tool {
        name: "kick_member",
        title: "Kick someone",
        description: "Removes a member; they can join again. Needs Kick Members, and to rank above them.",
        properties: || json!({ "server_id": server_id(), "user_id": user_id(), "reason": reason() }),
        required: &["server_id", "user_id"],
        read_only: false,
        destructive: true,
    },
    Tool {
        name: "ban_member",
        title: "Ban someone",
        description: "Removes someone and keeps them out. Needs Ban Members, and to rank above them.",
        properties: || {
            json!({
                "server_id": server_id(),
                "user_id": user_id(),
                "reason": reason(),
                "delete_message_seconds": { "type": "integer", "minimum": 0, "maximum": 604800, "description": "Also deletes what they sent in this many seconds before the ban, up to 7 days." }
            })
        },
        required: &["server_id", "user_id"],
        read_only: false,
        destructive: true,
    },
    Tool {
        name: "unban_member",
        title: "Lift a ban",
        description: "Lets a banned person join again. Needs Ban Members.",
        properties: || json!({ "server_id": server_id(), "user_id": user_id() }),
        required: &["server_id", "user_id"],
        read_only: false,
        destructive: false,
    },
    Tool {
        name: "add_member_role",
        title: "Give a role",
        description: "Gives a member a role ranked below this agent's own. Needs Manage Roles.",
        properties: || json!({ "server_id": server_id(), "user_id": user_id(), "role_id": { "type": "string" } }),
        required: &["server_id", "user_id", "role_id"],
        read_only: false,
        destructive: false,
    },
    Tool {
        name: "remove_member_role",
        title: "Take a role",
        description: "Takes a role ranked below this agent's own from a member. Needs Manage Roles.",
        properties: || json!({ "server_id": server_id(), "user_id": user_id(), "role_id": { "type": "string" } }),
        required: &["server_id", "user_id", "role_id"],
        read_only: false,
        destructive: true,
    },
    Tool {
        name: "upload_picture",
        title: "Upload a picture",
        description: "Stores a picture on the instance and returns its URL, for create_emoji (purpose emoji) or a \
                      server icon (purpose server_icon). PNG, JPEG, GIF, WebP or AVIF, at most the instance's \
                      picture size limit.",
        properties: || {
            json!({
                "purpose": { "type": "string", "enum": ["emoji", "server_icon"] },
                "server_id": server_id(),
                "content_type": { "type": "string", "enum": ["image/png", "image/jpeg", "image/gif", "image/webp", "image/avif"] },
                "data_base64": { "type": "string", "description": "The file's bytes, base64." }
            })
        },
        required: &["purpose", "server_id", "content_type", "data_base64"],
        read_only: false,
        destructive: false,
    },
    Tool {
        name: "create_emoji",
        title: "Add an emoji",
        description: "Adds a custom emoji to a server from a picture upload_picture stored. Needs Manage Emoji.",
        properties: || {
            json!({
                "server_id": server_id(),
                "name": { "type": "string", "description": "2 to 32 letters, digits and underscores." },
                "url": { "type": "string", "description": "The url upload_picture returned." }
            })
        },
        required: &["server_id", "name", "url"],
        read_only: false,
        destructive: false,
    },
];

/// Every tool, as `tools/list` answers.
pub fn list() -> Vec<Value> {
    TOOLS
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "title": tool.title,
                "description": tool.description,
                "inputSchema": {
                    "type": "object",
                    "properties": (tool.properties)(),
                    "required": tool.required,
                    "additionalProperties": false
                },
                "annotations": {
                    "title": tool.title,
                    "readOnlyHint": tool.read_only,
                    "destructiveHint": tool.destructive,
                    "idempotentHint": tool.read_only,
                    "openWorldHint": false
                }
            })
        })
        .collect()
}

/// A tool's arguments.
pub(crate) struct Args<'a>(&'a Map<String, Value>);

impl Args<'_> {
    pub(crate) fn text(&self, key: &str) -> Result<String, RpcError> {
        match self.0.get(key) {
            Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
            _ => Err(RpcError::invalid(format!("{key} is needed, as a string"))),
        }
    }

    fn optional(&self, key: &str) -> Result<String, RpcError> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(String::new()),
            Some(Value::String(s)) => Ok(s.clone()),
            _ => Err(RpcError::invalid(format!("{key} must be a string"))),
        }
    }

    fn number(&self, key: &str) -> Result<Option<i64>, RpcError> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => {
                value.as_i64().map(Some).ok_or_else(|| RpcError::invalid(format!("{key} must be a whole number")))
            }
        }
    }
}

/// What set_commands was given, as commands; the API checks the rest.
fn commands(value: Option<&Value>) -> Result<Vec<pb::Command>, RpcError> {
    let Some(Value::Array(list)) = value else { return Err(RpcError::invalid("commands is needed, as a list")) };
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    list.iter()
        .map(|c| {
            let options = match c.get("options") {
                None | Some(Value::Null) => vec![],
                Some(Value::Array(options)) => options
                    .iter()
                    .map(|o| {
                        let kind = match o.get("type").and_then(Value::as_str).unwrap_or("string") {
                            "string" => pb::CommandOptionType::String,
                            "integer" => pb::CommandOptionType::Integer,
                            "boolean" => pb::CommandOptionType::Boolean,
                            "user" => pb::CommandOptionType::User,
                            "channel" => pb::CommandOptionType::Channel,
                            "role" => pb::CommandOptionType::Role,
                            other => return Err(RpcError::invalid(format!("an option's type can't be {other:?}"))),
                        };
                        let choices = match o.get("choices") {
                            None | Some(Value::Null) => vec![],
                            Some(Value::Array(c)) => c.iter().filter_map(Value::as_str).map(str::to_string).collect(),
                            Some(_) => return Err(RpcError::invalid("an option's choices are a list of strings")),
                        };
                        Ok(pb::CommandOption {
                            name: text(o, "name"),
                            description: text(o, "description"),
                            r#type: kind as i32,
                            required: o.get("required").and_then(Value::as_bool).unwrap_or(false),
                            choices,
                        })
                    })
                    .collect::<Result<_, _>>()?,
                Some(_) => return Err(RpcError::invalid("a command's options are a list")),
            };
            Ok(pb::Command { name: text(c, "name"), description: text(c, "description"), options })
        })
        .collect()
}

/// What send_message's buttons were, as rows; the API checks the rest.
fn buttons(value: Option<&Value>) -> Result<Vec<pb::ComponentRow>, RpcError> {
    let rows = match value {
        None | Some(Value::Null) => return Ok(vec![]),
        Some(Value::Array(rows)) => rows,
        Some(_) => return Err(RpcError::invalid("buttons are a list of rows")),
    };
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    rows.iter()
        .map(|row| {
            let Value::Array(list) = row else { return Err(RpcError::invalid("each row of buttons is a list")) };
            let buttons = list
                .iter()
                .map(|b| {
                    let style = match b.get("style").and_then(Value::as_str).unwrap_or("secondary") {
                        "primary" => pb::ButtonStyle::Primary,
                        "secondary" => pb::ButtonStyle::Secondary,
                        "success" => pb::ButtonStyle::Success,
                        "danger" => pb::ButtonStyle::Danger,
                        "link" => pb::ButtonStyle::Link,
                        other => return Err(RpcError::invalid(format!("a button's style can't be {other:?}"))),
                    };
                    Ok(pb::Button {
                        custom_id: text(b, "custom_id"),
                        label: text(b, "label"),
                        style: style as i32,
                        url: text(b, "url"),
                        disabled: b.get("disabled").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect::<Result<_, _>>()?;
            Ok(pb::ComponentRow { buttons })
        })
        .collect()
}

/// `tools/call`: runs a tool. A call the API refused is a tool result with
/// `isError`, so the model reads why; bad arguments are a protocol error.
pub async fn call(cx: &Cx, params: &Value) -> Result<Value, RpcError> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
    let Some(tool) = TOOLS.iter().find(|tool| tool.name == name) else {
        return Err(RpcError::invalid(format!("there's no tool {name:?}")));
    };
    let empty = Map::new();
    let args = match params.get("arguments") {
        None | Some(Value::Null) => Args(&empty),
        Some(Value::Object(map)) => Args(map),
        Some(_) => return Err(RpcError::invalid("arguments must be an object")),
    };
    crate::reports::server_used(&format!("mcp.{}", tool.name), 1);
    if tool.required.contains(&"server_id") {
        let server_id = args.text("server_id")?;
        if let Err(status) = may_use(cx, &server_id).await {
            return Ok(failed(&status));
        }
    }
    Ok(match run(cx, tool.name, &args).await? {
        Ok(value) => done(value),
        Err(status) => failed(&status),
    })
}

/// Whether the server's managers let this agent use it through MCP.
pub(crate) async fn may_use(cx: &Cx, server_id: &str) -> Result<(), Status> {
    let access = call!(
        cx,
        agent_service_client::AgentServiceClient
            .get_mcp_access(pb::GetMcpAccessRequest { server_id: server_id.into() })
    )?
    .access
    .unwrap_or_default();
    let allowed = match pb::McpAccessMode::try_from(access.mode).unwrap_or_default() {
        pb::McpAccessMode::Unspecified | pb::McpAccessMode::All => true,
        pb::McpAccessMode::Chosen => access.agent_ids.contains(&cx.me.id),
        pb::McpAccessMode::Off => false,
    };
    if !allowed {
        return Err(Status::permission_denied("this server's managers haven't let this agent use it through MCP"));
    }
    Ok(())
}

fn done(value: Value) -> Value {
    json!({ "content": [{ "type": "text", "text": value.to_string() }], "structuredContent": value })
}

fn failed(status: &Status) -> Value {
    let why = match status.code() {
        tonic::Code::PermissionDenied => "not allowed",
        tonic::Code::NotFound => "not found",
        tonic::Code::InvalidArgument | tonic::Code::OutOfRange => "invalid",
        tonic::Code::FailedPrecondition | tonic::Code::AlreadyExists => "can't do that now",
        tonic::Code::ResourceExhausted => "limited",
        tonic::Code::Unauthenticated => "signed out",
        _ => "failed",
    };
    if matches!(status.code(), tonic::Code::Internal | tonic::Code::Unknown | tonic::Code::Unavailable) {
        crate::reports::server_error("mcp_tool", Some("mcp.tools/call"));
    }
    // Slow mode and rate limits say how long to wait, for the agent to wait it.
    let retry_after_ms =
        status.metadata().get(crate::error::RETRY_AFTER_MS).and_then(|value| value.to_str().ok()?.parse::<i64>().ok());
    match retry_after_ms {
        Some(ms) => json!({
            "content": [{ "type": "text", "text": format!("{why}: {} (retry after {ms} ms)", status.message()) }],
            "structuredContent": { "error": why, "retry_after_ms": ms },
            "isError": true,
        }),
        None => {
            json!({ "content": [{ "type": "text", "text": format!("{why}: {}", status.message()) }], "isError": true })
        }
    }
}

/// Runs one tool. The outer error is bad arguments; the inner one is the API's answer.
async fn run(cx: &Cx, name: &str, args: &Args<'_>) -> Result<Result<Value, Status>, RpcError> {
    let sid = || args.text("server_id");
    Ok(match name {
        "get_me" => call!(cx, auth_service_client::AuthServiceClient.get_me(pb::GetMeRequest {})).map(|me| {
            json!({ "user": me.user.as_ref().map(view::user), "instance": cx.instance, "version": crate::VERSION })
        }),
        "list_servers" => usable_servers(cx).await.map(|servers| json!({ "servers": servers.iter().map(view::server).collect::<Vec<_>>() })),
        "get_server" => {
            let req = pb::GetServerRequest { server_id: sid()? };
            call!(cx, server_service_client::ServerServiceClient.get_server(req))
                .map(|r| json!({ "server": r.server.as_ref().map(view::server) }))
        }
        "list_channels" => list_channels(cx, sid()?).await,
        "list_messages" => {
            let limit = args.number("limit")?.unwrap_or(50).clamp(1, 100) as i32;
            let req = pb::ListMessagesRequest {
                server_id: sid()?,
                channel_id: args.text("channel_id")?,
                limit,
                before_id: args.optional("before_id")?,
                after_id: args.optional("after_id")?,
                thread_id: String::new(),
            };
            list_messages(cx, req).await
        }
        "get_message" => {
            let req = pb::GetMessageRequest {
                server_id: sid()?,
                message_id: args.text("message_id")?,
                channel_id: args.optional("channel_id")?,
            };
            call!(cx, message_service_client::MessageServiceClient.get_message(req)).map(|r| {
                let authors: Vec<pb::User> = r.author.into_iter().collect();
                json!({ "message": view::messages(r.message.as_slice(), &authors).pop() })
            })
        }
        "send_message" => {
            let req = pb::SendMessageRequest {
                server_id: sid()?,
                channel_id: args.text("channel_id")?,
                content: args.text("content")?,
                reply_to_id: args.optional("reply_to_id")?,
                interaction_id: args.optional("interaction_id")?,
                components: buttons(args.0.get("buttons"))?,
                ..Default::default()
            };
            call!(cx, message_service_client::MessageServiceClient.send_message(req))
                .map(|r| json!({ "message": view::messages(r.message.as_slice(), std::slice::from_ref(&cx.me)).pop() }))
        }
        "set_commands" => {
            let req = pb::SetCommandsRequest { server_id: sid()?, commands: commands(args.0.get("commands"))? };
            call!(cx, command_service_client::CommandServiceClient.set_commands(req))
                .map(|r| json!({ "commands": r.commands.iter().map(view::command).collect::<Vec<_>>() }))
        }
        "list_commands" => {
            let req = pb::ListCommandsRequest { server_id: sid()? };
            call!(cx, command_service_client::CommandServiceClient.list_commands(req)).map(|r| {
                let commands: Vec<Value> = r
                    .commands
                    .iter()
                    .map(|c| {
                        let mut shown = c.command.as_ref().map(view::command).unwrap_or_default();
                        shown["agent_id"] = Value::from(c.agent_id.clone());
                        shown
                    })
                    .collect();
                json!({ "commands": commands, "agents": r.agents.iter().map(view::user).collect::<Vec<_>>() })
            })
        }
        "update_message" => {
            let req = pb::UpdateMessageRequest {
                server_id: sid()?,
                message_id: args.text("message_id")?,
                content: args.text("content")?,
                channel_id: args.optional("channel_id")?,
                ..Default::default()
            };
            call!(cx, message_service_client::MessageServiceClient.update_message(req))
                .map(|r| json!({ "message": view::messages(r.message.as_slice(), std::slice::from_ref(&cx.me)).pop() }))
        }
        "delete_message" => {
            let req = pb::DeleteMessageRequest {
                server_id: sid()?,
                message_id: args.text("message_id")?,
                channel_id: args.optional("channel_id")?,
            };
            call!(cx, message_service_client::MessageServiceClient.delete_message(req)).map(|_| json!({ "deleted": true }))
        }
        "list_events" => {
            let limit = args.number("limit")?.unwrap_or(50).clamp(1, 200) as i32;
            match args.number("after_sequence")? {
                Some(after) if after < 0 => return Err(RpcError::invalid("after_sequence can't be negative")),
                Some(after) => events_after(cx, sid()?, after, limit).await,
                None => head(cx, sid()?).await.map(|cursor| json!({ "cursor": cursor, "events": [], "has_more": false })),
            }
        }
        "list_members" => {
            let offset = args.number("offset")?.unwrap_or(0).max(0) as usize;
            let limit = args.number("limit")?.unwrap_or(100).clamp(1, 500) as usize;
            call!(cx, server_service_client::ServerServiceClient.list_members(pb::ListMembersRequest { server_id: sid()? }))
                .map(|r| {
                    let page: Vec<Value> = r.members.iter().skip(offset).take(limit).map(view::member).collect();
                    json!({ "total": r.members.len(), "members": page, "has_more": offset + limit < r.members.len() })
                })
        }
        "list_roles" => list_roles(cx, sid()?).await,
        "list_emojis" => call!(cx, emoji_service_client::EmojiServiceClient.list_emojis(pb::ListEmojisRequest { server_id: sid()?, ..Default::default() }))
            .map(|r| json!({ "emojis": r.emojis.iter().map(view::emoji).collect::<Vec<_>>() })),
        "get_profile" => {
            let req = pb::GetProfileRequest { user_id: args.text("user_id")? };
            call!(cx, auth_service_client::AuthServiceClient.get_profile(req))
                .map(|r| json!({ "profile": r.profile.as_ref().map(view::profile) }))
        }
        "time_out_member" => {
            let req = pb::TimeOutMemberRequest {
                server_id: sid()?,
                user_id: args.text("user_id")?,
                seconds: args.number("seconds")?.ok_or_else(|| RpcError::invalid("seconds is needed"))?,
                reason: args.optional("reason")?,
            };
            call!(cx, server_service_client::ServerServiceClient.time_out_member(req))
                .map(|r| json!({ "member": r.member.as_ref().map(view::member) }))
        }
        "kick_member" => {
            let req = pb::KickMemberRequest { server_id: sid()?, user_id: args.text("user_id")?, reason: args.optional("reason")? };
            call!(cx, server_service_client::ServerServiceClient.kick_member(req)).map(|_| json!({ "kicked": true }))
        }
        "ban_member" => {
            let req = pb::BanMemberRequest {
                server_id: sid()?,
                user_id: args.text("user_id")?,
                reason: args.optional("reason")?,
                delete_message_seconds: args.number("delete_message_seconds")?.unwrap_or(0),
            };
            call!(cx, server_service_client::ServerServiceClient.ban_member(req))
                .map(|r| json!({ "banned": true, "deleted_messages": r.deleted_messages }))
        }
        "unban_member" => {
            let req = pb::UnbanMemberRequest { server_id: sid()?, user_id: args.text("user_id")? };
            call!(cx, server_service_client::ServerServiceClient.unban_member(req)).map(|_| json!({ "unbanned": true }))
        }
        "add_member_role" => {
            let req =
                pb::AddMemberRoleRequest { server_id: sid()?, user_id: args.text("user_id")?, role_id: args.text("role_id")? };
            call!(cx, role_service_client::RoleServiceClient.add_member_role(req))
                .map(|r| json!({ "member": r.member.as_ref().map(view::member) }))
        }
        "remove_member_role" => {
            let req =
                pb::RemoveMemberRoleRequest { server_id: sid()?, user_id: args.text("user_id")?, role_id: args.text("role_id")? };
            call!(cx, role_service_client::RoleServiceClient.remove_member_role(req))
                .map(|r| json!({ "member": r.member.as_ref().map(view::member) }))
        }
        "upload_picture" => upload_picture(cx, args).await?,
        "create_emoji" => {
            let req = pb::CreateEmojiRequest { server_id: sid()?, name: args.text("name")?, url: args.text("url")? };
            call!(cx, emoji_service_client::EmojiServiceClient.create_emoji(req))
                .map(|r| json!({ "emoji": r.emoji.as_ref().map(view::emoji) }))
        }
        _ => return Err(RpcError::invalid(format!("there's no tool {name:?}"))),
    })
}

/// The agent's servers whose managers let it use them through MCP.
pub(crate) async fn usable_servers(cx: &Cx) -> Result<Vec<pb::Server>, Status> {
    let servers =
        call!(cx, server_service_client::ServerServiceClient.list_servers(pb::ListServersRequest {}))?.servers;
    let allowed = futures::future::join_all(servers.iter().map(|s| may_use(cx, &s.id))).await;
    Ok(servers.into_iter().zip(allowed).filter(|(_, allowed)| allowed.is_ok()).map(|(s, _)| s).collect())
}

pub(crate) async fn list_channels(cx: &Cx, server_id: String) -> Result<Value, Status> {
    call!(cx, channel_service_client::ChannelServiceClient.list_channels(pb::ListChannelsRequest { server_id }))
        .map(|r| json!({ "channels": r.channels.iter().map(view::channel).collect::<Vec<_>>() }))
}

pub(crate) async fn list_roles(cx: &Cx, server_id: String) -> Result<Value, Status> {
    call!(cx, role_service_client::RoleServiceClient.list_roles(pb::ListRolesRequest { server_id }))
        .map(|r| json!({ "roles": r.roles.iter().map(view::role).collect::<Vec<_>>() }))
}

pub(crate) async fn list_messages(cx: &Cx, req: pb::ListMessagesRequest) -> Result<Value, Status> {
    call!(cx, message_service_client::MessageServiceClient.list_messages(req))
        .map(|r| json!({ "messages": view::messages(&r.messages, &r.authors), "has_more": r.has_more }))
}

/// The server's events after `after`, and the cursor to ask from next.
async fn events_after(cx: &Cx, server_id: String, after: i64, limit: i32) -> Result<Value, Status> {
    let req = pb::ListEventsRequest { server_id, after_sequence: after, limit };
    let r = call!(cx, event_service_client::EventServiceClient.list_events(req))?;
    let cursor = r.events.iter().map(|e| e.sequence).max().unwrap_or(after).max(after);
    Ok(
        json!({ "cursor": cursor, "events": r.events.iter().map(view::event).collect::<Vec<_>>(), "has_more": r.has_more }),
    )
}

/// The server's latest sequence: a live stream's `ready`, taken and let go.
async fn head(cx: &Cx, server_id: String) -> Result<i64, Status> {
    let req = pb::SubscribeRequest {
        servers: vec![pb::ServerCursor { server_id: server_id.clone(), after_sequence: None }],
        ..Default::default()
    };
    let mut stream = call!(cx, event_service_client::EventServiceClient.subscribe(req))?;
    let waiting = async {
        while let Some(response) = stream.message().await? {
            if let Some(ready) = response.ready {
                return Ok(ready.servers.iter().find(|s| s.server_id == server_id).map_or(0, |s| s.sequence));
            }
            if let Some(event) = response.event
                && event.sequence == 0
                && matches!(
                    event.payload,
                    Some(pb::event::Payload::ServerDeleted(_)) | Some(pb::event::Payload::MemberLeft(_))
                )
            {
                return Err(Status::not_found("that server isn't one of this agent's"));
            }
        }
        Err(Status::unavailable("the server's events ended early; try again"))
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), waiting)
        .await
        .unwrap_or_else(|_| Err(Status::unavailable("the server's events took too long; try again")))
}

/// CreateUpload, then the PUT of its bytes, both through the instance's router.
async fn upload_picture(cx: &Cx, args: &Args<'_>) -> Result<Result<Value, Status>, RpcError> {
    let purpose = match args.text("purpose")?.as_str() {
        "emoji" => pb::MediaPurpose::Emoji,
        "server_icon" => pb::MediaPurpose::ServerIcon,
        _ => return Err(RpcError::invalid("purpose must be emoji or server_icon")),
    };
    let encoded = match args.0.get("data_base64") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.trim(),
        _ => return Err(RpcError::invalid("data_base64 is needed, as a string")),
    };
    if encoded.len() > super::MAX_BODY {
        return Err(RpcError::invalid("that picture is too big"));
    }
    let data = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| RpcError::invalid("data_base64 isn't base64"))?;
    let req = pb::CreateUploadRequest {
        purpose: purpose as i32,
        content_type: args.text("content_type")?,
        size: data.len() as i64,
        server_id: args.text("server_id")?,
    };
    let upload = match call!(cx, media_service_client::MediaServiceClient.create_upload(req)) {
        Ok(upload) => upload,
        Err(status) => return Ok(Err(status)),
    };
    // Only the path: the bytes never leave this process for the public address.
    let Some(path) = url::Url::parse(&upload.upload_url).ok().map(|u| u.path().to_string()) else {
        return Ok(Err(Status::internal("the upload address didn't read")));
    };
    let put =
        http::Request::put(path).body(axum::body::Body::from(data)).map_err(|_| RpcError::invalid("bad upload"))?;
    // A router is always ready, so it's called straight away.
    let mut router = cx.inner.clone();
    let stored = tonic::codegen::Service::call(&mut router, put).await.is_ok_and(|r| r.status().is_success());
    if !stored {
        return Ok(Err(Status::invalid_argument("the instance didn't take those bytes as that kind of picture")));
    }
    let media = upload.media.unwrap_or_default();
    Ok(Ok(json!({ "url": media.url, "content_type": media.content_type, "size": media.size })))
}
