-- Gacha banners (docs/economy.md): what members pull on, their progress,
-- and every pull with its rolls.

-- Listings won only from banners, never bought.
ALTER TABLE shop_listings ADD COLUMN gacha_only INTEGER NOT NULL DEFAULT 0;

-- The banner a GACHA ledger entry is about.
ALTER TABLE currency_ledger ADD COLUMN banner_id TEXT NOT NULL DEFAULT '';

-- `banner` is the whole GachaBanner as a protobuf; the columns beside it
-- are what's looked up or sorted by.
CREATE TABLE gacha_banners (
  id TEXT NOT NULL PRIMARY KEY,
  currency_id TEXT NOT NULL,
  position INTEGER NOT NULL DEFAULT 0,
  banner BLOB NOT NULL,
  created_at INTEGER NOT NULL
);

-- Each member's pulls on a banner, and since their last pity-tier prize.
CREATE TABLE gacha_progress (
  banner_id TEXT NOT NULL,
  account_id TEXT NOT NULL,
  pulls INTEGER NOT NULL DEFAULT 0,
  since_pity INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (banner_id, account_id)
);
CREATE INDEX gacha_progress_by_account ON gacha_progress (account_id);

-- Every pull, kept after its banner is gone. `results` is the draws, as a
-- protobuf (GachaResults in api/gacha.rs).
CREATE TABLE gacha_pulls (
  id TEXT NOT NULL PRIMARY KEY,
  banner_id TEXT NOT NULL,
  banner_name TEXT NOT NULL,
  account_id TEXT NOT NULL,
  currency_id TEXT NOT NULL,
  cost INTEGER NOT NULL,
  ledger_id TEXT NOT NULL,
  results BLOB NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX gacha_pulls_by_account ON gacha_pulls (account_id, id);
CREATE INDEX gacha_pulls_by_banner ON gacha_pulls (banner_id, id);
