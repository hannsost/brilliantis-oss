//! Narrow financial evidence intake. The service verifies Stripe signatures and re-reads
//! provider objects; this boundary independently checks the immutable commercial contract.
//! No browser input, caller prices, payment execution or provider credentials belong here.
use super::*;

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Period {
    pub cadence: String,
    pub start: String,
    pub end: String,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Payment {
    pub id: String,
    pub amount_cents: i64,
    pub booking_date: String,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
pub struct BillingInput {
    pub version: i64,
    pub provider: String,
    pub provider_account_id: String,
    pub provider_event_id: String,
    #[serde(flatten)]
    pub evidence: Evidence,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Evidence {
    Bind {
        customer_id: String,
        provider_customer_id: String,
        provider_subscription_id: String,
        checkout_reference: String,
        started_on: String,
    },
    Invoice {
        provider_invoice_id: String,
        provider_customer_id: String,
        provider_subscription_id: String,
        initial: bool,
        periods: Vec<Period>,
        issue_date: String,
        due_date: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        voided_on: Option<String>,
        status: String,
        currency: String,
        total_cents: i64,
        paid_cents: i64,
        payments: Vec<Payment>,
    },
    Payment {
        provider_payment_id: String,
        provider_charge_id: String,
        provider_customer_id: String,
        checkout_reference: String,
        issue_date: String,
        booking_date: String,
        currency: String,
        amount_cents: i64,
    },
    Refund {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_invoice_id: Option<String>,
        provider_customer_id: String,
        provider_subscription_id: String,
        provider_refund_id: String,
        payment_id: String,
        amount_cents: i64,
        currency: String,
        booking_date: String,
    },
    Subscription {
        provider_customer_id: String,
        provider_subscription_id: String,
        status: String,
        cancel_at_period_end: bool,
        observed_at: i64,
    },
}
fn iso_date(value: &str) -> Result<()> {
    if value.len() != 10
        || !value.bytes().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
    {
        return Err(invalid("Datum im Format YYYY-MM-DD erwartet."));
    }
    business::date(value)
}
fn operation(e: &Evidence) -> &'static str {
    match e {
        Evidence::Bind { .. } => "bind",
        Evidence::Invoice { .. } => "invoice",
        Evidence::Payment { .. } => "payment",
        Evidence::Refund { .. } => "refund",
        Evidence::Subscription { .. } => "subscription",
    }
}
fn recurring(order: &Value) -> BTreeSet<String> {
    order["billing_plan"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| {
            p["cadence"]
                .as_str()
                .filter(|c| *c != "once")
                .map(String::from)
        })
        .collect()
}
async fn binding(tx: &mut Transaction<'_, Sqlite>, order: &str) -> Result<Value> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT body FROM commerce_billing_bindings WHERE order_id=?")
            .bind(order)
            .fetch_optional(&mut **tx)
            .await?;
    decode(&raw.ok_or_else(|| conflict("Checkout-Bindung fehlt; zuerst Checkout abgleichen."))?)
}
fn matches_binding(p: &BillingInput, b: &Value, customer: &str, subscription: &str) -> Result<()> {
    if b["provider"] != p.provider
        || b["provider_account_id"] != p.provider_account_id
        || b["provider_customer_id"] != customer
        || b["provider_subscription_id"] != subscription
    {
        return Err(conflict(
            "Provider-, Kunden- oder Abonnementreferenz gehört nicht zum Auftrag.",
        ));
    }
    Ok(())
}
async fn account(
    tx: &mut Transaction<'_, Sqlite>,
    state: &AppState,
    actor: &str,
    p: &BillingInput,
) -> Result<String> {
    if let Some(id) = sqlx::query_scalar("SELECT account_id FROM commerce_billing_accounts WHERE provider=? AND provider_account_id=?")
        .bind(&p.provider).bind(&p.provider_account_id).fetch_optional(&mut **tx).await? { return Ok(id); }
    let a = core(tx, state, actor, "bank_account.create", None, json!({"name":format!("Stripe {}",p.provider_account_id),"iban":"","kind":"psp","connection_id":""})).await?;
    let id = a["id"].as_str().unwrap().to_string();
    sqlx::query("INSERT INTO commerce_billing_accounts(provider,provider_account_id,account_id) VALUES(?,?,?)")
        .bind(&p.provider).bind(&p.provider_account_id).bind(&id).execute(&mut **tx).await?;
    Ok(id)
}
// Anchor anniversaries to the original date, including Jan 31 and Feb 29. Clipping
// each period independently avoids drifting permanently after a short month.
fn anniversary(anchor: &str, months: i32) -> Result<String> {
    iso_date(anchor)?;
    let year: i32 = anchor[..4]
        .parse()
        .map_err(|_| invalid("Datum ungültig."))?;
    let month: i32 = anchor[5..7].parse().unwrap();
    let day: i32 = anchor[8..].parse().unwrap();
    let index = year * 12 + month - 1 + months;
    let y = index.div_euclid(12);
    let m = index.rem_euclid(12) + 1;
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let max = match m {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    let date = format!("{y:04}-{m:02}-{:02}", day.min(max));
    iso_date(&date)?;
    Ok(date)
}
fn check_period(period: &Period, anchor: &str, initial: bool) -> Result<()> {
    iso_date(&period.start)?;
    iso_date(&period.end)?;
    let months = (period.start[..4].parse::<i32>().unwrap() - anchor[..4].parse::<i32>().unwrap())
        * 12
        + period.start[5..7].parse::<i32>().unwrap()
        - anchor[5..7].parse::<i32>().unwrap();
    let step = match period.cadence.as_str() {
        "monthly" => 1,
        "yearly" => 12,
        _ => return Err(invalid("Abrechnungsrhythmus ungültig.")),
    };
    if months < 0
        || months % step != 0
        || initial != (months == 0)
        || anniversary(anchor, months)? != period.start
        || anniversary(anchor, months + step)? != period.end
    {
        return Err(invalid(
            "Zeitraum entspricht nicht dem vertraglichen Abrechnungsbeginn.",
        ));
    }
    Ok(())
}

pub(super) async fn apply(
    tx: &mut Transaction<'_, Sqlite>,
    state: &AppState,
    actor: &str,
    id: &str,
    input: &Value,
) -> Result<Value> {
    let p: BillingInput = parse(input)?;
    if p.provider != "stripe" {
        return Err(invalid("Provider nicht unterstützt."));
    }
    identifier(&p.provider_account_id)?;
    identifier(&p.provider_event_id)?;
    let mut order = load(tx, "order", id).await?;
    let mut semantic = input.clone();
    semantic.as_object_mut().unwrap().remove("version");
    semantic["order_id"] = json!(id);
    let semantic = semantic.to_string();
    if let Some(previous) = sqlx::query_scalar::<_,String>("SELECT request FROM commerce_billing_events WHERE provider=? AND provider_account_id=? AND provider_event_id=? AND operation=?")
        .bind(&p.provider).bind(&p.provider_account_id).bind(&p.provider_event_id).bind(operation(&p.evidence)).fetch_optional(&mut **tx).await? {
        if previous != semantic { return Err(conflict("Provider-Ereignis mit abweichenden Belegen wiederverwendet.")); }
        project(tx, &mut order).await?;
        return Ok(order);
    }
    business::version(input, &order)?;
    match &p.evidence {
        Evidence::Bind {
            customer_id,
            provider_customer_id,
            provider_subscription_id,
            checkout_reference,
            started_on,
        } => {
            identifier(provider_customer_id)?;
            identifier(checkout_reference)?;
            iso_date(started_on)?;
            if !provider_subscription_id.is_empty() {
                identifier(provider_subscription_id)?;
            }
            if order["customer_id"] != *customer_id
                || recurring(&order).is_empty() != provider_subscription_id.is_empty()
            {
                return Err(invalid(
                    "Kunde oder Abonnement passt nicht zum angenommenen Auftrag.",
                ));
            }
            let b = json!({"provider":p.provider,"provider_account_id":p.provider_account_id,"provider_customer_id":provider_customer_id,"provider_subscription_id":provider_subscription_id,"checkout_reference":checkout_reference,"started_on":started_on});
            let prior: Option<String> =
                sqlx::query_scalar("SELECT body FROM commerce_billing_bindings WHERE order_id=?")
                    .bind(id)
                    .fetch_optional(&mut **tx)
                    .await?;
            if let Some(prior) = prior {
                let mut original = decode(&prior)?;
                original.as_object_mut().unwrap().remove("subscription");
                if original != b {
                    return Err(conflict("Checkout-Bindung ist unveränderlich."));
                }
            } else {
                let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM commerce_billing_bindings WHERE provider=? AND provider_account_id=? AND (checkout_reference=? OR (provider_subscription_id!='' AND provider_subscription_id=?))")
                    .bind(&p.provider).bind(&p.provider_account_id).bind(checkout_reference).bind(provider_subscription_id).fetch_one(&mut **tx).await?;
                if exists > 0 {
                    return Err(conflict("Checkout oder Abonnement ist bereits gebunden."));
                }
                sqlx::query("INSERT INTO commerce_billing_bindings(order_id,provider,provider_account_id,provider_customer_id,provider_subscription_id,checkout_reference,started_on,body) VALUES(?,?,?,?,?,?,?,?)")
                    .bind(id).bind(&p.provider).bind(&p.provider_account_id).bind(provider_customer_id).bind(provider_subscription_id).bind(checkout_reference).bind(started_on).bind(b.to_string()).execute(&mut **tx).await?;
            }
        }
        Evidence::Invoice { .. } => invoice(tx, state, actor, &p, &order).await?,
        Evidence::Payment { .. } => capture(tx, state, actor, &p, &mut order).await?,
        Evidence::Refund { .. } => refund(tx, state, actor, &p, &order).await?,
        Evidence::Subscription {
            provider_customer_id,
            provider_subscription_id,
            status,
            cancel_at_period_end,
            observed_at,
        } => {
            let mut b = binding(tx, id).await?;
            matches_binding(&p, &b, provider_customer_id, provider_subscription_id)?;
            if provider_subscription_id.is_empty()
                || ![
                    "active",
                    "past_due",
                    "unpaid",
                    "paused",
                    "canceled",
                    "incomplete",
                    "incomplete_expired",
                    "trialing",
                ]
                .contains(&status.as_str())
                || *observed_at < 1
                || *observed_at > crate::auth::now() + 300
            {
                return Err(invalid("Abonnementzustand oder Beobachtungszeit ungültig."));
            }
            let snapshot = json!({"status":status,"cancel_at_period_end":cancel_at_period_end,"observed_at":observed_at});
            let before = b["subscription"]["observed_at"].as_i64().unwrap_or(0);
            if *observed_at == before && b["subscription"] != snapshot {
                return Err(conflict(
                    "Gleichzeitige widersprüchliche Abonnementbelege; erneut beim Provider lesen.",
                ));
            }
            if *observed_at > before {
                if b["subscription"]["status"] == "canceled" && status != "canceled" {
                    return Err(conflict(
                        "Beendetes Abonnement kann nicht reaktiviert werden.",
                    ));
                }
                b["subscription"] = snapshot;
                sqlx::query("UPDATE commerce_billing_bindings SET body=? WHERE order_id=?")
                    .bind(b.to_string())
                    .bind(id)
                    .execute(&mut **tx)
                    .await?;
            }
        }
    }
    sqlx::query("INSERT INTO commerce_billing_events(provider,provider_account_id,provider_event_id,operation,request) VALUES(?,?,?,?,?)")
        .bind(&p.provider).bind(&p.provider_account_id).bind(&p.provider_event_id).bind(operation(&p.evidence)).bind(semantic).execute(&mut **tx).await?;
    order["version"] = json!(p.version + 1);
    save(tx, "order", &order, actor, "commerce.order.billing_changed").await?;
    project(tx, &mut order).await?;
    Ok(order)
}

async fn invoice(
    tx: &mut Transaction<'_, Sqlite>,
    state: &AppState,
    actor: &str,
    p: &BillingInput,
    order: &Value,
) -> Result<()> {
    if order.get("payment_capture").is_some() {
        return Err(conflict(
            "Einmalzahlung ist bereits ohne Provider-Rechnung abgeglichen.",
        ));
    }
    let Evidence::Invoice {
        provider_invoice_id,
        provider_customer_id,
        provider_subscription_id,
        initial,
        periods,
        issue_date,
        due_date,
        voided_on,
        status,
        currency,
        total_cents,
        paid_cents,
        payments,
    } = &p.evidence
    else {
        unreachable!()
    };
    let id = order["id"].as_str().unwrap();
    let b = binding(tx, id).await?;
    matches_binding(p, &b, provider_customer_id, provider_subscription_id)?;
    identifier(provider_invoice_id)?;
    iso_date(issue_date)?;
    iso_date(due_date)?;
    if currency != "EUR"
        || !["open", "paid", "payment_failed", "void"].contains(&status.as_str())
        || due_date < issue_date
        || payments.len() > 100
        || *paid_cents < 0
        || *paid_cents > *total_cents
        || (status == "paid" && paid_cents != total_cents)
        || (status == "void" && *paid_cents != 0)
    {
        return Err(invalid(
            "Rechnungsbeleg, Währung oder Zahlungszustand ungültig.",
        ));
    }
    if status == "void" {
        let correction = voided_on.as_deref().ok_or_else(|| {
            invalid("Stornobeleg benötigt das tatsächliche Provider-Stornodatum voided_on.")
        })?;
        iso_date(correction)?;
        if correction < issue_date.as_str() {
            return Err(invalid("Stornodatum liegt vor Rechnungsdatum."));
        }
    } else if voided_on.is_some() {
        return Err(invalid("voided_on ist nur für Stornobelege zulässig."));
    }
    let recurring = recurring(order);
    let mut cadences = BTreeSet::new();
    for period in periods {
        if !recurring.contains(&period.cadence) || !cadences.insert(period.cadence.clone()) {
            return Err(invalid("Rechnungszeitraum unbekannt oder doppelt."));
        }
        check_period(period, b["started_on"].as_str().unwrap(), *initial)?;
    }
    if (*initial && cadences != recurring) || (!*initial && periods.is_empty()) {
        return Err(invalid(
            "Rechnung enthält nicht die erforderlichen Abrechnungszeiträume.",
        ));
    }
    let selected: Vec<business::LineInput> = order["billing_plan"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| {
            (l["cadence"] == "once" && *initial)
                || l["cadence"].as_str().is_some_and(|c| cadences.contains(c))
        })
        .map(|l| parse(&l["line"]))
        .collect::<Result<_>>()?;
    let (calculated, net, tax) = business::calculate(&selected)?;
    if *total_cents != net + tax + business::rounding_cents(&calculated) || *total_cents <= 0 {
        return Err(invalid(
            "Provider-Rechnungsbetrag weicht von den angenommenen Katalogpositionen ab.",
        ));
    }
    let mut payment_ids = BTreeSet::new();
    let mut paid = 0i64;
    for payment in payments {
        identifier(&payment.id)?;
        iso_date(&payment.booking_date)?;
        if !payment_ids.insert(&payment.id)
            || payment.amount_cents <= 0
            || payment.amount_cents > *total_cents
            || payment.booking_date < *issue_date
        {
            return Err(invalid(
                "Zahlungsbeleg doppelt, nicht positiv oder vor Rechnungsdatum.",
            ));
        }
        paid = paid
            .checked_add(payment.amount_cents)
            .ok_or_else(|| invalid("Zahlbetrag zu groß."))?;
    }
    if paid != *paid_cents {
        return Err(invalid(
            "Zahlbetrag ist nicht vollständig durch einzelne Provider-Zahlungen belegt.",
        ));
    }
    let snapshot = json!({"order_id":id,"initial":initial,"periods":periods,"issue_date":issue_date,"due_date":due_date,"total_cents":total_cents,"currency":currency}).to_string();
    let old = sqlx::query("SELECT invoice_id,snapshot,provider_status FROM commerce_billing_invoices WHERE provider=? AND provider_account_id=? AND provider_invoice_id=?")
        .bind(&p.provider).bind(&p.provider_account_id).bind(provider_invoice_id).fetch_optional(&mut **tx).await?;
    let invoice_id: String;
    let mut previous_status = String::new();
    if let Some(old) = old {
        if old.get::<String, _>("snapshot") != snapshot {
            return Err(conflict(
                "Provider-Rechnung mit geänderten Vertragsdaten wiederverwendet.",
            ));
        }
        invoice_id = old.get("invoice_id");
        previous_status = old.get("provider_status");
    } else {
        // Late delivery of an already completed historical period remains reconcilable.
        if b["subscription"]["status"] == "canceled" {
            let cutoff: String = sqlx::query_scalar("SELECT date(?,'unixepoch')")
                .bind(b["subscription"]["observed_at"].as_i64())
                .fetch_one(&mut **tx)
                .await?;
            if periods.iter().any(|period| period.start > cutoff) {
                return Err(conflict("Neuer Zeitraum nach beendetem Abonnement."));
            }
        }
        let duplicate: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM commerce_billing_invoices WHERE order_id=? AND initial=1",
        )
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
        if *initial && duplicate > 0 {
            return Err(conflict(
                "Erstrechnung ist bereits an einen anderen Provider-Beleg gebunden.",
            ));
        }
        for period in periods {
            let overlap: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM commerce_billing_periods WHERE order_id=? AND cadence=? AND period_start<? AND period_end>?")
                .bind(id).bind(&period.cadence).bind(&period.end).bind(&period.start).fetch_one(&mut **tx).await?;
            if overlap > 0 {
                return Err(conflict("Abrechnungszeitraum wurde bereits fakturiert."));
            }
        }
        let existing_initial: Option<String> = if *initial {
            sqlx::query_scalar("SELECT invoice_id FROM commerce_invoices WHERE order_id=?")
                .bind(id)
                .fetch_optional(&mut **tx)
                .await?
        } else {
            None
        };
        if let Some(existing) = existing_initial {
            let inv = business::record(tx, "invoice", &existing).await?;
            if inv["gross_cents"] != *total_cents
                || inv["issue_date"] != *issue_date
                || inv["due_date"] != *due_date
                || inv["customer_id"] != order["customer_id"]
                || inv["status"] == "cancelled"
            {
                return Err(conflict(
                    "Vorhandene Erstrechnung stimmt nicht mit dem Provider-Beleg überein.",
                ));
            }
            invoice_id = existing;
        } else {
            let inv = core(tx, state, actor, "invoice.create", None, json!({"customer_id":order["customer_id"],"title":order["quote"]["title"],"issue_date":issue_date,"due_date":due_date,"service_date":periods.first().map(|p|p.start.as_str()).unwrap_or(b["started_on"].as_str().unwrap()),"buyer_reference":"","notes":format!("Auftrag {id}; Provider-Rechnung {provider_invoice_id}; vertragliche Zeiträume {}.",json!(periods)),"direction":"outgoing","external_number":"","lines":selected})).await?;
            let inv = core(
                tx,
                state,
                actor,
                "invoice.issue",
                inv["id"].as_str(),
                json!({"version":inv["version"]}),
            )
            .await?;
            invoice_id = inv["id"].as_str().unwrap().to_string();
            if *initial {
                sqlx::query("INSERT INTO commerce_invoices(order_id,invoice_id) VALUES(?,?)")
                    .bind(id)
                    .bind(&invoice_id)
                    .execute(&mut **tx)
                    .await?;
            }
        }
        sqlx::query("INSERT INTO commerce_billing_invoices(provider,provider_account_id,provider_invoice_id,order_id,invoice_id,initial,snapshot,provider_status) VALUES(?,?,?,?,?,?,?,?)")
            .bind(&p.provider).bind(&p.provider_account_id).bind(provider_invoice_id).bind(id).bind(&invoice_id).bind(initial).bind(snapshot).bind(status).execute(&mut **tx).await?;
        for period in periods {
            sqlx::query("INSERT INTO commerce_billing_periods(order_id,cadence,period_start,period_end,invoice_id) VALUES(?,?,?,?,?)")
                .bind(id).bind(&period.cadence).bind(&period.start).bind(&period.end).bind(&invoice_id).execute(&mut **tx).await?;
        }
    }
    for payment in payments {
        let existing = sqlx::query("SELECT invoice_id,amount_cents,booking_date FROM commerce_billing_payments WHERE provider=? AND provider_account_id=? AND payment_id=?")
            .bind(&p.provider).bind(&p.provider_account_id).bind(&payment.id).fetch_optional(&mut **tx).await?;
        if let Some(old) = existing {
            if old.get::<String, _>("invoice_id") != invoice_id
                || old.get::<i64, _>("amount_cents") != payment.amount_cents
                || old.get::<String, _>("booking_date") != payment.booking_date
            {
                return Err(conflict(
                    "Zahlungsreferenz mit abweichendem Beleg wiederverwendet.",
                ));
            }
            continue;
        }
        let a = account(tx, state, actor, p).await?;
        let transaction = core(tx, state, actor, "bank_transaction.create", None, json!({"account_id":a,"external_id":format!("payment:{}",payment.id),"booking_date":payment.booking_date,"amount_cents":payment.amount_cents,"description":format!("Stripe Zahlung {provider_invoice_id}"),"counterparty":provider_customer_id})).await?;
        core(tx, state, actor, "bank_transaction.allocate", transaction["id"].as_str(), json!({"version":transaction["version"],"invoice_id":invoice_id,"amount_cents":payment.amount_cents})).await?;
        sqlx::query("INSERT INTO commerce_billing_payments(provider,provider_account_id,payment_id,invoice_id,transaction_id,amount_cents,booking_date) VALUES(?,?,?,?,?,?,?)")
            .bind(&p.provider).bind(&p.provider_account_id).bind(&payment.id).bind(&invoice_id).bind(transaction["id"].as_str()).bind(payment.amount_cents).bind(&payment.booking_date).execute(&mut **tx).await?;
    }
    // Old unpaid/failed events cannot undo allocated money or a final cancellation.
    let inv = business::record(tx, "invoice", &invoice_id).await?;
    let provider_paid: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(amount_cents),0) FROM commerce_billing_payments WHERE invoice_id=?",
    )
    .bind(&invoice_id)
    .fetch_one(&mut **tx)
    .await?;
    let provider_status = if previous_status == "paid" || provider_paid >= *total_cents {
        "paid"
    } else if previous_status == "void" {
        "void"
    } else {
        status.as_str()
    };
    if status == "void" && inv["status"] != "cancelled" {
        if inv["paid_cents"].as_i64().unwrap_or(0) > 0 {
            return Err(conflict(
                "Bezahlte Rechnung benötigt Erstattungsbeleg statt Stornomarkierung.",
            ));
        }
        core(
            tx,
            state,
            actor,
            "invoice.cancel",
            Some(&invoice_id),
            json!({"version":inv["version"],"booking_date":voided_on}),
        )
        .await?;
    }
    sqlx::query("UPDATE commerce_billing_invoices SET provider_status=? WHERE provider=? AND provider_account_id=? AND provider_invoice_id=?")
        .bind(provider_status).bind(&p.provider).bind(&p.provider_account_id).bind(provider_invoice_id).execute(&mut **tx).await?;
    Ok(())
}

// A once-only Checkout capture is payment evidence, not a provider invoice.
// Keep its immutable references on the order and use the existing core invoice
// and payment tables, so refunds and the journal share the same financial path.
async fn capture(
    tx: &mut Transaction<'_, Sqlite>,
    state: &AppState,
    actor: &str,
    p: &BillingInput,
    order: &mut Value,
) -> Result<()> {
    let Evidence::Payment {
        provider_payment_id,
        provider_charge_id,
        provider_customer_id,
        checkout_reference,
        issue_date,
        booking_date,
        currency,
        amount_cents,
    } = &p.evidence
    else {
        unreachable!()
    };
    let id = order["id"].as_str().unwrap().to_string();
    let b = binding(tx, &id).await?;
    matches_binding(p, &b, provider_customer_id, "")?;
    identifier(provider_payment_id)?;
    identifier(provider_charge_id)?;
    identifier(checkout_reference)?;
    iso_date(issue_date)?;
    iso_date(booking_date)?;
    if !recurring(order).is_empty()
        || !provider_payment_id.starts_with("pi_")
        || !provider_charge_id.starts_with("ch_")
        || b["checkout_reference"] != *checkout_reference
        || currency != "EUR"
        || *amount_cents <= 0
        || booking_date < issue_date
    {
        return Err(invalid(
            "Einmal-Zahlungsbeleg, Checkout, Betrag oder Datum ungültig.",
        ));
    }
    let selected: Vec<business::LineInput> = order["billing_plan"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| parse(&line["line"]))
        .collect::<Result<_>>()?;
    let (calculated, net, tax) = business::calculate(&selected)?;
    if *amount_cents != net + tax + business::rounding_cents(&calculated) {
        return Err(invalid(
            "Zahlbetrag weicht von den angenommenen Katalogpositionen ab.",
        ));
    }
    let evidence = serde_json::to_value(&p.evidence).unwrap();
    if let Some(previous) = order.get("payment_capture") {
        if previous["evidence"] != evidence {
            return Err(conflict(
                "Einmalzahlung mit abweichenden Belegen wiederverwendet.",
            ));
        }
        // A fresh observation of the same original capture never reverses refunds.
        return Ok(());
    }
    let reused: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM commerce_billing_payments WHERE provider=? AND provider_account_id=? AND payment_id=?")
        .bind(&p.provider).bind(&p.provider_account_id).bind(provider_payment_id).fetch_one(&mut **tx).await?;
    let reused_charge: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM commerce_records WHERE kind='order' AND json_extract(body,'$.payment_capture.provider')=? AND json_extract(body,'$.payment_capture.provider_account_id')=? AND json_extract(body,'$.payment_capture.evidence.provider_charge_id')=?")
        .bind(&p.provider).bind(&p.provider_account_id).bind(provider_charge_id).fetch_one(&mut **tx).await?;
    let invoiced: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM commerce_invoices WHERE order_id=?")
            .bind(&id)
            .fetch_one(&mut **tx)
            .await?;
    if reused > 0 || reused_charge > 0 || invoiced > 0 {
        return Err(conflict(
            "Zahlung, Charge oder Erstrechnung ist bereits zugeordnet.",
        ));
    }
    let inv = core(tx, state, actor, "invoice.create", None, json!({"customer_id":order["customer_id"],"title":order["quote"]["title"],"issue_date":issue_date,"due_date":issue_date,"service_date":b["started_on"],"buyer_reference":"","notes":format!("Auftrag {id}; Stripe Zahlung {provider_payment_id}; Charge {provider_charge_id}; Checkout {checkout_reference}."),"direction":"outgoing","external_number":"","lines":selected})).await?;
    let inv = core(
        tx,
        state,
        actor,
        "invoice.issue",
        inv["id"].as_str(),
        json!({"version":inv["version"]}),
    )
    .await?;
    let invoice_id = inv["id"].as_str().unwrap();
    sqlx::query("INSERT INTO commerce_invoices(order_id,invoice_id) VALUES(?,?)")
        .bind(&id)
        .bind(invoice_id)
        .execute(&mut **tx)
        .await?;
    let a = account(tx, state, actor, p).await?;
    let transaction = core(tx, state, actor, "bank_transaction.create", None, json!({"account_id":a,"external_id":format!("payment:{provider_payment_id}"),"booking_date":booking_date,"amount_cents":amount_cents,"description":format!("Stripe Zahlung {provider_payment_id}; Charge {provider_charge_id}"),"counterparty":provider_customer_id})).await?;
    core(tx, state, actor, "bank_transaction.allocate", transaction["id"].as_str(), json!({"version":transaction["version"],"invoice_id":invoice_id,"amount_cents":amount_cents})).await?;
    sqlx::query("INSERT INTO commerce_billing_payments(provider,provider_account_id,payment_id,invoice_id,transaction_id,amount_cents,booking_date) VALUES(?,?,?,?,?,?,?)")
        .bind(&p.provider).bind(&p.provider_account_id).bind(provider_payment_id).bind(invoice_id).bind(transaction["id"].as_str()).bind(amount_cents).bind(booking_date).execute(&mut **tx).await?;
    order["payment_capture"] = json!({"provider":p.provider,"provider_account_id":p.provider_account_id,"invoice_id":invoice_id,"evidence":evidence});
    Ok(())
}

async fn refund(
    tx: &mut Transaction<'_, Sqlite>,
    state: &AppState,
    actor: &str,
    p: &BillingInput,
    order: &Value,
) -> Result<()> {
    let Evidence::Refund {
        provider_invoice_id,
        provider_customer_id,
        provider_subscription_id,
        provider_refund_id,
        payment_id,
        amount_cents,
        currency,
        booking_date,
    } = &p.evidence
    else {
        unreachable!()
    };
    let b = binding(tx, order["id"].as_str().unwrap()).await?;
    matches_binding(p, &b, provider_customer_id, provider_subscription_id)?;
    if let Some(invoice_id) = provider_invoice_id {
        identifier(invoice_id)?;
    }
    identifier(provider_refund_id)?;
    identifier(payment_id)?;
    iso_date(booking_date)?;
    if currency != "EUR" || *amount_cents <= 0 {
        return Err(invalid("Erstattungsbetrag oder Währung ungültig."));
    }
    let inv_id: String = if let Some(provider_invoice_id) = provider_invoice_id {
        sqlx::query_scalar("SELECT invoice_id FROM commerce_billing_invoices WHERE provider=? AND provider_account_id=? AND provider_invoice_id=? AND order_id=?")
            .bind(&p.provider).bind(&p.provider_account_id).bind(provider_invoice_id).bind(order["id"].as_str()).fetch_optional(&mut **tx).await?.ok_or_else(||conflict("Zugehöriger Rechnungsbeleg fehlt; zuerst Rechnung abgleichen."))?
    } else {
        let capture = &order["payment_capture"];
        if capture["evidence"]["provider_payment_id"] != *payment_id {
            return Err(conflict(
                "Zugehöriger Einmal-Zahlungsbeleg fehlt; zuerst Zahlung abgleichen.",
            ));
        }
        sqlx::query_scalar("SELECT p.invoice_id FROM commerce_billing_payments p JOIN commerce_invoices i ON i.invoice_id=p.invoice_id WHERE p.provider=? AND p.provider_account_id=? AND p.payment_id=? AND i.order_id=?")
            .bind(&p.provider).bind(&p.provider_account_id).bind(payment_id).bind(order["id"].as_str()).fetch_optional(&mut **tx).await?.ok_or_else(||conflict("Zugehöriger Einmal-Zahlungsbeleg fehlt; zuerst Zahlung abgleichen."))?
    };
    if let Some(old) = sqlx::query("SELECT invoice_id,payment_id,amount_cents,booking_date FROM commerce_billing_refunds WHERE provider=? AND provider_account_id=? AND refund_id=?")
        .bind(&p.provider).bind(&p.provider_account_id).bind(provider_refund_id).fetch_optional(&mut **tx).await? {
        if old.get::<String,_>("invoice_id") != inv_id || old.get::<String,_>("payment_id") != *payment_id || old.get::<i64,_>("amount_cents") != *amount_cents || old.get::<String,_>("booking_date") != *booking_date { return Err(conflict("Erstattungsreferenz mit abweichendem Beleg wiederverwendet.")); }
        return Ok(());
    }
    let paid = sqlx::query("SELECT amount_cents,booking_date FROM commerce_billing_payments WHERE provider=? AND provider_account_id=? AND payment_id=? AND invoice_id=?")
        .bind(&p.provider).bind(&p.provider_account_id).bind(payment_id).bind(&inv_id).fetch_optional(&mut **tx).await?.ok_or_else(||conflict("Erstattung benötigt bereits abgeglichenen Zahlungsbeleg."))?;
    let refunded: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(amount_cents),0) FROM commerce_billing_refunds WHERE provider=? AND provider_account_id=? AND payment_id=?")
        .bind(&p.provider).bind(&p.provider_account_id).bind(payment_id).fetch_one(&mut **tx).await?;
    if *amount_cents > paid.get::<i64, _>("amount_cents") - refunded
        || *booking_date < paid.get::<String, _>("booking_date")
    {
        return Err(invalid(
            "Erstattung überschreitet Zahlung oder liegt vor deren Datum.",
        ));
    }
    let credit_id = credit_refund(tx, actor, &inv_id, *amount_cents, booking_date).await?;
    let a = account(tx, state, actor, p).await?;
    let transaction = core(tx, state, actor, "bank_transaction.create", None, json!({"account_id":a,"external_id":format!("refund:{provider_refund_id}"),"booking_date":booking_date,"amount_cents":-amount_cents,"description":format!("Stripe Erstattung {provider_refund_id}"),"counterparty":provider_customer_id})).await?;
    core(
        tx,
        state,
        actor,
        "bank_transaction.refund",
        transaction["id"].as_str(),
        json!({"version":transaction["version"],"invoice_id":inv_id,"amount_cents":amount_cents}),
    )
    .await?;
    sqlx::query("INSERT INTO commerce_billing_refunds(provider,provider_account_id,refund_id,invoice_id,payment_id,amount_cents,booking_date,transaction_id,credit_note_id) VALUES(?,?,?,?,?,?,?,?,?)")
        .bind(&p.provider).bind(&p.provider_account_id).bind(provider_refund_id).bind(&inv_id).bind(payment_id).bind(amount_cents).bind(booking_date).bind(transaction["id"].as_str()).bind(credit_id).execute(&mut **tx).await?;
    Ok(())
}

// Credit only the refunded fraction, grouped by original VAT rate. Cumulative
// rounding ensures a final full refund exactly reverses every original tax cent.
async fn credit_refund(
    tx: &mut Transaction<'_, Sqlite>,
    actor: &str,
    invoice_id: &str,
    amount: i64,
    date: &str,
) -> Result<String> {
    let mut inv = business::record(tx, "invoice", invoice_id).await?;
    let gross = inv["gross_cents"].as_i64().unwrap();
    let credited = inv["credited_cents"].as_i64().unwrap_or(0);
    if inv["status"] == "cancelled" {
        if amount > inv["refund_due_cents"].as_i64().unwrap_or(0) {
            return Err(invalid(
                "Erstattung überschreitet den offenen Stornoausgleich.",
            ));
        }
        let credit_id = inv["credit_note_id"].as_str().unwrap().to_string();
        // An operator may have issued the full correction before provider delivery.
        // Reuse that credit instead of reversing revenue a second time.
        if credited == 0 {
            let prior_refunds: i64 = sqlx::query_scalar(
                "SELECT COALESCE(SUM(amount_cents),0) FROM payment_refunds WHERE invoice_id=?",
            )
            .bind(invoice_id)
            .fetch_one(&mut **tx)
            .await?;
            inv["credited_cents"] = json!(gross);
            inv["paid_cents"] = json!(inv["paid_cents"].as_i64().unwrap_or(0) - prior_refunds);
            inv["refunded_cents"] = json!(prior_refunds);
            for key in ["id", "version", "archived"] {
                inv.as_object_mut().unwrap().remove(key);
            }
            sqlx::query("UPDATE records SET version=version+1,body=?,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE kind='invoice' AND id=?")
                .bind(inv.to_string()).bind(invoice_id).execute(&mut **tx).await?;
        }
        return Ok(credit_id);
    }
    if amount > gross - credited || amount > inv["paid_cents"].as_i64().unwrap_or(0) {
        return Err(invalid(
            "Erstattung überschreitet den offenen Rechnungsbetrag.",
        ));
    }
    let mut groups: BTreeMap<i64, (i64, i64)> = BTreeMap::new();
    for line in inv["lines"].as_array().unwrap() {
        let g = groups
            .entry(line["tax_basis_points"].as_i64().unwrap())
            .or_default();
        g.0 += line["net_cents"].as_i64().unwrap();
        g.1 += line["tax_cents"].as_i64().unwrap();
    }
    let mut lines = Vec::new();
    let mut net = 0i64;
    let mut tax = 0i64;
    // Allocate the actual payment between taxable consideration and its signed
    // rounding difference cumulatively, so the final refund reverses both exactly.
    let taxable_gross = inv["net_cents"].as_i64().unwrap() + inv["tax_cents"].as_i64().unwrap();
    let taxable_share = |paid: i64| -> i64 {
        ((i128::from(paid) * i128::from(taxable_gross) + i128::from(gross / 2)) / i128::from(gross))
            as i64
    };
    let taxable_before = taxable_share(credited);
    let taxable_amount = taxable_share(credited + amount) - taxable_before;
    let rounding = amount - taxable_amount;
    let mut remaining_amount = taxable_amount;
    let mut remaining_gross = taxable_gross - taxable_before;
    let mut credited_groups = inv["billing_refund_groups"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    for (rate, (gn, gt)) in groups {
        let gg = gn + gt;
        let before = credited_groups
            .get(&rate.to_string())
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let available = gg - before;
        let part = if remaining_gross == 0 {
            0
        } else {
            (i128::from(remaining_amount) * i128::from(available) / i128::from(remaining_gross))
                as i64
        };
        let after = before + part;
        let group_tax = |g: i64| -> i64 {
            if gg == 0 {
                0
            } else {
                ((i128::from(g) * i128::from(gt) + i128::from(gg / 2)) / i128::from(gg)) as i64
            }
        };
        let t = group_tax(after) - group_tax(before);
        let n = part - t;
        if part > 0 {
            lines.push(json!({"description":format!("Teilerstattung zu {} ({} Basispunkte)",inv["number"].as_str().unwrap(),rate),"quantity_milli":1000,"unit_price_cents":n,"tax_basis_points":rate,"net_cents":n,"tax_cents":t,"gross_cents":part}));
        }
        credited_groups.insert(rate.to_string(), json!(after));
        net += n;
        tax += t;
        remaining_amount -= part;
        remaining_gross -= available;
    }
    if remaining_amount != 0 {
        return Err(invalid(
            "Erstattung kann nicht auf Originalpositionen verteilt werden.",
        ));
    }
    inv["billing_refund_groups"] = json!(credited_groups);
    let mut credit = inv.clone();
    for key in [
        "id",
        "version",
        "archived",
        "paid_cents",
        "credited_cents",
        "refunded_cents",
        "refund_due_cents",
        "credit_note_id",
        "billing_refund_groups",
    ] {
        credit.as_object_mut().unwrap().remove(key);
    }
    credit["invoice_id"] = json!(invoice_id);
    credit["original_number"] = inv["number"].clone();
    credit["number"] = json!(business::number(tx, "credit_note", "ST").await?);
    credit["status"] = json!("issued");
    credit["issue_date"] = json!(date);
    credit["lines"] = json!(lines);
    credit["net_cents"] = json!(net);
    credit["tax_cents"] = json!(tax);
    credit["gross_cents"] = json!(amount);
    if inv.get("rounding_cents").is_some() {
        credit["rounding_cents"] = json!(rounding);
    }
    let credit_id = new_id();
    credit["journal_id"] = json!(
        crate::finance::journal(
            tx,
            &format!("credit:{credit_id}"),
            date,
            "Provider-Erstattung / anteilige Rechnungskorrektur",
            crate::finance::invoice_lines(&credit, true)
        )
        .await?
    );
    sqlx::query("INSERT INTO records(kind,id,version,body) VALUES('credit_note',?,1,?)")
        .bind(&credit_id)
        .bind(credit.to_string())
        .execute(&mut **tx)
        .await?;
    business::persist_lines(tx, "credit_note", &credit_id, &credit).await?;
    inv["credited_cents"] = json!(credited + amount);
    inv["refund_due_cents"] = json!(inv["refund_due_cents"].as_i64().unwrap_or(0) + amount);
    if credited + amount == gross {
        inv["status"] = json!("cancelled");
        inv["credit_note_id"] = json!(credit_id);
    }
    for key in ["id", "version", "archived"] {
        inv.as_object_mut().unwrap().remove(key);
    }
    sqlx::query("UPDATE records SET version=version+1,body=?,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE kind='invoice' AND id=?").bind(inv.to_string()).bind(invoice_id).execute(&mut **tx).await?;
    event_in(tx, "commerce.credit_note.created", &credit_id, 1, actor).await?;
    Ok(credit_id)
}

pub(super) async fn project(tx: &mut Transaction<'_, Sqlite>, order: &mut Value) -> Result<()> {
    let id = order["id"].as_str().unwrap();
    let raw: Option<String> =
        sqlx::query_scalar("SELECT body FROM commerce_billing_bindings WHERE order_id=?")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    let Some(raw) = raw else { return Ok(()) };
    let mut b = decode(&raw)?;
    b["version"] = json!(1);
    let rows = sqlx::query("SELECT provider_invoice_id,invoice_id,initial,snapshot,provider_status FROM commerce_billing_invoices WHERE order_id=? ORDER BY json_extract(snapshot,'$.issue_date'),provider_invoice_id")
        .bind(id).fetch_all(&mut **tx).await?;
    let mut invoices = Vec::new();
    for row in rows {
        let inv = business::record(tx, "invoice", row.get("invoice_id")).await?;
        let snapshot = decode(row.get("snapshot"))?;
        let mut summary = json!({"provider_invoice_id":row.get::<String,_>("provider_invoice_id"),"provider_status":row.get::<String,_>("provider_status"),"initial":row.get::<bool,_>("initial"),"periods":snapshot["periods"],"invoice_id":inv["id"],"invoice_version":inv["version"],"number":inv["number"],"status":inv["status"],"issue_date":inv["issue_date"],"gross_cents":inv["gross_cents"],"paid_cents":inv["paid_cents"],"credited_cents":inv["credited_cents"].as_i64().unwrap_or(0),"refunded_cents":inv["refunded_cents"].as_i64().unwrap_or(0),"refund_due_cents":inv["refund_due_cents"].as_i64().unwrap_or(0),"journal_id":inv["journal_id"],"currency":"EUR"});
        if let Some(rounding) = inv.get("rounding_cents") {
            summary["rounding_cents"] = rounding.clone();
        }
        invoices.push(summary);
    }
    if let Some(capture) = order.get("payment_capture") {
        let inv = business::record(tx, "invoice", capture["invoice_id"].as_str().unwrap()).await?;
        let evidence = &capture["evidence"];
        let mut summary = json!({"provider_payment_id":evidence["provider_payment_id"],"provider_charge_id":evidence["provider_charge_id"],"provider_status":"paid","initial":true,"periods":[],"invoice_id":inv["id"],"invoice_version":inv["version"],"number":inv["number"],"status":inv["status"],"issue_date":inv["issue_date"],"gross_cents":inv["gross_cents"],"paid_cents":inv["paid_cents"],"credited_cents":inv["credited_cents"].as_i64().unwrap_or(0),"refunded_cents":inv["refunded_cents"].as_i64().unwrap_or(0),"refund_due_cents":inv["refund_due_cents"].as_i64().unwrap_or(0),"journal_id":inv["journal_id"],"currency":"EUR"});
        if let Some(rounding) = inv.get("rounding_cents") {
            summary["rounding_cents"] = rounding.clone();
        }
        invoices.push(summary);
    }
    b["invoices"] = json!(invoices);
    order["billing"] = b;
    Ok(())
}
