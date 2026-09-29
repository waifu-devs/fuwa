-- A person's custom status travels with their name and face.
ALTER TABLE users ADD COLUMN status TEXT NOT NULL DEFAULT '';
ALTER TABLE users ADD COLUMN status_expires_at INTEGER;
