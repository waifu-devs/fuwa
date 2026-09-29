-- Server settings and moderation.

-- fuwa.v1.NotificationLevel members start with; 0 leaves it to each person.
ALTER TABLE server ADD COLUMN default_notifications INTEGER NOT NULL DEFAULT 0;
-- Where "someone joined" messages go; NULL for nowhere.
ALTER TABLE server ADD COLUMN system_channel_id TEXT;
-- Servers that were already here post them in their first text channel, as new ones do.
UPDATE server SET system_channel_id = (SELECT id FROM channels WHERE type = 1 ORDER BY position, id LIMIT 1);

-- Timed out: can read but not send until then (unix ms).
ALTER TABLE members ADD COLUMN timed_out_until INTEGER;

-- Seconds each member waits between messages in the channel; 0 for off.
ALTER TABLE channels ADD COLUMN slowmode_seconds INTEGER NOT NULL DEFAULT 0;

-- fuwa.v1.MessageKind; 0 for messages people wrote.
ALTER TABLE messages ADD COLUMN kind INTEGER NOT NULL DEFAULT 0;

-- When each member last sent a message in a slow-mode channel. One row per
-- member and channel, so two messages sent at once clash and only one gets in.
CREATE TABLE slowmode (
  channel_id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  sent_at INTEGER NOT NULL,
  PRIMARY KEY (channel_id, user_id)
);

-- People kept out. Their look stays in `users`.
CREATE TABLE bans (
  user_id TEXT NOT NULL PRIMARY KEY,
  reason TEXT NOT NULL DEFAULT '',
  banned_by TEXT NOT NULL,
  created_at INTEGER NOT NULL
);

-- What owners and admins did. Kept apart from the event log, which every
-- member follows, so reasons stay between the people who run the server.
CREATE TABLE audit (
  id TEXT NOT NULL PRIMARY KEY,       -- ULID, so ids sort by time
  actor_id TEXT NOT NULL,
  action INTEGER NOT NULL,            -- fuwa.v1.AuditAction
  target_id TEXT NOT NULL DEFAULT '',
  channel_name TEXT NOT NULL DEFAULT '',
  reason TEXT NOT NULL DEFAULT '',
  changes BLOB,                       -- fuwa.v1.AuditChange list, protobuf-encoded
  created_at INTEGER NOT NULL
);

-- Deleting what a banned person sent recently.
CREATE INDEX messages_by_author ON messages (author_id, id);
