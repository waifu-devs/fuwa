-- Sign-ins through waifu.dev (linked accounts) on their way back. The state
-- travels in the browser's address; the PKCE verifier never leaves the
-- instance, and only the app holding the secret behind secret_hash can finish.
CREATE TABLE linked_sign_ins (
  state TEXT NOT NULL PRIMARY KEY,
  secret_hash TEXT NOT NULL,          -- sha256 of the starting app's secret, hex
  verifier TEXT NOT NULL,             -- PKCE code verifier
  issuer TEXT NOT NULL,               -- as they were when it started, in case
  client_id TEXT NOT NULL,            -- an admin changes them meanwhile
  return_origin TEXT NOT NULL,        -- the app that started it
  expires_at INTEGER NOT NULL
);
