-- Agents' slash commands and the interactions people start with them
-- (proto/fuwa/v1/command.proto, docs/commands.md).

-- One row per command an agent set in this server. `body` is the Command as
-- protobuf; `position` keeps the order the agent gave.
CREATE TABLE commands (
    agent_id TEXT NOT NULL,
    name TEXT NOT NULL,
    position INTEGER NOT NULL,
    body BLOB NOT NULL,
    PRIMARY KEY (agent_id, name)
);

-- Interactions an agent may still answer: 15 minutes from `created_at`
-- (Unix milliseconds), at most 5 answers. Older rows are dropped as new ones
-- come. What was typed isn't kept here, only in the agent's event.
CREATE TABLE interactions (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    kind INTEGER NOT NULL,
    command TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    answers INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX interactions_by_age ON interactions (created_at);
