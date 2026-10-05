-- Pinned records (DirectMessageService.PinRecord): only which record, by its
-- place in the conversation, and when it was pinned. What it says stays in
-- the record's ciphertext, and who pinned it isn't kept at all.
CREATE TABLE pins (
  conversation_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  pinned_at INTEGER NOT NULL,             -- unix ms
  -- Someone the pin isn't shown to: a blocker, when the person they blocked
  -- pinned it, or the blocked person, when they unpinned one the blocker
  -- keeps. Nothing a blocked person does reaches whoever blocked them.
  hidden_from TEXT,
  PRIMARY KEY (conversation_id, seq)
);
