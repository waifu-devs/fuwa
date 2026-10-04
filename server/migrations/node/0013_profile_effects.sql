-- The animated effect over someone's profile card, by the id of one the apps
-- ship with (docs/profile-effects.md). Empty for none.
ALTER TABLE accounts ADD COLUMN profile_effect TEXT NOT NULL DEFAULT '';
