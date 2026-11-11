CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS records (
    kind TEXT NOT NULL CHECK(kind IN ('lead','campaign','creator')),
    id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK(version > 0),
    body TEXT NOT NULL CHECK(json_valid(body)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (kind,id)
);
CREATE TABLE IF NOT EXISTS commands (
    idempotency_key TEXT PRIMARY KEY,
    request TEXT NOT NULL,
    response TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE TABLE IF NOT EXISTS events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    event_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    entity_version INTEGER NOT NULL,
    actor_label TEXT NOT NULL,
    summary TEXT NOT NULL,
    delivery TEXT NOT NULL DEFAULT 'local_only' CHECK(delivery = 'local_only'),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE TABLE IF NOT EXISTS external_refs (
    system TEXT NOT NULL,
    external_id TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    PRIMARY KEY(system, external_id)
);
