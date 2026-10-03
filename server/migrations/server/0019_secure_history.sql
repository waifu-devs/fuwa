-- Secure channels can pass their earlier messages on to devices added later
-- (still end-to-end encrypted: devices share them, the server only relays).

-- Whether the channel shares history now.
ALTER TABLE secure_groups ADD COLUMN share_history INTEGER NOT NULL DEFAULT 0;
-- For SETTINGS records: what it was set to.
ALTER TABLE secure_records ADD COLUMN share_history INTEGER NOT NULL DEFAULT 0;
