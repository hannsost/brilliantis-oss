use axum::{
    body::Body,
    http::{HeaderMap, Request},
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use brilliantis::{
    auth, connect,
    oidc::{Configuration, Oidc},
    profile::{initialize_workspace, Profile},
    router,
    storage::hash,
    AppState,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tower::ServiceExt;

fn config(sub: &str) -> Configuration {
    serde_json::from_value(json!({"issuer":"https://identity.example.invalid/realms/test","audience":"brilliantis","client_id":"brilliantis","client_secret":{"provider":"file","path":"/unused-fixture"},"browser_login":true,"bootstrap_admin":{"subject":sub,"email":"leon@example.invalid","name":"Leon"}})).unwrap()
}
async fn setup() -> (sqlx::SqlitePool, AppState, Router) {
    let pool = connect(":memory:").await.unwrap();
    initialize_workspace(&pool, false, &Profile::standalone())
        .await
        .unwrap();
    let oidc = Oidc::new(config("verified-subject")).unwrap();
    oidc.bootstrap_identity(&pool).await.unwrap();
    let state = AppState {
        pool: pool.clone(),
        oidc: Some(oidc),
        demo: false,
        port: 4317,
        profile: Profile::standalone(),
        security: auth::Security::new("https://sales.example.invalid", false).unwrap(),
        connectors: brilliantis::connectors::Hub::empty(),
    };
    (pool, state.clone(), router(state))
}
async fn call(
    app: &Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Value,
) -> (u16, HeaderMap, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "sales.example.invalid")
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let r = app
        .clone()
        .oneshot(
            request
                .body(if method == "GET" {
                    Body::empty()
                } else {
                    Body::from(body.to_string())
                })
                .unwrap(),
        )
        .await
        .unwrap();
    let status = r.status().as_u16();
    let headers = r.headers().clone();
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
fn cookie(h: &HeaderMap) -> String {
    h["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}
#[tokio::test]
async fn bootstrap_binds_verified_issuer_subject_once_and_never_reenables_or_re_elevates() {
    let (pool, state, _) = setup().await;
    let row=sqlx::query("SELECT u.id,u.role,u.password_hash,i.issuer,i.subject FROM users u JOIN oidc_identities i ON i.user_id=u.id").fetch_one(&pool).await.unwrap();
    assert_eq!(row.get::<String, _>("role"), "admin");
    assert!(row.get::<Option<String>, _>("password_hash").is_none());
    assert_eq!(row.get::<String, _>("subject"), "verified-subject");
    assert_eq!(
        row.get::<String, _>("issuer"),
        "https://identity.example.invalid/realms/test"
    );
    assert!(Oidc::new(config("unverified-other-subject"))
        .unwrap()
        .bootstrap_identity(&pool)
        .await
        .is_err());
    sqlx::query("UPDATE users SET role='viewer',disabled=1")
        .execute(&pool)
        .await
        .unwrap();
    state.oidc.unwrap().bootstrap_identity(&pool).await.unwrap();
    let row = sqlx::query("SELECT role,disabled FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("role"), "viewer");
    assert_eq!(row.get::<i64, _>("disabled"), 1);
    let mut changed = config("verified-subject");
    changed.issuer = "https://other.example.invalid/realms/test".into();
    assert!(Oidc::new(changed)
        .unwrap()
        .bootstrap_identity(&pool)
        .await
        .is_err());
}
#[tokio::test]
async fn pkce_nonce_and_one_use_state_are_bound_to_browser_and_canonical_redirect() {
    let (pool, _, app) = setup().await;
    let (status, headers, _) = call(&app, "GET", "/api/v1/auth/oidc/login", &[], Value::Null).await;
    assert_eq!(status, 303);
    let raw = headers["set-cookie"].to_str().unwrap();
    assert!(raw.starts_with("__Host-brilliantis_oidc="));
    assert!(raw.contains("SameSite=Lax") && raw.contains("HttpOnly") && raw.contains("Secure"));
    let url = url::Url::parse(headers["location"].to_str().unwrap()).unwrap();
    let params: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(params["response_type"], "code");
    assert_eq!(
        params["redirect_uri"],
        "https://sales.example.invalid/api/v1/auth/oidc/callback"
    );
    assert_eq!(params["scope"], "openid profile email");
    let row = sqlx::query("SELECT * FROM oidc_flows")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("state_hash"), hash(&params["state"]));
    assert_eq!(row.get::<String, _>("nonce_hash"), hash(&params["nonce"]));
    assert_eq!(
        URL_SAFE_NO_PAD.encode(Sha256::digest(row.get::<String, _>("verifier").as_bytes())),
        params["code_challenge"]
    );
    let path = format!(
        "/api/v1/auth/oidc/callback?state={}&code=fixture&iss=https%3A%2F%2Fwrong.example.invalid",
        params["state"]
    );
    let failed = call(&app, "GET", &path, &[], Value::Null).await;
    assert_eq!(failed.1["location"], "/?sso_error=login_failed");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM oidc_flows")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1,
        "Missing browser binding must not consume somebody else's state"
    );
    let cookie = cookie(&headers);
    let failed = call(&app, "GET", &path, &[("cookie", &cookie)], Value::Null).await;
    assert_eq!(failed.1["location"], "/?sso_error=login_failed");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM oidc_flows")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        call(&app, "GET", &path, &[("cookie", &cookie)], Value::Null)
            .await
            .1["location"],
        "/?sso_error=login_failed"
    );
}
#[tokio::test]
async fn expired_flows_and_issuerless_legacy_mappings_do_not_authenticate() {
    let (pool, state, app) = setup().await;
    let (_, headers, _) = call(&app, "GET", "/api/v1/auth/oidc/login", &[], Value::Null).await;
    let url = url::Url::parse(headers["location"].to_str().unwrap()).unwrap();
    let token = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .to_string();
    sqlx::query("UPDATE oidc_flows SET expires_at=0")
        .execute(&pool)
        .await
        .unwrap();
    let path = format!("/api/v1/auth/oidc/callback?state={token}&error=access_denied");
    assert_eq!(
        call(
            &app,
            "GET",
            &path,
            &[("cookie", &cookie(&headers))],
            Value::Null
        )
        .await
        .1["location"],
        "/?sso_error=login_failed"
    );
    let user: String = sqlx::query_scalar("SELECT id FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    let session = "c".repeat(64);
    sqlx::query("INSERT INTO sessions(token_hash,user_id,csrf,created_at,expires_at,last_seen,oidc_issuer,oidc_subject) VALUES(?,?,'csrf',?,?,?,?,'verified-subject')").bind(hash(&session)).bind(&user).bind(auth::now()).bind(auth::now()+3600).bind(auth::now()).bind(state.oidc.as_ref().unwrap().issuer()).execute(&pool).await.unwrap();
    let headers = [("cookie", format!("__Host-brilliantis_session={session}"))];
    let h = [("cookie", headers[0].1.as_str())];
    assert_eq!(
        call(&app, "GET", "/api/v1/dashboard", &h, Value::Null)
            .await
            .0,
        200
    );
    sqlx::query("DELETE FROM oidc_identities")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET oidc_subject='verified-subject'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(&app, "GET", "/api/v1/dashboard", &h, Value::Null)
            .await
            .0,
        401
    );
}
#[tokio::test]
async fn csrf_protected_logout_revokes_session_before_provider_redirect_and_binds_return() {
    let (pool, state, app) = setup().await;
    let user: String = sqlx::query_scalar("SELECT id FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    let session = "d".repeat(64);
    let cookie = format!("__Host-brilliantis_session={session}");
    sqlx::query("INSERT INTO sessions(token_hash,user_id,csrf,created_at,expires_at,last_seen,oidc_issuer,oidc_subject) VALUES(?,?,'csrf',?,?,?,?,'verified-subject')").bind(hash(&session)).bind(&user).bind(auth::now()).bind(auth::now()+3600).bind(auth::now()).bind(state.oidc.as_ref().unwrap().issuer()).execute(&pool).await.unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/auth/logout",
            &[("cookie", &cookie)],
            Value::Null
        )
        .await
        .0,
        403
    );
    let (code, headers, body) = call(
        &app,
        "POST",
        "/api/v1/auth/logout",
        &[
            ("cookie", &cookie),
            ("origin", "https://sales.example.invalid"),
            ("x-csrf-token", "csrf"),
        ],
        Value::Null,
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    let url = url::Url::parse(body["redirect_url"].as_str().unwrap()).unwrap();
    let p: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(
        p["post_logout_redirect_uri"],
        "https://sales.example.invalid/api/v1/auth/oidc/logout/callback"
    );
    assert_eq!(p["client_id"], "brilliantis");
    assert!(!p.contains_key("id_token_hint"));
    let binder = headers
        .get_all("set-cookie")
        .iter()
        .map(|h| h.to_str().unwrap())
        .find(|h| h.starts_with("__Host-brilliantis_oidc="))
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let path = format!("/api/v1/auth/oidc/logout/callback?state={}", p["state"]);
    assert_eq!(
        call(&app, "GET", &path, &[("cookie", binder)], Value::Null)
            .await
            .1["location"],
        "/?logged_out=1"
    );
    assert_eq!(
        call(&app, "GET", &path, &[("cookie", binder)], Value::Null)
            .await
            .1["location"],
        "/?sso_error=logout_failed"
    );
}

#[tokio::test]
async fn administrative_rebinding_is_versioned_unique_and_revokes_prior_access() {
    let (pool, _, app) = setup().await;
    let user: String = sqlx::query_scalar("SELECT id FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users(id,email,name,role) VALUES('operator','operator@example.invalid','Operator','admin')").execute(&pool).await.unwrap();
    let session = "f".repeat(64);
    let cookie = format!("__Host-brilliantis_session={session}");
    sqlx::query("INSERT INTO sessions(token_hash,user_id,csrf,created_at,expires_at,last_seen) VALUES(?,'operator','csrf',?,?,?)").bind(hash(&session)).bind(auth::now()).bind(auth::now()+3600).bind(auth::now()).execute(&pool).await.unwrap();
    let headers = [
        ("cookie", cookie.as_str()),
        ("origin", "https://sales.example.invalid"),
        ("x-csrf-token", "csrf"),
    ];
    let token = format!("brl_{}", "e".repeat(64));
    let bearer = format!("Bearer {token}");
    sqlx::query("INSERT INTO api_tokens(id,user_id,name,token_hash,scopes,created_at,expires_at) VALUES('fixture',?,'Old access',?,'[\"crm:read\"]',?,?)").bind(&user).bind(hash(&token)).bind(auth::now()).bind(auth::now()+3600).execute(&pool).await.unwrap();
    let path = format!("/api/v1/access/users/{user}/oidc");
    assert_eq!(
        call(
            &app,
            "GET",
            &path,
            &[("authorization", &bearer)],
            Value::Null
        )
        .await
        .0,
        403
    );
    let before = call(&app, "GET", &path, &headers, Value::Null).await.2;
    assert_eq!(before["identities"][0]["subject"], "verified-subject");
    assert_eq!(call(&app,"PUT","/api/v1/access/users/operator/oidc",&headers,json!({"version":1,"issuer":"https://identity.example.invalid/realms/test","subject":"verified-subject"})).await.0,409);
    let mut input = json!({"version":0,"issuer":"https://identity.example.invalid/realms/test","subject":"replacement-subject"});
    assert_eq!(
        call(&app, "PUT", &path, &headers, input.clone()).await.0,
        409
    );
    input["version"] = before["version"].clone();
    assert_eq!(
        call(&app, "PUT", &path, &headers, input.clone()).await.0,
        200
    );
    assert_eq!(call(&app, "PUT", &path, &headers, input).await.0, 409);
    let after = call(&app, "GET", &path, &headers, Value::Null).await.2;
    assert_eq!(after["identities"][0]["subject"], "replacement-subject");
    assert_eq!(
        after["version"].as_i64().unwrap(),
        before["version"].as_i64().unwrap() + 1
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/dashboard",
            &[("authorization", &bearer)],
            Value::Null
        )
        .await
        .0,
        401
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/dashboard", &headers, Value::Null)
            .await
            .0,
        200
    );
}
