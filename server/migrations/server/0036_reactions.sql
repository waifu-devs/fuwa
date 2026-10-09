-- Reactions (MessageService.React): one row per person, emoji and message.
-- `emoji` is a standard emoji's characters, or a custom emoji's id (a ULID,
-- which no standard emoji looks like); reads skip custom ones that have been
-- deleted since. Deleting a message, its channel or its thread, or the
-- person's account, deletes its rows.
CREATE TABLE reactions (
    message_id TEXT NOT NULL,
    emoji TEXT NOT NULL,
    account_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (message_id, emoji, account_id)
);

CREATE INDEX reactions_by_account ON reactions (account_id);

-- ADD_REACTIONS (bit 28) goes to whoever could send messages (bit 12), and a
-- channel that allowed or kept someone from sending does the same for
-- reactions. Nothing could have named this bit before, so nobody loses a
-- choice they made.
UPDATE roles SET permissions = permissions | 268435456 WHERE permissions & 4096 = 4096;
UPDATE channel_overwrites SET allow = allow | 268435456 WHERE allow & 4096 = 4096;
UPDATE channel_overwrites SET deny = deny | 268435456 WHERE deny & 4096 = 4096;
