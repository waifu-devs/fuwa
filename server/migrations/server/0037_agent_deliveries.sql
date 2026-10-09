-- How far each agent with an endpoint has had this server's events
-- (docs/agent-endpoints.md). `epoch` is its endpoint's when the row was
-- made: a newer one starts again from the head of the log.
CREATE TABLE agent_deliveries (
  agent_id TEXT NOT NULL PRIMARY KEY,
  epoch INTEGER NOT NULL,
  sequence INTEGER NOT NULL
);
