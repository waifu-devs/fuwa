-- Invite links and who may join.

-- Seconds an account must have existed to join; 0 for no minimum.
ALTER TABLE server ADD COLUMN min_account_age_seconds INTEGER NOT NULL DEFAULT 0;

CREATE TABLE invites (
  code TEXT NOT NULL PRIMARY KEY,
  channel_id TEXT,                    -- NULL for the server itself
  inviter_id TEXT NOT NULL,
  max_uses INTEGER NOT NULL DEFAULT 0, -- 0 for no limit
  uses INTEGER NOT NULL DEFAULT 0,
  expires_at INTEGER,                 -- NULL for never
  created_at INTEGER NOT NULL
);

-- Everyone could invite people before invites were a permission: anyone could
-- share a discoverable server. @everyone (position 0) gets CREATE_INVITE (bit 17).
UPDATE roles SET permissions = permissions | 131072 WHERE position = 0;
