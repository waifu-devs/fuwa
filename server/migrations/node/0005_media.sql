-- Pictures people upload (avatars, banners, server icons). The bytes are files
-- under media/ in the data directory; a row is made when an upload is
-- reserved, gets `stored_at` when the bytes arrive, and `used_at` once it's
-- set as a picture. Rows that never got their bytes, or were never used, are
-- swept along with their files.
CREATE TABLE media (
  id TEXT PRIMARY KEY,                -- also the file's name
  account_id TEXT NOT NULL,           -- who uploaded it
  purpose INTEGER NOT NULL,           -- MediaPurpose
  content_type TEXT NOT NULL,         -- as sniffed from the bytes once stored
  size INTEGER NOT NULL,
  upload_hash TEXT,                   -- sha256 of the upload token, hex, until the upload starts
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,        -- an upload not stored by then is dropped
  stored_at INTEGER,
  used_at INTEGER,
  server_id TEXT                      -- the server whose icon it is
);
CREATE UNIQUE INDEX media_by_upload ON media (upload_hash);
CREATE INDEX media_by_account ON media (account_id);
