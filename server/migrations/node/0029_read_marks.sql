-- How far each person has read in each place (docs/read-marks.md): a server
-- channel, a thread, a direct message conversation or a secure channel. Apps
-- say so with AccountService.MarkRead and hear each other's marks on their
-- live connections, so a place read on one device stops showing unread on
-- the others. Channels and threads go by message id, conversations by
-- record sequence; a mark only ever moves forward. `at_ms` is when it last
-- moved, for connections that ask for the marks since they last heard.
CREATE TABLE read_marks (
  account_id TEXT NOT NULL,
  place TEXT NOT NULL,
  message_id TEXT NOT NULL,
  sequence INTEGER NOT NULL,
  at_ms INTEGER NOT NULL,
  PRIMARY KEY (account_id, place)
);
CREATE INDEX read_marks_by_time ON read_marks (account_id, at_ms);
