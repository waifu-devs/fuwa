-- Roles and permissions, in place of the fixed member, admin and owner ranks.
-- members.role is no longer read: the owner is server.owner_id, and admins
-- get an Admin role the first time the server opens with this version.

-- Every server has @everyone, whose id is the server's, at position 0.
CREATE TABLE roles (
  id TEXT NOT NULL PRIMARY KEY,
  name TEXT NOT NULL,
  color INTEGER,                      -- 0xRRGGBB, NULL for none
  position INTEGER NOT NULL,          -- higher ranks above
  permissions INTEGER NOT NULL,       -- bit (1 << n) for each fuwa.v1.Permission n
  hoist INTEGER NOT NULL DEFAULT 0,
  mentionable INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

-- Who has which role, besides @everyone.
CREATE TABLE member_roles (
  user_id TEXT NOT NULL,
  role_id TEXT NOT NULL,
  PRIMARY KEY (user_id, role_id)
);

CREATE INDEX member_roles_by_role ON member_roles (role_id);

-- How each channel changes a role's or a member's permissions.
CREATE TABLE channel_overwrites (
  channel_id TEXT NOT NULL,
  target_id TEXT NOT NULL,
  target INTEGER NOT NULL,            -- fuwa.v1.OverwriteTarget
  allow INTEGER NOT NULL DEFAULT 0,
  deny INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (channel_id, target_id)
);

-- A role's name when the entry was written, since roles can be renamed or deleted.
ALTER TABLE audit ADD COLUMN role_name TEXT NOT NULL DEFAULT '';
