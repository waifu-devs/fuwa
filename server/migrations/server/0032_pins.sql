-- Pinned messages (MessageService.PinMessage): when a message was pinned to
-- its channel, or a reply to its thread. NULL when it isn't. Kept on the row,
-- so every read of a message carries it and deleting one drops its pin.
ALTER TABLE messages ADD COLUMN pinned_at INTEGER;

CREATE INDEX messages_pinned ON messages (channel_id, pinned_at) WHERE pinned_at IS NOT NULL;
