-- Files attached to messages (MEDIA_PURPOSE_ATTACHMENT uploads). The bytes
-- are kept with the server's pictures (on its shard, or under media/ in one
-- process); a row lives as long as its message has the file, and the file is
-- served only while it does.
CREATE TABLE attachments (
  media_id TEXT PRIMARY KEY,          -- the upload's id, which names the file
  message_id TEXT NOT NULL,
  channel_id TEXT NOT NULL,
  filename TEXT NOT NULL,             -- what it downloads as
  content_type TEXT NOT NULL,         -- from its bytes, when it arrived
  size INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX attachments_by_message ON attachments (message_id);
CREATE INDEX attachments_by_channel ON attachments (channel_id);

-- Files uploaded for this server that no message has: sent to its shard but
-- not sent yet, or let go of by a deleted message. Their bytes are never
-- served, whatever they look like, until a message takes them.
CREATE TABLE loose_files (
  media_id TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL
);
