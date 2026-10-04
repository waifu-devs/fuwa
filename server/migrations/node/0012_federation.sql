-- Other fuwa instances this one has pinned a key for (docs/federation.md).
-- This instance's own key is in meta ('federation_key').
CREATE TABLE federation_peers (
  origin TEXT PRIMARY KEY,            -- https://chat.example.com
  public_key BLOB NOT NULL,           -- Ed25519, 32 bytes
  first_seen INTEGER NOT NULL,
  last_heard INTEGER NOT NULL
);
