-- Boards (docs/boards.md): channels of cards in columns. A card's body is
-- the message with the card's id, and its comments the thread under it.

CREATE TABLE board_columns (
  id TEXT NOT NULL PRIMARY KEY,
  channel_id TEXT NOT NULL,
  name TEXT NOT NULL,
  color INTEGER NOT NULL DEFAULT 0,
  position INTEGER NOT NULL,
  done INTEGER NOT NULL DEFAULT 0,
  card_limit INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX board_columns_by_channel ON board_columns (channel_id, position);

CREATE TABLE board_labels (
  id TEXT NOT NULL PRIMARY KEY,
  channel_id TEXT NOT NULL,
  name TEXT NOT NULL,
  color INTEGER NOT NULL,
  position INTEGER NOT NULL
);
CREATE INDEX board_labels_by_channel ON board_labels (channel_id, position);

-- `rank` orders a column's cards (then `id`): a fractional key, so a move
-- writes only the card moved. `label_ids` and `assignee_ids` are
-- space-separated; `board_assignees` finds a person's cards.
CREATE TABLE board_cards (
  id TEXT NOT NULL PRIMARY KEY,
  channel_id TEXT NOT NULL,
  column_id TEXT NOT NULL,
  rank TEXT NOT NULL,
  title TEXT NOT NULL,
  label_ids TEXT NOT NULL DEFAULT '',
  assignee_ids TEXT NOT NULL DEFAULT '',
  due_at INTEGER NOT NULL DEFAULT 0,
  author_id TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  archived_at INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX board_cards_by_column ON board_cards (column_id, archived_at, rank, id);
CREATE INDEX board_cards_archived ON board_cards (channel_id, archived_at, id);

CREATE TABLE board_assignees (
  card_id TEXT NOT NULL,
  account_id TEXT NOT NULL,
  PRIMARY KEY (card_id, account_id)
);
CREATE INDEX board_assignees_by_account ON board_assignees (account_id);

-- Managing cards (1 << 32) goes wherever sending messages (1 << 12) did,
-- and stays off wherever a channel turned sending off.
UPDATE roles SET permissions = permissions | 4294967296 WHERE permissions & 4096 = 4096;
UPDATE channel_overwrites SET allow = allow | 4294967296 WHERE allow & 4096 = 4096;
UPDATE channel_overwrites SET deny = deny | 4294967296 WHERE deny & 4096 = 4096;
