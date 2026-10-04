-- Keys other instances moved to by rotating their own, each one vouched for
-- by the key before it (docs/federation.md), shown to this instance's admins.
CREATE TABLE federation_moves (
  origin TEXT NOT NULL,
  previous_key BLOB NOT NULL,          -- Ed25519, 32 bytes
  key BLOB NOT NULL,
  moved_at INTEGER NOT NULL
);
CREATE INDEX federation_moves_origin ON federation_moves (origin, moved_at);

-- An instance whose key moved in a way this one can't follow: nothing is
-- taken from or sent to it until an admin here checks it again.
ALTER TABLE federation_peers ADD COLUMN needs_check INTEGER NOT NULL DEFAULT 0;
