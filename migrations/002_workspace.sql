ALTER TABLE records RENAME TO records_v1;
CREATE TABLE records (
    kind TEXT NOT NULL CHECK(kind IN ('lead','campaign','creator','product','customer','quote','contract','invoice','credit_note','bank_account','bank_transaction','order','channel_account','content','collaboration','seo_keyword')),
    id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK(version > 0),
    body TEXT NOT NULL CHECK(json_valid(body)),
    archived INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0,1)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (kind,id)
);
INSERT INTO records(kind,id,version,body,created_at,updated_at)
SELECT kind,id,version,body,created_at,updated_at FROM records_v1;
DROP TABLE records_v1;
CREATE INDEX records_page ON records(kind,archived,id);
CREATE TABLE users (
    id TEXT PRIMARY KEY,
    email TEXT NOT NULL COLLATE NOCASE UNIQUE,
    name TEXT NOT NULL,
    password_hash TEXT,
    oidc_subject TEXT UNIQUE,
    version INTEGER NOT NULL DEFAULT 1,
    role TEXT NOT NULL CHECK(role IN ('admin','sales','accountant','viewer')),
    disabled INTEGER NOT NULL DEFAULT 0 CHECK(disabled IN (0,1)),
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    csrf TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    last_seen INTEGER NOT NULL
);
CREATE INDEX sessions_user ON sessions(user_id);
CREATE TABLE api_tokens (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    scopes TEXT NOT NULL CHECK(json_valid(scopes)),
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    revoked INTEGER NOT NULL DEFAULT 0 CHECK(revoked IN (0,1))
);
CREATE INDEX tokens_user ON api_tokens(user_id);
CREATE TABLE security_events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    actor TEXT NOT NULL,
    action TEXT NOT NULL,
    target TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE TABLE organization (
    id INTEGER PRIMARY KEY CHECK(id=1),
    version INTEGER NOT NULL CHECK(version>0),
    body TEXT NOT NULL CHECK(json_valid(body))
);
CREATE TABLE document_numbers (
    kind TEXT PRIMARY KEY,
    next_number INTEGER NOT NULL CHECK(next_number>0)
);
CREATE TABLE document_lines (
    document_kind TEXT NOT NULL,
    document_id TEXT NOT NULL,
    position INTEGER NOT NULL CHECK(position>0),
    description TEXT NOT NULL,
    quantity_milli INTEGER NOT NULL CHECK(quantity_milli>0),
    unit_price_cents INTEGER NOT NULL CHECK(unit_price_cents>=0),
    tax_basis_points INTEGER NOT NULL CHECK(tax_basis_points BETWEEN 0 AND 10000),
    net_cents INTEGER NOT NULL CHECK(net_cents>=0),
    tax_cents INTEGER NOT NULL CHECK(tax_cents>=0),
    PRIMARY KEY(document_kind,document_id,position),
    FOREIGN KEY(document_kind,document_id) REFERENCES records(kind,id)
);
CREATE TABLE document_links (
    source_kind TEXT NOT NULL,
    source_id TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_id TEXT NOT NULL,
    UNIQUE(source_kind,source_id,target_kind),
    FOREIGN KEY(source_kind,source_id) REFERENCES records(kind,id),
    FOREIGN KEY(target_kind,target_id) REFERENCES records(kind,id)
);
