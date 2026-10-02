-- Where community servers are when the instance runs split (FUWA_ROLE): the
-- shards that have registered, and which one holds each server. Kept so the
-- directory knows a server exists while its shard is still starting up. A
-- single-process instance leaves these empty.
CREATE TABLE shards (
  id TEXT PRIMARY KEY,
  url TEXT NOT NULL,                  -- how the directory and gateways reach it
  registered_at INTEGER NOT NULL
);
CREATE TABLE placements (
  server_id TEXT PRIMARY KEY,
  shard_id TEXT NOT NULL
);
CREATE INDEX placements_by_shard ON placements (shard_id);
