-- +goose Up
CREATE TABLE embed_fields (
  field_id INTEGER PRIMARY KEY AUTOINCREMENT,
  embed_id INTEGER NOT NULL,
  name TEXT NOT NULL,
  value TEXT NOT NULL,
  inline INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_embed_fields_embed_id ON embed_fields(embed_id);

-- +goose Down
DROP TABLE embed_fields;