-- Google, X and Twitch linked to accounts: any number of ways in per account.
-- Only the provider's id for the person and their name there are kept (shown
-- to them alone); never an email, a token or a picture's address.
CREATE TABLE account_providers (
  provider TEXT NOT NULL,             -- 'google', 'x', 'twitch'
  subject TEXT NOT NULL,              -- the provider's stable id for the person
  account_id TEXT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  name TEXT NOT NULL DEFAULT '',      -- their name or handle there
  linked_at INTEGER NOT NULL,
  PRIMARY KEY (provider, subject)
);
CREATE UNIQUE INDEX account_providers_one_each ON account_providers (account_id, provider);
