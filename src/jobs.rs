use crate::{
    auth::{self, Principal},
    command_headers,
    connectors::{plans::Plan, Hub},
    invalid, new_id, AppState, Result,
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Extension, Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};

pub fn snapshot(plan: &Plan) -> Value {
    json!({"method":plan.method.as_str(),"url":plan.url.as_str(),"body":plan.body,"form":plan.form,"safe_retry":plan.safe_retry,"idempotency_header":plan.idempotency_header,"scope":plan.scope})
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobInput {
    pub operation: String,
    pub input: Value,
}
pub async fn connections(State(state): State<AppState>) -> Json<Value> {
    Json(json!({"items":state.connectors.catalog()}))
}
pub async fn enqueue(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(connection_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<JobInput>,
) -> Result<Json<Value>> {
    if state.security.demo {
        return Err(invalid("Externe Ausführung ist in der Demo ausgeschaltet."));
    }
    let (key, actor) = command_headers(&headers)?;
    let plan = state
        .connectors
        .plan(&connection_id, &input.operation, &input.input)?;
    if !principal.allows(plan.scope) {
        return Err(auth::forbidden());
    }
    let request=json!({"connection":connection_id,"operation":input.operation,"input":input.input,"actor":actor}).to_string();
    let mut tx = state.pool.begin().await?;
    if let Some(row) =
        sqlx::query("SELECT id,request,status FROM integration_jobs WHERE idempotency_key=?")
            .bind(&key)
            .fetch_optional(&mut *tx)
            .await?
    {
        if row.get::<String, _>("request") != request {
            return Err(crate::conflict(
                "Idempotency-Key bezeichnet einen anderen Integrationsauftrag.",
            ));
        }
        return Ok(Json(
            json!({"id":row.get::<String,_>("id"),"status":row.get::<String,_>("status")}),
        ));
    }
    let id = new_id();
    let time = auth::now();
    sqlx::query("INSERT INTO integration_jobs(id,idempotency_key,request,connection_id,operation,payload,plan,actor,user_id,required_scope,safe_retry,status,next_attempt,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,'queued',?,?,?)")
        .bind(&id).bind(key).bind(request).bind(connection_id).bind(input.operation).bind(input.input.to_string()).bind(snapshot(&plan).to_string()).bind(&actor).bind(principal.id).bind(plan.scope).bind(plan.safe_retry).bind(time).bind(time).bind(time).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO security_events(actor,action,target) VALUES(?,'integration.queued',?)",
    )
    .bind(actor)
    .bind(&id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"status":"queued"})))
}
pub async fn list(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<Value>> {
    let rows=sqlx::query("SELECT id,connection_id,operation,status,attempts,error_code,result,created_at,updated_at FROM integration_jobs WHERE user_id=? OR ?='admin' ORDER BY created_at DESC LIMIT 100").bind(p.id).bind(p.role).fetch_all(&state.pool).await?;
    Ok(Json(
        json!({"items":rows.iter().map(|r|json!({"id":r.get::<String,_>("id"),"connection_id":r.get::<String,_>("connection_id"),"operation":r.get::<String,_>("operation"),"status":r.get::<String,_>("status"),"attempts":r.get::<i64,_>("attempts"),"error_code":r.get::<Option<String>,_>("error_code"),"result":r.get::<Option<String>,_>("result").and_then(|s|serde_json::from_str::<Value>(&s).ok()),"created_at":r.get::<i64,_>("created_at"),"updated_at":r.get::<i64,_>("updated_at")})).collect::<Vec<_>>()}),
    ))
}
pub async fn cancel(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    let result=sqlx::query("UPDATE integration_jobs SET status='cancelled',updated_at=? WHERE id=? AND status IN ('queued','retry') AND (user_id=? OR ?='admin')").bind(auth::now()).bind(&id).bind(&p.id).bind(p.role).execute(&mut *tx).await?;
    if result.rows_affected() != 1 {
        return Err(crate::conflict(
            "Auftrag ist nicht mehr abbrechbar oder nicht zugänglich.",
        ));
    }
    sqlx::query(
        "INSERT INTO security_events(actor,action,target) VALUES(?,'integration.cancelled',?)",
    )
    .bind(p.actor)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"status":"cancelled"})))
}
/// Claims commit before network I/O. Crashes during non-idempotent requests never trigger blind retries.
pub async fn run_one(pool: &SqlitePool, hub: &Hub) -> Result<bool> {
    let time = auth::now();
    let worker = new_id();
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE integration_jobs SET status=CASE WHEN safe_retry=1 THEN 'retry' ELSE 'uncertain' END,error_code='lease_expired',lease_owner=NULL,lease_until=NULL WHERE status='running' AND lease_until<?").bind(time).execute(&mut *tx).await?;
    let Some(row)=sqlx::query("SELECT * FROM integration_jobs WHERE status IN ('queued','retry') AND next_attempt<=? ORDER BY created_at LIMIT 1").bind(time).fetch_optional(&mut *tx).await? else {tx.commit().await?;return Ok(false);};
    let id: String = row.get("id");
    let user: String = row.get("user_id");
    let required: String = row.get("required_scope");
    let actor: String = row.get("actor");
    let role: Option<String> =
        sqlx::query_scalar("SELECT role FROM users WHERE id=? AND disabled=0")
            .bind(&user)
            .fetch_optional(&mut *tx)
            .await?;
    let principal = Principal {
        id: user,
        name: String::new(),
        role: role.unwrap_or_default(),
        actor: actor.clone(),
        scopes: vec!["*".into()],
        session_hash: None,
        csrf: None,
    };
    let mut permitted = principal.allows(&required);
    if let Some((_, token_id)) = actor.split_once("/token:") {
        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM api_tokens WHERE id=? AND revoked=0 AND expires_at>?",
        )
        .bind(token_id)
        .bind(time)
        .fetch_one(&mut *tx)
        .await?;
        permitted &= active == 1;
    }
    if !permitted {
        sqlx::query("UPDATE integration_jobs SET status='failed',error_code='authorization_revoked',updated_at=? WHERE id=?").bind(time).bind(&id).execute(&mut *tx).await?;
        tx.commit().await?;
        return Ok(true);
    }
    sqlx::query("UPDATE integration_jobs SET status='running',attempts=attempts+1,lease_owner=?,lease_until=?,updated_at=? WHERE id=?").bind(&worker).bind(time+180).bind(time).bind(&id).execute(&mut *tx).await?;
    tx.commit().await?;
    let connection: String = row.get("connection_id");
    let operation: String = row.get("operation");
    let input: Value =
        serde_json::from_str(row.get("payload")).map_err(|_| invalid("Ungültiger Auftrag."))?;
    let expected: Value =
        serde_json::from_str(row.get("plan")).map_err(|_| invalid("Ungültiger Auftragsplan."))?;
    let result = match hub.plan(&connection, &operation, &input) {
        Ok(plan) if snapshot(&plan) == expected => {
            hub.execute(&connection, &operation, &input, &id).await
        }
        Ok(_) => Err(invalid("Adapterkonfiguration wurde geändert.")),
        Err(e) => Err(e),
    };
    let attempts = row.get::<i64, _>("attempts") + 1;
    let safe = row.get::<i64, _>("safe_retry") == 1;
    let (status, data, error) = match result {
        Ok(value) => ("succeeded", Some(value.to_string()), None),
        Err(e) => {
            let uncertain = matches!(
                e.1,
                "delivery_uncertain"
                    | "provider_unavailable"
                    | "provider_response"
                    | "provider_response_format"
                    | "provider_response_limit"
            ) && !safe;
            let retry = safe
                && attempts < 5
                && matches!(
                    e.1,
                    "provider_network"
                        | "provider_rate_limit"
                        | "provider_unavailable"
                        | "authentication_network"
                        | "secret_timeout"
                );
            (
                if uncertain {
                    "uncertain"
                } else if retry {
                    "retry"
                } else {
                    "failed"
                },
                None,
                Some(e.1.to_string()),
            )
        }
    };
    let next = auth::now() + 5 * 2i64.pow(attempts.min(8) as u32);
    sqlx::query("UPDATE integration_jobs SET status=?,result=?,error_code=?,next_attempt=?,lease_owner=NULL,lease_until=NULL,updated_at=? WHERE id=? AND lease_owner=?")
        .bind(status).bind(data).bind(error).bind(next).bind(auth::now()).bind(id).bind(worker).execute(pool).await?;
    Ok(true)
}
