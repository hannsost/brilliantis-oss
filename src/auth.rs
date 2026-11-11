use crate::{invalid, new_id, storage::hash, AppState, Error, Result};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Semaphore;

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub(crate) fn secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
pub fn unauthorized() -> Error {
    Error(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "Bitte anmelden oder einen gültigen Agentenzugang verwenden.".into(),
    )
}
pub fn forbidden() -> Error {
    Error(
        StatusCode::FORBIDDEN,
        "forbidden",
        "Für diese Aktion fehlt die Berechtigung.".into(),
    )
}

#[derive(Clone)]
pub struct Security {
    pub origin: String,
    pub authority: String,
    pub secure: bool,
    pub demo: bool,
    attempts: Arc<Mutex<HashMap<String, (i64, usize)>>>,
    password_workers: Arc<Semaphore>,
}
impl Security {
    pub fn new(origin: &str, demo: bool) -> Result<Self> {
        let url = url::Url::parse(origin).map_err(|_| invalid("public-url ist ungültig."))?;
        if !["http", "https"].contains(&url.scheme())
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
            || url.host_str().is_none()
        {
            return Err(invalid(
                "public-url muss ein HTTP(S)-Origin ohne Pfad oder Zugangsdaten sein.",
            ));
        }
        let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if (url.scheme() != "https" && !local) || (demo && !local) {
            return Err(invalid(
                "Netzwerkbetrieb benötigt HTTPS; eine ungeschützte Demo bleibt lokal.",
            ));
        }
        let origin = url.origin().ascii_serialization();
        let authority = origin.split_once("://").unwrap().1.to_string();
        Ok(Self {
            origin,
            authority,
            secure: url.scheme() == "https",
            demo,
            attempts: Arc::new(Mutex::new(HashMap::new())),
            password_workers: Arc::new(Semaphore::new(2)),
        })
    }
    pub fn local_demo(port: u16) -> Self {
        Self::new(&format!("http://127.0.0.1:{port}"), true).unwrap()
    }
    fn cookie_name(&self) -> &'static str {
        if self.secure {
            "__Host-brilliantis_session"
        } else {
            "brilliantis_session"
        }
    }
    pub(crate) fn cookie(&self, value: &str, age: i64) -> String {
        format!(
            "{}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={age}{}",
            self.cookie_name(),
            if self.secure { "; Secure" } else { "" }
        )
    }
    fn attempt(&self, identity: &str) -> Result<()> {
        let mut limits = self.attempts.lock().map_err(|_| rate_limited())?;
        let time = now();
        limits.retain(|_, (started, _)| time - *started < 900);
        if limits.len() >= 2048 {
            return Err(rate_limited());
        }
        for (key, maximum) in [("global".to_owned(), 100), (hash(identity), 8)] {
            let item = limits.entry(key).or_insert((time, 0));
            item.1 += 1;
            if item.1 > maximum {
                return Err(rate_limited());
            }
        }
        Ok(())
    }
}
fn rate_limited() -> Error {
    Error(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limited",
        "Zu viele Anmeldeversuche. Bitte später erneut versuchen.".into(),
    )
}

#[derive(Clone, Debug)]
pub struct Principal {
    pub id: String,
    pub name: String,
    pub role: String,
    pub actor: String,
    pub scopes: Vec<String>,
    pub session_hash: Option<String>,
    pub csrf: Option<String>,
}
impl Principal {
    pub fn allows(&self, scope: &str) -> bool {
        let role_allows = match self.role.as_str() {
            "machine" => matches!(
                scope,
                "crm:write" | "commerce:read" | "commerce:write" | "commerce:billing"
            ),
            "admin" => true,
            "sales" => matches!(
                scope,
                "crm:read" | "crm:write" | "finance:read" | "commerce:read" | "commerce:write"
            ),
            "accountant" => matches!(
                scope,
                "crm:read" | "finance:read" | "finance:write" | "commerce:read"
            ),
            "viewer" => matches!(scope, "crm:read" | "finance:read" | "commerce:read"),
            _ => false,
        };
        role_allows && (self.scopes.iter().any(|s| s == "*" || s == scope))
    }
}
fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|h| h.to_str().ok())
}
pub(crate) fn session_cookie<'a>(security: &Security, headers: &'a HeaderMap) -> Option<&'a str> {
    let values: Vec<_> = header(headers, "cookie")?
        .split(';')
        .filter_map(|piece| piece.trim().split_once('='))
        .filter(|(name, _)| *name == security.cookie_name())
        .collect();
    if values.len() == 1 {
        Some(values[0].1)
    } else {
        None
    }
}

pub async fn identify(state: &AppState, headers: &HeaderMap) -> Result<Principal> {
    if state.security.demo {
        return Ok(Principal {
            id: "demo".into(),
            name: "Lokale Demo".into(),
            role: "admin".into(),
            actor: "demo".into(),
            scopes: vec!["*".into()],
            session_hash: None,
            csrf: None,
        });
    }
    if let Some(auth) = header(headers, "authorization") {
        let bearer = auth.strip_prefix("Bearer ").ok_or_else(unauthorized)?;
        if bearer.starts_with("brm_") {
            return crate::machine::identify(&state.pool, bearer).await;
        }
        if !bearer.starts_with("brl_") {
            return state
                .oidc
                .as_ref()
                .ok_or_else(unauthorized)?
                .identify(&state.pool, bearer)
                .await;
        }
        let token = auth
            .strip_prefix("Bearer ")
            .filter(|value| value.starts_with("brl_") && value.len() == 68)
            .ok_or_else(unauthorized)?;
        let row = sqlx::query("SELECT t.id AS token_id,t.scopes,u.id,u.name,u.role FROM api_tokens t JOIN users u ON u.id=t.user_id WHERE t.token_hash=? AND t.revoked=0 AND t.expires_at>? AND u.disabled=0")
            .bind(hash(token)).bind(now()).fetch_optional(&state.pool).await?.ok_or_else(unauthorized)?;
        let id: String = row.get("id");
        let token_id: String = row.get("token_id");
        return Ok(Principal {
            actor: format!("user:{id}/token:{token_id}"),
            id,
            name: row.get("name"),
            role: row.get("role"),
            scopes: serde_json::from_str(row.get("scopes")).map_err(|_| unauthorized())?,
            session_hash: None,
            csrf: None,
        });
    }
    let token = session_cookie(&state.security, headers)
        .filter(|v| v.len() == 64)
        .ok_or_else(unauthorized)?;
    let token_hash = hash(token);
    let row = sqlx::query("SELECT u.id,u.name,u.role,s.csrf,s.oidc_issuer FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=? AND s.expires_at>? AND s.last_seen>? AND u.disabled=0 AND (s.oidc_issuer IS NULL OR EXISTS(SELECT 1 FROM oidc_identities i WHERE i.issuer=s.oidc_issuer AND i.subject=s.oidc_subject AND i.user_id=u.id))")
        .bind(&token_hash).bind(now()).bind(now()-1800).fetch_optional(&state.pool).await?.ok_or_else(unauthorized)?;
    if let Some(issuer) = row.get::<Option<String>, _>("oidc_issuer") {
        if state
            .oidc
            .as_ref()
            .is_none_or(|o| !o.browser_enabled() || o.issuer() != issuer)
        {
            return Err(unauthorized());
        }
    }
    sqlx::query("UPDATE sessions SET last_seen=? WHERE token_hash=?")
        .bind(now())
        .bind(&token_hash)
        .execute(&state.pool)
        .await?;
    let id: String = row.get("id");
    Ok(Principal {
        actor: format!("user:{id}"),
        id,
        name: row.get("name"),
        role: row.get("role"),
        scopes: vec!["*".into()],
        session_hash: Some(token_hash),
        csrf: Some(row.get("csrf")),
    })
}

pub fn authorize(
    principal: &Principal,
    headers: &HeaderMap,
    method: &str,
    path: &str,
    security: &Security,
) -> Result<()> {
    // These operator-issued service identities only serve website/portal transport.
    // They cannot create human tokens, sessions or provider integration jobs.
    if principal.role == "machine"
        && path != "/api/v1/import/website"
        && !path.starts_with("/api/v1/commerce/")
    {
        return Err(forbidden());
    }
    let writing = !matches!(method, "GET" | "HEAD" | "OPTIONS");
    if writing
        && principal.session_hash.is_some()
        && (header(headers, "origin") != Some(security.origin.as_str())
            || header(headers, "x-csrf-token") != principal.csrf.as_deref())
    {
        return Err(Error(
            StatusCode::FORBIDDEN,
            "csrf",
            "Die Sitzung muss neu geladen werden.".into(),
        ));
    }
    if path.starts_with("/api/v1/connections") || path.starts_with("/api/v1/jobs") {
        return if principal.allows(if writing { "crm:write" } else { "crm:read" })
            || principal.allows(if writing {
                "finance:write"
            } else {
                "finance:read"
            }) {
            Ok(())
        } else {
            Err(forbidden())
        };
    }
    let financial_resource = path
        .strip_prefix("/api/v1/records/")
        .and_then(|p| p.split('/').next())
        .is_some_and(|r| {
            [
                "invoices",
                "credit-notes",
                "bank-accounts",
                "bank-transactions",
            ]
            .contains(&r)
        });
    let scope = if path.starts_with("/api/v1/commerce/") {
        if !writing {
            "commerce:read"
        } else if path.ends_with("/catalog") || path.ends_with("/price") {
            "admin"
        } else if path.ends_with("/billing") {
            "commerce:billing"
        } else if path.ends_with("/provisioning") {
            "commerce:fulfill"
        } else if path.ends_with("/invoice") {
            "finance:write"
        } else {
            "commerce:write"
        }
    } else if path.starts_with("/api/v1/access/users")
        || path.starts_with("/api/v1/settings")
        || path.starts_with("/api/v1/security-events")
    {
        "admin"
    } else if path.starts_with("/api/v1/documents/invoice/")
        || path.starts_with("/api/v1/documents/credit_note/")
        || financial_resource
        || path.starts_with("/api/v1/invoices")
        || path.starts_with("/api/v1/payments")
        || path.starts_with("/api/v1/finance")
    {
        if writing {
            "finance:write"
        } else {
            "finance:read"
        }
    } else if path.starts_with("/api/v1/auth/") || path.starts_with("/api/v1/access/tokens") {
        return Ok(());
    } else if writing {
        "crm:write"
    } else {
        "crm:read"
    };
    if !principal.allows(scope) {
        return Err(forbidden());
    }
    Ok(())
}

pub async fn password_hash(password: String) -> Result<String> {
    if password.len() < 12 || password.len() > 1024 {
        return Err(invalid("Passwort benötigt 12 bis 1024 Bytes."));
    }
    tokio::task::spawn_blocking(move || {
        let entropy = uuid::Uuid::new_v4();
        let salt = SaltString::encode_b64(entropy.as_bytes())
            .map_err(|_| invalid("Passwort konnte nicht verarbeitet werden."))?;
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|v| v.to_string())
            .map_err(|_| invalid("Passwort konnte nicht verarbeitet werden."))
    })
    .await
    .map_err(|_| invalid("Passwortverarbeitung fehlgeschlagen."))?
}
async fn password_matches(password: String, encoded: String) -> bool {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&encoded).is_ok_and(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
    })
    .await
    .unwrap_or(false)
}
pub(crate) fn email(value: &str) -> Result<String> {
    let value = value.trim().to_lowercase();
    if value.len() > 254
        || value.matches('@').count() != 1
        || value.starts_with('@')
        || value.ends_with('@')
        || value.chars().any(char::is_whitespace)
    {
        return Err(invalid("Gültige E-Mail-Adresse angeben."));
    }
    Ok(value)
}
pub async fn bootstrap(pool: &SqlitePool, address: &str, password: String) -> Result<()> {
    let address = email(address)?;
    let encoded = password_hash(password).await?;
    let mut tx = pool.begin().await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&mut *tx)
        .await?;
    if count != 0 {
        return Err(crate::conflict("Die Instanz wurde bereits eingerichtet."));
    }
    let id = new_id();
    sqlx::query("INSERT INTO users(id,email,name,password_hash,role) VALUES(?,?,?,?, 'admin')")
        .bind(&id)
        .bind(&address)
        .bind(&address)
        .bind(encoded)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO security_events(actor,action,target) VALUES('local.bootstrap','user.bootstrap',?)").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Login {
    email: String,
    password: String,
}
pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Login>,
) -> Result<Response> {
    if state.security.demo {
        return Err(invalid("Die Demo verwendet keine Benutzerkonten."));
    }
    if header(&headers, "origin") != Some(state.security.origin.as_str()) {
        return Err(forbidden());
    }
    state.security.attempt(&input.email.to_lowercase())?;
    if input.email.len() > 254 || input.password.len() > 1024 {
        return Err(unauthorized());
    }
    let _permit = state
        .security
        .password_workers
        .try_acquire()
        .map_err(|_| rate_limited())?;
    let row=sqlx::query("SELECT id,name,role,password_hash FROM users WHERE email=? AND disabled=0 AND password_hash IS NOT NULL").bind(input.email.trim().to_lowercase()).fetch_optional(&state.pool).await?;
    // A valid dummy hash keeps unknown users on the same password verification path.
    let encoded=row.as_ref().map(|r|r.get::<String,_>("password_hash")).unwrap_or_else(|| "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$U4PaXgDlAzat2NZvI78SqGPaSVRsSnGuQIvZVo1AuZw".into());
    let matched = password_matches(input.password, encoded).await;
    if !matched || row.is_none() {
        return Err(unauthorized());
    }
    let row = row.unwrap();
    let id: String = row.get("id");
    let token = secret();
    let csrf = secret();
    let time = now();
    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM sessions WHERE expires_at<? OR last_seen<?")
        .bind(time)
        .bind(time - 1800)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO sessions(token_hash,user_id,csrf,created_at,expires_at,last_seen) VALUES(?,?,?,?,?,?)")
        .bind(hash(&token)).bind(&id).bind(&csrf).bind(time).bind(time+43200).bind(time).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO security_events(actor,action,target) VALUES(?,'session.login',?)")
        .bind(format!("user:{id}"))
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(([("set-cookie",state.security.cookie(&token,43200))],Json(json!({"authenticated":true,"user":{"id":id,"name":row.get::<String,_>("name"),"role":row.get::<String,_>("role")},"csrf":csrf}))).into_response())
}
pub async fn session(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>> {
    let oidc = state.oidc.as_ref().map(|o| o.login_descriptor());
    match identify(&state, &headers).await {
        Ok(p) => Ok(Json(
            json!({"authenticated":true,"demo":state.security.demo,"user":{"id":p.id,"name":p.name,"role":p.role},"csrf":p.csrf,"oidc":oidc}),
        )),
        Err(e) if e.0 == StatusCode::UNAUTHORIZED => {
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
                .fetch_one(&state.pool)
                .await?;
            Ok(Json(
                json!({"authenticated":false,"setup_required":count==0,"distribution":state.profile.descriptor()["label"],"oidc":oidc}),
            ))
        }
        Err(e) => Err(e),
    }
}
pub async fn logout(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Response> {
    crate::oidc::browser::logout(&state, &p).await
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewUser {
    #[serde(default)]
    oidc_subject: Option<String>,
    email: String,
    name: String,
    role: String,
    password: String,
}
pub async fn create_user(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Json(input): Json<NewUser>,
) -> Result<Json<Value>> {
    let address = email(&input.email)?;
    if input
        .oidc_subject
        .as_ref()
        .is_some_and(|s| s.is_empty() || s.len() > 200)
    {
        return Err(invalid("OIDC-Subject ungültig."));
    }
    if !["admin", "sales", "accountant", "viewer"].contains(&input.role.as_str())
        || input.name.trim().is_empty()
        || input.name.len() > 160
    {
        return Err(invalid("Name oder Rolle ungültig."));
    }
    let _permit = state
        .security
        .password_workers
        .acquire()
        .await
        .map_err(|_| rate_limited())?;
    let encoded = password_hash(input.password).await?;
    let id = new_id();
    let mut tx = state.pool.begin().await?;
    let exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE email=? OR oidc_subject=?")
            .bind(&address)
            .bind(&input.oidc_subject)
            .fetch_one(&mut *tx)
            .await?;
    if exists > 0 {
        return Err(crate::conflict("Benutzerkonto besteht bereits."));
    }
    sqlx::query(
        "INSERT INTO users(id,email,name,role,password_hash,oidc_subject) VALUES(?,?,?,?,?,?)",
    )
    .bind(&id)
    .bind(&address)
    .bind(input.name.trim())
    .bind(&input.role)
    .bind(encoded)
    .bind(&input.oidc_subject)
    .execute(&mut *tx)
    .await?;
    if let Some(subject) = &input.oidc_subject {
        let oidc = state
            .oidc
            .as_ref()
            .ok_or_else(|| invalid("Zuerst OIDC-Issuer konfigurieren."))?;
        let exists: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM oidc_identities WHERE issuer=? AND subject=?")
                .bind(oidc.issuer())
                .bind(subject)
                .fetch_one(&mut *tx)
                .await?;
        if exists > 0 {
            return Err(crate::conflict("OIDC-Identität ist bereits zugeordnet."));
        }
        sqlx::query("INSERT INTO oidc_identities(issuer,subject,user_id) VALUES(?,?,?)")
            .bind(oidc.issuer())
            .bind(subject)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("INSERT INTO security_events(actor,action,target) VALUES(?,'user.create',?)")
        .bind(p.actor)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"id":id,"email":address,"name":input.name.trim(),"role":input.role}),
    ))
}
pub async fn users(State(state): State<AppState>) -> Result<Json<Value>> {
    let rows=sqlx::query("SELECT id,email,name,role,disabled,created_at,version FROM users ORDER BY created_at DESC LIMIT 100").fetch_all(&state.pool).await?;
    Ok(Json(
        json!({"items":rows.iter().map(|r|json!({"id":r.get::<String,_>("id"),"email":r.get::<String,_>("email"),"name":r.get::<String,_>("name"),"role":r.get::<String,_>("role"),"version":r.get::<i64,_>("version"),"disabled":r.get::<i64,_>("disabled")!=0,"created_at":r.get::<i64,_>("created_at")})).collect::<Vec<_>>()}),
    ))
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TokenInput {
    name: String,
    scopes: Vec<String>,
    expires_in_days: i64,
}
pub async fn create_token(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Json(input): Json<TokenInput>,
) -> Result<Json<Value>> {
    if p.session_hash.is_none() {
        return Err(forbidden());
    }
    if input.name.trim().is_empty()
        || input.name.len() > 120
        || !(1..=365).contains(&input.expires_in_days)
        || input.scopes.is_empty()
        || input.scopes.len() > 8
        || input.scopes.iter().any(|s| {
            ![
                "crm:read",
                "crm:write",
                "finance:read",
                "finance:write",
                "admin",
                "commerce:read",
                "commerce:write",
                "commerce:fulfill",
                "commerce:billing",
            ]
            .contains(&s.as_str())
                || !p.allows(s)
        })
    {
        return Err(invalid(
            "Name, Laufzeit oder Rechte des Zugangs sind ungültig.",
        ));
    }
    let token = format!("brl_{}", secret());
    let id = new_id();
    let expires = now() + input.expires_in_days * 86400;
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO api_tokens(id,user_id,name,token_hash,scopes,created_at,expires_at) VALUES(?,?,?,?,?,?,?)")
        .bind(&id).bind(&p.id).bind(input.name.trim()).bind(hash(&token)).bind(json!(input.scopes).to_string()).bind(now()).bind(expires).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO security_events(actor,action,target) VALUES(?,'token.create',?)")
        .bind(p.actor)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"id":id,"name":input.name.trim(),"token":token,"expires_at":expires,"scopes":input.scopes}),
    ))
}
pub async fn tokens(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<Value>> {
    let rows=sqlx::query("SELECT id,name,scopes,created_at,expires_at,revoked FROM api_tokens WHERE user_id=? ORDER BY created_at DESC LIMIT 100").bind(p.id).fetch_all(&state.pool).await?;
    Ok(Json(
        json!({"items":rows.iter().map(|r|json!({"id":r.get::<String,_>("id"),"name":r.get::<String,_>("name"),"scopes":serde_json::from_str::<Value>(r.get("scopes")).unwrap_or(Value::Null),"expires_at":r.get::<i64,_>("expires_at"),"revoked":r.get::<i64,_>("revoked")!=0})).collect::<Vec<_>>()}),
    ))
}
pub async fn revoke_token(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    let result = sqlx::query("UPDATE api_tokens SET revoked=1 WHERE id=? AND user_id=?")
        .bind(&id)
        .bind(p.id)
        .execute(&mut *tx)
        .await?;
    if result.rows_affected() == 0 {
        return Err(Error(
            StatusCode::NOT_FOUND,
            "not_found",
            "Zugang nicht gefunden.".into(),
        ));
    }
    sqlx::query("INSERT INTO security_events(actor,action,target) VALUES(?,'token.revoke',?)")
        .bind(p.actor)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"revoked":true})))
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserUpdate {
    pub version: i64,
    pub name: String,
    pub role: String,
    pub disabled: bool,
}
pub async fn update_user(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
    Json(input): Json<UserUpdate>,
) -> Result<Json<Value>> {
    if input.name.trim().is_empty()
        || input.name.len() > 160
        || !["admin", "sales", "accountant", "viewer"].contains(&input.role.as_str())
    {
        return Err(invalid("Name oder Rolle ungültig."));
    }
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("SELECT role,disabled,version FROM users WHERE id=?")
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(unauthorized)?;
    if row.get::<i64, _>("version") != input.version {
        return Err(crate::conflict("Benutzerkonto wurde geändert."));
    }
    if row.get::<String, _>("role") == "admin"
        && row.get::<i64, _>("disabled") == 0
        && (input.disabled || input.role != "admin")
    {
        let admins: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role='admin' AND disabled=0")
                .fetch_one(&mut *tx)
                .await?;
        if admins <= 1 {
            return Err(crate::conflict(
                "Mindestens ein aktiver Administrator muss verbleiben.",
            ));
        }
    }
    sqlx::query("UPDATE users SET name=?,role=?,disabled=?,version=version+1 WHERE id=?")
        .bind(input.name.trim())
        .bind(&input.role)
        .bind(input.disabled)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    if input.disabled {
        sqlx::query("DELETE FROM sessions WHERE user_id=?")
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE api_tokens SET revoked=1 WHERE user_id=?")
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("INSERT INTO security_events(actor,action,target) VALUES(?,'user.update',?)")
        .bind(p.actor)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"id":id,"version":input.version+1,"role":input.role,"disabled":input.disabled}),
    ))
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PasswordChange {
    current_password: String,
    new_password: String,
}
pub async fn change_password(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Json(input): Json<PasswordChange>,
) -> Result<Response> {
    if p.session_hash.is_none() {
        return Err(forbidden());
    }
    state.security.attempt(&p.id)?;
    let _permit = state
        .security
        .password_workers
        .acquire()
        .await
        .map_err(|_| rate_limited())?;
    let old: Option<String> = sqlx::query_scalar("SELECT password_hash FROM users WHERE id=?")
        .bind(&p.id)
        .fetch_one(&state.pool)
        .await?;
    let old = old.ok_or_else(unauthorized)?;
    if input.current_password.len() > 1024
        || !password_matches(input.current_password, old.clone()).await
    {
        return Err(unauthorized());
    }
    let encoded = password_hash(input.new_password).await?;
    let mut tx = state.pool.begin().await?;
    let result = sqlx::query(
        "UPDATE users SET password_hash=?,version=version+1 WHERE id=? AND password_hash=?",
    )
    .bind(encoded)
    .bind(&p.id)
    .bind(old)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() != 1 {
        return Err(crate::conflict("Passwort wurde zwischenzeitlich geändert."));
    }
    sqlx::query("DELETE FROM sessions WHERE user_id=?")
        .bind(&p.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE api_tokens SET revoked=1 WHERE user_id=?")
        .bind(&p.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO security_events(actor,action,target) VALUES(?,'password.change',?)")
        .bind(p.actor)
        .bind(p.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((
        [("set-cookie", state.security.cookie("", 0))],
        Json(json!({"reauthenticate":true})),
    )
        .into_response())
}
