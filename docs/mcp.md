# MCP: agents in AI apps

Every fuwa instance is also an [MCP](https://modelcontextprotocol.io) server.
An AI app (Claude Code, Claude Desktop, or anything else that speaks MCP)
connects to it with an [agent](../README.md#agents)'s token and can then read
and write in the servers that agent was added to, with that agent's roles
and nothing more.

It's **on by default**. Instance admins turn it off with `FUWA_MCP=off` or
"Agents through MCP" in Instance settings, Sign-ups. A server's managers
pick which of its agents may use it (below).

## Connecting

The endpoint is `/mcp` on the instance's public address, for example
`https://chat.example.com/mcp`. The app signs in with the agent's token as a
bearer token, the same token programs use with the gRPC API. Make an agent
in a fuwa app under Settings, Agents, copy its token, and add the agent to a
server from that server's settings (Integrations).

### Claude Code

```sh
claude mcp add --transport http fuwa https://chat.example.com/mcp \
  --header "Authorization: Bearer $FUWA_AGENT_TOKEN"
```

Or in a project's `.mcp.json`, which reads the token from the environment
so it never lands in the repository:

```json
{
  "mcpServers": {
    "fuwa": {
      "type": "http",
      "url": "https://chat.example.com/mcp",
      "headers": { "Authorization": "Bearer ${FUWA_AGENT_TOKEN}" }
    }
  }
}
```

### Claude Desktop

Claude Desktop's own connectors sign in with OAuth, which fuwa doesn't offer
yet (see [Later](#later-signing-in-with-oauth)). Until then, connect through
the `mcp-remote` bridge, which runs on your computer and adds the header, in
`claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "fuwa": {
      "command": "npx",
      "args": ["-y", "mcp-remote", "https://chat.example.com/mcp", "--header", "Authorization:${FUWA_AUTH}"],
      "env": { "FUWA_AUTH": "Bearer <the agent's token>" }
    }
  }
}
```

A self-hosted instance on your own computer works the same way with
`http://localhost:8080/mcp`.

### Finding it

- `GET /.well-known/mcp.json` describes the endpoint: its address, transport,
  protocol versions, how to sign in, and the [SDK](#live-events) for
  programs. It needs no token, and answers 404 while MCP is off.
- `NodeService.GetNode` says `mcp: true` while it's on.

## What's there

### Tools

Each tool is one call (or two) of the public gRPC API, made inside the
instance through the same routes apps reach, so it's routed, permission
checked, limited and timed exactly like the call it wraps. What an agent
can't do in the apps' API, it can't do here.

| Tool | Wraps |
| --- | --- |
| `get_me` | `AuthService.GetMe` |
| `list_servers`, `get_server` | `ServerService.ListServers`, `GetServer` |
| `list_channels` | `ChannelService.ListChannels` |
| `list_messages`, `get_message` | `MessageService.ListMessages` (pages by `before_id` / `after_id`), `GetMessage` |
| `send_message`, `update_message`, `delete_message` | `MessageService.SendMessage`, `UpdateMessage`, `DeleteMessage` |
| `list_events` | `EventService.ListEvents` (a cursor, see below) |
| `list_members` | `ServerService.ListMembers`, a page at a time |
| `list_roles`, `list_emojis`, `get_profile` | `RoleService.ListRoles`, `EmojiService.ListEmojis`, `AuthService.GetProfile` |
| `time_out_member`, `kick_member`, `ban_member`, `unban_member` | `ServerService`'s moderation calls |
| `add_member_role`, `remove_member_role` | `RoleService.AddMemberRole`, `RemoveMemberRole` |
| `upload_picture` | `MediaService.CreateUpload` and the upload itself |
| `create_emoji` | `EmojiService.CreateEmoji` |

Tools say whether they only read, and which ones can't be undone (deleting,
kicking, banning), so apps can ask before running them. A call the API
refuses comes back as a tool error with the API's reason ("not allowed:
…"), so the model reads why. Polls and threads get their tools when they
land.

### Resources

- `fuwa://instance`: the instance and the agent's account.
- `fuwa://servers/{server_id}`: a server with its channels and roles.
- `fuwa://servers/{server_id}/channels/{channel_id}`: a channel's latest 50
  messages.

### Prompts

`catch_up` (sum up a channel), `answer_mentions`, `moderate_channel` (look
over a channel and suggest, never act) and `welcome_members`.

## Live events

MCP is request and answer, so it carries no live stream. `list_events` reads a
server's event log from a cursor instead: called without `after_sequence` it
returns the current cursor, then each call returns what happened after the
cursor it's given and the cursor to ask from next. Each call is one read of
the server's log and nothing stays open between calls, which is cheaper for
the instance than a long poll holding a stream per agent, and any gateway
can answer the next call.

Programs that want events as they happen use `EventService.Subscribe` from
the `@waifu-devs/fuwa` SDK (or any gRPC client) with the same token.

## How it works

- **Stateless Streamable HTTP** (MCP 2025-03-26, 2025-06-18 and 2025-11-25).
  Each request is one JSON-RPC message in a POST, answered with plain JSON.
  There's no `Mcp-Session-Id` and no stream (GET and DELETE answer 405), so
  any gateway replica answers any request and a deploy loses nothing.
  `initialize` works but isn't needed first. Batches aren't taken.
- **Agents only.** A person's session token or the operator's admin token is
  refused (403). A missing or unknown token gets 401 with
  `WWW-Authenticate: Bearer`.
- **Limits per agent**, never per address: 120 requests a minute with bursts
  of 60, on each gateway (or the single process), answered 429 with
  `Retry-After`. The calls a tool makes keep their own limits too.
- **Browsers**: a request with an `Origin` must come from the instance's own
  address or one of its allowed origins (`FUWA_ALLOWED_ORIGINS`), as the MCP
  spec asks against DNS rebinding; with any origin allowed (the default), the
  token is still what keeps other sites out.
- **Requests** are at most 12 MB (a picture travels as base64).

### Which agents, per server

`AgentService.GetMcpAccess` and `SetMcpAccess`: every agent in the server
(the default), only chosen ones, or none. Any member reads it; changing it
needs Manage Server and goes in the audit log as a server update of
`mcp_access`. The apps show it in Server settings, Integrations.

This closes the MCP door only: the agent's token still works with the gRPC
API, and what it can do anywhere is still up to its roles. To stop an agent
altogether, take its roles away or remove it.

## Privacy and security

- Nothing about the caller's address is read, kept, logged or passed on: the
  calls tools make carry the agent's token and nothing else.
- The token is never logged, stored or echoed back in an answer.
- Tool descriptions and answers name nothing beyond what the agent can
  already see; picture links are the instance's own `/media/` addresses.
- The endpoint fetches nothing from other sites.
- Failures and slow requests are counted in the anonymous health report
  (`mcp.request` timings, `mcp.<tool>` usage, `mcp_tool` errors, `mcp.limited`),
  with no account, server or content.

## Later: signing in with OAuth

So that apps like Claude Desktop connect from their own connector screen
(no bridge, no pasted token), the instance would add, per the MCP
authorization spec:

- **Protected resource metadata** (RFC 9728) at
  `/.well-known/oauth-protected-resource`, naming the instance as the
  authorization server, and a `resource_metadata` link in the 401's
  `WWW-Authenticate`.
- **An authorization server** on the instance (RFC 8414 metadata at
  `/.well-known/oauth-authorization-server`): the authorization code flow
  with PKCE, and client registration through Client ID Metadata Documents
  (the app's client ID is an https URL describing it) or dynamic client
  registration (RFC 7591), with a cap on registered clients and no
  per-address keys.
- **A consent page** in the web app where the person signed in picks which
  of their agents the app acts as, and sees which servers it's in.
- **Tokens bound to the instance** (RFC 8707 `resource`), short-lived and
  refreshable, stored hashed like sessions, revocable from Settings, Agents,
  and checked to be agent sessions exactly as tokens are now.
