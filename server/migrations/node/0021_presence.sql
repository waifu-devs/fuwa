-- What a person picked for their presence (docs/presence.md): their status
-- and what they share. What they're doing is never stored: it lives in
-- memory only.
CREATE TABLE presence_settings (
  account_id TEXT PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
  status INTEGER NOT NULL,            -- PresenceStatus: online, idle, do not disturb or invisible
  show_activity INTEGER NOT NULL,     -- "Show what I'm doing"
  hidden_servers TEXT NOT NULL,       -- JSON array of server ids where activity isn't shown
  updated_at INTEGER NOT NULL
);
