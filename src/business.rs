use crate::{conflict, domain::field, invalid, new_id, Error, Result};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Row, Sqlite, Transaction};

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomerInput {
    pub name: String,
    pub email: String,
    pub contact: String,
    pub address: String,
    pub postal_code: String,
    pub city: String,
    pub country: String,
    pub vat_id: String,
    pub notes: String,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProductInput {
    pub name: String,
    pub category: String,
    pub description: String,
    pub billing: String,
    pub unit: String,
    pub price_cents: i64,
    pub tax_basis_points: i64,
}
#[derive(Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LineInput {
    pub description: String,
    pub quantity_milli: i64,
    pub unit_price_cents: i64,
    /// Optional agreed gross unit price; net price must be its rounded VAT-exclusive value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_gross_cents: Option<i64>,
    pub tax_basis_points: i64,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuoteInput {
    pub customer_id: String,
    pub title: String,
    pub valid_until: String,
    pub notes: String,
    pub lines: Vec<LineInput>,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Organization {
    #[serde(default)]
    pub phone: String,
    pub name: String,
    pub address: String,
    pub postal_code: String,
    pub city: String,
    pub country: String,
    pub email: String,
    pub vat_id: String,
    pub tax_number: String,
    pub registration: String,
    pub iban: String,
    pub payment_terms: String,
}
fn parsed<T: serde::de::DeserializeOwned>(input: &Value) -> Result<T> {
    serde_json::from_value(input.clone())
        .map_err(|_| invalid("Felder oder Datentypen passen nicht zum Geschäftsobjekt."))
}
pub fn customer(input: &Value) -> Result<Value> {
    let _: CustomerInput = parsed(input)?;
    for (name, min, max) in [
        ("name", 1, 160),
        ("email", 0, 254),
        ("contact", 0, 160),
        ("address", 0, 500),
        ("postal_code", 0, 30),
        ("city", 0, 160),
        ("vat_id", 0, 60),
        ("notes", 0, 4000),
    ] {
        field(input, name, min, max)?;
    }
    crate::domain::email(input)?;
    country(input)?;
    Ok(input.clone())
}
fn country(input: &Value) -> Result<()> {
    let value = field(input, "country", 2, 2)?;
    if !value.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(invalid("Ländercode muss zwei Großbuchstaben enthalten."));
    }
    Ok(())
}
pub fn product(input: &Value) -> Result<Value> {
    let parsed: ProductInput = parsed(input)?;
    for (name, min, max) in [
        ("name", 1, 160),
        ("category", 1, 80),
        ("description", 0, 1000),
        ("billing", 1, 100),
        ("unit", 1, 30),
    ] {
        field(input, name, min, max)?;
    }
    if !(0..=100_000_000_000).contains(&parsed.price_cents)
        || !(0..=10000).contains(&parsed.tax_basis_points)
    {
        return Err(invalid("Preis oder Steuersatz ist ungültig."));
    }
    let mut value = input.clone();
    value["currency"] = json!("EUR");
    value["price_status"] = json!("configured");
    Ok(value)
}
pub fn organization(input: &Value) -> Result<Value> {
    let _: Organization = parsed(input)?;
    for (name, min, max) in [
        ("name", 1, 160),
        ("address", 1, 500),
        ("postal_code", 1, 30),
        ("city", 1, 160),
        ("email", 1, 254),
        ("vat_id", 0, 60),
        ("tax_number", 0, 60),
        ("registration", 0, 500),
        ("iban", 0, 40),
        ("payment_terms", 1, 1000),
    ] {
        field(input, name, min, max)?;
    }
    crate::domain::email(input)?;
    if input.get("phone").is_some() {
        field(input, "phone", 0, 100)?;
    }
    country(input)?;
    Ok(input.clone())
}
pub fn calculate(lines: &[LineInput]) -> Result<(Vec<Value>, i64, i64)> {
    if lines.is_empty() || lines.len() > 200 {
        return Err(invalid("Ein Dokument benötigt 1 bis 200 Positionen."));
    }
    let mut result = Vec::new();
    let mut net = 0i64;
    let mut tax = 0i64;
    let mut groups = std::collections::BTreeMap::<i64, i128>::new();
    for line in lines {
        if line.description.trim().is_empty()
            || line.description.chars().count() > 1000
            || !(1..=1_000_000_000).contains(&line.quantity_milli)
            || !(0..=100_000_000_000).contains(&line.unit_price_cents)
            || !(0..=10000).contains(&line.tax_basis_points)
        {
            return Err(invalid("Ungültige Dokumentposition."));
        }
        if let Some(gross) = line.unit_gross_cents {
            let divisor = 10_000 + i128::from(line.tax_basis_points);
            if !(0..=100_000_000_000).contains(&gross)
                || i128::from(line.unit_price_cents)
                    != (i128::from(gross) * 10_000 + divisor / 2) / divisor
            {
                return Err(invalid(
                    "Brutto- und Netto-Einzelpreis passen nicht zusammen.",
                ));
            }
        }
        let line_net =
            (i128::from(line.quantity_milli) * i128::from(line.unit_price_cents) + 500) / 1000;
        // VAT is rounded per rate group; allocate the cumulative difference to each line.
        let group = groups.entry(line.tax_basis_points).or_default();
        let previous_tax = (*group * i128::from(line.tax_basis_points) + 5000) / 10000;
        *group += line_net;
        let line_tax = (*group * i128::from(line.tax_basis_points) + 5000) / 10000 - previous_tax;
        if line_net + line_tax > 100_000_000_000 {
            return Err(invalid("Positionsbetrag zu groß."));
        }
        net = net
            .checked_add(line_net as i64)
            .ok_or_else(|| invalid("Betrag zu groß."))?;
        tax = tax
            .checked_add(line_tax as i64)
            .ok_or_else(|| invalid("Betrag zu groß."))?;
        if net + tax > 100_000_000_000 {
            return Err(invalid("Dokumentbetrag zu groß."));
        }
        let mut value = serde_json::to_value(line).unwrap();
        value["net_cents"] = json!(line_net as i64);
        value["tax_cents"] = json!(line_tax as i64);
        if let Some(gross) = line.unit_gross_cents {
            let payable = (i128::from(line.quantity_milli) * i128::from(gross) + 500) / 1000;
            if payable > 100_000_000_000 {
                return Err(invalid("Positionsbetrag zu groß."));
            }
            value["gross_cents"] = json!(payable as i64);
            value["rounding_cents"] = json!((payable - line_net - line_tax) as i64);
        }
        result.push(value);
    }
    let payable = net + tax + rounding_cents(&result);
    if !(0..=100_000_000_000).contains(&payable) {
        return Err(invalid("Dokumentbetrag zu groß."));
    }
    Ok((result, net, tax))
}

/// Payable rounding is derived from validated gross prices, never supplied by a caller.
pub fn rounding_cents(lines: &[Value]) -> i64 {
    lines
        .iter()
        .map(|line| line["rounding_cents"].as_i64().unwrap_or(0))
        .sum()
}

/// Keep the exact legacy shape when no gross price was agreed.
pub fn apply_rounding(body: &mut Value) {
    let lines = body["lines"].as_array().expect("calculated document lines");
    if lines.iter().any(|line| line["unit_gross_cents"].is_i64()) {
        let rounding = rounding_cents(lines);
        body["rounding_cents"] = json!(rounding);
        body["gross_cents"] = json!(
            body["net_cents"].as_i64().unwrap() + body["tax_cents"].as_i64().unwrap() + rounding
        );
    }
}
pub fn date(value: &str) -> Result<()> {
    let parts: Vec<_> = value.split('-').collect();
    if parts.len() != 3 || parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
        return Err(invalid("Datum im Format JJJJ-MM-TT erwartet."));
    }
    let year: i32 = parts[0].parse().map_err(|_| invalid("Ungültiges Datum."))?;
    let month: usize = parts[1].parse().map_err(|_| invalid("Ungültiges Datum."))?;
    let day: u32 = parts[2].parse().map_err(|_| invalid("Ungültiges Datum."))?;
    if !(1900..=9999).contains(&year) || !(1..=12).contains(&month) {
        return Err(invalid("Ungültiges Datum."));
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day == 0 || day > days[month - 1] {
        return Err(invalid("Ungültiges Datum."));
    }
    Ok(())
}
pub async fn record(tx: &mut Transaction<'_, Sqlite>, kind: &str, id: &str) -> Result<Value> {
    let row = sqlx::query("SELECT body,version,id,archived FROM records WHERE kind=? AND id=?")
        .bind(kind)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| {
            Error(
                StatusCode::NOT_FOUND,
                "not_found",
                "Geschäftsobjekt nicht gefunden.".into(),
            )
        })?;
    let mut body: Value =
        serde_json::from_str(row.get("body")).map_err(|_| invalid("Ungültiger Datensatz."))?;
    body["version"] = json!(row.get::<i64, _>("version"));
    body["id"] = json!(id);
    body["archived"] = json!(row.get::<i64, _>("archived") != 0);
    Ok(body)
}
pub fn version(input: &Value, body: &Value) -> Result<i64> {
    let current = body["version"].as_i64().unwrap_or(0);
    if input["version"].as_i64() != Some(current) {
        return Err(conflict(
            "Datensatz wurde geändert. Aktuelle Version laden.",
        ));
    }
    Ok(current)
}
pub async fn quote(tx: &mut Transaction<'_, Sqlite>, input: &Value) -> Result<Value> {
    let value: QuoteInput = parsed(input)?;
    field(input, "title", 1, 200)?;
    field(input, "notes", 0, 4000)?;
    date(&value.valid_until)?;
    let customer = record(tx, "customer", &value.customer_id).await?;
    if customer["archived"] == true {
        return Err(invalid("Kunde ist archiviert."));
    }
    let (lines, net, tax) = calculate(&value.lines)?;
    let mut body = json!({"customer_id":value.customer_id,"title":value.title,"valid_until":value.valid_until,"notes":value.notes,"lines":lines,"currency":"EUR","net_cents":net,"tax_cents":tax,"gross_cents":net+tax,"status":"draft"});
    apply_rounding(&mut body);
    Ok(body)
}
pub async fn persist_lines(
    tx: &mut Transaction<'_, Sqlite>,
    kind: &str,
    id: &str,
    body: &Value,
) -> Result<()> {
    if let Some(lines) = body["lines"].as_array() {
        sqlx::query("DELETE FROM document_lines WHERE document_kind=? AND document_id=?")
            .bind(kind)
            .bind(id)
            .execute(&mut **tx)
            .await?;
        for (index, line) in lines.iter().enumerate() {
            sqlx::query("INSERT INTO document_lines(document_kind,document_id,position,description,quantity_milli,unit_price_cents,tax_basis_points,net_cents,tax_cents) VALUES(?,?,?,?,?,?,?,?,?)")
                .bind(kind).bind(id).bind(index as i64+1).bind(line["description"].as_str().unwrap_or(""))
                .bind(line["quantity_milli"].as_i64().unwrap_or(0)).bind(line["unit_price_cents"].as_i64().unwrap_or(0)).bind(line["tax_basis_points"].as_i64().unwrap_or(0))
                .bind(line["net_cents"].as_i64().unwrap_or(0)).bind(line["tax_cents"].as_i64().unwrap_or(0)).execute(&mut **tx).await?;
        }
    }
    Ok(())
}
pub async fn number(tx: &mut Transaction<'_, Sqlite>, kind: &str, prefix: &str) -> Result<String> {
    sqlx::query("INSERT OR IGNORE INTO document_numbers(kind,next_number) VALUES(?,1)")
        .bind(kind)
        .execute(&mut **tx)
        .await?;
    let next: i64 = sqlx::query_scalar("SELECT next_number FROM document_numbers WHERE kind=?")
        .bind(kind)
        .fetch_one(&mut **tx)
        .await?;
    sqlx::query("UPDATE document_numbers SET next_number=next_number+1 WHERE kind=?")
        .bind(kind)
        .execute(&mut **tx)
        .await?;
    Ok(format!("{prefix}-{next:08}"))
}
pub async fn prepare(
    tx: &mut Transaction<'_, Sqlite>,
    command: &str,
    id: &str,
    input: &Value,
) -> Result<(Value, i64, String)> {
    let (kind, action) = command
        .split_once('.')
        .ok_or_else(|| invalid("Ungültiger Befehl."))?;
    if action == "create" {
        let mut body = match kind {
            "customer" => customer(input)?,
            "product" => product(input)?,
            "quote" => quote(tx, input).await?,
            _ => return Err(invalid("Objekt kann nicht direkt angelegt werden.")),
        };
        if body.get("status").is_none() {
            body["status"] = json!("active");
        }
        return Ok((body, 1, format!("{kind} angelegt")));
    }
    let mut body = record(tx, kind, id).await?;
    let version = version(input, &body)?;
    if body["archived"] == true && action != "restore" {
        return Err(conflict("Datensatz ist archiviert."));
    }
    match action {
        "edit" => {
            if kind == "quote" && body["status"] != "draft" {
                return Err(conflict("Ein ausgegebenes Angebot ist unveränderlich. Neues Angebot mit eigener Revision anlegen."));
            }
            body = match kind {
                "customer" => customer(&input["fields"])?,
                "product" => product(&input["fields"])?,
                "quote" => quote(tx, &input["fields"]).await?,
                "lead" | "campaign" | "creator" => {
                    let mut edited = crate::domain::validate_edit(kind, &input["fields"])?;
                    edited["status"] = body["status"].clone();
                    if let Some(inquiry) = body.get("inquiry") {
                        edited["inquiry"] = inquiry.clone();
                    }
                    edited
                }
                _ => return Err(invalid("Bearbeitung nicht unterstützt.")),
            };
            if body.get("status").is_none() {
                body["status"] = json!("active");
            }
        }
        "archive" | "restore" => {
            if !["customer", "product", "lead", "campaign", "creator"].contains(&kind) {
                return Err(invalid(
                    "Dokumente werden über ihren fachlichen Status geschlossen.",
                ));
            }
            body["archived"] = json!(action == "archive");
        }
        "send" if kind == "quote" => {
            if body["status"] != "draft" {
                return Err(conflict("Nur ein Entwurf kann ausgegeben werden."));
            }
            let organization: Option<String> =
                sqlx::query_scalar("SELECT body FROM organization WHERE id=1")
                    .fetch_optional(&mut **tx)
                    .await?;
            let organization: Value = serde_json::from_str(
                &organization.ok_or_else(|| invalid("Zuerst Unternehmensdaten einrichten."))?,
            )
            .map_err(|_| invalid("Unternehmensdaten ungültig."))?;
            let customer =
                record(tx, "customer", body["customer_id"].as_str().unwrap_or("")).await?;
            for name in ["name", "address", "postal_code", "city"] {
                field(&customer, name, 1, 500)?;
            }
            body["supplier"] = organization;
            body["customer"] = customer;
            body["number"] = json!(number(tx, "quote", "AN").await?);
            body["status"] = json!("sent");
            body["issued_at"] = json!(crate::auth::now());
            body["issue_date"] = json!(
                sqlx::query_scalar::<_, String>("SELECT date('now')")
                    .fetch_one(&mut **tx)
                    .await?
            );
        }
        "accept" if kind == "quote" => {
            if body["status"] != "sent" {
                return Err(conflict(
                    "Nur ein ausgegebenes Angebot kann angenommen werden.",
                ));
            }
            let current_date: String = sqlx::query_scalar("SELECT date('now')")
                .fetch_one(&mut **tx)
                .await?;
            if body["valid_until"].as_str().unwrap_or("") < current_date.as_str() {
                return Err(conflict("Angebot ist abgelaufen."));
            }
            let contract_id = new_id();
            let mut contract = body.clone();
            contract["status"] = json!("active");
            contract["quote_id"] = json!(id);
            contract["number"] = json!(number(tx, "contract", "VT").await?);
            for key in ["version", "id", "archived", "valid_until"] {
                contract.as_object_mut().unwrap().remove(key);
            }
            sqlx::query("INSERT INTO records(kind,id,version,body) VALUES('contract',?,1,?)")
                .bind(&contract_id)
                .bind(contract.to_string())
                .execute(&mut **tx)
                .await?;
            persist_lines(tx, "contract", &contract_id, &contract).await?;
            sqlx::query("INSERT INTO document_links(source_kind,source_id,target_kind,target_id) VALUES('quote',?,'contract',?)").bind(id).bind(&contract_id).execute(&mut **tx).await?;
            body["status"] = json!("accepted");
            body["contract_id"] = json!(contract_id);
        }
        "reject" if kind == "quote" => {
            if body["status"] != "sent" {
                return Err(conflict("Angebot ist nicht offen."));
            }
            body["status"] = json!("rejected");
        }
        "cancel" if kind == "contract" => {
            if body["status"] != "active" {
                return Err(conflict("Vertrag ist nicht aktiv."));
            }
            body["status"] = json!("cancelled");
        }
        _ => return Err(invalid("Aktion passt nicht zum Geschäftsobjekt.")),
    }
    for key in ["version", "id"] {
        body.as_object_mut().unwrap().remove(key);
    }
    Ok((body, version + 1, format!("{kind}: {action}")))
}
