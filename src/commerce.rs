//! Workspace-scoped commercial authority for trusted portal and fulfillment services.
//! Every write, its core business records, event and replay receipt share one transaction.
use crate::{
    business, command_headers, conflict, domain, invalid, new_id, AppState, Error, Result,
};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Row, Sqlite, Transaction};
use std::collections::{BTreeMap, BTreeSet};

pub mod billing;
pub mod operator;

pub const CONTRACT: &str = "brilliantis.commerce.v1";

#[derive(Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Charge {
    pub description: String,
    /// once, monthly or yearly; recurring positions cover their first period on the initial invoice.
    pub cadence: String,
    pub unit_price_cents: i64,
    /// Optional agreed gross unit price. Omit to retain legacy net pricing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_gross_cents: Option<i64>,
    pub tax_basis_points: i64,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Dimension {
    pub key: String,
    pub min: i64,
    pub max: i64,
    pub step: i64,
    pub charge: Charge,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CatalogItem {
    pub id: String,
    pub name: String,
    /// hosting, saas, website, webapp, or service.
    pub kind: String,
    /// Explicit operator opt-in for catalog-priced project bundles.
    #[serde(default)]
    pub auto_quote: bool,
    pub charges: Vec<Charge>,
    pub dimensions: Vec<Dimension>,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CatalogInput {
    /// Current catalog state version, or zero before the first publication.
    pub version: i64,
    pub revision: String,
    pub items: Vec<CatalogItem>,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomerInput {
    pub source: String,
    pub external_id: String,
    pub customer: business::CustomerInput,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomerUpdate {
    pub version: i64,
    pub customer_version: i64,
    pub customer: business::CustomerInput,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfferInput {
    pub source: String,
    pub external_id: String,
    pub external_customer_id: String,
    pub catalog_revision: String,
    pub configuration_revision: String,
    pub item_id: String,
    pub quantity: i64,
    pub configuration: BTreeMap<String, i64>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AcceptInput {
    pub version: i64,
    /// Portal's durable evidence/reference of the customer's acceptance.
    pub acceptance_reference: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PriceInput {
    pub version: i64,
    pub quote_id: String,
    pub quote_version: i64,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProvisionInput {
    pub version: i64,
    pub status: String,
    pub external_reference: String,
    pub error_code: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InvoiceInput {
    pub version: i64,
    pub issue_date: String,
    pub due_date: String,
    pub service_date: String,
    #[serde(default)]
    pub buyer_reference: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Lookup {
    pub source: String,
    pub external_id: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EventsQuery {
    #[serde(default)]
    pub after: i64,
    #[serde(default = "limit")]
    pub limit: i64,
}
fn limit() -> i64 {
    100
}
fn parse<T: serde::de::DeserializeOwned>(v: &Value) -> Result<T> {
    serde_json::from_value(v.clone())
        .map_err(|_| invalid("Commerce-Felder oder Datentypen ungültig."))
}
fn identifier(s: &str) -> Result<()> {
    if s.is_empty()
        || s.len() > 120
        || !s
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(&c))
    {
        return Err(invalid("ID: 1–120 Zeichen aus Buchstaben, Zahlen, Punkt, Doppelpunkt, Unterstrich und Bindestrich."));
    }
    Ok(())
}
fn missing() -> Error {
    Error(
        StatusCode::NOT_FOUND,
        "not_found",
        "Commerce-Datensatz nicht gefunden.".into(),
    )
}
fn decode(s: &str) -> Result<Value> {
    serde_json::from_str(s).map_err(|_| invalid("Commerce-Datensatz ungültig."))
}
async fn load(tx: &mut Transaction<'_, Sqlite>, kind: &str, id: &str) -> Result<Value> {
    let raw: String = sqlx::query_scalar("SELECT body FROM commerce_records WHERE kind=? AND id=?")
        .bind(kind)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(missing)?;
    decode(&raw)
}
async fn external(
    tx: &mut Transaction<'_, Sqlite>,
    kind: &str,
    source: &str,
    id: &str,
) -> Result<Option<String>> {
    identifier(source)?;
    identifier(id)?;
    Ok(sqlx::query_scalar(
        "SELECT id FROM commerce_records WHERE kind=? AND source=? AND external_id=?",
    )
    .bind(kind)
    .bind(source)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?)
}
async fn unique(
    tx: &mut Transaction<'_, Sqlite>,
    kind: &str,
    source: &str,
    id: &str,
) -> Result<()> {
    if let Some(existing) = external(tx, kind, source, id).await? {
        return Err(conflict(&format!("Externe Referenz bereits vorhanden: {existing}. Per GET lesen oder ursprünglichen Idempotency-Key wiederholen.")));
    }
    Ok(())
}
async fn save(
    tx: &mut Transaction<'_, Sqlite>,
    kind: &str,
    body: &Value,
    actor: &str,
    event: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO commerce_records(kind,id,source,external_id,version,body) VALUES(?,?,?,?,?,?) ON CONFLICT(kind,id) DO UPDATE SET version=excluded.version,body=excluded.body")
        .bind(kind).bind(body["id"].as_str()).bind(body["source"].as_str()).bind(body["external_id"].as_str()).bind(body["version"].as_i64()).bind(body.to_string()).execute(&mut **tx).await?;
    event_in(
        tx,
        event,
        body["id"].as_str().unwrap(),
        body["version"].as_i64().unwrap(),
        actor,
    )
    .await
}
async fn event_in(
    tx: &mut Transaction<'_, Sqlite>,
    event: &str,
    id: &str,
    version: i64,
    actor: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO commerce_events(event_type,entity_id,entity_version,actor) VALUES(?,?,?,?)",
    )
    .bind(event)
    .bind(id)
    .bind(version)
    .bind(actor)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
async fn core(
    tx: &mut Transaction<'_, Sqlite>,
    state: &AppState,
    actor: &str,
    command: &str,
    id: Option<&str>,
    body: Value,
) -> Result<Value> {
    // The outer receipt owns replay. Internal receipts are unique and never caller-controlled.
    Ok(domain::execute_in(
        tx,
        &state.profile,
        &format!("commerce:{}", new_id()),
        actor,
        command,
        id,
        body,
    )
    .await?["record"]
        .clone())
}
fn check_charge(c: &Charge) -> Result<()> {
    if !["once", "monthly", "yearly"].contains(&c.cadence.as_str()) {
        return Err(invalid("Abrechnungsrhythmus ungültig."));
    }
    business::calculate(&[line(c, 1)?])?;
    Ok(())
}
fn line(c: &Charge, count: i64) -> Result<business::LineInput> {
    Ok(business::LineInput {
        description: format!("{} [{}]", c.description, c.cadence),
        quantity_milli: count
            .checked_mul(1000)
            .ok_or_else(|| invalid("Menge zu groß."))?,
        unit_price_cents: c.unit_price_cents,
        unit_gross_cents: c.unit_gross_cents,
        tax_basis_points: c.tax_basis_points,
    })
}
fn raw_lines(body: &Value) -> Result<Value> {
    let lines = body["lines"]
        .as_array()
        .ok_or_else(|| invalid("Angebotspositionen fehlen."))?;
    Ok(Value::Array(lines.iter().map(|l| {
        let mut line = json!({"description":l["description"],"quantity_milli":l["quantity_milli"],"unit_price_cents":l["unit_price_cents"],"tax_basis_points":l["tax_basis_points"]});
        if let Some(gross) = l.get("unit_gross_cents") {
            line["unit_gross_cents"] = gross.clone();
        }
        line
    }).collect()))
}
async fn catalog_in(tx: &mut Transaction<'_, Sqlite>, input: &Value, actor: &str) -> Result<Value> {
    let p: CatalogInput = parse(input)?;
    identifier(&p.revision)?;
    let version: i64 =
        sqlx::query_scalar("SELECT COALESCE((SELECT version FROM commerce_state WHERE id=1),0)")
            .fetch_one(&mut **tx)
            .await?;
    if version != p.version {
        return Err(conflict("Katalogversion geändert."));
    }
    if p.items.is_empty() || p.items.len() > 100 {
        return Err(invalid("Katalog benötigt 1–100 Produkte."));
    }
    let mut ids = BTreeSet::new();
    for item in &p.items {
        identifier(&item.id)?;
        if !ids.insert(&item.id)
            || item.name.trim().is_empty()
            || item.name.len() > 180
            || !["hosting", "saas", "website", "webapp", "service"].contains(&item.kind.as_str())
        {
            return Err(invalid("Katalogprodukt ungültig oder doppelt."));
        }
        let project =
            ["website", "webapp", "service"].contains(&item.kind.as_str()) && !item.auto_quote;
        if (project && (!item.charges.is_empty() || !item.dimensions.is_empty()))
            || (!project && item.charges.is_empty() && item.dimensions.is_empty())
            || item.charges.len() + item.dimensions.len() > 100
        {
            return Err(invalid(
                "Hosting/SaaS benötigen Preise; Projektleistungen werden individuell angeboten.",
            ));
        }
        for c in &item.charges {
            check_charge(c)?;
        }
        let mut keys = BTreeSet::new();
        for d in &item.dimensions {
            identifier(&d.key)?;
            check_charge(&d.charge)?;
            if !keys.insert(&d.key) || d.min < 0 || d.max < d.min || d.max > 100000 || d.step < 1 {
                return Err(invalid("Konfigurationsdimension ungültig."));
            }
        }
    }
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM commerce_catalog WHERE revision=?")
        .bind(&p.revision)
        .fetch_one(&mut **tx)
        .await?;
    if exists > 0 {
        return Err(conflict(
            "Katalogrevision ist unveränderlich; neue Revision verwenden.",
        ));
    }
    let response = json!({"contract":CONTRACT,"version":version+1,"revision":p.revision,"currency":"EUR","items":p.items});
    sqlx::query("INSERT INTO commerce_catalog(revision,body) VALUES(?,?)")
        .bind(&p.revision)
        .bind(response.to_string())
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO commerce_state(id,version,revision) VALUES(1,?,?) ON CONFLICT(id) DO UPDATE SET version=excluded.version,revision=excluded.revision").bind(version+1).bind(&p.revision).execute(&mut **tx).await?;
    event_in(
        tx,
        "commerce.catalog.published",
        &p.revision,
        version + 1,
        actor,
    )
    .await?;
    Ok(response)
}
async fn customer_in(
    tx: &mut Transaction<'_, Sqlite>,
    state: &AppState,
    actor: &str,
    input: &Value,
) -> Result<Value> {
    let p: CustomerInput = parse(input)?;
    unique(tx, "customer", &p.source, &p.external_id).await?;
    let c = core(
        tx,
        state,
        actor,
        "customer.create",
        None,
        serde_json::to_value(p.customer).unwrap(),
    )
    .await?;
    let body = json!({"contract":CONTRACT,"id":new_id(),"version":1,"source":p.source,"external_id":p.external_id,"customer_id":c["id"]});
    save(tx, "customer", &body, actor, "commerce.customer.registered").await?;
    Ok(body)
}
async fn customer_update_in(
    tx: &mut Transaction<'_, Sqlite>,
    state: &AppState,
    actor: &str,
    id: &str,
    input: &Value,
) -> Result<Value> {
    let p: CustomerUpdate = parse(input)?;
    let mut body = load(tx, "customer", id).await?;
    business::version(input, &body)?;
    let customer = core(
        tx,
        state,
        actor,
        "customer.edit",
        body["customer_id"].as_str(),
        json!({"version":p.customer_version,"fields":p.customer}),
    )
    .await?;
    body["version"] = json!(p.version + 1);
    save(tx, "customer", &body, actor, "commerce.customer.updated").await?;
    body["customer_version"] = customer["version"].clone();
    body["customer"] = customer;
    Ok(body)
}
async fn offer_in(
    tx: &mut Transaction<'_, Sqlite>,
    state: &AppState,
    actor: &str,
    input: &Value,
) -> Result<Value> {
    let p: OfferInput = parse(input)?;
    unique(tx, "offer", &p.source, &p.external_id).await?;
    identifier(&p.configuration_revision)?;
    if !(1..=1000).contains(&p.quantity) {
        return Err(invalid("Menge: 1–1000 erwartet."));
    }
    let customer_ref = external(tx, "customer", &p.source, &p.external_customer_id)
        .await?
        .ok_or_else(missing)?;
    let customer = load(tx, "customer", &customer_ref).await?;
    let raw: Option<String>=sqlx::query_scalar("SELECT c.body FROM commerce_catalog c JOIN commerce_state s ON c.revision=s.revision WHERE c.revision=?").bind(&p.catalog_revision).fetch_optional(&mut **tx).await?;
    let catalog =
        decode(&raw.ok_or_else(|| conflict("Katalogrevision nicht aktuell. Katalog neu laden."))?)?;
    let item: CatalogItem = parse(
        catalog["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == p.item_id)
            .ok_or_else(|| invalid("Katalogprodukt unbekannt."))?,
    )?;
    if p.configuration.len() != item.dimensions.len() {
        return Err(invalid(
            "Konfiguration muss genau die Katalogdimensionen enthalten.",
        ));
    }
    let mut lines = Vec::new();
    let mut plan = Vec::new();
    for c in &item.charges {
        lines.push(line(c, p.quantity)?);
        plan.push(json!({"cadence":c.cadence,"line":lines.last()}));
    }
    for d in &item.dimensions {
        let n = *p
            .configuration
            .get(&d.key)
            .ok_or_else(|| invalid("Konfigurationsdimension fehlt."))?;
        if n < d.min || n > d.max || (n - d.min) % d.step != 0 {
            return Err(invalid("Konfigurationswert außerhalb der Kataloggrenzen."));
        }
        if n > 0 {
            lines.push(line(&d.charge, n * p.quantity)?);
            plan.push(json!({"cadence":d.charge.cadence,"dimension":d.key,"line":lines.last()}));
        }
    }
    let project =
        ["website", "webapp", "service"].contains(&item.kind.as_str()) && !item.auto_quote;
    let mut body = json!({"contract":CONTRACT,"id":new_id(),"version":1,"source":p.source,"external_id":p.external_id,"external_customer_id":p.external_customer_id,"customer_id":customer["customer_id"],"catalog_revision":p.catalog_revision,"configuration_revision":p.configuration_revision,"item_id":p.item_id,"kind":item.kind,"quantity":p.quantity,"configuration":p.configuration,"status":"quote_requested","billing_plan":plan});
    if !project {
        let (_, net, _) = business::calculate(&lines)?;
        if net == 0 {
            return Err(invalid(
                "Konfiguration benötigt einen positiven Gesamtpreis.",
            ));
        }
        let valid: String = sqlx::query_scalar("SELECT date('now','+30 days')")
            .fetch_one(&mut **tx)
            .await?;
        let q=core(tx,state,actor,"quote.create",None,json!({"customer_id":customer["customer_id"],"title":item.name,"valid_until":valid,"notes":"Einmalleistungen und erster Abrechnungszeitraum. Folgezeiträume gemäß billing_plan des Auftrags.","lines":lines})).await?;
        let q = core(
            tx,
            state,
            actor,
            "quote.send",
            q["id"].as_str(),
            json!({"version":q["version"]}),
        )
        .await?;
        body["status"] = json!("offered");
        body["quote"] = q;
    }
    save(
        tx,
        "offer",
        &body,
        actor,
        if project {
            "commerce.offer.requested"
        } else {
            "commerce.offer.priced"
        },
    )
    .await?;
    Ok(body)
}
async fn price_in(
    tx: &mut Transaction<'_, Sqlite>,
    actor: &str,
    id: &str,
    input: &Value,
) -> Result<Value> {
    let p: PriceInput = parse(input)?;
    let mut body = load(tx, "offer", id).await?;
    business::version(input, &body)?;
    if body["status"] != "quote_requested" {
        return Err(conflict("Anfrage hat bereits ein Angebot."));
    }
    let q = business::record(tx, "quote", &p.quote_id).await?;
    if q["version"] != p.quote_version
        || q["status"] != "sent"
        || q["customer_id"] != body["customer_id"]
    {
        return Err(conflict(
            "Aktuelles ausgegebenes Angebot für denselben Kunden erforderlich.",
        ));
    }
    let reused: i64=sqlx::query_scalar("SELECT COUNT(*) FROM commerce_records WHERE kind='offer' AND json_extract(body,'$.quote.id')=?").bind(&p.quote_id).fetch_one(&mut **tx).await?;
    if reused > 0 {
        return Err(conflict("Angebot ist bereits einer Anfrage zugeordnet."));
    }
    body["billing_plan"] = json!(raw_lines(&q)?
        .as_array()
        .unwrap()
        .iter()
        .map(|l| json!({"cadence":"once","line":l}))
        .collect::<Vec<_>>());
    body["quote"] = q;
    body["version"] = json!(p.version + 1);
    body["status"] = json!("offered");
    save(tx, "offer", &body, actor, "commerce.offer.priced").await?;
    Ok(body)
}
async fn accept_in(
    tx: &mut Transaction<'_, Sqlite>,
    state: &AppState,
    actor: &str,
    id: &str,
    input: &Value,
) -> Result<Value> {
    let p: AcceptInput = parse(input)?;
    domain::field(input, "acceptance_reference", 1, 500)?;
    let mut offer = load(tx, "offer", id).await?;
    business::version(input, &offer)?;
    if offer["status"] != "offered" {
        return Err(conflict(
            "Angebot nicht zur Annahme offen. Aktuellen Status laden.",
        ));
    }
    let q = core(
        tx,
        state,
        actor,
        "quote.accept",
        offer["quote"]["id"].as_str(),
        json!({"version":offer["quote"]["version"]}),
    )
    .await?;
    let today: String = sqlx::query_scalar("SELECT date('now')")
        .fetch_one(&mut **tx)
        .await?;
    let order=core(tx,state,actor,"order.create",None,json!({"name":q["title"],"customer_id":offer["customer_id"],"source":"commerce","external_id":id,"ordered_date":today,"notes":format!("Commerce-Angebot {id}"),"lines":raw_lines(&q)?})).await?;
    let order = core(
        tx,
        state,
        actor,
        "order.status",
        order["id"].as_str(),
        json!({"version":order["version"],"status":"confirmed"}),
    )
    .await?;
    let mut body = offer.clone();
    body["id"] = order["id"].clone();
    body["version"] = json!(1);
    body["offer_id"] = json!(id);
    body["status"] = json!("ordered");
    body["quote"] = q.clone();
    body["contract_id"] = q["contract_id"].clone();
    body["acceptance_reference"] = json!(p.acceptance_reference);
    body["provisioning"] = json!({"status":"pending","external_reference":"","error_code":""});
    save(tx, "order", &body, actor, "commerce.order.created").await?;
    offer["status"] = json!("accepted");
    offer["version"] = json!(p.version + 1);
    offer["order_id"] = body["id"].clone();
    offer["quote"] = q;
    save(tx, "offer", &offer, actor, "commerce.offer.accepted").await?;
    Ok(body)
}
async fn provision_in(
    tx: &mut Transaction<'_, Sqlite>,
    state: &AppState,
    actor: &str,
    id: &str,
    input: &Value,
) -> Result<Value> {
    let p: ProvisionInput = parse(input)?;
    let mut body = load(tx, "order", id).await?;
    business::version(input, &body)?;
    let order = business::record(tx, "order", id).await?;
    let contract = business::record(tx, "contract", body["contract_id"].as_str().unwrap()).await?;
    if p.status != "failed" && (contract["status"] != "active" || order["status"] == "cancelled") {
        return Err(conflict(
            "Auftrag oder Vertrag wurde beendet. Bereitstellung prüfen.",
        ));
    }
    let old = body["provisioning"]["status"].as_str().unwrap_or("");
    let allowed = matches!(
        (old, p.status.as_str()),
        ("pending", "running")
            | ("pending", "failed")
            | ("running", "ready")
            | ("running", "failed")
            | ("failed", "running")
    );
    if !allowed {
        return Err(conflict(
            "Bereitstellungsstatus erlaubt diesen Übergang nicht.",
        ));
    }
    domain::field(
        input,
        "external_reference",
        if p.status == "ready" { 1 } else { 0 },
        500,
    )?;
    domain::field(
        input,
        "error_code",
        if p.status == "failed" { 1 } else { 0 },
        120,
    )?;
    if p.status != "failed" && !p.error_code.is_empty() {
        return Err(invalid(
            "Fehlercode nur bei fehlgeschlagener Bereitstellung.",
        ));
    }
    body["version"] = json!(p.version + 1);
    body["provisioning"] = json!({"status":p.status,"external_reference":p.external_reference,"error_code":p.error_code});
    if p.status == "ready" && order["status"] == "confirmed" {
        core(
            tx,
            state,
            actor,
            "order.status",
            Some(id),
            json!({"version":order["version"],"status":"fulfilled"}),
        )
        .await?;
    }
    save(
        tx,
        "order",
        &body,
        actor,
        "commerce.order.provisioning_changed",
    )
    .await?;
    Ok(body)
}
async fn invoice_in(
    tx: &mut Transaction<'_, Sqlite>,
    state: &AppState,
    actor: &str,
    id: &str,
    input: &Value,
) -> Result<Value> {
    let p: InvoiceInput = parse(input)?;
    let mut body = load(tx, "order", id).await?;
    business::version(input, &body)?;
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM commerce_invoices WHERE order_id=?")
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    if exists > 0 {
        return Err(conflict(
            "Erstrechnung bereits vorhanden. Auftrag neu laden.",
        ));
    }
    let contract = business::record(tx, "contract", body["contract_id"].as_str().unwrap()).await?;
    if contract["status"] != "active" {
        return Err(conflict("Vertrag ist nicht aktiv."));
    }
    let invoice=core(tx,state,actor,"invoice.create",None,json!({"customer_id":body["customer_id"],"title":body["quote"]["title"],"issue_date":p.issue_date,"due_date":p.due_date,"service_date":p.service_date,"buyer_reference":p.buyer_reference,"notes":format!("Auftrag {id}; Erstrechnung: Einmalleistungen und erster Zeitraum gemäß Vertrag."),"direction":"outgoing","external_number":"","lines":raw_lines(&body["quote"])?})).await?;
    let invoice = core(
        tx,
        state,
        actor,
        "invoice.issue",
        invoice["id"].as_str(),
        json!({"version":invoice["version"]}),
    )
    .await?;
    sqlx::query("INSERT INTO commerce_invoices(order_id,invoice_id) VALUES(?,?)")
        .bind(id)
        .bind(invoice["id"].as_str())
        .execute(&mut **tx)
        .await?;
    body["version"] = json!(p.version + 1);
    body["invoice_id"] = invoice["id"].clone();
    save(tx, "order", &body, actor, "commerce.order.invoiced").await?;
    Ok(body)
}

pub async fn write(
    State(state): State<AppState>,
    Path((resource, id, action)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    write_command(
        &state,
        &headers,
        &format!("{resource}.{action}"),
        Some(&id),
        input,
    )
    .await
    .map(Json)
}
pub async fn create(
    State(state): State<AppState>,
    Path(resource): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    write_command(&state, &headers, &resource, None, input)
        .await
        .map(Json)
}
async fn write_command(
    state: &AppState,
    headers: &HeaderMap,
    command: &str,
    id: Option<&str>,
    input: Value,
) -> Result<Value> {
    let (key, actor) = command_headers(headers)?;
    crate::connectors::reject_secrets(&input)?;
    let request = json!({"command":command,"id":id,"actor":actor,"input":input}).to_string();
    let mut tx = state.pool.begin().await?;
    if let Some(row) =
        sqlx::query("SELECT request,response FROM commerce_receipts WHERE idempotency_key=?")
            .bind(&key)
            .fetch_optional(&mut *tx)
            .await?
    {
        if row.get::<String, _>("request") != request {
            return Err(conflict(
                "Idempotency-Key mit anderem Body, Akteur oder Pfad wiederverwendet.",
            ));
        }
        return decode(row.get("response"));
    }
    let body = match (command, id) {
        ("catalog", None) => catalog_in(&mut tx, &input, &actor).await?,
        ("customers", None) => customer_in(&mut tx, state, &actor, &input).await?,
        ("offers", None) => offer_in(&mut tx, state, &actor, &input).await?,
        ("customers.update", Some(id)) => {
            customer_update_in(&mut tx, state, &actor, id, &input).await?
        }
        ("offers.price", Some(id)) => price_in(&mut tx, &actor, id, &input).await?,
        ("offers.accept", Some(id)) => accept_in(&mut tx, state, &actor, id, &input).await?,
        ("orders.provisioning", Some(id)) => {
            provision_in(&mut tx, state, &actor, id, &input).await?
        }
        ("orders.billing", Some(id)) => billing::apply(&mut tx, state, &actor, id, &input).await?,
        ("orders.invoice", Some(id)) => invoice_in(&mut tx, state, &actor, id, &input).await?,
        _ => return Err(missing()),
    };
    let response = json!({"contract":CONTRACT,"record":body});
    sqlx::query("INSERT INTO commerce_receipts(idempotency_key,request,response) VALUES(?,?,?)")
        .bind(key)
        .bind(request)
        .bind(response.to_string())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(response)
}
pub async fn catalog(State(state): State<AppState>) -> Result<Json<Value>> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT c.body FROM commerce_catalog c JOIN commerce_state s ON c.revision=s.revision",
    )
    .fetch_optional(&state.pool)
    .await?;
    Ok(Json(match raw {
        Some(s) => decode(&s)?,
        None => json!({"contract":CONTRACT,"version":0,"revision":null,"items":[]}),
    }))
}
fn kind(resource: &str) -> Result<&str> {
    match resource {
        "customers" => Ok("customer"),
        "offers" => Ok("offer"),
        "orders" => Ok("order"),
        _ => Err(missing()),
    }
}
async fn read_in(tx: &mut Transaction<'_, Sqlite>, kind: &str, id: &str) -> Result<Value> {
    let mut body = load(tx, kind, id).await?;
    if kind == "customer" {
        let customer =
            business::record(tx, "customer", body["customer_id"].as_str().unwrap()).await?;
        body["customer_version"] = customer["version"].clone();
        body["customer"] = customer;
    }
    if kind == "offer" && body["quote"]["id"].is_string() {
        let q = business::record(tx, "quote", body["quote"]["id"].as_str().unwrap()).await?;
        body["quote_status"] = q["status"].clone();
        body["quote_version"] = q["version"].clone();
    }
    if kind == "order" {
        billing::project(tx, &mut body).await?;
        let order = business::record(tx, "order", id).await?;
        body["order_status"] = order["status"].clone();
        let invoice_id: Option<String> =
            sqlx::query_scalar("SELECT invoice_id FROM commerce_invoices WHERE order_id=?")
                .bind(id)
                .fetch_optional(&mut **tx)
                .await?;
        let payment = if let Some(invoice_id) = invoice_id {
            let invoice = business::record(tx, "invoice", &invoice_id).await?;
            let mut payment = json!({"status":invoice["status"],"invoice_id":invoice_id,"number":invoice["number"],"invoice_version":invoice["version"],"gross_cents":invoice["gross_cents"],"paid_cents":invoice["paid_cents"],"refund_due_cents":invoice["refund_due_cents"],"journal_id":invoice["journal_id"],"currency":"EUR"});
            if let Some(rounding) = invoice.get("rounding_cents") {
                payment["rounding_cents"] = rounding.clone();
            }
            payment
        } else {
            json!({"status":"not_invoiced"})
        };
        body["payment"] = payment;
        let contract =
            business::record(tx, "contract", body["contract_id"].as_str().unwrap()).await?;
        body["contract_status"] = contract["status"].clone();
    }
    Ok(json!({"contract":CONTRACT,"record":body}))
}
pub async fn read(
    State(state): State<AppState>,
    Path((resource, id)): Path<(String, String)>,
) -> Result<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    read_in(&mut tx, kind(&resource)?, &id).await.map(Json)
}
pub async fn lookup(
    State(state): State<AppState>,
    Path(resource): Path<String>,
    Query(q): Query<Lookup>,
) -> Result<Json<Value>> {
    let k = kind(&resource)?;
    let mut tx = state.pool.begin().await?;
    let id = external(&mut tx, k, &q.source, &q.external_id)
        .await?
        .ok_or_else(missing)?;
    read_in(&mut tx, k, &id).await.map(Json)
}
pub async fn events(
    State(state): State<AppState>,
    Query(q): Query<EventsQuery>,
) -> Result<Json<Value>> {
    if q.after < 0 || !(1..=200).contains(&q.limit) {
        return Err(invalid("Ungültiger Ereigniscursor."));
    }
    let rows = sqlx::query("SELECT * FROM commerce_events WHERE seq>? ORDER BY seq LIMIT ?")
        .bind(q.after)
        .bind(q.limit + 1)
        .fetch_all(&state.pool)
        .await?;
    let has_more = rows.len() > q.limit as usize;
    let items: Vec<Value>=rows.iter().take(q.limit as usize).map(|r|json!({"seq":r.get::<i64,_>("seq"),"type":r.get::<String,_>("event_type"),"entity_id":r.get::<String,_>("entity_id"),"version":r.get::<i64,_>("entity_version"),"created_at":r.get::<String,_>("created_at")})).collect();
    Ok(Json(
        json!({"contract":CONTRACT,"next_cursor":items.last().map(|v|v["seq"].as_i64().unwrap()).unwrap_or(q.after),"has_more":has_more,"items":items}),
    ))
}

pub async fn create_catalog(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    write_command(&state, &headers, "catalog", None, input)
        .await
        .map(Json)
}
