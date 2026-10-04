-- Recordings with video: the server's setting (off unless turned on), and
-- whether each recording kept cameras and screens.
ALTER TABLE server ADD COLUMN record_video INTEGER NOT NULL DEFAULT 0;
ALTER TABLE recordings ADD COLUMN video INTEGER NOT NULL DEFAULT 0;
