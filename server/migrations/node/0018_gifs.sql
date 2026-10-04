-- GIFs people can send (GifService). A GIF from the search provider is
-- stored once, however many people send it: its file is a media row owned
-- by no account ('gifs'), kept for good. A GIF someone uploaded keeps its
-- own media row and gets a row here once it's sent or saved.
CREATE TABLE gif_files (
  key TEXT PRIMARY KEY,               -- sha256 hex of where it came from
  media_id TEXT NOT NULL,             -- the stored file
  provider INTEGER NOT NULL,          -- GifProvider; 0 for an upload
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  title TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX gif_files_by_media ON gif_files (media_id);
-- Each account's saved GIFs (favorites and its own uploads).
CREATE TABLE saved_gifs (
  account_id TEXT NOT NULL,
  media_id TEXT NOT NULL,
  saved_at INTEGER NOT NULL,
  PRIMARY KEY (account_id, media_id)
);
-- Calls made to the GIF provider each day (UTC), for the instance's cap.
-- Every call adds to the day's row, so calls made at once clash on it.
CREATE TABLE gif_days (
  day INTEGER PRIMARY KEY,            -- unix milliseconds / 86400000
  calls INTEGER NOT NULL
);
