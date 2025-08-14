-- +goose Up
CREATE TABLE config_values (
  scope TEXT NOT NULL,
  key TEXT NOT NULL,
  value TEXT NOT NULL, -- JSON as TEXT for ConfigValue
  type INTEGER NOT NULL, -- ConfigValueType enum as INTEGER
  is_sensitive INTEGER NOT NULL DEFAULT 0, -- Boolean as INTEGER (0/1)
  constraints TEXT, -- JSON as TEXT for ConfigConstraints
  created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
  updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
  PRIMARY KEY (scope, key)
);

CREATE INDEX idx_config_values_scope ON config_values(scope);
CREATE INDEX idx_config_values_type ON config_values(type);
CREATE INDEX idx_config_values_updated_at ON config_values(updated_at);

-- +goose Down
DROP TABLE config_values;