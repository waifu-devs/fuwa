-- Sealed files a message carries (a voice message): ciphertext kept in the
-- instance's uploads (node.db's media), whose key travels only inside the
-- message. Each file belongs to one record and goes when it's deleted.
CREATE TABLE record_media (
  media_id TEXT NOT NULL PRIMARY KEY,
  conversation_id TEXT NOT NULL,
  seq INTEGER NOT NULL
);

CREATE INDEX record_media_by_record ON record_media (conversation_id, seq);

-- Bytes of sealed files each account reserved each day (UTC), for the
-- instance's daily cap on them; apart from node.db's picture counts.
CREATE TABLE sealed_days (
  account_id TEXT NOT NULL,
  day INTEGER NOT NULL,
  bytes INTEGER NOT NULL,
  PRIMARY KEY (account_id, day)
);
