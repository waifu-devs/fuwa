-- Nameplates (docs/profile-items.md): one of the server's, shown on a
-- member's profile here instead of their own (empty for their own), and the
-- one a user wears everywhere (one of the instance's items), as the copy of
-- their look kept here.
ALTER TABLE members ADD COLUMN profile_nameplate TEXT NOT NULL DEFAULT '';
ALTER TABLE users ADD COLUMN nameplate_id TEXT NOT NULL DEFAULT '';
