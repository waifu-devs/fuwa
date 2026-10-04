// The instance's MCP endpoint (docs/mcp.md), driven by the official MCP
// client: server/tests/mcp.rs starts an instance with an agent and runs this
// with FUWA_MCP_URL, FUWA_MCP_TOKEN, FUWA_SERVER_ID and FUWA_CHANNEL_ID set.
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";

const { FUWA_MCP_URL: url, FUWA_MCP_TOKEN: token, FUWA_SERVER_ID: serverId, FUWA_CHANNEL_ID: channelId } = process.env;
if (!url || !token || !serverId || !channelId) {
  console.error("FUWA_MCP_URL, FUWA_MCP_TOKEN, FUWA_SERVER_ID and FUWA_CHANNEL_ID are needed");
  process.exit(2);
}

function check(condition, what) {
  if (!condition) throw new Error(`conformance: ${what}`);
}

const transport = new StreamableHTTPClientTransport(new URL(url), {
  requestInit: { headers: { Authorization: `Bearer ${token}` } },
});
const client = new Client({ name: "fuwa-conformance", version: "1.0.0" });
await client.connect(transport);

check(client.getServerVersion()?.name === "fuwa", "serverInfo names fuwa");
check(client.getServerCapabilities()?.tools, "it offers tools");
check(transport.sessionId === undefined, "it keeps no session");
await client.ping();

const { tools } = await client.listTools();
for (const name of ["list_servers", "list_channels", "list_messages", "send_message", "get_events"]) {
  check(tools.some((t) => t.name === name), `tools/list has ${name}`);
}

const servers = await client.callTool({ name: "list_servers", arguments: {} });
check(!servers.isError && servers.structuredContent.servers.some((s) => s.id === serverId), "list_servers finds the server");

const start = await client.callTool({ name: "get_events", arguments: { server_id: serverId } });
const cursor = start.structuredContent.cursor;
check(typeof cursor === "number", "get_events returns a cursor");

const sent = await client.callTool({
  name: "send_message",
  arguments: { server_id: serverId, channel_id: channelId, content: "hello from the MCP client" },
});
check(!sent.isError, "send_message works");

const events = await client.callTool({ name: "get_events", arguments: { server_id: serverId, after_sequence: cursor } });
check(
  events.structuredContent.events.some((e) => e.type === "message_created" && e.message.content === "hello from the MCP client"),
  "the message comes back as an event",
);

const refused = await client.callTool({ name: "kick_member", arguments: { server_id: serverId, user_id: "01J000000000000000000000AA" } });
check(refused.isError === true, "a refused call is a tool error");

const { resourceTemplates } = await client.listResourceTemplates();
check(resourceTemplates.length === 2, "two resource templates");
const resource = await client.readResource({ uri: `fuwa://servers/${serverId}` });
check(JSON.parse(resource.contents[0].text).server.id === serverId, "the server resource reads");

const { prompts } = await client.listPrompts();
check(prompts.some((p) => p.name === "catch_up"), "prompts/list has catch_up");
const prompt = await client.getPrompt({ name: "catch_up", arguments: { server_id: serverId, channel_id: channelId } });
check(prompt.messages[0].content.text.includes(channelId), "the prompt names the channel");

await client.close();
console.log("conformance: ok");
