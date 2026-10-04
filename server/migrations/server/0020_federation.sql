-- Channels shared with servers on other instances (docs/federation.md).
-- A server there is known here as "<its id>@<its instance>", so its ids
-- never stand for this instance's own.

-- Home and guest: the other server's instance, as its origin
-- (https://chat.example.com), and the fingerprint of the key pinned for it
-- when the share was asked. NULL for a server on this instance.
ALTER TABLE channel_guests ADD COLUMN instance TEXT;
ALTER TABLE channel_guests ADD COLUMN instance_fingerprint TEXT;
ALTER TABLE channel_links ADD COLUMN instance TEXT;
ALTER TABLE channel_links ADD COLUMN instance_fingerprint TEXT;

-- Home: a code servers on other instances may use too, not only this one's.
ALTER TABLE share_codes ADD COLUMN other_instances INTEGER NOT NULL DEFAULT 0;
