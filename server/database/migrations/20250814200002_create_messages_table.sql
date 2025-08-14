-- +goose Up
CREATE TABLE messages (
  message_id TEXT NOT NULL PRIMARY KEY,
  channel_id TEXT NOT NULL,
  author_id TEXT NOT NULL,
  content TEXT NOT NULL,
  created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
  updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
  reply_to_id TEXT
);

CREATE INDEX idx_messages_channel_id ON messages(channel_id);
CREATE INDEX idx_messages_author_id ON messages(author_id);
CREATE INDEX idx_messages_created_at ON messages(created_at);
CREATE INDEX idx_messages_reply_to_id ON messages(reply_to_id);

-- +goose Down
DROP TABLE messages;