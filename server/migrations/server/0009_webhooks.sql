-- Webhooks: addresses other apps post messages to, each into one channel.
CREATE TABLE webhooks (
  id TEXT NOT NULL PRIMARY KEY,
  channel_id TEXT NOT NULL,
  name TEXT NOT NULL,
  avatar_url TEXT NOT NULL DEFAULT '',
  token TEXT NOT NULL,
  creator_id TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  last_used_at INTEGER,
  messages INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX webhooks_by_channel ON webhooks (channel_id);

-- Whoever could manage the server gets MANAGE_WEBHOOKS (bit 19) along with
-- MANAGE_SERVER (bit 2).
UPDATE roles SET permissions = permissions | 524288 WHERE permissions & 4 = 4;
