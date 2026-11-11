//! Portable editorial briefs. Preparing/downloading a revision never publishes content.
use crate::{
    business, command_headers, conflict, domain::field, invalid, new_id, AppState, Error, Result,
};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Row, Sqlite, Transaction};

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BriefKind {
    Core,
    Addition,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BriefInput {
    pub title: String,
    pub kind: BriefKind,
    pub recipient: String,
    pub language: String,
    pub channels: String,
    pub campaign_id: String,
    pub creator_id: String,
    pub core_briefing_id: String,
    pub core_revision: i64,
    pub body: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HandoffInput {
    pub version: i64,
    pub revision: i64,
    pub note: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VersionInput {
    pub version: i64,
}

fn missing() -> Error {
    Error(
        StatusCode::NOT_FOUND,
        "not_found",
        "Briefing oder Fassung nicht gefunden.".into(),
    )
}
fn parse<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T> {
    serde_json::from_value(value.clone()).map_err(|_| invalid("Felder passen nicht zum Briefing."))
}
async fn read(tx: &mut Transaction<'_, Sqlite>, id: &str) -> Result<Value> {
    let row = sqlx::query(
        "SELECT id,version,body,archived,created_at,updated_at FROM briefings WHERE id=?",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(missing)?;
    record(&row)
}
fn record(row: &sqlx::sqlite::SqliteRow) -> Result<Value> {
    let mut body: Value =
        serde_json::from_str(row.get("body")).map_err(|_| invalid("Briefing ungültig."))?;
    body["id"] = json!(row.get::<String, _>("id"));
    body["version"] = json!(row.get::<i64, _>("version"));
    body["archived"] = json!(row.get::<i64, _>("archived") != 0);
    body["created_at"] = json!(row.get::<String, _>("created_at"));
    body["updated_at"] = json!(row.get::<String, _>("updated_at"));
    Ok(body)
}
async fn validate(tx: &mut Transaction<'_, Sqlite>, input: &Value) -> Result<Value> {
    let _: BriefInput = parse(input)?;
    let mut body = input.clone();
    for (key, min, max) in [
        ("title", 1, 200),
        ("recipient", 1, 160),
        ("language", 2, 30),
        ("channels", 1, 300),
        ("body", 1, 16000),
        ("campaign_id", 0, 100),
        ("creator_id", 0, 100),
        ("core_briefing_id", 0, 100),
    ] {
        body[key] = json!(field(input, key, min, max)?);
    }
    for (key, kind) in [("campaign_id", "campaign"), ("creator_id", "creator")] {
        let id = body[key].as_str().unwrap();
        if !id.is_empty() && business::record(tx, kind, id).await?["archived"] == true {
            return Err(invalid("Verknüpftes Profil oder Kampagne ist archiviert."));
        }
    }
    if body["kind"] == "core" {
        if body["core_briefing_id"] != "" || body["core_revision"] != 0 {
            return Err(invalid(
                "Ein Grundbriefing hat keine übergeordnete Fassung.",
            ));
        }
    } else {
        let id = body["core_briefing_id"].as_str().unwrap();
        let revision = body["core_revision"].as_i64().unwrap();
        if id.is_empty() || revision < 1 {
            return Err(invalid(
                "Ergänzung benötigt Grundbriefing und bereitgestellte Fassung.",
            ));
        }
        if read(tx, id).await?["archived"] == true {
            return Err(conflict("Grundbriefing ist archiviert."));
        }
        let snapshot: Option<String> = sqlx::query_scalar(
            "SELECT snapshot FROM briefing_editions WHERE briefing_id=? AND revision=?",
        )
        .bind(id)
        .bind(revision)
        .fetch_optional(&mut **tx)
        .await?;
        let core: Value = serde_json::from_str(&snapshot.ok_or_else(missing)?)
            .map_err(|_| invalid("Grundbriefing ungültig."))?;
        if core["kind"] != "core"
            || core["recipient"] != body["recipient"]
            || core["language"] != body["language"]
        {
            return Err(invalid(
                "Ergänzung muss zu Empfänger und Sprache der Grundfassung passen.",
            ));
        }
    }
    body["status"] = json!("draft");
    Ok(body)
}
fn render_text(body: &Value, id: &str, revision: i64) -> String {
    let s = |key: &str| body[key].as_str().unwrap_or("");
    let basis = if body["kind"] == "core" {
        "Grundbriefing · gilt bis zur Ablösung durch eine neue übergebene Fassung.\n".to_string()
    } else {
        format!(
            "Ergänzung zu Grundbriefing {} · Fassung {}.\n",
            s("core_briefing_id"),
            body["core_revision"]
        )
    };
    format!("{}\n\nEmpfänger: {}\nSprache: {}\nKanäle: {}\nBriefing: {id} · Fassung {revision}\n{basis}\n{}\n",
        s("title"),s("recipient"),s("language"),s("channels"),s("body"))
}
async fn execute(
    state: &AppState,
    key: &str,
    actor: &str,
    action: &str,
    id: Option<&str>,
    input: Value,
) -> Result<Value> {
    crate::connectors::reject_secrets(&input)?;
    let mut tx = state.pool.begin().await?;
    let request =
        json!({"command":format!("briefing.{action}"),"actor":actor,"id":id,"input":input})
            .to_string();
    if let Some(row) = sqlx::query("SELECT request,response FROM commands WHERE idempotency_key=?")
        .bind(key)
        .fetch_optional(&mut *tx)
        .await?
    {
        if row.get::<String, _>("request") != request {
            return Err(conflict("Idempotency-Key gehört zu einem anderen Befehl."));
        }
        return serde_json::from_str(row.get("response"))
            .map_err(|_| invalid("Gespeicherte Antwort ungültig."));
    }
    let id = id.map(str::to_owned).unwrap_or_else(new_id);
    let (mut body, version) = if action == "create" {
        let mut body = validate(&mut tx, &input).await?;
        body["latest_revision"] = json!(0);
        (body, 1)
    } else {
        let mut body = read(&mut tx, &id).await?;
        let version = business::version(&input, &body)?;
        if body["archived"] == true && action != "restore" {
            return Err(conflict("Briefing ist archiviert."));
        }
        match action {
            "edit" => {
                if input.as_object().is_none_or(|o| {
                    o.keys()
                        .any(|k| !["version", "fields"].contains(&k.as_str()))
                }) {
                    return Err(invalid("Bearbeitung benötigt version und fields."));
                }
                let mut updated = validate(&mut tx, &input["fields"]).await?;
                // Identity and base relation stay stable once a portable edition exists.
                if body["latest_revision"].as_i64().unwrap_or(0) > 0
                    && [
                        "kind",
                        "recipient",
                        "language",
                        "core_briefing_id",
                        "core_revision",
                    ]
                    .iter()
                    .any(|k| updated[k] != body[k])
                {
                    return Err(conflict("Empfänger, Sprache und Grundlage nach der ersten Fassung über ein neues Briefing ändern."));
                }
                updated["latest_revision"] = body["latest_revision"].clone();
                updated["archived"] = json!(false);
                body = updated;
            }
            "release" => {
                let _: VersionInput = parse(&input)?;
                if body["status"] != "draft" {
                    return Err(conflict(
                        "Diese Fassung wurde bereits bereitgestellt. Änderungen zuerst speichern.",
                    ));
                }
                // Revalidate links at release; they may have been archived since editing.
                let fields: BriefInput = serde_json::from_value({
                    let mut v = body.clone();
                    for k in [
                        "id",
                        "version",
                        "archived",
                        "created_at",
                        "updated_at",
                        "status",
                        "latest_revision",
                    ] {
                        v.as_object_mut().unwrap().remove(k);
                    }
                    v
                })
                .map_err(|_| invalid("Briefingfelder ungültig."))?;
                validate(&mut tx, &serde_json::to_value(fields).unwrap()).await?;
                let revision = body["latest_revision"].as_i64().unwrap_or(0) + 1;
                let text = render_text(&body, &id, revision);
                sqlx::query("INSERT INTO briefing_editions(briefing_id,revision,snapshot,text,sha256,actor) VALUES(?,?,?,?,?,?)")
                    .bind(&id).bind(revision).bind(body.to_string()).bind(&text).bind(crate::storage::hash(&text)).bind(actor).execute(&mut *tx).await?;
                body["latest_revision"] = json!(revision);
                body["status"] = json!("ready");
            }
            "handoff" => {
                let handoff: HandoffInput = parse(&input)?;
                let note = field(&input, "note", 1, 1000)?;
                let exists: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM briefing_editions WHERE briefing_id=? AND revision=?",
                )
                .bind(&id)
                .bind(handoff.revision)
                .fetch_one(&mut *tx)
                .await?;
                if exists != 1 {
                    return Err(missing());
                }
                let done: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM briefing_handovers WHERE briefing_id=? AND revision=?",
                )
                .bind(&id)
                .bind(handoff.revision)
                .fetch_one(&mut *tx)
                .await?;
                if done != 0 {
                    return Err(conflict("Übergabe dieser Fassung wurde bereits erfasst."));
                }
                sqlx::query("INSERT INTO briefing_handovers(briefing_id,revision,actor,note) VALUES(?,?,?,?)")
                    .bind(&id).bind(handoff.revision).bind(actor).bind(note).execute(&mut *tx).await?;
            }
            "archive" | "restore" => {
                let _: VersionInput = parse(&input)?;
                body["archived"] = json!(action == "archive");
            }
            _ => return Err(invalid("Briefing-Aktion unbekannt.")),
        }
        (body, version + 1)
    };
    for k in ["id", "version", "created_at", "updated_at"] {
        body.as_object_mut().unwrap().remove(k);
    }
    if version == 1 {
        sqlx::query("INSERT INTO briefings(id,version,body) VALUES(?,?,?)")
            .bind(&id)
            .bind(version)
            .bind(body.to_string())
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query("UPDATE briefings SET version=?,body=?,archived=?,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?")
            .bind(version).bind(body.to_string()).bind(body["archived"] == true).bind(&id).execute(&mut *tx).await?;
    }
    let event_id = new_id();
    sqlx::query("INSERT INTO events(id,event_type,entity_id,entity_version,actor_label,summary) VALUES(?,?,?,?,?,?)")
        .bind(&event_id).bind(format!("briefing.{action}")).bind(&id).bind(version).bind(actor).bind(format!("Briefing: {action}")).execute(&mut *tx).await?;
    let response = json!({"record":read(&mut tx,&id).await?,"event_id":event_id});
    sqlx::query("INSERT INTO commands(idempotency_key,request,response) VALUES(?,?,?)")
        .bind(key)
        .bind(request)
        .bind(response.to_string())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(response)
}
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let (key, actor) = command_headers(&headers)?;
    Ok(Json(
        execute(&state, &key, &actor, "create", None, input).await?,
    ))
}
pub async fn edit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let (key, actor) = command_headers(&headers)?;
    Ok(Json(
        execute(&state, &key, &actor, "edit", Some(&id), input).await?,
    ))
}
pub async fn action(
    State(state): State<AppState>,
    Path((id, action)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    if !["release", "handoff", "archive", "restore"].contains(&action.as_str()) {
        return Err(invalid("Briefing-Aktion unbekannt."));
    }
    let (key, actor) = command_headers(&headers)?;
    Ok(Json(
        execute(&state, &key, &actor, &action, Some(&id), input).await?,
    ))
}
pub async fn get(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>> {
    Ok(Json(read(&mut state.pool.begin().await?, &id).await?))
}
pub async fn list(
    State(state): State<AppState>,
    Query(page): Query<crate::api::Page>,
) -> Result<Json<Value>> {
    if page.after < 0 || !(1..=200).contains(&page.limit) || page.q.chars().count() > 160 {
        return Err(invalid("Seitenparameter ungültig."));
    }
    let pattern = format!(
        "%{}%",
        page.q
            .replace('!', "!!")
            .replace('%', "!%")
            .replace('_', "!_")
    );
    let rows = sqlx::query("SELECT rowid AS cursor,* FROM briefings WHERE archived=? AND rowid>? AND (json_extract(body,'$.title') LIKE ? ESCAPE '!' OR json_extract(body,'$.recipient') LIKE ? ESCAPE '!') ORDER BY rowid LIMIT ?")
        .bind(page.archived).bind(page.after).bind(&pattern).bind(&pattern).bind(page.limit+1).fetch_all(&state.pool).await?;
    let has_more = rows.len() > page.limit as usize;
    let rows = &rows[..rows.len().min(page.limit as usize)];
    let items: Vec<Value> = rows.iter().map(record).collect::<Result<_>>()?;
    Ok(Json(
        json!({"items":items,"has_more":has_more,"next_cursor":rows.last().map(|r| r.get::<i64,_>("cursor")).unwrap_or(page.after)}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditionPage {
    #[serde(default)]
    pub before: i64,
}
pub async fn editions(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(page): Query<EditionPage>,
) -> Result<Json<Value>> {
    if page.before < 0 {
        return Err(invalid("Cursor ungültig."));
    }
    read(&mut state.pool.begin().await?, &id).await?;
    let rows = sqlx::query("SELECT e.revision,e.sha256,e.actor,e.created_at,h.actor AS handed_over_by,h.note AS handover_note,h.created_at AS handed_over_at FROM briefing_editions e LEFT JOIN briefing_handovers h USING(briefing_id,revision) WHERE e.briefing_id=? AND (?=0 OR e.revision<?) ORDER BY e.revision DESC LIMIT 31")
        .bind(&id).bind(page.before).bind(page.before).fetch_all(&state.pool).await?;
    let items: Vec<Value> = rows.iter().take(30).map(|r| json!({"revision":r.get::<i64,_>("revision"),"sha256":r.get::<String,_>("sha256"),"actor":r.get::<String,_>("actor"),"created_at":r.get::<String,_>("created_at"),"handed_over_by":r.get::<Option<String>,_>("handed_over_by"),"handover_note":r.get::<Option<String>,_>("handover_note"),"handed_over_at":r.get::<Option<String>,_>("handed_over_at")})).collect();
    Ok(Json(
        json!({"has_more":rows.len()>30,"next_cursor":items.last().map(|v| &v["revision"]),"items":items}),
    ))
}
pub async fn download(
    State(state): State<AppState>,
    Path((id, revision)): Path<(String, i64)>,
) -> Result<Response> {
    // Safe ASCII filename regardless of a malformed user-supplied path segment.
    let safe_id = uuid::Uuid::parse_str(&id)
        .map_err(|_| missing())?
        .to_string();
    let text: String =
        sqlx::query_scalar("SELECT text FROM briefing_editions WHERE briefing_id=? AND revision=?")
            .bind(&safe_id)
            .bind(revision)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(missing)?;
    Ok((
        [
            ("content-type", "text/plain; charset=utf-8".to_string()),
            (
                "content-disposition",
                format!("attachment; filename=\"briefing-{safe_id}-v{revision}.txt\""),
            ),
        ],
        text,
    )
        .into_response())
}
