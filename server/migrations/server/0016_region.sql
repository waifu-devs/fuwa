-- The region the shard holding this server is in (docs/regions.md); empty for
-- the instance's home region. Set by the shard that opens the file, so it
-- follows the server when it moves.
ALTER TABLE server ADD COLUMN region TEXT NOT NULL DEFAULT '';
