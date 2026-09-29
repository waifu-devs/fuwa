-- One community server. The whole server lives in this one file, so it can be
-- backed up, moved or inspected with any SQLite tool.

CREATE TABLE server (
  id TEXT NOT NULL PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  icon_url TEXT NOT NULL DEFAULT '',
  owner_id TEXT NOT NULL,
  discoverable INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,        -- unix ms, like every timestamp here
  updated_at INTEGER NOT NULL
);

-- Everyone who has ever been a member, as they looked last. Kept after they
-- leave so their messages still have a name and a face.
CREATE TABLE users (
  id TEXT NOT NULL PRIMARY KEY,
  username TEXT NOT NULL,
  display_name TEXT NOT NULL,
  avatar_url TEXT NOT NULL DEFAULT '',
  kind INTEGER NOT NULL               -- fuwa.v1.AccountKind
);

CREATE TABLE members (
  user_id TEXT NOT NULL PRIMARY KEY REFERENCES users (id),
  nickname TEXT NOT NULL DEFAULT '',
  role INTEGER NOT NULL,              -- fuwa.v1.MemberRole
  joined_at INTEGER NOT NULL
);

CREATE TABLE channels (
  id TEXT NOT NULL PRIMARY KEY,
  name TEXT NOT NULL,
  type INTEGER NOT NULL,              -- fuwa.v1.ChannelType
  parent_id TEXT,
  topic TEXT NOT NULL DEFAULT '',
  position INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE messages (
  id TEXT NOT NULL PRIMARY KEY,       -- ULID, so ids sort by time
  channel_id TEXT NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
  author_id TEXT NOT NULL,
  content TEXT NOT NULL,
  size INTEGER NOT NULL,              -- bytes of content
  extras BLOB,                        -- attachments and embeds, protobuf-encoded
  attachment_count INTEGER NOT NULL DEFAULT 0,
  reply_to_id TEXT,
  created_at INTEGER NOT NULL,
  edited_at INTEGER
);

CREATE INDEX messages_by_channel ON messages (channel_id, id);

-- The server's event log. Every change lands here in the same transaction.
CREATE TABLE events (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL UNIQUE,
  actor_id TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  payload BLOB NOT NULL               -- the fuwa.v1.Event, protobuf-encoded
);

-- Running totals, kept in step with every write, for usage reporting and limits.
CREATE TABLE usage (
  id INTEGER NOT NULL PRIMARY KEY CHECK (id = 1),
  members INTEGER NOT NULL DEFAULT 0,
  channels INTEGER NOT NULL DEFAULT 0,
  messages INTEGER NOT NULL DEFAULT 0,
  messages_sent INTEGER NOT NULL DEFAULT 0,
  message_bytes INTEGER NOT NULL DEFAULT 0,
  attachments INTEGER NOT NULL DEFAULT 0,
  attachment_bytes INTEGER NOT NULL DEFAULT 0,
  events INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL DEFAULT 0
);

INSERT INTO usage (id) VALUES (1);

-- This server's own caps; NULL falls back to the instance defaults (unlimited
-- unless the operator sets them).
CREATE TABLE limits (
  id INTEGER NOT NULL PRIMARY KEY CHECK (id = 1),
  members INTEGER,
  channels INTEGER,
  storage_bytes INTEGER,
  attachment_bytes INTEGER
);

INSERT INTO limits (id) VALUES (1);
