use crate::{business, conflict, domain::field, invalid, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Sqlite, Transaction};
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelAccount {
    name: String,
    platform: String,
    handle: String,
    identity: String,
    creator_id: String,
    connection_id: String,
    notes: String,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Content {
    title: String,
    campaign_id: String,
    channel_account_id: String,
    body: String,
    media_url: String,
    disclosure: String,
    planned_date: String,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Collaboration {
    name: String,
    creator_id: String,
    campaign_id: String,
    deliverables: String,
    due_date: String,
    budget_cents: i64,
    rights: String,
    notes: String,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Keyword {
    name: String,
    campaign_id: String,
    target_url: String,
    intent: String,
    notes: String,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Order {
    name: String,
    customer_id: String,
    source: String,
    external_id: String,
    ordered_date: String,
    notes: String,
    lines: Vec<business::LineInput>,
}
fn parse<T: serde::de::DeserializeOwned + Serialize>(input: &Value) -> Result<Value> {
    let value: T = serde_json::from_value(input.clone())
        .map_err(|_| invalid("Felder passen nicht zum Datensatz."))?;
    Ok(serde_json::to_value(value).unwrap())
}
async fn reference(
    tx: &mut Transaction<'_, Sqlite>,
    input: &Value,
    key: &str,
    kind: &str,
    optional: bool,
) -> Result<()> {
    let id = field(input, key, if optional { 0 } else { 1 }, 100)?;
    if !id.is_empty() {
        let record = business::record(tx, kind, &id).await?;
        if record["archived"] == true {
            return Err(invalid("Verknüpfter Datensatz ist archiviert."));
        }
    }
    Ok(())
}
fn https(input: &Value, key: &str) -> Result<()> {
    let value = field(input, key, 0, 2000)?;
    if !value.is_empty() {
        let url = url::Url::parse(&value).map_err(|_| invalid("Gültige HTTPS-URL erwartet."))?;
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            return Err(invalid("Gültige HTTPS-URL ohne Zugangsdaten erwartet."));
        }
    }
    Ok(())
}
async fn validate(tx: &mut Transaction<'_, Sqlite>, kind: &str, input: &Value) -> Result<Value> {
    let mut body = match kind {
        "channel_account" => {
            let value = parse::<ChannelAccount>(input)?;
            field(input, "name", 1, 160)?;
            field(input, "handle", 1, 300)?;
            field(input, "notes", 0, 4000)?;
            field(input, "connection_id", 0, 80)?;
            if ![
                "gmail",
                "instagram",
                "tiktok",
                "youtube",
                "linkedin",
                "discourse",
                "blog",
                "other",
            ]
            .contains(&input["platform"].as_str().unwrap_or(""))
                || !["brand", "virtual", "external"]
                    .contains(&input["identity"].as_str().unwrap_or(""))
            {
                return Err(invalid("Plattform oder Profilart ungültig."));
            }
            reference(tx, input, "creator_id", "creator", true).await?;
            value
        }
        "content" => {
            let value = parse::<Content>(input)?;
            field(input, "title", 1, 200)?;
            field(input, "body", 1, 16000)?;
            field(input, "disclosure", 0, 1000)?;
            https(input, "media_url")?;
            business::date(&field(input, "planned_date", 10, 10)?)?;
            reference(tx, input, "campaign_id", "campaign", true).await?;
            reference(tx, input, "channel_account_id", "channel_account", false).await?;
            value
        }
        "collaboration" => {
            let value = parse::<Collaboration>(input)?;
            field(input, "name", 1, 160)?;
            field(input, "deliverables", 1, 4000)?;
            field(input, "rights", 0, 4000)?;
            field(input, "notes", 0, 4000)?;
            business::date(&field(input, "due_date", 10, 10)?)?;
            if !input["budget_cents"]
                .as_i64()
                .is_some_and(|v| (0..=100_000_000_000).contains(&v))
            {
                return Err(invalid("Budget ungültig."));
            }
            reference(tx, input, "creator_id", "creator", false).await?;
            reference(tx, input, "campaign_id", "campaign", true).await?;
            value
        }
        "seo_keyword" => {
            let value = parse::<Keyword>(input)?;
            field(input, "name", 1, 300)?;
            field(input, "intent", 1, 300)?;
            field(input, "notes", 0, 4000)?;
            https(input, "target_url")?;
            reference(tx, input, "campaign_id", "campaign", true).await?;
            value
        }
        "order" => {
            let value: Order = serde_json::from_value(input.clone())
                .map_err(|_| invalid("Bestellfelder ungültig."))?;
            field(input, "name", 1, 200)?;
            field(input, "source", 1, 100)?;
            field(input, "external_id", 1, 160)?;
            field(input, "notes", 0, 4000)?;
            business::date(&value.ordered_date)?;
            reference(tx, input, "customer_id", "customer", false).await?;
            let (lines, net, tax) = business::calculate(&value.lines)?;
            let mut body = input.clone();
            body["lines"] = json!(lines);
            body["net_cents"] = json!(net);
            body["tax_cents"] = json!(tax);
            body["gross_cents"] = json!(net + tax);
            business::apply_rounding(&mut body);
            body["currency"] = json!("EUR");
            body
        }
        _ => return Err(invalid("Unbekannter Arbeitsbereich.")),
    };
    body["status"] = json!(if ["channel_account", "seo_keyword"].contains(&kind) {
        "active"
    } else {
        "draft"
    });
    Ok(body)
}
pub async fn prepare(
    tx: &mut Transaction<'_, Sqlite>,
    command: &str,
    id: &str,
    input: &Value,
) -> Result<(Value, i64, String)> {
    let (kind, action) = command
        .split_once('.')
        .ok_or_else(|| invalid("Befehl ungültig."))?;
    if action == "create" {
        let body = validate(tx, kind, input).await?;
        if kind == "order" {
            let system = format!("shop:{}", field(input, "source", 1, 100)?);
            let external = field(input, "external_id", 1, 160)?;
            let exists: Option<String> = sqlx::query_scalar(
                "SELECT entity_id FROM external_refs WHERE system=? AND external_id=?",
            )
            .bind(&system)
            .bind(&external)
            .fetch_optional(&mut **tx)
            .await?;
            if exists.is_some() {
                return Err(conflict("Bestellung ist bereits erfasst."));
            }
            sqlx::query("INSERT INTO external_refs(system,external_id,entity_id) VALUES(?,?,?)")
                .bind(system)
                .bind(external)
                .bind(id)
                .execute(&mut **tx)
                .await?;
        }
        return Ok((body, 1, format!("{kind} angelegt")));
    }
    let mut body = business::record(tx, kind, id).await?;
    let version = business::version(input, &body)?;
    if body["archived"] == true && action != "restore" {
        return Err(conflict("Datensatz ist archiviert."));
    }
    match action {
        "edit" => {
            if !["draft", "active"].contains(&body["status"].as_str().unwrap_or(""))
                || kind == "order"
            {
                return Err(conflict("Zum Bearbeiten zunächst als Entwurf öffnen."));
            }
            body = validate(tx, kind, &input["fields"]).await?;
        }
        "archive" | "restore" if kind != "order" => body["archived"] = json!(action == "archive"),
        "status" => {
            let target = field(input, "status", 1, 30)?;
            let old = body["status"].as_str().unwrap_or("");
            let allowed: &[&str] = match (kind, old) {
                ("content", "draft") => &["planned"],
                ("content", "planned") => &["draft", "published"],
                ("collaboration", "draft") => &["agreed", "cancelled"],
                ("collaboration", "agreed") => &["delivered", "cancelled"],
                ("collaboration", "delivered") => &["completed"],
                ("order", "draft") => &["confirmed", "cancelled"],
                ("order", "confirmed") => &["fulfilled", "cancelled"],
                _ => &[],
            };
            if !allowed.contains(&target.as_str()) {
                return Err(invalid("Statuswechsel ist nicht möglich."));
            }
            if kind == "content" && target == "published" {
                https(input, "publication_url")?;
                field(input, "publication_url", 1, 2000)?;
                body["publication_url"] = input["publication_url"].clone();
            }
            body["status"] = json!(target);
        }
        _ => return Err(invalid("Aktion nicht unterstützt.")),
    }
    for key in ["id", "version"] {
        body.as_object_mut().unwrap().remove(key);
    }
    Ok((body, version + 1, format!("{kind}: {action}")))
}
