//! The MCP endpoint (docs/mcp.md), as an AI app's MCP client uses it: plain
//! JSON-RPC over HTTP POST, with an agent's token. The last test drives it
//! with the official TypeScript MCP client (`web/scripts/mcp-conformance.mjs`).

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use fuwa_server::app::App;
use fuwa_server::config::Config;
use fuwa_server::pb;
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tonic::Request;
use tonic::transport::Channel;

const ADMIN_TOKEN: &str = "test-admin-token-0123456789abcdef0123456789";

struct Instance {
    app: Arc<App>,
    addr: SocketAddr,
    serving: JoinHandle<()>,
}

async fn start(dir: &Path) -> Instance {
    let dir = dir.to_str().unwrap().to_string();
    let config = Config::from_lookup(|key| match key {
        "FUWA_DATA_PATH" => Some(dir.clone()),
        "FUWA_TELEMETRY" => Some("off".into()),
        "FUWA_ADMIN_TOKEN" => Some(ADMIN_TOKEN.into()),
        _ => None,
    })
    .unwrap();
    let app = App::open(config).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router();
    let shutdown = app.shutdown.clone();
    let serving = tokio::spawn(async move {
        axum::serve(listener, router).with_graceful_shutdown(async move { shutdown.cancelled().await }).await.unwrap();
    });
    Instance { app, addr, serving }
}

impl Instance {
    async fn stop(self) {
        self.app.shutdown.cancel();
        self.serving.await.unwrap();
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
}

fn authed<T>(token: &str, message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

/// A person who owns a server with an agent in it.
struct World {
    channel: Channel,
    owner: String,
    owner_id: String,
    agent: String,
    agent_id: String,
    server_id: String,
    general_id: String,
}

async fn world(instance: &Instance) -> World {
    let channel = Channel::from_shared(instance.url("")).unwrap().connect().await.unwrap();
    let mut auth = pb::auth_service_client::AuthServiceClient::new(channel.clone());
    let signed_up = auth
        .sign_up(pb::SignUpRequest {
            username: "juan".into(),
            password: "correct horse battery".into(),
            display_name: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    let owner = signed_up.token;
    let owner_id = signed_up.user.unwrap().id;
    let mut servers = pb::server_service_client::ServerServiceClient::new(channel.clone());
    let server = servers
        .create_server(authed(&owner, pb::CreateServerRequest { name: "Waifu Devs".into(), ..Default::default() }))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap();
    let mut channels = pb::channel_service_client::ChannelServiceClient::new(channel.clone());
    let general_id = channels
        .list_channels(authed(&owner, pb::ListChannelsRequest { server_id: server.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels
        .into_iter()
        .find(|c| c.name == "general")
        .unwrap()
        .id;
    let mut agents = pb::agent_service_client::AgentServiceClient::new(channel.clone());
    let made = agents
        .create_agent(authed(
            &owner,
            pb::CreateAgentRequest { username: "helper".into(), display_name: "Helper".into() },
        ))
        .await
        .unwrap()
        .into_inner();
    let agent_id = made.agent.unwrap().user.unwrap().id;
    agents
        .add_agent(authed(&owner, pb::AddAgentRequest { server_id: server.id.clone(), username: "helper".into() }))
        .await
        .unwrap();
    World { channel, owner, owner_id, agent: made.token, agent_id, server_id: server.id, general_id }
}

/// One JSON-RPC request: the HTTP response and its JSON, if any.
async fn post(instance: &Instance, token: Option<&str>, body: Value) -> (reqwest::Response, Value) {
    let mut request = reqwest::Client::new()
        .post(instance.url("/mcp"))
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18")
        .json(&body);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.unwrap();
    let headers = response.headers().clone();
    let status = response.status();
    let text = response.text().await.unwrap();
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    // Rebuild a response carrying only the head, so callers can check it.
    let mut rebuilt = http::Response::builder().status(status);
    for (name, value) in &headers {
        rebuilt = rebuilt.header(name, value);
    }
    (reqwest::Response::from(rebuilt.body(text).unwrap()), json)
}

async fn rpc(instance: &Instance, token: &str, method: &str, params: Value) -> Value {
    let (response, json) =
        post(instance, Some(token), json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })).await;
    assert_eq!(response.status(), 200, "{method}: {json}");
    assert_eq!(json["jsonrpc"], "2.0");
    json
}

/// A tool's result: its structured content, or the error text when it failed.
async fn tool(instance: &Instance, token: &str, name: &str, arguments: Value) -> Result<Value, String> {
    let json = rpc(instance, token, "tools/call", json!({ "name": name, "arguments": arguments })).await;
    let result = &json["result"];
    assert!(result.is_object(), "{name}: {json}");
    if result["isError"] == true {
        return Err(result["content"][0]["text"].as_str().unwrap().to_string());
    }
    assert_eq!(
        serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap(),
        result["structuredContent"]
    );
    Ok(result["structuredContent"].clone())
}

#[tokio::test]
async fn agents_sign_in_with_their_token_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let w = world(&instance).await;
    let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });

    let (response, _) = post(&instance, None, ping.clone()).await;
    assert_eq!(response.status(), 401);
    assert_eq!(response.headers()["www-authenticate"], "Bearer realm=\"fuwa\"");
    let (response, _) = post(&instance, Some("not-a-token"), ping.clone()).await;
    assert_eq!(response.status(), 401);
    let (response, _) = post(&instance, Some(&w.owner), ping.clone()).await;
    assert_eq!(response.status(), 403, "people use the apps, not MCP");
    let (response, _) = post(&instance, Some(ADMIN_TOKEN), ping.clone()).await;
    assert_eq!(response.status(), 403);
    let (response, json) = post(&instance, Some(&w.agent), ping).await;
    assert_eq!(response.status(), 200);
    assert_eq!(json, json!({ "jsonrpc": "2.0", "id": 1, "result": {} }));

    // Stateless: no session, no stream.
    let init = rpc(&instance, &w.agent, "initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } })).await;
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "fuwa");
    assert!(init["result"]["instructions"].as_str().unwrap().contains("@helper"));
    let (response, _) = post(
        &instance,
        Some(&w.agent),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": { "protocolVersion": "1999-01-01" } }),
    )
    .await;
    assert!(response.headers().get("mcp-session-id").is_none());
    let unknown = rpc(&instance, &w.agent, "initialize", json!({ "protocolVersion": "1999-01-01" })).await;
    assert_eq!(unknown["result"]["protocolVersion"], fuwa_server::mcp::PROTOCOL_VERSIONS[0]);
    let get = reqwest::Client::new().get(instance.url("/mcp")).bearer_auth(&w.agent).send().await.unwrap();
    assert_eq!(get.status(), 405);

    // Notifications get nothing back; batches and unknown methods are errors.
    let (response, _) =
        post(&instance, Some(&w.agent), json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;
    assert_eq!(response.status(), 202);
    let (_, json) = post(&instance, Some(&w.agent), json!([{ "jsonrpc": "2.0", "id": 1, "method": "ping" }])).await;
    assert_eq!(json["error"]["code"], -32600);
    let json = rpc(&instance, &w.agent, "sampling/createMessage", json!({})).await;
    assert_eq!(json["error"]["code"], -32601);
    let bad_version = reqwest::Client::new()
        .post(instance.url("/mcp"))
        .bearer_auth(&w.agent)
        .header("mcp-protocol-version", "1999-01-01")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(bad_version.status(), 400);

    // The token never comes back in anything the endpoint says.
    let me = tool(&instance, &w.agent, "get_me", json!({})).await.unwrap();
    assert_eq!(me["user"]["username"], "helper");
    assert_eq!(me["user"]["agent"], true);
    assert!(!me.to_string().contains(&w.agent));

    instance.stop().await;
}

#[tokio::test]
async fn tools_do_what_the_agent_could_do_anyway() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let w = world(&instance).await;
    let (sid, gid) = (w.server_id.as_str(), w.general_id.as_str());

    let tools = rpc(&instance, &w.agent, "tools/list", json!({})).await;
    let names: Vec<&str> =
        tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for name in ["list_servers", "list_messages", "send_message", "get_events", "ban_member", "upload_picture"] {
        assert!(names.contains(&name), "{name}");
    }
    for t in tools["result"]["tools"].as_array().unwrap() {
        assert_eq!(t["inputSchema"]["type"], "object");
        assert!(t["annotations"]["readOnlyHint"].is_boolean());
    }

    let servers = tool(&instance, &w.agent, "list_servers", json!({})).await.unwrap();
    assert_eq!(servers["servers"][0]["id"], sid);
    let channels = tool(&instance, &w.agent, "list_channels", json!({ "server_id": sid })).await.unwrap();
    assert!(channels["channels"].as_array().unwrap().iter().any(|c| c["id"] == gid && c["type"] == "text"));

    // Events from a cursor: none yet, then the message the agent sends.
    let start = tool(&instance, &w.agent, "get_events", json!({ "server_id": sid })).await.unwrap();
    let cursor = start["cursor"].as_i64().unwrap();
    assert!(cursor > 0);
    let sent = tool(
        &instance,
        &w.agent,
        "send_message",
        json!({ "server_id": sid, "channel_id": gid, "content": "hi <@".to_string() + &w.owner_id + ">" }),
    )
    .await
    .unwrap();
    let message_id = sent["message"]["id"].as_str().unwrap().to_string();
    assert_eq!(sent["message"]["author"]["username"], "helper");
    let events =
        tool(&instance, &w.agent, "get_events", json!({ "server_id": sid, "after_sequence": cursor })).await.unwrap();
    let created = events["events"].as_array().unwrap().iter().find(|e| e["type"] == "message_created").unwrap();
    assert_eq!(created["message"]["id"], message_id.as_str());
    assert!(events["cursor"].as_i64().unwrap() > cursor);
    let later =
        tool(&instance, &w.agent, "get_events", json!({ "server_id": sid, "after_sequence": events["cursor"] }))
            .await
            .unwrap();
    assert_eq!(later["events"], json!([]));
    assert_eq!(later["cursor"], events["cursor"]);

    let edited = tool(
        &instance,
        &w.agent,
        "edit_message",
        json!({ "server_id": sid, "message_id": message_id, "content": "hello" }),
    )
    .await
    .unwrap();
    assert_eq!(edited["message"]["content"], "hello");
    let page = tool(&instance, &w.agent, "list_messages", json!({ "server_id": sid, "channel_id": gid, "limit": 10 }))
        .await
        .unwrap();
    assert!(
        page["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == message_id.as_str() && m["edited_at"].is_string())
    );
    let members = tool(&instance, &w.agent, "list_members", json!({ "server_id": sid, "limit": 1 })).await.unwrap();
    assert_eq!(members["total"], 2);
    assert_eq!(members["members"].as_array().unwrap().len(), 1);
    assert_eq!(members["has_more"], true);
    let roles = tool(&instance, &w.agent, "list_roles", json!({ "server_id": sid })).await.unwrap();
    assert!(roles["roles"].as_array().unwrap().iter().any(|r| r["id"] == sid));
    let profile = tool(&instance, &w.agent, "get_profile", json!({ "user_id": w.owner_id })).await.unwrap();
    assert_eq!(profile["profile"]["user"]["username"], "juan");

    // The same checks as the gRPC calls: no moderating without the permission.
    let kick =
        tool(&instance, &w.agent, "kick_member", json!({ "server_id": sid, "user_id": w.owner_id })).await.unwrap_err();
    assert!(kick.starts_with("not allowed:"), "{kick}");
    let ban =
        tool(&instance, &w.agent, "ban_member", json!({ "server_id": sid, "user_id": w.owner_id })).await.unwrap_err();
    assert!(ban.starts_with("not allowed:"), "{ban}");
    let elsewhere =
        tool(&instance, &w.agent, "list_channels", json!({ "server_id": "01J000000000000000000000AA" })).await;
    assert!(elsewhere.is_err());
    let emoji = tool(
        &instance,
        &w.agent,
        "upload_picture",
        json!({ "purpose": "emoji", "server_id": sid, "content_type": "image/png", "data_base64": "aGk=" }),
    )
    .await;
    assert!(emoji.is_err(), "two bytes aren't a PNG");

    // Given the permission, the agent uploads a picture and makes an emoji of it.
    let mut roles_client = pb::role_service_client::RoleServiceClient::new(w.channel.clone());
    let role = roles_client
        .create_role(authed(
            &w.owner,
            pb::CreateRoleRequest {
                server_id: sid.into(),
                name: "Emoji".into(),
                permissions: vec![pb::Permission::ManageEmoji as i32],
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .role
        .unwrap();
    roles_client
        .add_member_role(authed(
            &w.owner,
            pb::AddMemberRoleRequest { server_id: sid.into(), user_id: w.agent_id.clone(), role_id: role.id },
        ))
        .await
        .unwrap();
    use base64::Engine;
    let png = base64::engine::general_purpose::STANDARD.encode(png());
    let picture = tool(
        &instance,
        &w.agent,
        "upload_picture",
        json!({ "purpose": "emoji", "server_id": sid, "content_type": "image/png", "data_base64": png }),
    )
    .await
    .unwrap();
    let url = picture["url"].as_str().unwrap();
    let made = tool(&instance, &w.agent, "create_emoji", json!({ "server_id": sid, "name": "fuwa", "url": url }))
        .await
        .unwrap();
    assert!(made["emoji"]["write_as"].as_str().unwrap().starts_with("<:fuwa:"));
    let emojis = tool(&instance, &w.agent, "list_emojis", json!({ "server_id": sid })).await.unwrap();
    assert_eq!(emojis["emojis"].as_array().unwrap().len(), 1);

    let deleted = tool(&instance, &w.agent, "delete_message", json!({ "server_id": sid, "message_id": message_id }))
        .await
        .unwrap();
    assert_eq!(deleted["deleted"], true);

    // Bad arguments are protocol errors, not tool failures.
    let json =
        rpc(&instance, &w.agent, "tools/call", json!({ "name": "send_message", "arguments": { "server_id": sid } }))
            .await;
    assert_eq!(json["error"]["code"], -32602);
    let json = rpc(&instance, &w.agent, "tools/call", json!({ "name": "nope", "arguments": {} })).await;
    assert_eq!(json["error"]["code"], -32602);

    instance.stop().await;
}

#[tokio::test]
async fn resources_and_prompts() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let w = world(&instance).await;
    let listed = rpc(&instance, &w.agent, "resources/list", json!({})).await;
    let uris: Vec<&str> =
        listed["result"]["resources"].as_array().unwrap().iter().map(|r| r["uri"].as_str().unwrap()).collect();
    assert_eq!(uris, vec!["fuwa://instance".to_string(), format!("fuwa://servers/{}", w.server_id)]);
    let templates = rpc(&instance, &w.agent, "resources/templates/list", json!({})).await;
    assert_eq!(templates["result"]["resourceTemplates"].as_array().unwrap().len(), 2);

    let read =
        rpc(&instance, &w.agent, "resources/read", json!({ "uri": format!("fuwa://servers/{}", w.server_id) })).await;
    let text: Value = serde_json::from_str(read["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["server"]["name"], "Waifu Devs");
    assert!(text["channels"].as_array().unwrap().iter().any(|c| c["name"] == "general"));
    let read = rpc(
        &instance,
        &w.agent,
        "resources/read",
        json!({ "uri": format!("fuwa://servers/{}/channels/{}", w.server_id, w.general_id) }),
    )
    .await;
    assert!(read["result"]["contents"][0]["text"].as_str().unwrap().contains("messages"));
    let missing = rpc(&instance, &w.agent, "resources/read", json!({ "uri": "fuwa://servers/../../etc" })).await;
    assert_eq!(missing["error"]["code"], -32002);

    let prompts = rpc(&instance, &w.agent, "prompts/list", json!({})).await;
    assert!(prompts["result"]["prompts"].as_array().unwrap().iter().any(|p| p["name"] == "catch_up"));
    let prompt = rpc(
        &instance,
        &w.agent,
        "prompts/get",
        json!({ "name": "catch_up", "arguments": { "server_id": w.server_id, "channel_id": w.general_id } }),
    )
    .await;
    assert!(prompt["result"]["messages"][0]["content"]["text"].as_str().unwrap().contains(&w.general_id));
    let bad =
        rpc(&instance, &w.agent, "prompts/get", json!({ "name": "catch_up", "arguments": { "server_id": "x y" } }))
            .await;
    assert_eq!(bad["error"]["code"], -32602);

    instance.stop().await;
}

#[tokio::test]
async fn managers_pick_which_agents_and_admins_turn_it_off() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let w = world(&instance).await;
    let sid = w.server_id.as_str();
    let mut agents = pb::agent_service_client::AgentServiceClient::new(w.channel.clone());
    let set = |mode: pb::McpAccessMode, ids: Vec<String>| pb::SetMcpAccessRequest {
        server_id: sid.into(),
        access: Some(pb::McpAccess { mode: mode as i32, agent_ids: ids }),
    };

    let access = agents
        .get_mcp_access(authed(&w.agent, pb::GetMcpAccessRequest { server_id: sid.into() }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(access.access.unwrap().mode, pb::McpAccessMode::All as i32);
    assert_eq!(
        agents.set_mcp_access(authed(&w.agent, set(pb::McpAccessMode::All, vec![]))).await.unwrap_err().code(),
        tonic::Code::PermissionDenied,
        "only managers"
    );

    agents.set_mcp_access(authed(&w.owner, set(pb::McpAccessMode::Off, vec![]))).await.unwrap();
    let off = tool(&instance, &w.agent, "list_channels", json!({ "server_id": sid })).await.unwrap_err();
    assert!(off.contains("haven't let this agent"), "{off}");
    let read = rpc(&instance, &w.agent, "resources/read", json!({ "uri": format!("fuwa://servers/{sid}") })).await;
    assert!(read["error"].is_object());
    // The agent's token still works everywhere else: this closes the MCP door only.
    let mut channels = pb::channel_service_client::ChannelServiceClient::new(w.channel.clone());
    channels.list_channels(authed(&w.agent, pb::ListChannelsRequest { server_id: sid.into() })).await.unwrap();

    agents
        .set_mcp_access(authed(&w.owner, set(pb::McpAccessMode::Chosen, vec!["01J000000000000000000000AA".into()])))
        .await
        .unwrap();
    assert!(tool(&instance, &w.agent, "list_channels", json!({ "server_id": sid })).await.is_err());
    let chosen = agents
        .set_mcp_access(authed(&w.owner, set(pb::McpAccessMode::Chosen, vec![w.agent_id.clone(), w.agent_id.clone()])))
        .await
        .unwrap()
        .into_inner()
        .access
        .unwrap();
    assert_eq!(chosen.agent_ids, vec![w.agent_id.clone()]);
    assert!(tool(&instance, &w.agent, "list_channels", json!({ "server_id": sid })).await.is_ok());
    assert_eq!(
        agents
            .set_mcp_access(authed(&w.owner, set(pb::McpAccessMode::Chosen, vec!["no such id!".into()])))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::InvalidArgument
    );

    // In the audit log, as a server update.
    let mut servers = pb::server_service_client::ServerServiceClient::new(w.channel.clone());
    let log = servers
        .list_audit_log(authed(&w.owner, pb::ListAuditLogRequest { server_id: sid.into(), ..Default::default() }))
        .await
        .unwrap()
        .into_inner();
    assert!(
        log.entries
            .iter()
            .any(|e| e.action == pb::AuditAction::ServerUpdate as i32
                && e.changes.iter().any(|c| c.field == "mcp_access"))
    );

    // Discovery, then the instance's switch.
    let card: Value = reqwest::get(instance.url("/.well-known/mcp.json")).await.unwrap().json().await.unwrap();
    assert!(card["endpoint"].as_str().unwrap().ends_with("/mcp"));
    assert_eq!(card["transport"]["type"], "streamable-http");
    let mut node = pb::node_service_client::NodeServiceClient::new(w.channel.clone());
    assert!(node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap().mcp);
    let mut admin = pb::admin_service_client::AdminServiceClient::new(w.channel.clone());
    admin
        .update_settings(authed(
            ADMIN_TOKEN,
            pb::UpdateSettingsRequest {
                settings: Some(pb::InstanceSettings { mcp: false, ..Default::default() }),
                update_mask: Some(prost_types::FieldMask { paths: vec!["mcp".into()] }),
                reset_mask: None,
            },
        ))
        .await
        .unwrap();
    let (response, _) = post(&instance, Some(&w.agent), json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })).await;
    assert_eq!(response.status(), 404);
    assert_eq!(reqwest::get(instance.url("/.well-known/mcp.json")).await.unwrap().status(), 404);
    assert!(!node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap().mcp);

    instance.stop().await;
}

/// The official MCP client (the TypeScript SDK) connects, lists and calls
/// tools, reads a resource and gets a prompt. CI installs it with the web
/// app's packages; elsewhere, without them, this says so and passes.
#[tokio::test]
async fn the_official_client_works() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/scripts/mcp-conformance.mjs");
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/node_modules/@modelcontextprotocol/sdk");
    if !sdk.exists() {
        assert!(std::env::var_os("CI").is_none(), "CI must run this: install web/'s packages first");
        eprintln!("skipped locally: run `pnpm install` in web/ for the official MCP client");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let w = world(&instance).await;
    let output = tokio::process::Command::new("node")
        .arg(&script)
        .env("FUWA_MCP_URL", instance.url("/mcp"))
        .env("FUWA_MCP_TOKEN", &w.agent)
        .env("FUWA_SERVER_ID", &w.server_id)
        .env("FUWA_CHANNEL_ID", &w.general_id)
        .output()
        .await
        .expect("node runs");
    let (stdout, stderr) = (String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "the MCP client failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("conformance: ok"), "{stdout}");
    instance.stop().await;
}

/// The smallest PNG the instance takes: 1x1, with a padding chunk.
fn png() -> Vec<u8> {
    let chunk =
        |kind: &[u8; 4], data: &[u8]| [&(data.len() as u32).to_be_bytes()[..], kind, data, &[0, 0, 0, 0]].concat();
    [
        &b"\x89PNG\r\n\x1a\n"[..],
        &chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]),
        &chunk(b"fuWa", &[7; 200]),
        &chunk(b"IEND", &[]),
    ]
    .concat()
}
