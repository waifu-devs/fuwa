-- Single sign-on through an identity provider the admins set up (OpenID
-- Connect or SAML). SSO accounts are keyed like linked ones: linked_issuer
-- holds the provider ("oidc <issuer>" or "saml <entity ID>") and
-- linked_subject who they are there.

-- Sign-ins on their way back from the provider. The state travels in the
-- browser's address; only the app holding the secret behind secret_hash can
-- finish, with the one-time code the instance gave the browser.
CREATE TABLE sso_sign_ins (
  state TEXT NOT NULL PRIMARY KEY,
  secret_hash TEXT NOT NULL,          -- sha256 of the starting app's secret, hex
  return_origin TEXT NOT NULL,        -- the app that started it
  account_id TEXT NOT NULL DEFAULT '',
  test INTEGER NOT NULL DEFAULT 0,    -- an admin checking the provider
  provider_key TEXT NOT NULL,         -- the provider when it started
  verifier TEXT NOT NULL DEFAULT '',  -- OIDC: PKCE code verifier
  nonce TEXT NOT NULL DEFAULT '',     -- OIDC
  request_id TEXT NOT NULL DEFAULT '',-- SAML: the AuthnRequest's ID
  code_hash TEXT,                     -- once the provider answered: sha256 of the code
  identity TEXT,                      -- and who signed in, JSON
  expires_at INTEGER NOT NULL
);
