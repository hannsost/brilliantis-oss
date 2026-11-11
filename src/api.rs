use crate::{business, command_headers, domain, invalid, AppState, Error, Result};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;

pub fn kind(resource: &str) -> Result<&'static str> {
    match resource {
        "leads" => Ok("lead"),
        "campaigns" => Ok("campaign"),
        "creators" => Ok("creator"),
        "products" => Ok("product"),
        "customers" => Ok("customer"),
        "quotes" => Ok("quote"),
        "contracts" => Ok("contract"),
        "invoices" => Ok("invoice"),
        "credit-notes" => Ok("credit_note"),
        "bank-accounts" => Ok("bank_account"),
        "bank-transactions" => Ok("bank_transaction"),
        "channel-accounts" => Ok("channel_account"),
        "content" => Ok("content"),
        "collaborations" => Ok("collaboration"),
        "seo-keywords" => Ok("seo_keyword"),
        "orders" => Ok("order"),
        _ => Err(Error(
            StatusCode::NOT_FOUND,
            "not_found",
            "Ressource nicht gefunden.".into(),
        )),
    }
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Page {
    #[serde(default)]
    pub after: i64,
    #[serde(default = "page_size")]
    pub limit: i64,
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub archived: bool,
}
fn page_size() -> i64 {
    100
}
impl Default for Page {
    fn default() -> Self {
        Self {
            after: 0,
            limit: 100,
            q: String::new(),
            archived: false,
        }
    }
}
pub async fn page(pool: &sqlx::SqlitePool, kind: &str, query: &Page) -> Result<Value> {
    if query.after < 0 || !(1..=200).contains(&query.limit) || query.q.chars().count() > 160 {
        return Err(invalid("Ungültige Seitengröße, Suchanfrage oder Cursor."));
    }
    let pattern = format!(
        "%{}%",
        query
            .q
            .replace('!', "!!")
            .replace('%', "!%")
            .replace('_', "!_")
    );
    let rows=sqlx::query("SELECT rowid AS cursor,id,version,body,archived,created_at,updated_at FROM records WHERE kind=? AND archived=? AND rowid>? AND (json_extract(body,'$.name') LIKE ? ESCAPE '!' OR json_extract(body,'$.company') LIKE ? ESCAPE '!' OR json_extract(body,'$.title') LIKE ? ESCAPE '!' OR json_extract(body,'$.email') LIKE ? ESCAPE '!' OR json_extract(body,'$.number') LIKE ? ESCAPE '!' OR ?='%') ORDER BY rowid LIMIT ?")
        .bind(kind).bind(query.archived).bind(query.after).bind(&pattern).bind(&pattern).bind(&pattern).bind(&pattern).bind(&pattern).bind(if query.q.is_empty(){"%"}else{""}).bind(query.limit+1).fetch_all(pool).await?;
    let has_more = rows.len() > query.limit as usize;
    let rows = &rows[..rows.len().min(query.limit as usize)];
    let mut items = Vec::new();
    for row in rows {
        let mut value: Value =
            serde_json::from_str(row.get("body")).map_err(|_| invalid("Ungültiger Datensatz."))?;
        value["id"] = json!(row.get::<String, _>("id"));
        value["version"] = json!(row.get::<i64, _>("version"));
        value["archived"] = json!(row.get::<i64, _>("archived") != 0);
        value["created_at"] = json!(row.get::<String, _>("created_at"));
        value["updated_at"] = json!(row.get::<String, _>("updated_at"));
        items.push(value);
    }
    let next = rows
        .last()
        .map(|r| r.get::<i64, _>("cursor"))
        .unwrap_or(query.after);
    Ok(json!({"items":items,"next_cursor":next,"has_more":has_more}))
}
pub async fn list(
    State(state): State<AppState>,
    Path(resource): Path<String>,
    Query(query): Query<Page>,
) -> Result<Json<Value>> {
    Ok(Json(page(&state.pool, kind(&resource)?, &query).await?))
}
pub async fn get(
    State(state): State<AppState>,
    Path((resource, id)): Path<(String, String)>,
) -> Result<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    let mut record = business::record(&mut tx, kind(&resource)?, &id).await?;
    if resource == "quotes" {
        let body: Option<String> = sqlx::query_scalar("SELECT body FROM commerce_records WHERE kind='offer' AND json_extract(body,'$.quote.id')=?")
            .bind(&id).fetch_optional(&mut *tx).await?;
        if let Some(body) = body {
            let offer: Value =
                serde_json::from_str(&body).map_err(|_| invalid("Commerce-Angebot ungültig."))?;
            record["commerce_offer_id"] = offer["id"].clone();
            record["commerce_offer_version"] = offer["version"].clone();
        }
    }
    if resource == "orders" {
        let exists: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM commerce_records WHERE kind='order' AND id=?")
                .bind(&id)
                .fetch_one(&mut *tx)
                .await?;
        if exists > 0 {
            record["commerce_order_id"] = json!(id);
        }
    }
    Ok(Json(record))
}
pub async fn create(
    State(state): State<AppState>,
    Path(resource): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let (key, actor) = command_headers(&headers)?;
    Ok(Json(
        domain::execute(
            &state.pool,
            &state.profile,
            &key,
            &actor,
            &format!("{}.create", kind(&resource)?),
            None,
            input,
        )
        .await?,
    ))
}
pub async fn edit(
    State(state): State<AppState>,
    Path((resource, id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let (key, actor) = command_headers(&headers)?;
    Ok(Json(
        domain::execute(
            &state.pool,
            &state.profile,
            &key,
            &actor,
            &format!("{}.edit", kind(&resource)?),
            Some(&id),
            input,
        )
        .await?,
    ))
}
pub async fn action(
    State(state): State<AppState>,
    Path((resource, id, action)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    if ![
        "archive",
        "restore",
        "send",
        "accept",
        "reject",
        "cancel",
        "issue",
        "allocate",
        "categorize",
        "refund",
        "status",
    ]
    .contains(&action.as_str())
    {
        return Err(invalid("Unbekannte Aktion."));
    }
    let (key, actor) = command_headers(&headers)?;
    Ok(Json(
        domain::execute(
            &state.pool,
            &state.profile,
            &key,
            &actor,
            &format!("{}.{action}", kind(&resource)?),
            Some(&id),
            input,
        )
        .await?,
    ))
}
pub async fn organization(State(state): State<AppState>) -> Result<Json<Value>> {
    let row = sqlx::query("SELECT version,body FROM organization WHERE id=1")
        .fetch_optional(&state.pool)
        .await?;
    Ok(Json(if let Some(row) = row {
        json!({"version":row.get::<i64,_>("version"),"fields":serde_json::from_str::<Value>(row.get("body")).map_err(|_|invalid("Unternehmensdaten ungültig."))?})
    } else {
        json!({"version":0,"fields":null})
    }))
}
pub async fn save_organization(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let (key, actor) = command_headers(&headers)?;
    crate::connectors::reject_secrets(&input)?;
    let body = business::organization(&input["fields"])?;
    let request = json!({"command":"organization.update","actor":actor,"input":input}).to_string();
    let mut tx = state.pool.begin().await?;
    if let Some(row) = sqlx::query("SELECT request,response FROM commands WHERE idempotency_key=?")
        .bind(&key)
        .fetch_optional(&mut *tx)
        .await?
    {
        if row.get::<String, _>("request") != request {
            return Err(crate::conflict("Idempotency-Key bereits belegt."));
        }
        return Ok(Json(
            serde_json::from_str(row.get("response")).map_err(|_| invalid("Ungültiger Beleg."))?,
        ));
    }
    let version: Option<i64> = sqlx::query_scalar("SELECT version FROM organization WHERE id=1")
        .fetch_optional(&mut *tx)
        .await?;
    let version = version.unwrap_or(0);
    if input["version"].as_i64() != Some(version) {
        return Err(crate::conflict("Unternehmensdaten wurden geändert."));
    }
    sqlx::query("INSERT INTO organization(id,version,body) VALUES(1,?,?) ON CONFLICT(id) DO UPDATE SET version=excluded.version,body=excluded.body").bind(version+1).bind(body.to_string()).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO security_events(actor,action,target) VALUES(?,'organization.update','organization')").bind(actor).execute(&mut *tx).await?;
    let response = json!({"version":version+1,"fields":body});
    sqlx::query("INSERT INTO commands(idempotency_key,request,response) VALUES(?,?,?)")
        .bind(key)
        .bind(request)
        .bind(response.to_string())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(response))
}
pub async fn catalog(State(state): State<AppState>) -> Result<Json<Value>> {
    let values = page(
        &state.pool,
        "product",
        &Page {
            limit: 200,
            ..Default::default()
        },
    )
    .await?;
    Ok(Json(values["items"].clone()))
}
