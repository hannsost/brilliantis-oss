use axum::{
    body::Body,
    http::{HeaderMap, Request},
    Router,
};
use brilliantis::{
    auth, connect,
    profile::{initialize_workspace, Profile},
    router, AppState,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
async fn setup() -> (sqlx::SqlitePool, Router) {
    let pool = connect(":memory:").await.unwrap();
    initialize_workspace(&pool, false, &Profile::standalone())
        .await
        .unwrap();
    auth::bootstrap(
        &pool,
        "admin@example.invalid",
        "test-password-long-enough".into(),
    )
    .await
    .unwrap();
    let app = router(AppState {
        pool: pool.clone(),
        oidc: None,
        demo: false,
        port: 4317,
        profile: Profile::standalone(),
        security: auth::Security::new("http://127.0.0.1:4317", false).unwrap(),
        connectors: brilliantis::connectors::Hub::empty(),
    });
    (pool, app)
}
async fn call(
    app: &Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    input: Value,
) -> (u16, HeaderMap, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(
            "host",
            headers
                .iter()
                .find(|(key, _)| *key == "host")
                .map(|(_, value)| *value)
                .unwrap_or("127.0.0.1:4317"),
        )
        .header("content-type", "application/json");
    for (k, v) in headers {
        if *k != "host" {
            request = request.header(*k, *v);
        }
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(if method == "GET" {
                    Body::empty()
                } else {
                    Body::from(input.to_string())
                })
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn login(app: &Router) -> (String, String) {
    let (status, headers, body) = call(
        app,
        "POST",
        "/api/v1/auth/login",
        &[("origin", "http://127.0.0.1:4317")],
        json!({"email":"admin@example.invalid","password":"test-password-long-enough"}),
    )
    .await;
    assert_eq!(status, 200);
    let cookie = headers["set-cookie"].to_str().unwrap();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
    (
        cookie.split(';').next().unwrap().to_string(),
        body["csrf"].as_str().unwrap().to_string(),
    )
}
#[tokio::test]
async fn commerce_service_scopes_separate_portal_fulfillment_and_finance() {
    let (pool, app) = setup().await;
    let (cookie, csrf) = login(&app).await;
    let session = [
        ("cookie", cookie.as_str()),
        ("origin", "http://127.0.0.1:4317"),
        ("x-csrf-token", csrf.as_str()),
    ];
    let (status, _, token) = call(
        &app,
        "POST",
        "/api/v1/access/tokens",
        &session,
        json!({"name":"portal","scopes":["commerce:read","commerce:write"],"expires_in_days":1}),
    )
    .await;
    assert_eq!(status, 200, "{token}");
    let bearer = format!("Bearer {}", token["token"].as_str().unwrap());
    let headers = [
        ("authorization", bearer.as_str()),
        ("idempotency-key", "portal-1"),
    ];
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/commerce/catalog",
            &headers,
            Value::Null
        )
        .await
        .0,
        200
    );
    for path in [
        "/api/v1/commerce/catalog",
        "/api/v1/commerce/offers/id/price",
        "/api/v1/commerce/orders/id/provisioning",
        "/api/v1/commerce/orders/id/invoice",
    ] {
        assert_eq!(
            call(&app, "POST", path, &headers, json!({})).await.0,
            403,
            "{path}"
        );
    }
    assert_eq!(
        call(&app, "GET", "/api/v1/finance/report", &headers, Value::Null)
            .await
            .0,
        403
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/commerce/catalog", &[], Value::Null)
            .await
            .0,
        401
    );
    let customer = json!({"source":"portal","external_id":"c1","customer":{"name":"Client","email":"test@example.invalid","contact":"","address":"Test 1","postal_code":"10115","city":"Berlin","country":"DE","vat_id":"","notes":""}});
    let (status, _, first) = call(
        &app,
        "POST",
        "/api/v1/commerce/customers",
        &headers,
        customer.clone(),
    )
    .await;
    assert_eq!(status, 200, "{first}");
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/commerce/customers",
            &headers,
            customer.clone()
        )
        .await
        .2,
        first
    );
    let mut other = customer;
    other["external_id"] = json!("c2");
    assert_eq!(
        call(&app, "POST", "/api/v1/commerce/customers", &headers, other)
            .await
            .0,
        409
    );
    let actor: String = sqlx::query_scalar("SELECT actor FROM commerce_events LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(actor.starts_with("user:"));
    // Current role still gates an existing service token.
    sqlx::query("UPDATE users SET role='viewer'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/commerce/customers",
            &headers,
            json!({})
        )
        .await
        .0,
        403
    );
}
#[tokio::test]
async fn authentication_csrf_and_actor_forgery_are_enforced() {
    let (pool, app) = setup().await;
    let (status, headers, _) = call(&app, "GET", "/api/v1/dashboard", &[], Value::Null).await;
    assert_eq!(status, 401);
    assert_eq!(headers["cache-control"], "no-store");
    let (cookie, csrf) = login(&app).await;
    let input = json!({"name":"Customer","email":"","contact":"","address":"","postal_code":"","city":"","country":"DE","vat_id":"","notes":""});
    let headers = [
        ("cookie", cookie.as_str()),
        ("origin", "http://127.0.0.1:4317"),
        ("idempotency-key", "create"),
    ];
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/records/customers",
            &headers,
            input.clone()
        )
        .await
        .0,
        403
    );
    let headers = [
        ("cookie", cookie.as_str()),
        ("origin", "http://127.0.0.1:4317"),
        ("x-csrf-token", csrf.as_str()),
        ("idempotency-key", "create"),
        ("x-brilliantis-client", "forged.actor"),
    ];
    assert_eq!(
        call(&app, "POST", "/api/v1/records/customers", &headers, input)
            .await
            .0,
        200
    );
    let actor: String = sqlx::query_scalar("SELECT actor_label FROM events LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(actor.starts_with("user:"));
    assert_ne!(actor, "forged.actor");
    let stored: String = sqlx::query_scalar("SELECT token_hash FROM sessions LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!cookie.contains(&stored));
    let headers = [
        ("cookie", cookie.as_str()),
        ("origin", "http://127.0.0.1:4317"),
        ("x-csrf-token", csrf.as_str()),
    ];
    assert_eq!(
        call(&app, "POST", "/api/v1/auth/logout", &headers, Value::Null)
            .await
            .0,
        200
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/dashboard",
            &[("cookie", &cookie)],
            Value::Null
        )
        .await
        .0,
        401
    );
}
#[tokio::test]
async fn scoped_agent_tokens_cannot_escalate_or_create_financial_records() {
    let (pool, app) = setup().await;
    let (cookie, csrf) = login(&app).await;
    let session = [
        ("cookie", cookie.as_str()),
        ("origin", "http://127.0.0.1:4317"),
        ("x-csrf-token", csrf.as_str()),
    ];
    let (status, _, body) = call(
        &app,
        "POST",
        "/api/v1/access/tokens",
        &session,
        json!({"name":"sales-agent","scopes":["crm:read","crm:write"],"expires_in_days":30}),
    )
    .await;
    assert_eq!(status, 200);
    let token = body["token"].as_str().unwrap();
    let authorization = format!("Bearer {token}");
    let headers = [
        ("authorization", authorization.as_str()),
        ("idempotency-key", "finance"),
    ];
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/records/invoices",
            &headers,
            json!({})
        )
        .await
        .0,
        403
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/finance/report?year=2025",
            &headers,
            Value::Null
        )
        .await
        .0,
        403
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/dashboard", &headers, Value::Null)
            .await
            .0,
        200
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/access/tokens",
            &headers,
            json!({"name":"escalation","scopes":["admin"],"expires_in_days":1})
        )
        .await
        .0,
        403
    );
    let stored: String = sqlx::query_scalar("SELECT token_hash FROM api_tokens LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, brilliantis::storage::hash(token));
    let list = call(&app, "GET", "/api/v1/access/tokens", &session, Value::Null)
        .await
        .2;
    assert!(!list.to_string().contains(token));
    let path = format!(
        "/api/v1/access/tokens/{}/revoke",
        body["id"].as_str().unwrap()
    );
    assert_eq!(
        call(&app, "POST", &path, &session, Value::Null).await.0,
        200
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/dashboard", &headers, Value::Null)
            .await
            .0,
        401
    );
}
#[tokio::test]
async fn roles_are_rechecked_last_admin_is_protected_and_sessions_expire() {
    let (pool, app) = setup().await;
    let (cookie, csrf) = login(&app).await;
    let session = [
        ("cookie", cookie.as_str()),
        ("origin", "http://127.0.0.1:4317"),
        ("x-csrf-token", csrf.as_str()),
    ];
    let id: String = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("/api/v1/access/users/{id}"),
            &session,
            json!({"version":1,"name":"Admin","role":"viewer","disabled":false})
        )
        .await
        .0,
        409
    );
    sqlx::query("UPDATE users SET role='viewer'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/records/customers",
            &session,
            json!({})
        )
        .await
        .0,
        403
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/access/users", &session, Value::Null)
            .await
            .0,
        403
    );
    sqlx::query("UPDATE sessions SET last_seen=unixepoch()-1801")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(&app, "GET", "/api/v1/dashboard", &session, Value::Null)
            .await
            .0,
        401
    );
}
#[test]
fn canonical_origins_reject_unsafe_configuration() {
    assert!(auth::Security::new("http://example.com", false).is_err());
    assert!(auth::Security::new("https://example.com/path", false).is_err());
    assert!(auth::Security::new("https://example.com", true).is_err());
    assert!(
        auth::Security::new("https://example.com", false)
            .unwrap()
            .secure
    );
}

#[tokio::test]
async fn integration_queue_deduplicates_rechecks_roles_and_recovers_expired_leases() {
    use axum::{
        extract::{Path, State},
        Extension, Json,
    };
    use brilliantis::connectors::{Configuration, Connection, Hub, SecretReference};
    let (pool, _) = setup().await;
    let user: String = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    let principal = auth::Principal {
        id: user.clone(),
        name: "Administrator".into(),
        role: "admin".into(),
        actor: format!("user:{user}"),
        scopes: vec!["*".into()],
        session_hash: None,
        csrf: None,
    };
    let hub = Hub::new(
        Configuration {
            connections: vec![Connection {
                id: "payments".into(),
                name: "Fixture".into(),
                provider: "mollie".into(),
                base_url: "https://api.mollie.com".into(),
                secret: SecretReference::File {
                    path: "/nonexistent/not-read-in-this-test".into(),
                },
                operations: vec!["payments.get".into()],
                settings: json!({}),
            }],
        },
        false,
    )
    .unwrap();
    let state = AppState {
        pool: pool.clone(),
        oidc: None,
        demo: false,
        port: 4317,
        profile: Profile::standalone(),
        security: auth::Security::new("http://127.0.0.1:4317", false).unwrap(),
        connectors: hub.clone(),
    };
    let enqueue = |key: &'static str, payment: &'static str| {
        let state = state.clone();
        let principal = principal.clone();
        async move {
            let mut headers = HeaderMap::new();
            headers.insert("idempotency-key", key.parse().unwrap());
            headers.insert("x-brilliantis-client", principal.actor.parse().unwrap());
            brilliantis::jobs::enqueue(
                State(state),
                Extension(principal),
                Path("payments".into()),
                headers,
                Json(brilliantis::jobs::JobInput {
                    operation: "payments.get".into(),
                    input: json!({"payment_id":payment}),
                }),
            )
            .await
        }
    };
    let original = enqueue("job-1", "pay_one").await.unwrap().0;
    assert_eq!(original, enqueue("job-1", "pay_one").await.unwrap().0);
    assert!(enqueue("job-1", "pay_other").await.is_err());
    let safe = enqueue("job-safe", "pay_two").await.unwrap().0;
    let unsafe_job = enqueue("job-unsafe", "pay_three").await.unwrap().0;
    // Simulate process interruption after claiming a request. No provider is contacted.
    sqlx::query("UPDATE integration_jobs SET status='running',lease_until=0 WHERE id IN (?,?)")
        .bind(safe["id"].as_str().unwrap())
        .bind(unsafe_job["id"].as_str().unwrap())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE integration_jobs SET safe_retry=0 WHERE id=?")
        .bind(unsafe_job["id"].as_str().unwrap())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET disabled=1 WHERE id=?")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    assert!(brilliantis::jobs::run_one(&pool, &hub).await.unwrap());
    assert!(brilliantis::jobs::run_one(&pool, &hub).await.unwrap());
    assert!(!brilliantis::jobs::run_one(&pool, &hub).await.unwrap());
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM integration_jobs WHERE status='failed' AND error_code='authorization_revoked' AND attempts=0").fetch_one(&pool).await.unwrap(),2);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM integration_jobs WHERE id=?")
            .bind(unsafe_job["id"].as_str().unwrap())
            .fetch_one(&pool)
            .await
            .unwrap(),
        "uncertain"
    );
}

#[tokio::test]
async fn public_origin_and_authenticated_website_intake_preserve_the_contract() {
    let (pool, _) = setup().await;
    let app = router(AppState {
        pool: pool.clone(),
        oidc: None,
        demo: false,
        port: 4317,
        profile: Profile::standalone(),
        security: auth::Security::new("https://sales.example.invalid", false).unwrap(),
        connectors: brilliantis::connectors::Hub::empty(),
    });
    let host = [("host", "sales.example.invalid")];
    for path in [
        "/api/v1/dashboard",
        "/api/v1/finance/report",
        "/api/v1/records/invoices",
        "/api/v1/events",
    ] {
        assert_eq!(
            call(&app, "GET", path, &host, Value::Null).await.0,
            401,
            "{path}"
        );
    }
    // Kubelet probes can use the pod IP; forwarded host headers cannot authorize APIs.
    assert_eq!(
        call(
            &app,
            "GET",
            "/readyz",
            &[("host", "10.0.0.3:4317")],
            Value::Null
        )
        .await
        .0,
        200
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/dashboard",
            &[("x-forwarded-host", "sales.example.invalid")],
            Value::Null
        )
        .await
        .0,
        403
    );
    let (status, headers, session) = call(
        &app,
        "POST",
        "/api/v1/auth/login",
        &[
            ("host", "sales.example.invalid"),
            ("origin", "https://sales.example.invalid"),
        ],
        json!({"email":"admin@example.invalid","password":"test-password-long-enough"}),
    )
    .await;
    assert_eq!(status, 200);
    let cookie_header = headers["set-cookie"].to_str().unwrap();
    assert!(
        cookie_header.starts_with("__Host-brilliantis_session=")
            && cookie_header.contains("; Secure")
    );
    let cookie = cookie_header.split(';').next().unwrap();
    let (status, _, token) = call(
        &app,
        "POST",
        "/api/v1/access/tokens",
        &[
            ("host", "sales.example.invalid"),
            ("origin", "https://sales.example.invalid"),
            ("cookie", cookie),
            ("x-csrf-token", session["csrf"].as_str().unwrap()),
        ],
        json!({"name":"website-intake","scopes":["crm:read","crm:write"],"expires_in_days":30}),
    )
    .await;
    assert_eq!(status, 200);
    let bearer = format!("Bearer {}", token["token"].as_str().unwrap());
    let auth = [
        ("host", "sales.example.invalid"),
        ("authorization", bearer.as_str()),
        ("idempotency-key", "website-inquiry-1"),
        ("x-brilliantis-client", "untrusted-label"),
    ];
    let input = json!({"id":"hosting-inquiry-1","created_at":"2026-10-07T12:00:00Z","name":"Fixture","email":"fixture@example.invalid","company":"Fixture GmbH","topic":"hosting","message":"Bitte ein Angebot erstellen.","consent":true,"hosting":{"schema_version":1,"quote":{"monthlyCents":99999},"selection":{"cpu":4}}});
    let (status, _, result) =
        call(&app, "POST", "/api/v1/import/website", &auth, input.clone()).await;
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["record"]["version"], 1);
    assert_eq!(result["record"]["amount_cents"], 0);
    assert_eq!(
        result["record"]["inquiry"]["hosting_snapshot"],
        input["hosting"]
    );
    assert_eq!(result["record"]["inquiry"]["marketing_consent"], false);
    assert_eq!(
        call(&app, "POST", "/api/v1/import/website", &auth, input.clone())
            .await
            .2,
        result
    );
    let mut changed = input.clone();
    changed["message"] = json!("Geändert");
    assert_eq!(
        call(&app, "POST", "/api/v1/import/website", &auth, changed)
            .await
            .0,
        409
    );
    let mut auth2 = auth;
    auth2[2] = ("idempotency-key", "another-key");
    assert_eq!(
        call(&app, "POST", "/api/v1/import/website", &auth2, input)
            .await
            .0,
        409
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/finance/report", &auth, Value::Null)
            .await
            .0,
        403
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM records WHERE kind='lead'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM journal_entries")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    let actor: String =
        sqlx::query_scalar("SELECT actor_label FROM events WHERE event_type='website.import'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(actor.starts_with("user:") && actor.contains("/token:"));
}

#[tokio::test]
async fn briefings_require_crm_scope_and_downloads_require_authentication() {
    let (_, app) = setup().await;
    let (cookie, csrf) = login(&app).await;
    let session = [
        ("cookie", cookie.as_str()),
        ("origin", "http://127.0.0.1:4317"),
        ("x-csrf-token", csrf.as_str()),
    ];
    for (name, scopes, read_status, write_status) in [
        ("reader", json!(["crm:read"]), 200, 403),
        ("writer", json!(["crm:write"]), 403, 422),
        (
            "accountant",
            json!(["finance:read", "finance:write"]),
            403,
            403,
        ),
    ] {
        let (status, _, token) = call(
            &app,
            "POST",
            "/api/v1/access/tokens",
            &session,
            json!({"name":name,"scopes":scopes,"expires_in_days":1}),
        )
        .await;
        assert_eq!(status, 200);
        let bearer = format!("Bearer {}", token["token"].as_str().unwrap());
        let headers = [
            ("authorization", bearer.as_str()),
            ("idempotency-key", name),
        ];
        assert_eq!(
            call(&app, "GET", "/api/v1/briefings", &headers, Value::Null)
                .await
                .0,
            read_status
        );
        assert_eq!(
            call(&app, "POST", "/api/v1/briefings", &headers, json!({}))
                .await
                .0,
            write_status
        );
        assert_eq!(
            call(
                &app,
                "GET",
                "/api/v1/briefings/unknown/editions/1/download",
                &headers,
                Value::Null
            )
            .await
            .0,
            if read_status == 200 { 404 } else { 403 }
        );
    }
    for path in [
        "/api/v1/briefings",
        "/api/v1/briefings/unknown/editions/1/download",
    ] {
        assert_eq!(call(&app, "GET", path, &[], Value::Null).await.0, 401);
    }
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/briefings",
            &[("cookie", cookie.as_str())],
            json!({})
        )
        .await
        .0,
        403
    );
    // Operator website identities remain limited to website/commerce transport.
    let principal = auth::Principal {
        actor: "machine:website".into(),
        role: "machine".into(),
        scopes: vec!["crm:write".into()],
        id: "website".into(),
        name: "Website".into(),
        session_hash: None,
        csrf: None,
    };
    assert!(auth::authorize(
        &principal,
        &HeaderMap::new(),
        "POST",
        "/api/v1/briefings",
        &auth::Security::local_demo(4317)
    )
    .is_err());
}
