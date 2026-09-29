-- Instance settings changed by admins from a client. Each row overrides the
-- FUWA_* environment variable (or built-in default) for one setting; deleting
-- the row returns the setting to that default.

CREATE TABLE settings (
  key TEXT NOT NULL PRIMARY KEY, -- a fuwa.v1.InstanceSettings field path, like "name" or "default_limits.members"
  value TEXT NOT NULL,           -- JSON; null for a limit means no cap
  updated_at INTEGER NOT NULL
);
