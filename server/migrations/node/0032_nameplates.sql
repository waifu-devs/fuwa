-- The nameplate behind someone's name in member lists: one of the
-- instance's profile items, or empty for none (docs/profile-items.md).
ALTER TABLE accounts ADD COLUMN profile_nameplate TEXT NOT NULL DEFAULT '';
