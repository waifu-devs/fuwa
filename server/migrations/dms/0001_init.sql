-- Direct messages: conversations between two accounts, end-to-end encrypted
-- with MLS (RFC 9420). This file keeps only what delivering them takes: the
-- devices' public keys and key packages, who is in each conversation, the
-- group's public state, and each record's ciphertext. Nothing here opens a
-- message.

-- One per signed-in session that uses direct messages. A device whose
-- session ended is gone, whether or not its row has been swept yet.
CREATE TABLE devices (
  id TEXT NOT NULL PRIMARY KEY,           -- hex of the first 16 bytes of sha256(signature_key)
  account_id TEXT NOT NULL,
  session_id TEXT NOT NULL UNIQUE,        -- node.db's sessions.id
  signature_key BLOB NOT NULL,            -- Ed25519
  label TEXT NOT NULL DEFAULT '',         -- the User-Agent it registered from
  last_resort BLOB,                       -- the key package handed out once the others run out
  created_at INTEGER NOT NULL             -- unix ms, like every time here
);

CREATE INDEX devices_by_account ON devices (account_id);

-- Single-use key packages, handed out (and deleted) one at a time.
CREATE TABLE key_packages (
  id TEXT NOT NULL PRIMARY KEY,
  device_id TEXT NOT NULL,
  data BLOB NOT NULL,                     -- an MLSMessage carrying a KeyPackage
  expires_at INTEGER NOT NULL
);

CREATE INDEX key_packages_by_device ON key_packages (device_id, expires_at);

CREATE TABLE conversations (
  id TEXT NOT NULL PRIMARY KEY,           -- also the MLS group id
  pair TEXT NOT NULL UNIQUE,              -- the two account ids, sorted, joined with ':'
  epoch INTEGER NOT NULL DEFAULT 0,       -- the epoch the next record must be in
  last_seq INTEGER NOT NULL DEFAULT 0,
  group_info BLOB,                        -- the group as the last commit left it
  created_by TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE participants (
  conversation_id TEXT NOT NULL,
  account_id TEXT NOT NULL,
  PRIMARY KEY (conversation_id, account_id)
);

CREATE INDEX participants_by_account ON participants (account_id);

-- Each conversation's log, in the order its devices read it.
CREATE TABLE records (
  conversation_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  kind INTEGER NOT NULL,                  -- fuwa.v1.ConversationRecordKind
  epoch INTEGER NOT NULL,
  sender_id TEXT NOT NULL,
  sender_device_id TEXT NOT NULL,
  data BLOB,                              -- the MLS message; NULL once deleted
  created_at INTEGER NOT NULL,
  deleted_at INTEGER,
  PRIMARY KEY (conversation_id, seq)
);

-- What a device added to a conversation joins with, until it's in.
CREATE TABLE welcomes (
  conversation_id TEXT NOT NULL,
  device_id TEXT NOT NULL,
  seq INTEGER NOT NULL,                   -- the commit that added it
  data BLOB NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (conversation_id, device_id)
);

CREATE INDEX welcomes_by_device ON welcomes (device_id);
