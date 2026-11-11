CREATE TABLE commerce_catalog (
    revision TEXT PRIMARY KEY,
    body TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE TABLE commerce_state (id INTEGER PRIMARY KEY CHECK(id=1),version INTEGER NOT NULL,revision TEXT NOT NULL REFERENCES commerce_catalog(revision));
CREATE TRIGGER commerce_catalog_update BEFORE UPDATE ON commerce_catalog BEGIN SELECT RAISE(ABORT,'catalog immutable'); END;
CREATE TRIGGER commerce_catalog_delete BEFORE DELETE ON commerce_catalog BEGIN SELECT RAISE(ABORT,'catalog immutable'); END;
CREATE TABLE commerce_records (
    kind TEXT NOT NULL CHECK(kind IN('customer','offer','order')),
    id TEXT NOT NULL,
    source TEXT NOT NULL,
    external_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY(kind,id),
    UNIQUE(kind,source,external_id)
);
CREATE TABLE commerce_receipts (
    idempotency_key TEXT PRIMARY KEY,
    request TEXT NOT NULL,
    response TEXT NOT NULL
);
CREATE TABLE commerce_events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    entity_version INTEGER NOT NULL,
    actor TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE TABLE commerce_invoices (
    order_id TEXT PRIMARY KEY,
    invoice_id TEXT NOT NULL UNIQUE
);
-- Finance allocations/refunds update invoice records directly in the same transaction.
-- These events carry the invoice ID/version; clients resolve their stored invoice reference.
CREATE TRIGGER commerce_invoice_changed AFTER UPDATE OF body ON records
WHEN NEW.kind='invoice' AND EXISTS(SELECT 1 FROM commerce_invoices WHERE invoice_id=NEW.id)
BEGIN
    INSERT INTO commerce_events(event_type,entity_id,entity_version,actor)
    VALUES('commerce.invoice.changed',NEW.id,NEW.version,'system:finance-projection');
END;
