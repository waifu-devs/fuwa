-- Rules members agree to before they talk, and applications to join.

-- People apply and someone who can kick members lets them in.
ALTER TABLE server ADD COLUMN applications INTEGER NOT NULL DEFAULT 0;
-- Only accounts that sign in with waifu.dev can join or apply.
ALTER TABLE server ADD COLUMN linked_only INTEGER NOT NULL DEFAULT 0;
-- JSON: an array of rules, each a line of Markdown.
ALTER TABLE server ADD COLUMN rules TEXT NOT NULL DEFAULT '[]';
-- JSON: an array of {"prompt", "paragraph", "required"}.
ALTER TABLE server ADD COLUMN questions TEXT NOT NULL DEFAULT '[]';

-- Joined but hasn't agreed to the rules yet: can read, can't talk.
ALTER TABLE members ADD COLUMN pending INTEGER NOT NULL DEFAULT 0;

CREATE TABLE applications (
  user_id TEXT NOT NULL PRIMARY KEY REFERENCES users (id),
  answers TEXT NOT NULL,               -- JSON: an array of {"question", "answer"}
  status INTEGER NOT NULL,             -- fuwa.v1.ApplicationStatus: pending or rejected
  reason TEXT NOT NULL DEFAULT '',
  account_created_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  reviewed_by TEXT,
  reviewed_at INTEGER
);
