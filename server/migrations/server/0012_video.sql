-- Cameras in voice channels: whoever could speak gets VIDEO (bit 24), and a
-- channel that allowed or kept someone from speaking does the same for
-- their camera. Nothing could have named this bit before, so nobody loses a
-- choice they made.
UPDATE roles SET permissions = permissions | 16777216 WHERE permissions & 2097152 = 2097152;
UPDATE channel_overwrites SET allow = allow | 16777216 WHERE allow & 2097152 = 2097152;
UPDATE channel_overwrites SET deny = deny | 16777216 WHERE deny & 2097152 = 2097152;
