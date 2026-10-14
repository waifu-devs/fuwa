-- Each person's app settings, the same on every device they sign in on
-- (docs/app-settings.md): a JSON document apps write and merge; the
-- instance never reads inside it. `version` goes up with each write, so a
-- write merged from an older one is refused.
CREATE TABLE app_settings (
  account_id TEXT PRIMARY KEY,
  document TEXT NOT NULL,
  version INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
