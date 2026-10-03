-- Voice channels came alive: everyone gets CONNECT (bit 20) and SPEAK (bit
-- 21), and whoever could kick gets MUTE_MEMBERS (bit 22) and MOVE_MEMBERS
-- (bit 23). Nothing could have named these bits before, so nobody loses a
-- choice they made.
UPDATE roles SET permissions = permissions | 1048576 | 2097152 WHERE permissions & 2048 = 2048;
UPDATE roles SET permissions = permissions | 4194304 | 8388608 WHERE permissions & 128 = 128;
