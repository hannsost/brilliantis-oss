use crate::{
    business::{self, LineInput},
    conflict,
    domain::field,
    invalid, new_id, Result,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{Sqlite, Transaction};
use std::collections::BTreeMap;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InvoiceInput {
    #[serde(default)]
    buyer_reference: String,
    customer_id: String,
    title: String,
    issue_date: String,
    due_date: String,
    service_date: String,
    notes: String,
    direction: String,
    external_number: String,
    lines: Vec<LineInput>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AccountInput {
    name: String,
    iban: String,
    kind: String,
    connection_id: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransactionInput {
    account_id: String,
    external_id: String,
    booking_date: String,
    amount_cents: i64,
    description: String,
    counterparty: String,
}
fn parse<T: serde::de::DeserializeOwned>(input: &Value) -> Result<T> {
    serde_json::from_value(input.clone())
        .map_err(|_| invalid("Finanzfelder oder Datentypen ungültig."))
}

pub async fn journal(
    tx: &mut Transaction<'_, Sqlite>,
    key: &str,
    date: &str,
    description: &str,
    lines: Vec<(String, i64, i64)>,
) -> Result<String> {
    business::date(date)?;
    let closed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM closed_periods WHERE period=?")
        .bind(&date[..7])
        .fetch_one(&mut **tx)
        .await?;
    if closed > 0 {
        return Err(conflict("Die Buchungsperiode ist abgeschlossen."));
    }
    let debit: i128 = lines.iter().map(|(_, d, _)| i128::from(*d)).sum();
    let credit: i128 = lines.iter().map(|(_, _, c)| i128::from(*c)).sum();
    if debit != credit
        || debit <= 0
        || lines
            .iter()
            .any(|(_, d, c)| *d < 0 || *c < 0 || (*d > 0 && *c > 0))
    {
        return Err(invalid("Buchung muss ausgeglichen und positiv sein."));
    }
    let id = new_id();
    sqlx::query("INSERT INTO journal_entries(id,source_key,booking_date,description,currency) VALUES(?,?,?,?,'EUR')").bind(&id).bind(key).bind(date).bind(description).execute(&mut **tx).await?;
    for (position, (account, debit, credit)) in lines
        .into_iter()
        .filter(|(_, d, c)| *d > 0 || *c > 0)
        .enumerate()
    {
        sqlx::query("INSERT INTO journal_lines(entry_id,position,account,debit_cents,credit_cents) VALUES(?,?,?,?,?)").bind(&id).bind(position as i64+1).bind(account).bind(debit).bind(credit).execute(&mut **tx).await?;
    }
    sqlx::query("UPDATE journal_entries SET posted=1 WHERE id=?")
        .bind(&id)
        .execute(&mut **tx)
        .await?;
    Ok(id)
}
async fn invoice(tx: &mut Transaction<'_, Sqlite>, input: &Value) -> Result<Value> {
    let value: InvoiceInput = parse(input)?;
    if input.get("buyer_reference").is_some() {
        field(input, "buyer_reference", 0, 200)?;
    }
    if !["outgoing", "incoming"].contains(&value.direction.as_str()) {
        return Err(invalid("Rechnungsrichtung ungültig."));
    }
    business::date(&value.issue_date)?;
    business::date(&value.due_date)?;
    business::date(&value.service_date)?;
    if value.due_date < value.issue_date {
        return Err(invalid("Fälligkeit liegt vor Rechnungsdatum."));
    }
    field(input, "title", 1, 200)?;
    field(input, "notes", 0, 4000)?;
    field(
        input,
        "external_number",
        if value.direction == "incoming" { 1 } else { 0 },
        100,
    )?;
    let customer = business::record(tx, "customer", &value.customer_id).await?;
    if customer["archived"] == true {
        return Err(invalid("Geschäftspartner ist archiviert."));
    }
    let (lines, net, tax) = business::calculate(&value.lines)?;
    if net == 0 {
        return Err(invalid("Rechnungsbetrag muss positiv sein."));
    }
    let mut body = json!({"buyer_reference":value.buyer_reference,"customer_id":value.customer_id,"title":value.title,"issue_date":value.issue_date,"due_date":value.due_date,"service_date":value.service_date,"notes":value.notes,"direction":value.direction,"external_number":value.external_number,"lines":lines,"net_cents":net,"tax_cents":tax,"gross_cents":net+tax,"currency":"EUR","status":"draft"});
    business::apply_rounding(&mut body);
    Ok(body)
}
pub(crate) fn invoice_lines(body: &Value, reverse: bool) -> Vec<(String, i64, i64)> {
    let incoming = body["direction"] == "incoming";
    let mut lines = Vec::new();
    let gross = body["gross_cents"].as_i64().unwrap();
    let net = body["net_cents"].as_i64().unwrap();
    let balance = if incoming { "payables" } else { "receivables" };
    let cost = if incoming { "expense" } else { "revenue" };
    lines.push((
        format!("{balance}:{}", body["customer_id"].as_str().unwrap()),
        if incoming { 0 } else { gross },
        if incoming { gross } else { 0 },
    ));
    lines.push((
        cost.to_owned(),
        if incoming { net } else { 0 },
        if incoming { 0 } else { net },
    ));
    let mut tax = BTreeMap::<i64, i64>::new();
    for line in body["lines"].as_array().unwrap() {
        *tax.entry(line["tax_basis_points"].as_i64().unwrap())
            .or_default() += line["tax_cents"].as_i64().unwrap();
    }
    for (rate, amount) in tax {
        if amount > 0 {
            lines.push((
                format!("vat_{}:{rate}", if incoming { "input" } else { "output" }),
                if incoming { amount } else { 0 },
                if incoming { 0 } else { amount },
            ));
        }
    }
    let rounding = body["rounding_cents"].as_i64().unwrap_or(0);
    if rounding != 0 {
        let debit = (rounding > 0) == incoming;
        lines.push((
            format!("{cost}:rounding"),
            if debit { rounding.abs() } else { 0 },
            if debit { 0 } else { rounding.abs() },
        ));
    }
    if reverse {
        for (_, debit, credit) in &mut lines {
            std::mem::swap(debit, credit);
        }
    }
    lines
}
pub async fn prepare(
    tx: &mut Transaction<'_, Sqlite>,
    command: &str,
    id: &str,
    input: &Value,
) -> Result<(Value, i64, String)> {
    let (kind, action) = command
        .split_once('.')
        .ok_or_else(|| invalid("Finanzbefehl ungültig."))?;
    if action == "create" {
        return match kind {
            "invoice" => Ok((
                invoice(tx, input).await?,
                1,
                "Rechnungsentwurf angelegt".into(),
            )),
            "bank_account" => {
                let value: AccountInput = parse(input)?;
                field(input, "name", 1, 160)?;
                field(input, "iban", 0, 40)?;
                field(input, "connection_id", 0, 80)?;
                if !["bank", "psp", "cash"].contains(&value.kind.as_str()) {
                    return Err(invalid("Kontoart ungültig."));
                }
                Ok((
                    json!({"name":value.name,"iban":value.iban,"kind":value.kind,"connection_id":value.connection_id,"currency":"EUR","status":"active"}),
                    1,
                    "Finanzkonto angelegt".into(),
                ))
            }
            "bank_transaction" => {
                let value: TransactionInput = parse(input)?;
                business::date(&value.booking_date)?;
                field(input, "external_id", 1, 160)?;
                field(input, "description", 1, 1000)?;
                field(input, "counterparty", 0, 160)?;
                if value.amount_cents == 0 || value.amount_cents.unsigned_abs() > 100_000_000_000 {
                    return Err(invalid("Umsatzbetrag ungültig."));
                }
                let account = business::record(tx, "bank_account", &value.account_id).await?;
                if account["archived"] == true {
                    return Err(invalid("Finanzkonto ist archiviert."));
                }
                let duplicate: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM financial_sources WHERE account_id=? AND external_id=?",
                )
                .bind(&value.account_id)
                .bind(&value.external_id)
                .fetch_one(&mut **tx)
                .await?;
                if duplicate > 0 {
                    return Err(conflict("Kontoumsatz wurde bereits importiert."));
                }
                sqlx::query("INSERT INTO financial_sources(account_id,external_id,transaction_id) VALUES(?,?,?)").bind(&value.account_id).bind(&value.external_id).bind(id).execute(&mut **tx).await?;
                let amount = value.amount_cents.abs();
                let positive = value.amount_cents > 0;
                let entry = journal(
                    tx,
                    &format!("transaction:{id}"),
                    &value.booking_date,
                    &value.description,
                    vec![
                        (
                            format!("asset:{}", value.account_id),
                            if positive { amount } else { 0 },
                            if positive { 0 } else { amount },
                        ),
                        (
                            "clearing".into(),
                            if positive { 0 } else { amount },
                            if positive { amount } else { 0 },
                        ),
                    ],
                )
                .await?;
                Ok((
                    json!({"account_id":value.account_id,"external_id":value.external_id,"booking_date":value.booking_date,"amount_cents":value.amount_cents,"description":value.description,"counterparty":value.counterparty,"currency":"EUR","allocated_cents":0,"journal_id":entry,"status":"unallocated"}),
                    1,
                    "Kontoumsatz erfasst".into(),
                ))
            }
            _ => Err(invalid("Finanzobjekt kann nicht direkt angelegt werden.")),
        };
    }
    let mut body = business::record(tx, kind, id).await?;
    let version = business::version(input, &body)?;
    match (kind, action) {
        ("invoice", "edit") => {
            if body["status"] != "draft" {
                return Err(conflict("Ausgestellte Rechnungen sind unveränderlich."));
            }
            body = invoice(tx, &input["fields"]).await?;
        }
        ("invoice", "issue") => {
            if body["status"] != "draft" {
                return Err(conflict("Rechnung ist bereits ausgestellt."));
            }
            let organization: Option<String> =
                sqlx::query_scalar("SELECT body FROM organization WHERE id=1")
                    .fetch_optional(&mut **tx)
                    .await?;
            let organization: Value = serde_json::from_str(
                &organization.ok_or_else(|| invalid("Unternehmensdaten fehlen."))?,
            )
            .map_err(|_| invalid("Unternehmensdaten ungültig."))?;
            let partner =
                business::record(tx, "customer", body["customer_id"].as_str().unwrap()).await?;
            for key in ["name", "address", "postal_code", "city"] {
                field(&partner, key, 1, 500)?;
            }
            if body["direction"] == "incoming" {
                let duplicate: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM records WHERE kind='invoice' AND id!=? AND json_extract(body,'$.direction')='incoming' AND json_extract(body,'$.customer_id')=? AND json_extract(body,'$.external_number')=? AND json_extract(body,'$.status')!='draft'")
                    .bind(id).bind(body["customer_id"].as_str()).bind(body["external_number"].as_str()).fetch_one(&mut **tx).await?;
                if duplicate > 0 {
                    return Err(conflict("Lieferantenrechnung wurde bereits gebucht."));
                }
                body["supplier"] = partner;
                body["customer"] = organization;
            } else {
                body["supplier"] = organization;
                body["customer"] = partner;
            }
            body["number"] = json!(
                business::number(
                    tx,
                    "invoice",
                    if body["direction"] == "incoming" {
                        "ER"
                    } else {
                        "RE"
                    }
                )
                .await?
            );
            let entry = journal(
                tx,
                &format!("invoice:{id}"),
                body["issue_date"].as_str().unwrap(),
                body["title"].as_str().unwrap(),
                invoice_lines(&body, false),
            )
            .await?;
            body["journal_id"] = json!(entry);
            body["status"] = json!("issued");
            body["paid_cents"] = json!(0);
        }
        ("invoice", "cancel") => {
            if !["issued", "partially_paid", "paid"]
                .contains(&body["status"].as_str().unwrap_or(""))
            {
                return Err(conflict("Rechnung kann nicht storniert werden."));
            }
            if body["credited_cents"].as_i64().unwrap_or(0) > 0 {
                return Err(conflict("Teilweise korrigierte Provider-Rechnung über den verbleibenden Erstattungsbeleg korrigieren."));
            }
            let correction_date = field(input, "booking_date", 10, 10)?;
            business::date(&correction_date)?;
            if correction_date.as_str() < body["issue_date"].as_str().unwrap() {
                return Err(invalid("Stornodatum liegt vor dem Rechnungsdatum."));
            }
            let credit_id = new_id();
            let mut credit = body.clone();
            credit["invoice_id"] = json!(id);
            credit["original_number"] = body["number"].clone();
            credit["number"] = json!(business::number(tx, "credit_note", "ST").await?);
            credit["status"] = json!("issued");
            credit["issue_date"] = json!(correction_date);
            for key in ["id", "version", "archived", "paid_cents"] {
                credit.as_object_mut().unwrap().remove(key);
            }
            let entry = journal(
                tx,
                &format!("credit:{credit_id}"),
                credit["issue_date"].as_str().unwrap(),
                "Rechnungsstorno",
                invoice_lines(&body, true),
            )
            .await?;
            credit["journal_id"] = json!(entry);
            sqlx::query("INSERT INTO records(kind,id,version,body) VALUES('credit_note',?,1,?)")
                .bind(&credit_id)
                .bind(credit.to_string())
                .execute(&mut **tx)
                .await?;
            business::persist_lines(tx, "credit_note", &credit_id, &credit).await?;
            sqlx::query("INSERT INTO invoice_corrections(invoice_id,credit_note_id) VALUES(?,?)")
                .bind(id)
                .bind(&credit_id)
                .execute(&mut **tx)
                .await?;
            body["status"] = json!("cancelled");
            body["credit_note_id"] = json!(credit_id);
            body["refund_due_cents"] = body["paid_cents"].clone();
        }
        ("bank_transaction", "allocate") => {
            let invoice_id = field(input, "invoice_id", 1, 100)?;
            let amount = input["amount_cents"]
                .as_i64()
                .filter(|v| *v > 0)
                .ok_or_else(|| invalid("Zuordnungsbetrag fehlt."))?;
            let mut invoice = business::record(tx, "invoice", &invoice_id).await?;
            if !["issued", "partially_paid"].contains(&invoice["status"].as_str().unwrap_or("")) {
                return Err(conflict("Rechnung ist nicht offen."));
            }
            let transaction = body["amount_cents"].as_i64().unwrap();
            let incoming = invoice["direction"] == "incoming";
            if (transaction > 0) == incoming {
                return Err(invalid("Zahlungsrichtung passt nicht zur Rechnung."));
            }
            let allocated:i64=sqlx::query_scalar("SELECT COALESCE(SUM(amount_cents),0) FROM (SELECT transaction_id,amount_cents FROM payment_allocations UNION ALL SELECT transaction_id,amount_cents FROM transaction_categories UNION ALL SELECT transaction_id,amount_cents FROM payment_refunds) WHERE transaction_id=?").bind(id).fetch_one(&mut **tx).await?;
            let paid = invoice["paid_cents"].as_i64().unwrap_or(0);
            let payable = invoice["gross_cents"].as_i64().unwrap()
                - invoice["credited_cents"].as_i64().unwrap_or(0);
            if amount > transaction.abs() - allocated || amount > payable - paid {
                return Err(invalid("Zuordnung überschreitet offenen Betrag."));
            }
            let allocation = new_id();
            sqlx::query("INSERT INTO payment_allocations(id,transaction_id,invoice_id,amount_cents) VALUES(?,?,?,?)").bind(&allocation).bind(id).bind(&invoice_id).bind(amount).execute(&mut **tx).await?;
            let account = format!(
                "{}:{}",
                if incoming { "payables" } else { "receivables" },
                invoice["customer_id"].as_str().unwrap()
            );
            journal(
                tx,
                &format!("allocation:{allocation}"),
                body["booking_date"].as_str().unwrap(),
                "Zahlungszuordnung",
                vec![
                    (
                        "clearing".into(),
                        if incoming { 0 } else { amount },
                        if incoming { amount } else { 0 },
                    ),
                    (
                        account,
                        if incoming { amount } else { 0 },
                        if incoming { 0 } else { amount },
                    ),
                ],
            )
            .await?;
            invoice["paid_cents"] = json!(paid + amount);
            invoice["status"] = json!(if paid + amount == payable {
                "paid"
            } else {
                "partially_paid"
            });
            for key in ["id", "version", "archived"] {
                invoice.as_object_mut().unwrap().remove(key);
            }
            sqlx::query("UPDATE records SET version=version+1,body=?,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE kind='invoice' AND id=?").bind(invoice.to_string()).bind(&invoice_id).execute(&mut **tx).await?;
            body["allocated_cents"] = json!(allocated + amount);
            body["status"] = json!(if allocated + amount == transaction.abs() {
                "allocated"
            } else {
                "partially_allocated"
            });
        }
        ("bank_transaction", "categorize" | "refund") => {
            let total = body["amount_cents"].as_i64().unwrap();
            let allocated = body["allocated_cents"].as_i64().unwrap_or(0);
            let amount = input["amount_cents"]
                .as_i64()
                .filter(|v| *v > 0 && *v <= total.abs() - allocated)
                .ok_or_else(|| invalid("Betrag überschreitet den ungeklärten Umsatz."))?;
            let positive = total > 0;
            let assignment = new_id();
            let account = if action == "categorize" {
                let account = field(input, "account", 1, 100)?;
                if !crate::reporting::account_allowed(&account)
                    || !(account.starts_with(if positive { "revenue:" } else { "expense:" })
                        || account.starts_with("asset:")
                        || account.starts_with("liability:")
                        || account.starts_with("equity:"))
                {
                    return Err(invalid(
                        "Passendes Sachkonto für Einnahme oder Ausgabe wählen.",
                    ));
                }
                sqlx::query("INSERT INTO transaction_categories(id,transaction_id,account,amount_cents) VALUES(?,?,?,?)").bind(&assignment).bind(id).bind(&account).bind(amount).execute(&mut **tx).await?;
                account
            } else {
                let invoice_id = field(input, "invoice_id", 1, 100)?;
                let mut invoice = business::record(tx, "invoice", &invoice_id).await?;
                if (invoice["status"] != "cancelled"
                    && invoice["credited_cents"].as_i64().unwrap_or(0) == 0)
                    || (positive != (invoice["direction"] == "incoming"))
                    || amount > invoice["refund_due_cents"].as_i64().unwrap_or(0)
                {
                    return Err(invalid(
                        "Erstattung passt nicht zu Richtung oder offenem Stornoausgleich.",
                    ));
                }
                sqlx::query("INSERT INTO payment_refunds(id,transaction_id,invoice_id,amount_cents) VALUES(?,?,?,?)").bind(&assignment).bind(id).bind(&invoice_id).bind(amount).execute(&mut **tx).await?;
                invoice["refund_due_cents"] =
                    json!(invoice["refund_due_cents"].as_i64().unwrap() - amount);
                if invoice["credited_cents"].as_i64().unwrap_or(0) > 0 {
                    invoice["paid_cents"] =
                        json!(invoice["paid_cents"].as_i64().unwrap_or(0) - amount);
                    invoice["refunded_cents"] =
                        json!(invoice["refunded_cents"].as_i64().unwrap_or(0) + amount);
                }
                let account = format!(
                    "{}:{}",
                    if positive { "payables" } else { "receivables" },
                    invoice["customer_id"].as_str().unwrap()
                );
                for key in ["id", "version", "archived"] {
                    invoice.as_object_mut().unwrap().remove(key);
                }
                sqlx::query("UPDATE records SET version=version+1,body=?,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE kind='invoice' AND id=?").bind(invoice.to_string()).bind(&invoice_id).execute(&mut **tx).await?;
                account
            };
            journal(
                tx,
                &format!("{action}:{assignment}"),
                body["booking_date"].as_str().unwrap(),
                if action == "refund" {
                    "Stornoausgleich"
                } else {
                    "Sachkontenzuordnung"
                },
                vec![
                    (
                        "clearing".into(),
                        if positive { amount } else { 0 },
                        if positive { 0 } else { amount },
                    ),
                    (
                        account,
                        if positive { 0 } else { amount },
                        if positive { amount } else { 0 },
                    ),
                ],
            )
            .await?;
            body["allocated_cents"] = json!(allocated + amount);
            body["status"] = json!(if allocated + amount == total.abs() {
                "allocated"
            } else {
                "partially_allocated"
            });
        }
        _ => return Err(invalid("Finanzaktion nicht unterstützt.")),
    }
    for key in ["id", "version"] {
        body.as_object_mut().unwrap().remove(key);
    }
    Ok((body, version + 1, format!("{kind}: {action}")))
}
