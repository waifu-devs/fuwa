-- Live tiles (proto/fuwa/v1/live_tile.proto, docs/live-tiles.md): which
-- kinds the server shows, and the tiles its agents and webhooks keep.

-- The kinds someone with MANAGE_SERVER chose, as a list of LiveTileKind
-- numbers in JSON; NULL until anyone chooses, for the default.
ALTER TABLE server ADD COLUMN live_tiles TEXT;

-- One row per tile. `source_id` is the agent's account or the webhook,
-- `source_kind` a LiveTileSource; `content` is the LiveTileContent as
-- protobuf. Times are Unix milliseconds. Rows past `expires_at` are left
-- out when read and dropped now and then.
CREATE TABLE live_tiles (
    channel_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    tile_id TEXT NOT NULL,
    source_kind INTEGER NOT NULL,
    content BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (channel_id, source_id, tile_id)
);
CREATE INDEX live_tiles_expiry ON live_tiles (expires_at);
