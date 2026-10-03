-- Regions (docs/regions.md): the region each shard said it's in (empty for
-- the instance's home region), and servers being moved between shards, so a
-- directory that restarts mid-move finishes or undoes it.
ALTER TABLE shards ADD COLUMN region TEXT NOT NULL DEFAULT '';
CREATE TABLE moves (
  server_id TEXT PRIMARY KEY,
  from_shard TEXT NOT NULL,
  to_shard TEXT NOT NULL,
  -- 1 once the server is open on to_shard and placed there.
  committed INTEGER NOT NULL DEFAULT 0,
  started_at INTEGER NOT NULL
);
