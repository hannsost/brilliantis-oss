-- Legacy users.oidc_subject is deliberately not rebound to an unknown issuer.
-- Operators explicitly bind existing accounts or configure bootstrap_admin.
CREATE TABLE oidc_identities (
    issuer TEXT NOT NULL,
    subject TEXT NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY(issuer,subject),
    UNIQUE(issuer,user_id)
);
CREATE TABLE oidc_flows (
    state_hash TEXT PRIMARY KEY,
    browser_hash TEXT NOT NULL,
    purpose TEXT NOT NULL CHECK(purpose IN('login','logout')),
    issuer TEXT NOT NULL,
    client_id TEXT NOT NULL,
    redirect_uri TEXT NOT NULL,
    verifier TEXT NOT NULL,
    nonce_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX oidc_flow_expiry ON oidc_flows(expires_at);
ALTER TABLE sessions ADD COLUMN oidc_issuer TEXT;
ALTER TABLE sessions ADD COLUMN oidc_subject TEXT;
