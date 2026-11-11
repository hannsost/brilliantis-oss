//! Authorization Code + PKCE/S256 for Keycloak. Provider tokens never reach application JS.
use super::Oidc;
use crate::{auth, conflict, invalid, new_id, storage::hash, AppState, Error, Result};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Extension, Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{decode, decode_header, jwk::JwkSet, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};

const FLOW_TTL: i64 = 600;
const SESSION_TTL: i64 = 28800;
pub const CALLBACK: &str = "/api/v1/auth/oidc/callback";
pub const LOGOUT_CALLBACK: &str = "/api/v1/auth/oidc/logout/callback";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapAdmin {
    pub subject: String,
    pub email: String,
    pub name: String,
}
fn subject(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 200 || value.chars().any(char::is_control) {
        return Err(invalid("OIDC-Subject ungültig."));
    }
    Ok(())
}
impl Oidc {
    fn endpoint(&self, suffix: &str) -> String {
        format!("{}/protocol/openid-connect/{suffix}", self.config.issuer)
    }
    /// Trusted operator setup, never driven by an email claim. Idempotent after first binding.
    pub async fn bootstrap_identity(&self, pool: &SqlitePool) -> Result<()> {
        let Some(p) = &self.config.bootstrap_admin else {
            return Ok(());
        };
        subject(&p.subject)?;
        let address = auth::email(&p.email)?;
        if p.name.trim().is_empty() || p.name.len() > 160 {
            return Err(invalid("OIDC-Administratorname ungültig."));
        }
        let mut tx = pool.begin().await?;
        if let Some(row)=sqlx::query("SELECT u.email FROM oidc_identities i JOIN users u ON u.id=i.user_id WHERE i.issuer=? AND i.subject=?")
            .bind(&self.config.issuer).bind(&p.subject).fetch_optional(&mut *tx).await? {
            if row.get::<String,_>("email").to_lowercase()!=address {return Err(conflict("OIDC-Identität gehört bereits zu einem anderen Konto."));}
            // Do not re-enable accounts or undo a subsequent operator role change on restart.
            return Ok(());
        }
        let existing = sqlx::query("SELECT id,disabled FROM users WHERE email=?")
            .bind(&address)
            .fetch_optional(&mut *tx)
            .await?;
        let id = if let Some(row) = existing {
            let id: String = row.get("id");
            let linked: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM oidc_identities WHERE user_id=?")
                    .bind(&id)
                    .fetch_one(&mut *tx)
                    .await?;
            if linked > 0 || row.get::<i64, _>("disabled") != 0 {
                return Err(conflict("Bestehendes Konto ist bereits verknüpft oder deaktiviert; Administrator muss Zuordnung explizit prüfen."));
            }
            sqlx::query("UPDATE users SET role='admin',version=version+1 WHERE id=?")
                .bind(&id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM sessions WHERE user_id=?")
                .bind(&id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE api_tokens SET revoked=1 WHERE user_id=?")
                .bind(&id)
                .execute(&mut *tx)
                .await?;
            id
        } else {
            let id = new_id();
            sqlx::query("INSERT INTO users(id,email,name,role) VALUES(?,?,?,'admin')")
                .bind(&id)
                .bind(&address)
                .bind(p.name.trim())
                .execute(&mut *tx)
                .await?;
            id
        };
        sqlx::query("INSERT INTO oidc_identities(issuer,subject,user_id) VALUES(?,?,?)")
            .bind(&self.config.issuer)
            .bind(&p.subject)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO security_events(actor,action,target) VALUES('operator.config','user.oidc_bootstrap',?)").bind(&id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    async fn json_response(&self, response: reqwest::Response) -> Result<Value> {
        if !response.status().is_success() || response.content_length().is_some_and(|n| n > 65536) {
            return Err(auth::unauthorized());
        }
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| auth::unauthorized())? {
            if bytes.len() + chunk.len() > 65536 {
                return Err(auth::unauthorized());
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| auth::unauthorized())
    }
    async fn exchange(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
        nonce_hash: &str,
        created: i64,
    ) -> Result<Value> {
        let _permit = self
            .gate
            .acquire()
            .await
            .map_err(|_| auth::unauthorized())?;
        let secret = self.config.client_secret.resolve().await?;
        let response = self
            .client
            .post(self.endpoint("token"))
            .basic_auth(&self.config.client_id, Some(secret))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", redirect_uri),
                ("code_verifier", verifier),
            ])
            .send()
            .await
            .map_err(|_| auth::unauthorized())?;
        let response = self.json_response(response).await?;
        let token = response["id_token"]
            .as_str()
            .filter(|s| s.len() <= 16000)
            .ok_or_else(auth::unauthorized)?;
        let keys = self
            .client
            .get(self.endpoint("certs"))
            .send()
            .await
            .map_err(|_| auth::unauthorized())?;
        let keys: JwkSet = serde_json::from_value(self.json_response(keys).await?)
            .map_err(|_| auth::unauthorized())?;
        validate_id_token(
            token,
            &keys,
            &self.config.issuer,
            &self.config.client_id,
            nonce_hash,
            created,
        )
    }
}

fn validate_id_token(
    token: &str,
    keys: &JwkSet,
    issuer: &str,
    client: &str,
    nonce_hash: &str,
    created: i64,
) -> Result<Value> {
    let header = decode_header(token).map_err(|_| auth::unauthorized())?;
    if header.alg != Algorithm::RS256 {
        return Err(auth::unauthorized());
    }
    let kid = header
        .kid
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(auth::unauthorized)?;
    let matches: Vec<_> = keys
        .keys
        .iter()
        .filter(|k| k.common.key_id.as_deref() == Some(kid))
        .collect();
    if matches.len() != 1 {
        return Err(auth::unauthorized());
    }
    let key = matches[0];
    if key
        .common
        .public_key_use
        .as_ref()
        .is_some_and(|v| *v != jsonwebtoken::jwk::PublicKeyUse::Signature)
        || key
            .common
            .key_algorithm
            .is_some_and(|v| v != jsonwebtoken::jwk::KeyAlgorithm::RS256)
    {
        return Err(auth::unauthorized());
    }
    let key = DecodingKey::from_jwk(key).map_err(|_| auth::unauthorized())?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[client]);
    validation.set_issuer(&[issuer]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    validation.validate_nbf = true;
    validation.leeway = 0;
    let claims = decode::<Value>(token, &key, &validation)
        .map_err(|_| auth::unauthorized())?
        .claims;
    let nonce = claims["nonce"].as_str().ok_or_else(auth::unauthorized)?;
    let iat = claims["iat"].as_i64().ok_or_else(auth::unauthorized)?;
    if hash(nonce) != nonce_hash
        || iat > auth::now() + 30
        || iat < created - 60
        || claims
            .get("azp")
            .is_some_and(|v| v.as_str() != Some(client))
        || (claims["aud"].as_array().is_some_and(|a| a.len() > 1) && claims["azp"] != client)
    {
        return Err(auth::unauthorized());
    }
    subject(claims["sub"].as_str().ok_or_else(auth::unauthorized)?)
        .map_err(|_| auth::unauthorized())?;
    Ok(claims)
}
fn provider(state: &AppState) -> Result<&Oidc> {
    if state.demo {
        return Err(auth::forbidden());
    }
    state
        .oidc
        .as_ref()
        .filter(|o| o.browser_enabled())
        .ok_or_else(auth::unauthorized)
}
fn flow_cookie_name(state: &AppState) -> &'static str {
    if state.security.secure {
        "__Host-brilliantis_oidc"
    } else {
        "brilliantis_oidc"
    }
}
fn flow_cookie(state: &AppState, value: &str, age: i64) -> String {
    format!(
        "{}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={age}{}",
        flow_cookie_name(state),
        if state.security.secure {
            "; Secure"
        } else {
            ""
        }
    )
}
fn browser_cookie(state: &AppState, headers: &HeaderMap) -> Result<String> {
    let cookies: Vec<_> = headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .filter_map(|p| p.trim().split_once('='))
        .filter(|(name, _)| *name == flow_cookie_name(state))
        .collect();
    if cookies.len() != 1 || cookies[0].1.len() != 64 {
        return Err(auth::unauthorized());
    }
    Ok(cookies[0].1.to_owned())
}
async fn new_flow(
    state: &AppState,
    oidc: &Oidc,
    purpose: &str,
    verifier: &str,
    nonce: &str,
) -> Result<(String, String)> {
    let token = auth::secret();
    let binder = auth::secret();
    let now = auth::now();
    let redirect = format!(
        "{}{}",
        state.security.origin,
        if purpose == "login" {
            CALLBACK
        } else {
            LOGOUT_CALLBACK
        }
    );
    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM oidc_flows WHERE expires_at<=?")
        .bind(now)
        .execute(&mut *tx)
        .await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oidc_flows")
        .fetch_one(&mut *tx)
        .await?;
    if count >= 2048 {
        return Err(Error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Zu viele laufende SSO-Anmeldungen.".into(),
        ));
    }
    sqlx::query("INSERT INTO oidc_flows(state_hash,browser_hash,purpose,issuer,client_id,redirect_uri,verifier,nonce_hash,created_at,expires_at) VALUES(?,?,?,?,?,?,?,?,?,?)")
        .bind(hash(&token)).bind(hash(&binder)).bind(purpose).bind(&oidc.config.issuer).bind(&oidc.config.client_id).bind(redirect).bind(verifier).bind(hash(nonce)).bind(now).bind(now+FLOW_TTL).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((token, binder))
}
async fn consume_flow(
    state: &AppState,
    oidc: &Oidc,
    headers: &HeaderMap,
    token: &str,
    purpose: &str,
) -> Result<sqlx::sqlite::SqliteRow> {
    if token.len() != 64 {
        return Err(auth::unauthorized());
    }
    let binder = browser_cookie(state, headers)?;
    let redirect = format!(
        "{}{}",
        state.security.origin,
        if purpose == "login" {
            CALLBACK
        } else {
            LOGOUT_CALLBACK
        }
    );
    // DELETE RETURNING atomically consumes a browser-bound state before exchanging any code.
    sqlx::query("DELETE FROM oidc_flows WHERE state_hash=? AND browser_hash=? AND purpose=? AND issuer=? AND client_id=? AND redirect_uri=? AND expires_at>? RETURNING *")
        .bind(hash(token)).bind(hash(&binder)).bind(purpose).bind(&oidc.config.issuer).bind(&oidc.config.client_id).bind(redirect).bind(auth::now()).fetch_optional(&state.pool).await?.ok_or_else(auth::unauthorized)
}
pub async fn login(State(state): State<AppState>) -> Result<Response> {
    let oidc = provider(&state)?;
    let verifier = auth::secret();
    let nonce = auth::secret();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let (token, binder) = new_flow(&state, oidc, "login", &verifier, &nonce).await?;
    let mut url = url::Url::parse(&oidc.endpoint("auth")).map_err(|_| auth::unauthorized())?;
    url.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("response_mode", "query"),
        ("client_id", oidc.config.client_id.as_str()),
        (
            "redirect_uri",
            &format!("{}{CALLBACK}", state.security.origin),
        ),
        ("scope", "openid profile email"),
        ("state", &token),
        ("nonce", &nonce),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
    ]);
    Ok((
        [("set-cookie", flow_cookie(&state, &binder, FLOW_TTL))],
        Redirect::to(url.as_str()),
    )
        .into_response())
}
#[derive(Deserialize)]
pub struct Callback {
    state: String,
    code: Option<String>,
    error: Option<String>,
    iss: Option<String>,
}
async fn complete(state: &AppState, headers: &HeaderMap, q: Callback) -> Result<String> {
    let oidc = provider(state)?;
    let flow = consume_flow(state, oidc, headers, &q.state, "login").await?;
    if q.error.is_some() || q.iss.as_deref().is_some_and(|v| v != oidc.config.issuer) {
        return Err(auth::unauthorized());
    }
    let code = q
        .code
        .as_deref()
        .filter(|c| !c.is_empty() && c.len() <= 16000 && !c.chars().any(char::is_control))
        .ok_or_else(auth::unauthorized)?;
    let claims = oidc
        .exchange(
            code,
            flow.get("verifier"),
            flow.get("redirect_uri"),
            flow.get("nonce_hash"),
            flow.get("created_at"),
        )
        .await?;
    let sub = claims["sub"].as_str().unwrap();
    let token = auth::secret();
    let csrf = auth::secret();
    let now = auth::now();
    let mut tx = state.pool.begin().await?;
    let id: String=sqlx::query_scalar("SELECT u.id FROM oidc_identities i JOIN users u ON u.id=i.user_id WHERE i.issuer=? AND i.subject=? AND u.disabled=0")
        .bind(&oidc.config.issuer).bind(sub).fetch_optional(&mut *tx).await?.ok_or_else(auth::unauthorized)?;
    if let Some(old) = auth::session_cookie(&state.security, headers) {
        sqlx::query("DELETE FROM sessions WHERE token_hash=?")
            .bind(hash(old))
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("DELETE FROM sessions WHERE expires_at<=? OR last_seen<=?")
        .bind(now)
        .bind(now - 1800)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO sessions(token_hash,user_id,csrf,created_at,expires_at,last_seen,oidc_issuer,oidc_subject) VALUES(?,?,?,?,?,?,?,?)")
        .bind(hash(&token)).bind(&id).bind(csrf).bind(now).bind(now+SESSION_TTL).bind(now).bind(&oidc.config.issuer).bind(sub).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO security_events(actor,action,target) VALUES(?,'session.oidc_login',?)",
    )
    .bind(format!("user:{id}"))
    .bind(&id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(token)
}
pub async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<Callback>,
) -> Response {
    let result = complete(&state, &headers, q).await;
    let mut response = match result {
        Ok(token) => (
            [("set-cookie", state.security.cookie(&token, SESSION_TTL))],
            Redirect::to("/"),
        )
            .into_response(),
        Err(_) => Redirect::to("/?sso_error=login_failed").into_response(),
    };
    response
        .headers_mut()
        .append("set-cookie", flow_cookie(&state, "", 0).parse().unwrap());
    response
}
/// Called only after normal session authentication and Origin/CSRF validation.
pub async fn logout(state: &AppState, principal: &auth::Principal) -> Result<Response> {
    let mut tx = state.pool.begin().await?;
    let issuer: Option<String> = if let Some(token) = &principal.session_hash {
        sqlx::query_scalar("DELETE FROM sessions WHERE token_hash=? RETURNING oidc_issuer")
            .bind(token)
            .fetch_optional(&mut *tx)
            .await?
            .flatten()
    } else {
        None
    };
    tx.commit().await?;
    let mut body = json!({"authenticated":false});
    let mut binder = None;
    if let Some(oidc) = state
        .oidc
        .as_ref()
        .filter(|o| o.browser_enabled() && issuer.as_deref() == Some(o.issuer()))
    {
        // The local session is already gone even if provider logout cannot be started.
        if let Ok((token, cookie)) = new_flow(state, oidc, "logout", "", "").await {
            let mut url =
                url::Url::parse(&oidc.endpoint("logout")).map_err(|_| auth::unauthorized())?;
            url.query_pairs_mut().extend_pairs([
                ("client_id", oidc.config.client_id.as_str()),
                (
                    "post_logout_redirect_uri",
                    &format!("{}{LOGOUT_CALLBACK}", state.security.origin),
                ),
                ("state", &token),
            ]);
            body["redirect_url"] = json!(url.as_str());
            binder = Some(cookie);
        }
    }
    let mut response = ([("set-cookie", state.security.cookie("", 0))], Json(body)).into_response();
    response.headers_mut().append(
        "set-cookie",
        flow_cookie(
            state,
            binder.as_deref().unwrap_or(""),
            if binder.is_some() { FLOW_TTL } else { 0 },
        )
        .parse()
        .unwrap(),
    );
    Ok(response)
}
#[derive(Deserialize)]
pub struct LogoutCallback {
    state: String,
}
pub async fn logout_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<LogoutCallback>,
) -> Response {
    let success = if let Ok(oidc) = provider(&state) {
        consume_flow(&state, oidc, &headers, &q.state, "logout")
            .await
            .is_ok()
    } else {
        false
    };
    (
        [("set-cookie", flow_cookie(&state, "", 0))],
        Redirect::to(if success {
            "/?logged_out=1"
        } else {
            "/?sso_error=logout_failed"
        }),
    )
        .into_response()
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdentityInput {
    version: i64,
    issuer: String,
    subject: String,
}
pub async fn get_identity(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let version: i64 = sqlx::query_scalar("SELECT version FROM users WHERE id=?")
        .bind(&id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(auth::unauthorized)?;
    let rows =
        sqlx::query("SELECT issuer,subject FROM oidc_identities WHERE user_id=? ORDER BY issuer")
            .bind(&id)
            .fetch_all(&state.pool)
            .await?;
    Ok(Json(
        json!({"id":id,"version":version,"identities":rows.iter().map(|r|json!({"issuer":r.get::<String,_>("issuer"),"subject":r.get::<String,_>("subject")})).collect::<Vec<_>>()}),
    ))
}
pub async fn bind_identity(
    State(state): State<AppState>,
    Extension(p): Extension<auth::Principal>,
    Path(id): Path<String>,
    Json(input): Json<IdentityInput>,
) -> Result<Json<Value>> {
    subject(&input.subject)?;
    if state
        .oidc
        .as_ref()
        .is_none_or(|o| o.issuer() != input.issuer)
    {
        return Err(invalid(
            "Zuordnung muss den konfigurierten Issuer verwenden.",
        ));
    }
    let mut tx = state.pool.begin().await?;
    let version: i64 = sqlx::query_scalar("SELECT version FROM users WHERE id=?")
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(auth::unauthorized)?;
    if version != input.version {
        return Err(conflict("Benutzerkonto wurde geändert."));
    }
    let owner: Option<String> =
        sqlx::query_scalar("SELECT user_id FROM oidc_identities WHERE issuer=? AND subject=?")
            .bind(&input.issuer)
            .bind(&input.subject)
            .fetch_optional(&mut *tx)
            .await?;
    if owner.as_ref().is_some_and(|v| v != &id) {
        return Err(conflict(
            "OIDC-Identität gehört bereits zu einem anderen Benutzer.",
        ));
    }
    sqlx::query("DELETE FROM oidc_identities WHERE user_id=?")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO oidc_identities(issuer,subject,user_id) VALUES(?,?,?)")
        .bind(&input.issuer)
        .bind(&input.subject)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE users SET version=version+1,oidc_subject=NULL WHERE id=?")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE user_id=?")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE api_tokens SET revoked=1 WHERE user_id=?")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO security_events(actor,action,target) VALUES(?,'user.oidc_bind',?)")
        .bind(p.actor)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"id":id,"version":version+1,"issuer":input.issuer,"subject":input.subject}),
    ))
}
