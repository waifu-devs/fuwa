-- Search: a word index of the server's messages, kept in this file by the
-- search indexer (src/search.rs) as it follows the event log. The requests
-- that send, edit and delete messages never write here.

-- Every word in the index, once.
CREATE TABLE search_words (
  id INTEGER PRIMARY KEY,
  word TEXT NOT NULL UNIQUE
);

-- Each message in the index. `doc` orders them by time: the millisecond it
-- was sent times 65536, plus a little to keep it unique.
CREATE TABLE search_docs (
  doc INTEGER PRIMARY KEY,
  message_id TEXT NOT NULL UNIQUE,
  channel_id TEXT NOT NULL,
  author_id TEXT NOT NULL,
  -- What it has (links, files, pictures...), as search.rs's HAS_* bits.
  has INTEGER NOT NULL DEFAULT 0,
  -- The ids of the words it's under, as varints, to take it out again.
  words BLOB NOT NULL
);

CREATE INDEX search_docs_by_channel ON search_docs (channel_id, doc);
CREATE INDEX search_docs_by_author ON search_docs (author_id, doc);

-- Which messages have each word.
CREATE TABLE search_postings (
  word INTEGER NOT NULL,
  doc INTEGER NOT NULL
);

CREATE UNIQUE INDEX search_postings_by_word ON search_postings (word, doc);

-- How far the index is: the last event it took in, and the messages from
-- before it that still have to be added (those with ids below `backfill`;
-- NULL once there are none). `version` is the way words are cut up, so a
-- new way builds the index again.
CREATE TABLE search_state (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  version INTEGER NOT NULL,
  sequence INTEGER NOT NULL,
  backfill TEXT
);

INSERT INTO search_state (id, version, sequence, backfill)
VALUES (
  1,
  1,
  (SELECT coalesce(max(sequence), 0) FROM events),
  CASE WHEN EXISTS (SELECT 1 FROM messages) THEN '~' ELSE NULL END
);
