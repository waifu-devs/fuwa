-- Threads: replies under a message, kept apart from the channel's own list.

-- A thread reply: the message the thread is under.
ALTER TABLE messages ADD COLUMN thread_id TEXT;
-- A thread reply its author also sent to the channel.
ALTER TABLE messages ADD COLUMN in_channel INTEGER NOT NULL DEFAULT 0;

CREATE INDEX messages_by_thread ON messages (thread_id, id);

-- Each thread, by the message it's under, worked out again from its replies
-- whenever they change.
CREATE TABLE threads (
  id TEXT NOT NULL PRIMARY KEY,       -- the message it's under
  channel_id TEXT NOT NULL,
  reply_count INTEGER NOT NULL DEFAULT 0,
  last_reply_id TEXT NOT NULL,        -- ULID: orders threads by their latest reply
  last_reply_at INTEGER NOT NULL,
  participants TEXT NOT NULL DEFAULT '', -- up to 5 ids, the latest first, comma-separated
  locked INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX threads_by_channel ON threads (channel_id, last_reply_id);

-- Who follows which thread. Unfollowing keeps the row (follow = 0), so
-- replying again doesn't follow it again.
CREATE TABLE thread_follows (
  thread_id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  follow INTEGER NOT NULL,
  PRIMARY KEY (thread_id, user_id)
);

CREATE INDEX thread_follows_by_user ON thread_follows (user_id, thread_id);

-- How long a quiet thread stays open, in hours: a week.
ALTER TABLE server ADD COLUMN thread_archive_hours INTEGER NOT NULL DEFAULT 168;

-- Starting threads (1 << 26) goes wherever sending messages (1 << 12) did,
-- and stays off wherever a channel turned sending off.
UPDATE roles SET permissions = permissions | 67108864 WHERE permissions & 4096 = 4096;
UPDATE channel_overwrites SET allow = allow | 67108864 WHERE allow & 4096 = 4096;
UPDATE channel_overwrites SET deny = deny | 67108864 WHERE deny & 4096 = 4096;
