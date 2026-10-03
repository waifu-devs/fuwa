-- Moderators can turn someone's camera and shared screen off, and it stays
-- off like server mute: with the person, not the call.
ALTER TABLE voice_moderation ADD COLUMN video_off INTEGER NOT NULL DEFAULT 0;
