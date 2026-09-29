-- The instance's own database: its accounts, their sessions, and a few settings.
-- Community servers each live in their own database under servers/.

CREATE TABLE meta (
  key TEXT NOT NULL PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE accounts (
  id TEXT NOT NULL PRIMARY KEY,
  kind INTEGER NOT NULL,              -- fuwa.v1.AccountKind
  username TEXT NOT NULL UNIQUE,      -- lowercase
  display_name TEXT NOT NULL,
  avatar_url TEXT NOT NULL DEFAULT '',
  password_hash TEXT,                 -- standalone accounts (argon2id, PHC string)
  linked_issuer TEXT,                 -- linked accounts: who vouches for them...
  linked_subject TEXT,                -- ...and who they are there
  admin INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,        -- unix ms, like every timestamp here
  updated_at INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX accounts_by_link ON accounts (linked_issuer, linked_subject);
CREATE INDEX accounts_by_last_seen ON accounts (last_seen_at);

CREATE TABLE sessions (
  token_hash TEXT NOT NULL PRIMARY KEY, -- sha256 of the bearer token, hex
  account_id TEXT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);

CREATE INDEX sessions_by_account ON sessions (account_id);
