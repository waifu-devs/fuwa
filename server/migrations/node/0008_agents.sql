-- Agents: accounts programs drive, made by a person (their owner). An agent
-- has no password; its one session is its token, which never runs out until
-- it's reset.
ALTER TABLE accounts ADD COLUMN owner_id TEXT;                         -- agents: who made it
ALTER TABLE accounts ADD COLUMN public INTEGER NOT NULL DEFAULT 0;     -- agents: anyone managing a server may add it
CREATE INDEX accounts_by_owner ON accounts (owner_id);
