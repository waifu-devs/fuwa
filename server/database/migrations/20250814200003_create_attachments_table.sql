-- +goose Up
CREATE TABLE attachments (
  attachment_id TEXT NOT NULL PRIMARY KEY,
  message_id TEXT NOT NULL,
  channel_id TEXT NOT NULL,
  author_id TEXT NOT NULL,
  filename TEXT NOT NULL,
  content_type TEXT NOT NULL,
  size INTEGER NOT NULL,
  url TEXT NOT NULL
);

CREATE INDEX idx_attachments_message_id ON attachments(message_id);
CREATE INDEX idx_attachments_channel_id ON attachments(channel_id);
CREATE INDEX idx_attachments_author_id ON attachments(author_id);

-- +goose Down
DROP TABLE attachments;