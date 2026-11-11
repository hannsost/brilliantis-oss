CREATE TABLE journal_entries (
    id TEXT PRIMARY KEY,
    source_key TEXT NOT NULL UNIQUE,
    booking_date TEXT NOT NULL,
    description TEXT NOT NULL,
    currency TEXT NOT NULL CHECK(currency='EUR'),
    posted INTEGER NOT NULL DEFAULT 0 CHECK(posted IN(0,1)),
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE TABLE journal_lines (
    entry_id TEXT NOT NULL REFERENCES journal_entries(id),
    position INTEGER NOT NULL,
    account TEXT NOT NULL,
    debit_cents INTEGER NOT NULL CHECK(debit_cents>=0),
    credit_cents INTEGER NOT NULL CHECK(credit_cents>=0),
    PRIMARY KEY(entry_id,position),
    CHECK((debit_cents>0 AND credit_cents=0) OR (credit_cents>0 AND debit_cents=0))
);
CREATE TABLE closed_periods (period TEXT PRIMARY KEY CHECK(length(period)=7),closed_at INTEGER NOT NULL DEFAULT (unixepoch()),actor TEXT NOT NULL);
CREATE TRIGGER journal_balance BEFORE UPDATE OF posted ON journal_entries WHEN NEW.posted=1 BEGIN
    SELECT CASE WHEN (SELECT COUNT(*) FROM journal_lines WHERE entry_id=NEW.id)<2 OR
      (SELECT COALESCE(SUM(debit_cents-credit_cents),0) FROM journal_lines WHERE entry_id=NEW.id)!=0 THEN RAISE(ABORT,'unbalanced journal') END;
    SELECT CASE WHEN EXISTS(SELECT 1 FROM closed_periods WHERE period=substr(NEW.booking_date,1,7)) THEN RAISE(ABORT,'period closed') END;
END;
CREATE TRIGGER journal_frozen_update BEFORE UPDATE ON journal_entries WHEN OLD.posted=1 BEGIN SELECT RAISE(ABORT,'posted journal immutable'); END;
CREATE TRIGGER journal_frozen_delete BEFORE DELETE ON journal_entries WHEN OLD.posted=1 BEGIN SELECT RAISE(ABORT,'posted journal immutable'); END;
CREATE TRIGGER journal_lines_frozen_insert BEFORE INSERT ON journal_lines WHEN (SELECT posted FROM journal_entries WHERE id=NEW.entry_id)=1 BEGIN SELECT RAISE(ABORT,'posted journal immutable'); END;
CREATE TRIGGER journal_lines_frozen_update BEFORE UPDATE ON journal_lines WHEN (SELECT posted FROM journal_entries WHERE id=OLD.entry_id)=1 BEGIN SELECT RAISE(ABORT,'posted journal immutable'); END;
CREATE TRIGGER journal_lines_frozen_delete BEFORE DELETE ON journal_lines WHEN (SELECT posted FROM journal_entries WHERE id=OLD.entry_id)=1 BEGIN SELECT RAISE(ABORT,'posted journal immutable'); END;
CREATE TABLE financial_sources (account_id TEXT NOT NULL,external_id TEXT NOT NULL,transaction_id TEXT NOT NULL UNIQUE,PRIMARY KEY(account_id,external_id));
CREATE TABLE payment_allocations (id TEXT PRIMARY KEY,transaction_id TEXT NOT NULL,invoice_id TEXT NOT NULL,amount_cents INTEGER NOT NULL CHECK(amount_cents>0),created_at INTEGER NOT NULL DEFAULT(unixepoch()));
CREATE TABLE invoice_corrections (invoice_id TEXT NOT NULL UNIQUE,credit_note_id TEXT NOT NULL UNIQUE);

CREATE TRIGGER journal_draft_insert BEFORE INSERT ON journal_entries WHEN NEW.posted!=0 BEGIN SELECT RAISE(ABORT,'create journal as draft'); END;
CREATE TRIGGER journal_lines_frozen_destination BEFORE UPDATE ON journal_lines WHEN (SELECT posted FROM journal_entries WHERE id=NEW.entry_id)=1 BEGIN SELECT RAISE(ABORT,'posted journal immutable'); END;
CREATE TABLE transaction_categories (id TEXT PRIMARY KEY,transaction_id TEXT NOT NULL,account TEXT NOT NULL,amount_cents INTEGER NOT NULL CHECK(amount_cents>0));
CREATE TABLE payment_refunds (id TEXT PRIMARY KEY,transaction_id TEXT NOT NULL,invoice_id TEXT NOT NULL,amount_cents INTEGER NOT NULL CHECK(amount_cents>0));
