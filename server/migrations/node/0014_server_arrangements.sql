-- How each person arranged their servers on the rail: order and folders, as
-- an encoded fuwa.v1.SetServerArrangementRequest (small: the API caps it).
CREATE TABLE server_arrangements (
  account_id TEXT PRIMARY KEY,
  items BLOB NOT NULL,
  updated_at INTEGER NOT NULL
);
