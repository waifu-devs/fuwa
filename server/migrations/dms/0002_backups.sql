-- Message backups: what each account's devices read in direct messages and
-- secure channels, encrypted with a recovery key only the account's owner
-- holds, so a new device can read what came before it. The instance keeps
-- the encrypted parts in order and can't open any of them.

-- One per account that turned backup on.
CREATE TABLE backups (
  account_id TEXT NOT NULL PRIMARY KEY,
  key_check BLOB NOT NULL,                -- derived from the recovery key, so a device can tell it has the right one
  size INTEGER NOT NULL DEFAULT 0,        -- bytes in its parts
  parts INTEGER NOT NULL DEFAULT 0,
  next_seq INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

-- Its encrypted parts, oldest first. A later part can bring a newer copy of
-- something an earlier one had (an edit, a deletion).
CREATE TABLE backup_parts (
  account_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  data BLOB NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (account_id, seq)
);
