-- Single sign-on for this server: its managers point it at an identity
-- provider, and once it's required members sign in through it to join and
-- every sso_recheck_days to keep seeing the server.
ALTER TABLE server ADD COLUMN sso TEXT NOT NULL DEFAULT '';          -- JSON: the provider, '' for none
ALTER TABLE server ADD COLUMN sso_required INTEGER NOT NULL DEFAULT 0;
ALTER TABLE server ADD COLUMN sso_recheck_days INTEGER NOT NULL DEFAULT 30;

-- Who signed in through it, member or not yet: one provider identity per account.
CREATE TABLE sso_identities (
  user_id TEXT NOT NULL PRIMARY KEY,
  subject TEXT NOT NULL UNIQUE,       -- OIDC sub or SAML NameID
  email TEXT NOT NULL DEFAULT '',
  name TEXT NOT NULL DEFAULT '',
  signed_in_at INTEGER NOT NULL
);

-- Sign-ins on their way back from the provider (see node.db's).
CREATE TABLE sso_sign_ins (
  state TEXT NOT NULL PRIMARY KEY,
  secret_hash TEXT NOT NULL,
  return_origin TEXT NOT NULL,
  account_id TEXT NOT NULL DEFAULT '',
  test INTEGER NOT NULL DEFAULT 0,
  provider_key TEXT NOT NULL,
  verifier TEXT NOT NULL DEFAULT '',
  nonce TEXT NOT NULL DEFAULT '',
  request_id TEXT NOT NULL DEFAULT '',
  code_hash TEXT,
  identity TEXT,
  expires_at INTEGER NOT NULL
);
