-- Polls: a message can be a poll (docs in proto/fuwa/v1/message.proto).

-- One row per poll, keyed by its message. `answers` is the answers as
-- protobuf (PollAnswers in api/polls.rs) and `tally` how many votes each
-- has and how many people voted (Tally). Every vote changes the tally, so
-- two votes at once clash on this row and one runs again: counts in events
-- always follow each other in order.
CREATE TABLE polls (
    message_id TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL,
    question TEXT NOT NULL,
    answers BLOB NOT NULL,
    multiple INTEGER NOT NULL DEFAULT 0,
    anonymous INTEGER NOT NULL DEFAULT 0,
    -- Unix milliseconds; NULL runs until someone ends it.
    ends_at INTEGER,
    ended_at INTEGER,
    ended_by_id TEXT,
    tally BLOB
);

CREATE INDEX polls_by_channel ON polls (channel_id);

-- Who picked what: one row per account and answer. Only ever read back for
-- the voter themselves, or for public polls.
CREATE TABLE poll_votes (
    message_id TEXT NOT NULL,
    account_id TEXT NOT NULL,
    answer_id INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (message_id, account_id, answer_id)
);

CREATE INDEX poll_votes_by_answer ON poll_votes (message_id, answer_id, account_id);

-- CREATE_POLLS (bit 27) goes to whoever could send messages (bit 12), and a
-- channel that allowed or kept someone from sending does the same for polls.
-- Nothing could have named this bit before, so nobody loses a choice they made.
UPDATE roles SET permissions = permissions | 134217728 WHERE permissions & 4096 = 4096;
UPDATE channel_overwrites SET allow = allow | 134217728 WHERE allow & 4096 = 4096;
UPDATE channel_overwrites SET deny = deny | 134217728 WHERE deny & 4096 = 4096;
