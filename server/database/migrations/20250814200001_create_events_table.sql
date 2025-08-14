-- +goose Up
CREATE TABLE events (
  event_id TEXT NOT NULL PRIMARY KEY,
  event_type TEXT NOT NULL,
  scope TEXT NOT NULL,
  actor_id TEXT NOT NULL,
  timestamp INTEGER NOT NULL,
  payload TEXT, -- JSON as TEXT
  metadata TEXT, -- JSON as TEXT  
  sequence INTEGER NOT NULL
);

CREATE INDEX idx_events_scope_sequence ON events(scope, sequence);
CREATE INDEX idx_events_timestamp ON events(timestamp);
CREATE INDEX idx_events_event_type ON events(event_type);
CREATE INDEX idx_events_actor_id ON events(actor_id);

-- +goose Down
DROP TABLE events;