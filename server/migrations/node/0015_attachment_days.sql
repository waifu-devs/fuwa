-- How many bytes of attachments each account reserved for upload, per day
-- (UTC), counted apart from pictures (upload_days) so each has its own cap.
CREATE TABLE attachment_days (
  account_id TEXT NOT NULL,
  day INTEGER NOT NULL,                -- unix milliseconds / 86400000
  bytes INTEGER NOT NULL,
  PRIMARY KEY (account_id, day)
);
