-- Which agents may use this server through the instance's MCP endpoint
-- (docs/mcp.md), as JSON {"mode": <McpAccessMode>, "agents": [ids]}. Empty
-- lets every agent in the server.
ALTER TABLE server ADD COLUMN mcp_access TEXT NOT NULL DEFAULT '';
