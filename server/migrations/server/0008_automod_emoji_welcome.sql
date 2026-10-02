-- AutoMod, custom emoji and the welcome screen.

-- Each rule is a fuwa.v1.AutoModRule, as protobuf.
CREATE TABLE automod_rules (
  id TEXT NOT NULL PRIMARY KEY,
  rule BLOB NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE TABLE emojis (
  id TEXT NOT NULL PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  url TEXT NOT NULL,
  animated INTEGER NOT NULL DEFAULT 0,
  creator_id TEXT NOT NULL,
  size INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);

-- Kept beside the other totals so adding emoji at once can't pass the cap.
ALTER TABLE usage ADD COLUMN emojis INTEGER NOT NULL DEFAULT 0;
ALTER TABLE limits ADD COLUMN emojis INTEGER;

-- JSON: {"enabled", "description", "channels": [{"channel_id", "description", "emoji"}]}.
ALTER TABLE server ADD COLUMN welcome TEXT NOT NULL DEFAULT '{}';

-- Emoji used to need nothing: whoever could manage the server gets
-- MANAGE_EMOJI (bit 18) along with MANAGE_SERVER (bit 2).
UPDATE roles SET permissions = permissions | 262144 WHERE permissions & 4 = 4;
