-- Instance admins can turn an account off: it can't sign in, and its devices
-- are signed out. The reason is for other admins.
ALTER TABLE accounts ADD COLUMN disabled_at INTEGER;                   -- unix ms; NULL while it can sign in
ALTER TABLE accounts ADD COLUMN disabled_reason TEXT NOT NULL DEFAULT '';
