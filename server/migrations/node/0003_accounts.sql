-- Devices, two-step sign-in, fuller profiles and notification settings.

-- Sessions get an id people can see (the token's hash never leaves the
-- instance), the device they signed in from, and when they were last used.
ALTER TABLE sessions ADD COLUMN id TEXT;
ALTER TABLE sessions ADD COLUMN user_agent TEXT NOT NULL DEFAULT '';
ALTER TABLE sessions ADD COLUMN last_active_at INTEGER NOT NULL DEFAULT 0;
UPDATE sessions SET id = lower(hex(randomblob(16))), last_active_at = created_at;
CREATE UNIQUE INDEX sessions_by_id ON sessions (id);

ALTER TABLE accounts ADD COLUMN pronouns TEXT NOT NULL DEFAULT '';
ALTER TABLE accounts ADD COLUMN bio TEXT NOT NULL DEFAULT '';         -- Markdown
ALTER TABLE accounts ADD COLUMN banner_url TEXT NOT NULL DEFAULT '';
ALTER TABLE accounts ADD COLUMN accent_color INTEGER;                  -- 0xRRGGBB
ALTER TABLE accounts ADD COLUMN status TEXT NOT NULL DEFAULT '';
ALTER TABLE accounts ADD COLUMN status_expires_at INTEGER;

-- Two-step sign-in with an authenticator app (TOTP, RFC 6238).
ALTER TABLE accounts ADD COLUMN totp_secret TEXT;                      -- base32, while it's on
ALTER TABLE accounts ADD COLUMN totp_pending TEXT;                     -- being set up, not confirmed yet
ALTER TABLE accounts ADD COLUMN totp_last_step INTEGER NOT NULL DEFAULT 0; -- codes up to this step are used up

CREATE TABLE backup_codes (
  account_id TEXT NOT NULL,
  code_hash TEXT NOT NULL,            -- sha256 of the code, hex
  used_at INTEGER,
  PRIMARY KEY (account_id, code_hash)
);

-- Sign-ins waiting on a two-step code: the password was right.
CREATE TABLE sign_in_tickets (
  ticket_hash TEXT NOT NULL PRIMARY KEY,
  account_id TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  expires_at INTEGER NOT NULL
);

-- How each server, or a channel in it, notifies someone. A row only exists
-- while it says something.
CREATE TABLE notification_settings (
  account_id TEXT NOT NULL,
  server_id TEXT NOT NULL,
  channel_id TEXT NOT NULL,           -- '' for the whole server
  level INTEGER NOT NULL DEFAULT 0,   -- fuwa.v1.NotificationLevel
  muted INTEGER NOT NULL DEFAULT 0,
  muted_until INTEGER,                -- unix ms; NULL while muted means until turned off
  suppress_everyone INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (account_id, server_id, channel_id)
);
