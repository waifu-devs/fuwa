-- Ready for concurrent writes (Turso MVCC), where writes run side by side and
-- two that touch the same row clash and one is retried.

-- Sequences are handed out by the server, in commit order, so the log reads in
-- the order changes happened. AUTOINCREMENT would number them as they're
-- written instead, which under concurrent writes isn't the order they commit.
CREATE TABLE events_ordered (
  sequence INTEGER PRIMARY KEY,
  id TEXT NOT NULL UNIQUE,
  actor_id TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  payload BLOB NOT NULL               -- the fuwa.v1.Event, protobuf-encoded
);
INSERT INTO events_ordered (sequence, id, actor_id, created_at, payload)
  SELECT sequence, id, actor_id, created_at, payload FROM events;
DROP TABLE events;
ALTER TABLE events_ordered RENAME TO events;

-- Changes to the message totals, one row per write, so messages sent side by
-- side never update the same row. `usage` holds the totals up to the last
-- time these were folded into it; members and channels stay in `usage`
-- itself, so writes that add them clash and their caps hold exactly. The
-- event total is the log's last sequence.
CREATE TABLE usage_changes (
  id TEXT NOT NULL PRIMARY KEY,       -- ULID
  messages INTEGER NOT NULL DEFAULT 0,
  messages_sent INTEGER NOT NULL DEFAULT 0,
  message_bytes INTEGER NOT NULL DEFAULT 0,
  attachments INTEGER NOT NULL DEFAULT 0,
  attachment_bytes INTEGER NOT NULL DEFAULT 0,
  at INTEGER NOT NULL
);

-- Messages left behind by channels deleted before this version (foreign keys
-- weren't switched on, so ON DELETE CASCADE never ran).
DELETE FROM messages WHERE channel_id NOT IN (SELECT id FROM channels);
