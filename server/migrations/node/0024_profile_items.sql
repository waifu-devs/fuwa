-- Profile effects and avatar decorations the instance offers
-- (docs/profile-items.md). A server's are in its own file, in the same shape.
CREATE TABLE profile_items (
  id TEXT NOT NULL PRIMARY KEY,
  -- fuwa.v1.ProfileItemKind.
  kind INTEGER NOT NULL,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  -- An effect's spec, as checked JSON; empty for a decoration.
  effect TEXT NOT NULL DEFAULT '',
  -- A decoration's picture; empty for an effect.
  picture_url TEXT NOT NULL DEFAULT '',
  animated INTEGER NOT NULL DEFAULT 0,
  creator_id TEXT NOT NULL,
  size INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);

-- The decoration around someone's avatar: one of the instance's profile
-- items, by id. Empty for none.
ALTER TABLE accounts ADD COLUMN profile_decoration TEXT NOT NULL DEFAULT '';
