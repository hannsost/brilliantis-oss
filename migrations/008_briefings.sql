CREATE TABLE briefings (
    id TEXT PRIMARY KEY,
    version INTEGER NOT NULL CHECK(version > 0),
    body TEXT NOT NULL CHECK(json_valid(body)),
    archived INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0,1)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX briefings_page ON briefings(archived);
CREATE TABLE briefing_editions (
    briefing_id TEXT NOT NULL REFERENCES briefings(id),
    revision INTEGER NOT NULL CHECK(revision > 0),
    snapshot TEXT NOT NULL CHECK(json_valid(snapshot)),
    text TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    actor TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY(briefing_id,revision)
);
CREATE TABLE briefing_handovers (
    briefing_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    actor TEXT NOT NULL,
    note TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY(briefing_id,revision),
    FOREIGN KEY(briefing_id,revision) REFERENCES briefing_editions(briefing_id,revision)
);
CREATE TRIGGER briefing_editions_frozen_update BEFORE UPDATE ON briefing_editions BEGIN
    SELECT RAISE(ABORT,'briefing edition immutable');
END;
CREATE TRIGGER briefing_editions_frozen_delete BEFORE DELETE ON briefing_editions BEGIN
    SELECT RAISE(ABORT,'briefing edition immutable');
END;
