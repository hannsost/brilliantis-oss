-- Additive provider evidence layer; existing first-invoice references remain valid.
CREATE TABLE commerce_billing_accounts (
    provider TEXT NOT NULL,
    provider_account_id TEXT NOT NULL,
    account_id TEXT NOT NULL UNIQUE,
    PRIMARY KEY(provider,provider_account_id)
);
CREATE TABLE commerce_billing_bindings (
    order_id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    provider_account_id TEXT NOT NULL,
    provider_customer_id TEXT NOT NULL,
    provider_subscription_id TEXT NOT NULL,
    checkout_reference TEXT NOT NULL,
    started_on TEXT NOT NULL,
    body TEXT NOT NULL,
    UNIQUE(provider,provider_account_id,checkout_reference)
);
CREATE UNIQUE INDEX commerce_subscription_unique ON commerce_billing_bindings(provider,provider_account_id,provider_subscription_id) WHERE provider_subscription_id!='';
CREATE TABLE commerce_billing_events (
    provider TEXT NOT NULL,
    provider_account_id TEXT NOT NULL,
    provider_event_id TEXT NOT NULL,
    operation TEXT NOT NULL,
    request TEXT NOT NULL,
    PRIMARY KEY(provider,provider_account_id,provider_event_id,operation)
);
CREATE TABLE commerce_billing_invoices (
    provider TEXT NOT NULL,
    provider_account_id TEXT NOT NULL,
    provider_invoice_id TEXT NOT NULL,
    order_id TEXT NOT NULL REFERENCES commerce_billing_bindings(order_id),
    invoice_id TEXT NOT NULL UNIQUE,
    initial INTEGER NOT NULL CHECK(initial IN(0,1)),
    snapshot TEXT NOT NULL,
    provider_status TEXT NOT NULL,
    PRIMARY KEY(provider,provider_account_id,provider_invoice_id)
);
CREATE UNIQUE INDEX commerce_billing_initial_unique ON commerce_billing_invoices(order_id) WHERE initial=1;
CREATE TABLE commerce_billing_periods (
    order_id TEXT NOT NULL,
    cadence TEXT NOT NULL CHECK(cadence IN('monthly','yearly')),
    period_start TEXT NOT NULL,
    period_end TEXT NOT NULL,
    invoice_id TEXT NOT NULL,
    PRIMARY KEY(order_id,cadence,period_start)
);
CREATE TABLE commerce_billing_payments (
    provider TEXT NOT NULL,
    provider_account_id TEXT NOT NULL,
    payment_id TEXT NOT NULL,
    invoice_id TEXT NOT NULL,
    transaction_id TEXT NOT NULL UNIQUE,
    amount_cents INTEGER NOT NULL CHECK(amount_cents>0),
    booking_date TEXT NOT NULL,
    PRIMARY KEY(provider,provider_account_id,payment_id)
);
CREATE TABLE commerce_billing_refunds (
    provider TEXT NOT NULL,
    provider_account_id TEXT NOT NULL,
    refund_id TEXT NOT NULL,
    invoice_id TEXT NOT NULL,
    payment_id TEXT NOT NULL,
    amount_cents INTEGER NOT NULL CHECK(amount_cents>0),
    booking_date TEXT NOT NULL,
    transaction_id TEXT NOT NULL UNIQUE,
    credit_note_id TEXT NOT NULL,
    PRIMARY KEY(provider,provider_account_id,refund_id)
);
CREATE TRIGGER commerce_recurring_invoice_changed AFTER UPDATE OF body ON records
WHEN NEW.kind='invoice' AND EXISTS(SELECT 1 FROM commerce_billing_invoices WHERE invoice_id=NEW.id) AND NOT EXISTS(SELECT 1 FROM commerce_invoices WHERE invoice_id=NEW.id)
BEGIN
    INSERT INTO commerce_events(event_type,entity_id,entity_version,actor)
    VALUES('commerce.invoice.changed',NEW.id,NEW.version,'system:finance-projection');
END;
