-- Signing in with a code sent by email (docs/email-sign-in.md). An account's
-- address is a row in account_providers with provider 'email' and the
-- address, lowercase, as the subject: the one place fuwa keeps an email,
-- shown to its owner alone. Each code waiting to be typed is a row here,
-- kept until it's used, runs out (ten minutes) or is guessed wrong five
-- times. Neither the ticket nor the code is stored, only a hash: the code's
-- is over the ticket too, so the table alone can't be turned back into codes.
CREATE TABLE email_codes (
  ticket_hash TEXT PRIMARY KEY,
  email TEXT NOT NULL,
  code_hash TEXT NOT NULL,
  -- The account adding the address; empty for a sign-in.
  account_id TEXT NOT NULL DEFAULT '',
  tries INTEGER NOT NULL DEFAULT 0,
  expires_at INTEGER NOT NULL
);
CREATE INDEX email_codes_by_expiry ON email_codes (expires_at);
