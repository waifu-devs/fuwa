-- +goose Up
CREATE TABLE channels (
  channel_id TEXT NOT NULL PRIMARY KEY,
  name TEXT NOT NULL,
  type INTEGER NOT NULL
);

-- +goose Down
DROP TABLE channels;
