-- Voice channels came alive: everyone gets CONNECT (bit 19) and SPEAK (bit
-- 20), and whoever could kick gets MUTE_MEMBERS (bit 21) and MOVE_MEMBERS
-- (bit 22). Nothing could have named these bits before, so nobody loses a
-- choice they made.
UPDATE roles SET permissions = permissions | 524288 | 1048576 WHERE permissions & 2048 = 2048;
UPDATE roles SET permissions = permissions | 2097152 | 4194304 WHERE permissions & 128 = 128;
