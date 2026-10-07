-- Profile effects and avatar decorations the server offers its members
-- (docs/profile-items.md), in the same shape as node.db's.
CREATE TABLE profile_items (
  id TEXT NOT NULL PRIMARY KEY,
  kind INTEGER NOT NULL,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  effect TEXT NOT NULL DEFAULT '',
  picture_url TEXT NOT NULL DEFAULT '',
  animated INTEGER NOT NULL DEFAULT 0,
  creator_id TEXT NOT NULL,
  size INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);

-- A member's profile in this server: an effect (a built-in one's id or one
-- of the server's items) and one of the server's decorations, shown here
-- instead of their own. Empty for their own.
ALTER TABLE members ADD COLUMN profile_effect TEXT NOT NULL DEFAULT '';
ALTER TABLE members ADD COLUMN profile_decoration TEXT NOT NULL DEFAULT '';

-- The decoration a user wears everywhere (one of the instance's items), as
-- the copy of their look kept here.
ALTER TABLE users ADD COLUMN decoration_id TEXT NOT NULL DEFAULT '';
