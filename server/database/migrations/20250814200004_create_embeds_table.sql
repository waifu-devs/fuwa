-- +goose Up
CREATE TABLE embeds (
  embed_id INTEGER PRIMARY KEY AUTOINCREMENT,
  message_id TEXT NOT NULL,
  title TEXT,
  description TEXT,
  url TEXT,
  color INTEGER,
  thumbnail_url TEXT,
  image_url TEXT
);

CREATE INDEX idx_embeds_message_id ON embeds(message_id);

-- +goose Down
DROP TABLE embeds;