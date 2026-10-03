-- Secure channels: end-to-end encrypted channels (MLS, RFC 9420, as direct
-- messages are). This file keeps only what delivering them takes: each
-- channel's group state, the ciphertext of every record in one order, and the
-- welcomes waiting for devices that were added. Nothing here opens a message.

-- One per secure channel, made with it.
CREATE TABLE secure_groups (
  channel_id TEXT NOT NULL PRIMARY KEY,   -- also the MLS group id
  epoch INTEGER NOT NULL DEFAULT 0,       -- the epoch the next record must be in
  last_seq INTEGER NOT NULL DEFAULT 0,
  group_info BLOB,                        -- the group as the last commit left it
  updated_at INTEGER NOT NULL
);

-- Each secure channel's log, in the order its devices read it.
CREATE TABLE secure_records (
  channel_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  kind INTEGER NOT NULL,                  -- fuwa.v1.SecureRecordKind
  epoch INTEGER NOT NULL,
  sender_id TEXT NOT NULL,
  sender_device_id TEXT NOT NULL,
  data BLOB,                              -- the MLS message; NULL once deleted
  created_at INTEGER NOT NULL,
  deleted_at INTEGER,
  deleted_by TEXT,                        -- a moderator who deleted someone else's
  PRIMARY KEY (channel_id, seq)
);

-- What a device added to a secure channel joins with, until it's in.
CREATE TABLE secure_welcomes (
  channel_id TEXT NOT NULL,
  device_id TEXT NOT NULL,
  seq INTEGER NOT NULL,                   -- the commit that added it
  data BLOB NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (channel_id, device_id)
);

CREATE INDEX secure_welcomes_by_device ON secure_welcomes (device_id);
