-- Channels shared between servers (docs/shared-channels.md). A channel's home
-- keeps the channel, its messages and who it's shared with; a guest keeps
-- only which channel of its own shows which home's channel.

-- Home: the servers each channel is shown in, or asked to be.
CREATE TABLE channel_guests (
  id TEXT NOT NULL PRIMARY KEY,         -- the connection, same id on both sides
  channel_id TEXT NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
  guest_server_id TEXT NOT NULL,
  guest_name TEXT NOT NULL,             -- the guest server as it last said
  guest_icon_url TEXT NOT NULL DEFAULT '',
  active INTEGER NOT NULL DEFAULT 0,    -- 0 while the home's admins haven't approved
  allowed INTEGER NOT NULL,             -- permission bits the guest's people may have here
  asked_by_id TEXT NOT NULL,
  approved_by_id TEXT,
  created_at INTEGER NOT NULL,
  UNIQUE (channel_id, guest_server_id)
);

-- Home: codes that let another server's admins ask for a channel.
CREATE TABLE share_codes (
  code TEXT NOT NULL PRIMARY KEY,
  channel_id TEXT NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
  creator_id TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);

-- Home: people from other servers kept out of a shared channel.
CREATE TABLE channel_blocks (
  channel_id TEXT NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
  user_id TEXT NOT NULL,
  guest_server_id TEXT NOT NULL,
  blocked_by_id TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (channel_id, user_id)
);

-- Home: which server a guest author is from. Their profile is kept in
-- `users` like a member's, so their messages show who wrote them; NULL for
-- everyone who isn't a guest here.
ALTER TABLE users ADD COLUMN guest_of TEXT;

-- Guest: the channels here that show another server's channel.
CREATE TABLE channel_links (
  id TEXT NOT NULL PRIMARY KEY,         -- the connection, same id on both sides
  channel_id TEXT UNIQUE REFERENCES channels (id) ON DELETE SET NULL, -- NULL until approved
  active INTEGER NOT NULL DEFAULT 0,    -- 0 while the home's admins haven't approved
  home_server_id TEXT NOT NULL,
  home_server_name TEXT NOT NULL,
  home_server_icon_url TEXT NOT NULL DEFAULT '',
  home_channel_id TEXT NOT NULL,
  home_channel_name TEXT NOT NULL,
  allowed INTEGER NOT NULL,
  name TEXT NOT NULL,                   -- what to call it here once approved
  parent_id TEXT,
  asked_by_id TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  UNIQUE (home_server_id, home_channel_id)
);
