-- Voice channels recorded on the server. The sound is in files under
-- <data>/recordings/<server>/<recording>/, one per person; a row says what
-- they are. `tracks` is a JSON list of {user_id, size_bytes, duration_ms},
-- written when the recording ends.
CREATE TABLE recordings (
  id TEXT NOT NULL PRIMARY KEY,
  channel_id TEXT NOT NULL,
  started_by TEXT NOT NULL,
  started_at INTEGER NOT NULL,
  ended_at INTEGER,
  sealed INTEGER NOT NULL DEFAULT 0,
  tracks TEXT NOT NULL DEFAULT '[]',
  size_bytes INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX recordings_by_channel ON recordings (channel_id, started_at);
