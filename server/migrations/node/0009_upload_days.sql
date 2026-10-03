-- How many bytes of pictures each account reserved for upload, per day (UTC).
-- Every reservation adds to its account's row for the day, so uploads made
-- at once clash on it and a daily cap holds. Old days are swept hourly.
CREATE TABLE upload_days (
  account_id TEXT NOT NULL,
  day INTEGER NOT NULL,                -- unix milliseconds / 86400000
  bytes INTEGER NOT NULL,
  PRIMARY KEY (account_id, day)
);
