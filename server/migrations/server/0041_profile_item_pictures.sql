-- An effect's own pictures (docs/profile-items.md), as JSON: a list of
-- {url, animated, size}, which its layers name by their place.
ALTER TABLE profile_items ADD COLUMN pictures TEXT NOT NULL DEFAULT '[]';
