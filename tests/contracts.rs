use axum::{body::Body, http::Request, Router};
use brilliantis::profile::{initialize_workspace, Profile};
use brilliantis::{connect, domain, records, router, seed_demo, AppState};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

async fn setup() -> (sqlx::SqlitePool, Router) {
    let pool = connect(":memory:").await.unwrap();
    initialize_workspace(&pool, false, &Profile::standalone())
        .await
        .unwrap();
    let app = router(AppState {
        pool: pool.clone(),
        oidc: None,
        demo: false,
        port: 4317,
        profile: Profile::standalone(),
        security: brilliantis::auth::Security::local_demo(4317),
        connectors: brilliantis::connectors::Hub::empty(),
    });
    (pool, app)
}
fn lead() -> Value {
    json!({"company":"Test GmbH","contact":"Testperson","email":"test@example.invalid","title":"Hosting","product_id":"service","source":"manual","amount_cents":12345,"next_action":"Bedarf prüfen"})
}
fn campaign() -> Value {
    json!({"name":"SEO Test","product_id":"service","audience":"Softwareteams","objective":"Passende Anfragen","channel":"seo","budget_cents":10000})
}
async fn call(app: Router, method: &str, path: &str, key: &str, payload: Value) -> (u16, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1:4317")
        .header("content-type", "application/json")
        .header("idempotency-key", key)
        .header("x-brilliantis-client", "test.agent")
        .body(if method == "GET" {
            Body::empty()
        } else {
            Body::from(payload.to_string())
        })
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let code = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        code,
        serde_json::from_slice(&bytes).unwrap_or(json!({"text":String::from_utf8_lossy(&bytes)})),
    )
}

#[tokio::test]
async fn replay_is_exact_and_conflicts_do_not_write() {
    let (pool, app) = setup().await;
    let (code, a) = call(app.clone(), "POST", "/api/v1/leads", "same", lead()).await;
    assert_eq!(code, 200);
    let (_, b) = call(app.clone(), "POST", "/api/v1/leads", "same", lead()).await;
    assert_eq!(a, b);
    let mut changed = lead();
    changed["amount_cents"] = json!(999);
    assert_eq!(
        call(app.clone(), "POST", "/api/v1/leads", "same", changed)
            .await
            .0,
        409
    );
    assert_eq!(records(&pool, "lead").await.unwrap().len(), 1);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn concurrent_retries_commit_once() {
    let (pool, app) = setup().await;
    let (a, b) = tokio::join!(
        call(app.clone(), "POST", "/api/v1/leads", "parallel", lead()),
        call(app, "POST", "/api/v1/leads", "parallel", lead())
    );
    assert_eq!(a, b);
    assert_eq!(a.0, 200);
    assert_eq!(records(&pool, "lead").await.unwrap().len(), 1);
}

#[tokio::test]
async fn pipeline_fences_versions_and_enforces_transitions() {
    let (_, app) = setup().await;
    let (_, created) = call(app.clone(), "POST", "/api/v1/leads", "create", lead()).await;
    let path = format!(
        "/api/v1/leads/{}",
        created["record"]["id"].as_str().unwrap()
    );
    assert_eq!(
        call(
            app.clone(),
            "PATCH",
            &path,
            "premature",
            json!({"version":1,"status":"won"})
        )
        .await
        .0,
        422
    );
    let (ok, changed) = call(
        app.clone(),
        "PATCH",
        &path,
        "qualify",
        json!({"version":1,"status":"qualified"}),
    )
    .await;
    assert_eq!(ok, 200);
    assert_eq!(changed["record"]["version"], 2);
    assert_eq!(
        call(
            app.clone(),
            "PATCH",
            &path,
            "stale",
            json!({"version":1,"status":"lost"})
        )
        .await
        .0,
        409
    );
    assert_eq!(
        call(
            app.clone(),
            "PATCH",
            &path,
            "qualify",
            json!({"version":1,"status":"qualified"})
        )
        .await
        .1,
        changed
    );
    assert_eq!(
        call(
            app.clone(),
            "PATCH",
            &path,
            "offer",
            json!({"version":2,"status":"proposal"})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        call(
            app.clone(),
            "PATCH",
            &path,
            "win",
            json!({"version":3,"status":"won"})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        call(
            app,
            "PATCH",
            &path,
            "reopen",
            json!({"version":4,"status":"new"})
        )
        .await
        .0,
        422
    );
}

#[tokio::test]
async fn invalid_money_products_and_unknown_fields_are_rejected() {
    let (pool, app) = setup().await;
    for (field, value) in [
        ("amount_cents", json!(-1)),
        ("amount_cents", json!(1.2)),
        ("amount_cents", json!(100_000_000_001i64)),
        ("product_id", json!("unknown")),
        ("email", json!("no email")),
        ("company", json!(" ")),
        ("sql", json!("drop")),
    ] {
        let mut input = lead();
        input[field] = value;
        assert_eq!(
            call(app.clone(), "POST", "/api/v1/leads", "invalid", input)
                .await
                .0,
            422
        );
    }
    assert!(records(&pool, "lead").await.unwrap().is_empty());
}

#[tokio::test]
async fn campaigns_plan_without_external_execution() {
    let (_, app) = setup().await;
    let (code, created) = call(
        app.clone(),
        "POST",
        "/api/v1/campaigns",
        "campaign",
        campaign(),
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(created["record"]["status"], "draft");
    let path = format!(
        "/api/v1/campaigns/{}",
        created["record"]["id"].as_str().unwrap()
    );
    assert_eq!(
        call(
            app.clone(),
            "PATCH",
            &path,
            "send",
            json!({"version":1,"status":"sent"})
        )
        .await
        .0,
        422
    );
    assert_eq!(
        call(
            app.clone(),
            "PATCH",
            &path,
            "plan",
            json!({"version":1,"status":"planned"})
        )
        .await
        .0,
        200
    );
    let (_, events) = call(app, "GET", "/api/v1/events", "unused", Value::Null).await;
    assert!(events["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["delivery"] == "local_only"));
}

#[tokio::test]
async fn website_import_preserves_snapshot_without_turning_it_into_price_or_marketing_consent() {
    let (pool, app) = setup().await;
    let input = json!({"id":"inquiry-1","created_at":"2026-10-07T12:00:00Z","name":"Example","email":"test@example.invalid","company":"Example GmbH","topic":"betrieb","message":"Wir möchten Hosting besprechen.","consent":true,"hosting":{"quote":{"monthlyCents":99999},"selection":{"cpu":4}}});
    let (code, result) = call(
        app.clone(),
        "POST",
        "/api/v1/import/website",
        "import-1",
        input.clone(),
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(result["record"]["amount_cents"], 0);
    assert_eq!(result["record"]["inquiry"]["marketing_consent"], false);
    assert_eq!(
        result["record"]["inquiry"]["hosting_snapshot"],
        input["hosting"]
    );
    assert_eq!(
        call(
            app.clone(),
            "POST",
            "/api/v1/import/website",
            "import-1",
            input.clone()
        )
        .await
        .1,
        result
    );
    assert_eq!(
        call(app, "POST", "/api/v1/import/website", "import-2", input)
            .await
            .0,
        409
    );
    assert_eq!(records(&pool, "lead").await.unwrap().len(), 1);
}

#[tokio::test]
async fn creators_support_internal_external_and_versioned_cooperation() {
    let (_, app) = setup().await;
    for kind in ["brand", "virtual", "external"] {
        let input = json!({"name":"Creator Test","kind":kind,"niche":"Open Source","channels":["instagram","tiktok"],"handles":"@example","email":"","budget_cents":5000,"notes":"Briefing"});
        let (code, result) =
            call(app.clone(), "POST", "/api/v1/creators", kind, input.clone()).await;
        assert_eq!(code, 200);
        let path = format!(
            "/api/v1/creators/{}",
            result["record"]["id"].as_str().unwrap()
        );
        assert_eq!(
            call(
                app.clone(),
                "PATCH",
                &path,
                &format!("{kind}-brief"),
                json!({"version":1,"status":"briefing"})
            )
            .await
            .0,
            200
        );
        let mut invalid = input;
        invalid["channels"] = json!(["tiktok", "tiktok"]);
        assert_eq!(
            call(app.clone(), "POST", "/api/v1/creators", "bad", invalid)
                .await
                .0,
            422
        );
    }
}

#[tokio::test]
async fn local_boundary_rejects_foreign_hosts_origins_and_missing_command_keys() {
    let (_, app) = setup().await;
    for (host, origin) in [
        ("evil.test:4317", "http://evil.test:4317"),
        ("127.0.0.1:4317", "https://evil.test"),
        ("localhost:4317", "null"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/dashboard")
                    .header("host", host)
                    .header("origin", origin)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
    }
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/leads")
                .header("host", "127.0.0.1:4317")
                .header("content-type", "application/json")
                .header("x-brilliantis-client", "test")
                .body(Body::from(lead().to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 428);
}

#[tokio::test]
async fn events_are_pageable_and_totals_come_from_records() {
    let (pool, app) = setup().await;
    for i in 0..103 {
        domain::execute(
            &pool,
            &Profile::standalone(),
            &format!("lead-{i}"),
            "test",
            "lead.create",
            None,
            lead(),
        )
        .await
        .unwrap();
    }
    let (_, one) = call(app.clone(), "GET", "/api/v1/events", "", Value::Null).await;
    let (_, two) = call(
        app.clone(),
        "GET",
        "/api/v1/events?after=100",
        "",
        Value::Null,
    )
    .await;
    assert_eq!(one["items"].as_array().unwrap().len(), 100);
    assert_eq!(two["items"].as_array().unwrap().len(), 3);
    let (_, dashboard) = call(app, "GET", "/api/v1/dashboard", "", Value::Null).await;
    assert_eq!(dashboard["metrics"]["pipeline_cents"], 12345 * 103);
    assert_eq!(dashboard["metrics"]["weighted_cents"], 1234 * 103);
    assert_eq!(dashboard["metrics"]["won_cents"], 0);
}

#[tokio::test]
async fn demo_seed_is_repeatable_and_preserves_edits() {
    let (pool, _) = setup().await;
    seed_demo(&pool, &Profile::standalone()).await.unwrap();
    let rows = records(&pool, "lead").await.unwrap();
    let l = rows.iter().find(|l| l["status"] == "new").unwrap();
    domain::execute(
        &pool,
        &Profile::standalone(),
        "demo-user-change",
        "user",
        "lead.update",
        l["id"].as_str(),
        json!({"version":1,"status":"qualified","next_action":"Eigene Änderung"}),
    )
    .await
    .unwrap();
    seed_demo(&pool, &Profile::standalone()).await.unwrap();
    let after = records(&pool, "lead").await.unwrap();
    assert_eq!(after.len(), 3);
    assert_eq!(
        after.iter().find(|v| v["id"] == l["id"]).unwrap()["next_action"],
        "Eigene Änderung"
    );
}

#[tokio::test]
async fn data_and_replay_survive_reopening_database() {
    let path =
        std::env::temp_dir().join(format!("brilliantis-test-{}.sqlite", uuid::Uuid::new_v4()));
    let pool = connect(path.to_str().unwrap()).await.unwrap();
    initialize_workspace(&pool, false, &Profile::standalone())
        .await
        .unwrap();
    let response = domain::execute(
        &pool,
        &Profile::standalone(),
        "persistent",
        "test",
        "lead.create",
        None,
        lead(),
    )
    .await
    .unwrap();
    pool.close().await;
    let restored = connect(path.to_str().unwrap()).await.unwrap();
    assert_eq!(records(&restored, "lead").await.unwrap().len(), 1);
    assert_eq!(
        domain::execute(
            &restored,
            &Profile::standalone(),
            "persistent",
            "test",
            "lead.create",
            None,
            lead()
        )
        .await
        .unwrap(),
        response
    );
    restored.close().await;
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn request_body_limit_and_openapi_are_present() {
    let (_, app) = setup().await;
    let mut input = lead();
    input["next_action"] = json!("a".repeat(70_000));
    assert_eq!(
        call(app.clone(), "POST", "/api/v1/leads", "large", input)
            .await
            .0,
        413
    );
    let (code, spec) = call(app, "GET", "/api/v1/openapi.json", "", Value::Null).await;
    assert_eq!(code, 200);
    assert_eq!(spec["info"]["license"]["identifier"], "MIT");
    assert!(spec["paths"]["/api/v1/creators"]["post"].is_object());
}

#[tokio::test]
async fn standalone_has_no_neurofunken_dependency_or_product_mapping() {
    let (_, app) = setup().await;
    let (_, context) = call(app.clone(), "GET", "/api/v1/agent/context", "", Value::Null).await;
    assert_eq!(context["distribution"], "standalone");
    assert!(context["orchestrator"].is_null());
    let (_, dashboard) = call(app.clone(), "GET", "/api/v1/dashboard", "", Value::Null).await;
    assert!(!dashboard["catalog"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["id"] == "fireflint"));
    assert!(!dashboard["integrations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["id"] == "symphony"));
    let mut foreign = lead();
    foreign["product_id"] = json!("hosting");
    assert_eq!(
        call(app.clone(), "POST", "/api/v1/leads", "foreign", foreign)
            .await
            .0,
        422
    );
    let (_, spec) = call(app, "GET", "/api/v1/openapi.json", "", Value::Null).await;
    assert_eq!(
        spec["components"]["schemas"]["LeadInput"]["properties"]["product_id"]["examples"],
        json!(["service", "subscription", "goods"])
    );
}

#[tokio::test]
async fn mode_and_distribution_changes_do_not_mutate_existing_workspaces() {
    let (pool, _) = setup().await;
    let mut descriptor: Value =
        serde_json::from_str(include_str!("../distributions/standalone/profile.json")).unwrap();
    descriptor["id"] = json!("custom");
    let custom = Profile::load(&descriptor.to_string(), "[]").unwrap();
    assert!(initialize_workspace(&pool, false, &custom).await.is_err());
    assert!(initialize_workspace(&pool, true, &Profile::standalone())
        .await
        .is_err());
    assert!(domain::execute(
        &pool,
        &custom,
        "wrong-profile",
        "test",
        "lead.create",
        None,
        lead()
    )
    .await
    .is_err());
    let stored: String = sqlx::query_scalar("SELECT value FROM metadata WHERE key='profile'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, "standalone");
    assert!(records(&pool, "lead").await.unwrap().is_empty());
    descriptor["schema_version"] = json!(2);
    assert!(Profile::load(&descriptor.to_string(), "[]").is_err());
}

#[tokio::test]
async fn legacy_client_header_keeps_existing_replays_usable() {
    let (_, app) = setup().await;
    let (_, original) = call(
        app.clone(),
        "POST",
        "/api/v1/leads",
        "legacy-header",
        lead(),
    )
    .await;
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/leads")
        .header("host", "127.0.0.1:4317")
        .header("content-type", "application/json")
        .header("idempotency-key", "legacy-header")
        .header("x-orbit-client", "test.agent")
        .body(Body::from(lead().to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), original);
}
