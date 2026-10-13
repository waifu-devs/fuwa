-- The server's economy (docs/economy.md): its currencies, what members hold
-- and how they got it, and its shop.

-- `earn` is the rules as a protobuf (EarnRules in api/economy.rs).
CREATE TABLE currencies (
  id TEXT NOT NULL PRIMARY KEY,
  name TEXT NOT NULL,
  icon_emoji TEXT NOT NULL DEFAULT '',
  icon_emoji_id TEXT NOT NULL DEFAULT '',
  description TEXT NOT NULL DEFAULT '',
  earn BLOB,
  transferable INTEGER NOT NULL DEFAULT 0,
  position INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);

-- What someone holds, kept after they leave in case they come back.
-- `daily_day` is the UTC day (days since 1970) of their last daily claim.
CREATE TABLE currency_balances (
  currency_id TEXT NOT NULL,
  account_id TEXT NOT NULL,
  amount INTEGER NOT NULL DEFAULT 0,
  earned INTEGER NOT NULL DEFAULT 0,
  spent INTEGER NOT NULL DEFAULT 0,
  daily_day INTEGER NOT NULL DEFAULT 0,
  daily_at INTEGER NOT NULL DEFAULT 0,
  daily_streak INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (currency_id, account_id)
);
CREATE INDEX currency_balances_by_amount ON currency_balances (currency_id, amount);
CREATE INDEX currency_balances_by_account ON currency_balances (account_id);

-- What throttles earning: when someone last earned by a rule, and how much
-- on `day` (a UTC day). A row for JOIN or ONBOARDING means it was paid.
CREATE TABLE currency_earnings (
  currency_id TEXT NOT NULL,
  account_id TEXT NOT NULL,
  action INTEGER NOT NULL,
  last_at INTEGER NOT NULL,
  day INTEGER NOT NULL,
  day_total INTEGER NOT NULL,
  PRIMARY KEY (currency_id, account_id, action)
);
CREATE INDEX currency_earnings_by_account ON currency_earnings (account_id);

-- Every change to a balance.
CREATE TABLE currency_ledger (
  id TEXT NOT NULL PRIMARY KEY,
  currency_id TEXT NOT NULL,
  account_id TEXT NOT NULL,
  amount INTEGER NOT NULL,
  balance_after INTEGER NOT NULL,
  kind INTEGER NOT NULL,
  action INTEGER NOT NULL DEFAULT 0,
  actor_id TEXT NOT NULL DEFAULT '',
  counterpart_id TEXT NOT NULL DEFAULT '',
  listing_id TEXT NOT NULL DEFAULT '',
  role_id TEXT NOT NULL DEFAULT '',
  note TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL
);
CREATE INDEX currency_ledger_by_account ON currency_ledger (account_id, id);
CREATE INDEX currency_ledger_by_currency ON currency_ledger (currency_id, created_at);

-- What the shop sells. `stock` is NULL for no limit.
CREATE TABLE shop_listings (
  id TEXT NOT NULL PRIMARY KEY,
  currency_id TEXT NOT NULL,
  kind INTEGER NOT NULL,
  target_id TEXT NOT NULL DEFAULT '',
  name TEXT NOT NULL DEFAULT '',
  description TEXT NOT NULL DEFAULT '',
  price INTEGER NOT NULL,
  stock INTEGER,
  sold INTEGER NOT NULL DEFAULT 0,
  position INTEGER NOT NULL DEFAULT 0,
  available INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL
);

-- Who bought which of the server's profile items.
CREATE TABLE profile_item_owners (
  item_id TEXT NOT NULL,
  account_id TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (item_id, account_id)
);
CREATE INDEX profile_item_owners_by_account ON profile_item_owners (account_id);
