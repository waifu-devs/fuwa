-- +goose Up
-- +goose StatementBegin
CREATE TABLE channels (
  channel_id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT
);
-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
DROP TABLE channels;
-- +goose StatementEnd
