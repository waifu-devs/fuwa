-- The servers the instance's admins feature in Browse, in order. A server
-- out of Browse keeps its place here but doesn't show.
CREATE TABLE featured_servers (
  server_id TEXT NOT NULL PRIMARY KEY,
  position INTEGER NOT NULL
);
