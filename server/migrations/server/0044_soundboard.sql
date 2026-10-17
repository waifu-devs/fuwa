-- The soundboard (docs/soundboard.md): short sounds anyone in a voice
-- channel can play for everyone there. `name_key` is the name folded to
-- lower case, so two sounds can't differ only by case. `volume` is 0 to 1.

CREATE TABLE sounds (
  id TEXT NOT NULL PRIMARY KEY,
  name TEXT NOT NULL,
  name_key TEXT NOT NULL UNIQUE,
  emoji TEXT NOT NULL DEFAULT '',
  url TEXT NOT NULL,
  volume REAL NOT NULL DEFAULT 1,
  duration_ms INTEGER NOT NULL DEFAULT 0,
  creator_id TEXT NOT NULL,
  size INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);

-- Managing the soundboard (1 << 33) goes wherever managing emoji (1 << 18)
-- did, and using it (1 << 34) wherever speaking (1 << 21) did, staying off
-- wherever a channel turned speaking off.
UPDATE roles SET permissions = permissions | 8589934592 WHERE permissions & 262144 = 262144;
UPDATE roles SET permissions = permissions | 17179869184 WHERE permissions & 2097152 = 2097152;
UPDATE channel_overwrites SET allow = allow | 17179869184 WHERE allow & 2097152 = 2097152;
UPDATE channel_overwrites SET deny = deny | 17179869184 WHERE deny & 2097152 = 2097152;
