-- Agents' endpoints (docs/agent-endpoints.md): a URL the instance posts the
-- agent's events to. One row per agent, made with its signing secret the
-- first time its owner asks; `url` is empty while it's off.
CREATE TABLE agent_endpoints (
  account_id TEXT NOT NULL PRIMARY KEY,   -- the agent
  url TEXT NOT NULL DEFAULT '',
  events TEXT NOT NULL DEFAULT '',        -- Event payload names, comma-separated; empty for all
  secret TEXT NOT NULL,                   -- whsec_..., needed in the clear to sign
  epoch INTEGER NOT NULL DEFAULT 0,       -- moves on each time the URL is set
  updated_at INTEGER NOT NULL,
  last_delivered_at INTEGER,
  failing_since INTEGER,
  last_error TEXT NOT NULL DEFAULT '',
  disabled_at INTEGER                     -- turned off after a day of failures
);
