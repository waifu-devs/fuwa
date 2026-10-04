-- Friends, friend requests and blocks (docs/friends.md). Each person keeps a
-- row about the other, so what one side does (a block, a declined request)
-- can stay on that side. `other_id` is an account here today; someone from
-- another instance would be `<id>@<host>` (docs/federation.md), so it points
-- at nothing.
CREATE TABLE friend_links (
  account_id TEXT NOT NULL,
  other_id TEXT NOT NULL,
  state INTEGER NOT NULL,             -- fuwa.v1.FriendState, from account_id's side
  created_at INTEGER NOT NULL,
  expires_at INTEGER,                 -- requests only
  PRIMARY KEY (account_id, other_id)
);

CREATE INDEX friend_links_by_other ON friend_links (other_id);
-- Requests that run out, for the hourly sweep.
CREATE INDEX friend_links_by_expiry ON friend_links (expires_at) WHERE expires_at IS NOT NULL;

-- One row per person who has sent a request, written with every new one, so
-- two requests from the same person at once clash and the second recounts
-- what's waiting.
CREATE TABLE friend_senders (
  account_id TEXT NOT NULL PRIMARY KEY,
  last_request_at INTEGER NOT NULL
);

-- Who may ask or message someone, and what their friends see. No row is
-- every default.
CREATE TABLE friend_settings (
  account_id TEXT NOT NULL PRIMARY KEY,
  requests_from INTEGER NOT NULL DEFAULT 0,        -- fuwa.v1.FriendRequestsFrom
  direct_messages_from INTEGER NOT NULL DEFAULT 0, -- fuwa.v1.DirectMessagesFrom
  hide_online INTEGER NOT NULL DEFAULT 0,
  hide_mutual_friends INTEGER NOT NULL DEFAULT 0
);
