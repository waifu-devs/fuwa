-- +goose Up
ALTER TABLE channels ADD COLUMN server_id TEXT;
ALTER TABLE channels ADD COLUMN parent_id TEXT;
ALTER TABLE channels ADD COLUMN metadata TEXT; -- JSON as TEXT
ALTER TABLE channels ADD COLUMN created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'));
ALTER TABLE channels ADD COLUMN updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'));

CREATE INDEX idx_channels_server_id ON channels(server_id);
CREATE INDEX idx_channels_parent_id ON channels(parent_id);

-- +goose Down
-- SQLite doesn't support dropping columns, so we recreate the table
CREATE TABLE channels_old AS SELECT channel_id, name, type FROM channels;
DROP TABLE channels;
CREATE TABLE channels (
  channel_id TEXT NOT NULL PRIMARY KEY,
  name TEXT NOT NULL,
  type INTEGER NOT NULL
);
INSERT INTO channels SELECT * FROM channels_old;
DROP TABLE channels_old;