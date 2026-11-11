use axum::{body::Body, http::Request, Router};
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
    let app = router(AppState {
        pool: pool.clone(),
        oidc: None,
        demo: false,
        port: 4317,
        profile: Profile::standalone(),
        security: auth::Security::local_demo(4317),
        connectors: brilliantis::connectors::Hub::empty(),
    });
    (pool, app)
}
async fn call(app: &Router, method: &str, path: &str, key: &str, input: Value) -> (u16, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("host", "127.0.0.1:4317")
                .header("content-type", "application/json")
                .header("idempotency-key", key)
                .header("x-brilliantis-client", "test.briefings")
                .body(if method == "GET" {
                    Body::empty()
                } else {
                    Body::from(input.to_string())
                })
                .unwrap(),
        )
        .await
        .unwrap();
    let code = response.status().as_u16();
    if path.ends_with("/download") && code == 200 {
        assert_eq!(
            response.headers()["content-type"],
            "text/plain; charset=utf-8"
        );
        assert!(response.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment; filename=\"briefing-"));
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        code,
        serde_json::from_slice(&bytes).unwrap_or(json!(String::from_utf8_lossy(&bytes))),
    )
}
fn brief() -> Value {
    json!({"title":"Produktbriefing","kind":"core","recipient":"Redaktion DE","language":"de","channels":"X, Instagram","campaign_id":"","creator_id":"","core_briefing_id":"","core_revision":0,"body":"Belegbare Produktdemos.\nVirtuelle Figur der Beispielmarke. <script>untrusted</script>"})
}
async fn ok(app: &Router, method: &str, path: &str, key: &str, input: Value) -> Value {
    let (code, value) = call(app, method, path, key, input).await;
    assert_eq!(code, 200, "{value}");
    value
}

#[tokio::test]
async fn editions_are_immutable_and_handoff_requires_explicit_versioned_evidence() {
    let (pool, app) = setup().await;
    let created = ok(&app, "POST", "/api/v1/briefings", "new", brief()).await;
    assert_eq!(
        ok(&app, "POST", "/api/v1/briefings", "new", brief()).await,
        created
    );
    let id = created["record"]["id"].as_str().unwrap();
    let base = format!("/api/v1/briefings/{id}");
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{base}/editions/1/download"),
            "",
            Value::Null
        )
        .await
        .0,
        404
    );
    let released = ok(
        &app,
        "POST",
        &format!("{base}/release"),
        "release1",
        json!({"version":1}),
    )
    .await;
    assert_eq!(released["record"]["latest_revision"], 1);
    assert_eq!(
        ok(
            &app,
            "POST",
            &format!("{base}/release"),
            "release1",
            json!({"version":1})
        )
        .await,
        released
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{base}/release"),
            "release1",
            json!({"version":2})
        )
        .await
        .0,
        409
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{base}/release"),
            "second-key",
            json!({"version":2})
        )
        .await
        .0,
        409
    );
    let txt = ok(
        &app,
        "GET",
        &format!("{base}/editions/1/download"),
        "",
        Value::Null,
    )
    .await;
    assert!(txt.as_str().unwrap().contains("<script>untrusted</script>"));
    let history = ok(&app, "GET", &format!("{base}/editions"), "", Value::Null).await;
    assert_eq!(
        history["items"][0]["sha256"],
        brilliantis::storage::hash(txt.as_str().unwrap())
    );
    assert!(history["items"][0]["handed_over_at"].is_null());
    let mut edited = brief();
    edited["body"] = json!("Neue belegbare Funktion.");
    assert_eq!(
        call(
            &app,
            "PUT",
            &base,
            "stale",
            json!({"version":1,"fields":edited})
        )
        .await
        .0,
        409
    );
    ok(
        &app,
        "PUT",
        &base,
        "edit",
        json!({"version":2,"fields":edited}),
    )
    .await;
    assert_eq!(
        ok(
            &app,
            "GET",
            &format!("{base}/editions/1/download"),
            "",
            Value::Null
        )
        .await,
        txt
    );
    let bad = json!({"version":3,"revision":2,"note":"Am Handy weitergegeben"});
    assert_eq!(
        call(&app, "POST", &format!("{base}/handoff"), "bad", bad)
            .await
            .0,
        404
    );
    let handoff = json!({"version":3,"revision":1,"note":"Von Leon am Handy weitergegeben"});
    let handed = ok(
        &app,
        "POST",
        &format!("{base}/handoff"),
        "handoff",
        handoff.clone(),
    )
    .await;
    assert_eq!(handed["record"]["status"], "draft");
    assert_eq!(
        ok(&app, "POST", &format!("{base}/handoff"), "handoff", handoff).await,
        handed
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{base}/handoff"),
            "again",
            json!({"version":4,"revision":1,"note":"Andere Angabe"})
        )
        .await
        .0,
        409
    );
    ok(
        &app,
        "POST",
        &format!("{base}/release"),
        "release2",
        json!({"version":4}),
    )
    .await;
    let newer = ok(
        &app,
        "GET",
        &format!("{base}/editions/2/download"),
        "",
        Value::Null,
    )
    .await;
    assert_ne!(newer, txt);
    let history = ok(&app, "GET", &format!("{base}/editions"), "", Value::Null).await;
    assert!(history["items"][0]["handed_over_at"].is_null());
    assert_eq!(history["items"][1]["handed_over_by"], "test.briefings");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM briefing_handovers")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM events WHERE event_type='briefing.release'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        2
    );
    ok(
        &app,
        "POST",
        &format!("{base}/archive"),
        "archive",
        json!({"version":5}),
    )
    .await;
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{base}/release"),
            "archived",
            json!({"version":6})
        )
        .await
        .0,
        409
    );
    assert_eq!(
        ok(
            &app,
            "GET",
            &format!("{base}/editions/1/download"),
            "",
            Value::Null
        )
        .await,
        txt
    );
    assert_eq!(
        ok(
            &app,
            "GET",
            "/api/v1/briefings?archived=true&q=Produkt",
            "",
            Value::Null
        )
        .await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        ok(&app, "GET", "/api/v1/briefings", "", Value::Null).await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    ok(
        &app,
        "POST",
        &format!("{base}/restore"),
        "restore",
        json!({"version":6}),
    )
    .await;
}

#[tokio::test]
async fn additions_pin_a_real_core_edition_and_validate_relationships() {
    let (_, app) = setup().await;
    let core = ok(&app, "POST", "/api/v1/briefings", "core", brief()).await;
    let id = core["record"]["id"].as_str().unwrap();
    let mut addition = brief();
    addition["kind"] = json!("addition");
    addition["core_briefing_id"] = json!(id);
    addition["core_revision"] = json!(1);
    assert_eq!(
        call(&app, "POST", "/api/v1/briefings", "add", addition.clone())
            .await
            .0,
        404
    );
    ok(
        &app,
        "POST",
        &format!("/api/v1/briefings/{id}/release"),
        "release",
        json!({"version":1}),
    )
    .await;
    for (field, value) in [
        ("recipient", json!("Other")),
        ("language", json!("en")),
        ("core_revision", json!(0)),
    ] {
        let mut bad = addition.clone();
        bad[field] = value;
        assert_eq!(
            call(&app, "POST", "/api/v1/briefings", "invalid", bad)
                .await
                .0,
            422
        );
    }
    let created = ok(&app, "POST", "/api/v1/briefings", "add", addition.clone()).await;
    let addition_id = created["record"]["id"].as_str().unwrap();
    ok(
        &app,
        "POST",
        &format!("/api/v1/briefings/{addition_id}/release"),
        "add-release",
        json!({"version":1}),
    )
    .await;
    let txt = ok(
        &app,
        "GET",
        &format!("/api/v1/briefings/{addition_id}/editions/1/download"),
        "",
        Value::Null,
    )
    .await;
    assert!(txt
        .as_str()
        .unwrap()
        .contains(&format!("Ergänzung zu Grundbriefing {id} · Fassung 1")));
    addition["core_briefing_id"] = json!(addition_id);
    assert_eq!(
        call(&app, "POST", "/api/v1/briefings", "nested", addition)
            .await
            .0,
        422
    );
    let mut bad = brief();
    bad["status"] = json!("ready");
    assert_eq!(
        call(&app, "POST", "/api/v1/briefings", "injected", bad)
            .await
            .0,
        422
    );
    let mut bad = brief();
    bad["recipient"] = json!("Changed recipient");
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("/api/v1/briefings/{id}"),
            "retarget",
            json!({"version":2,"fields":bad})
        )
        .await
        .0,
        409
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/briefings/{id}/release"),
            "extra",
            json!({"version":2,"actor":"spoofed"})
        )
        .await
        .0,
        422
    );
}

#[tokio::test]
async fn edition_pagination_does_not_skip_versions_and_list_search_is_literal() {
    let (_, app) = setup().await;
    let mut fields = brief();
    fields["title"] = json!("100%_test");
    let created = ok(&app, "POST", "/api/v1/briefings", "new", fields.clone()).await;
    let base = format!(
        "/api/v1/briefings/{}",
        created["record"]["id"].as_str().unwrap()
    );
    for n in 1..=31 {
        if n > 1 {
            fields["body"] = json!(format!("Version {n}"));
            ok(
                &app,
                "PUT",
                &base,
                &format!("e{n}"),
                json!({"version":2*n-2,"fields":fields}),
            )
            .await;
        }
        ok(
            &app,
            "POST",
            &format!("{base}/release"),
            &format!("r{n}"),
            json!({"version":2*n-1}),
        )
        .await;
    }
    let first = ok(&app, "GET", &format!("{base}/editions"), "", Value::Null).await;
    assert_eq!(first["items"].as_array().unwrap().len(), 30);
    assert_eq!(first["has_more"], true);
    assert_eq!(first["next_cursor"], 2);
    let last = ok(
        &app,
        "GET",
        &format!("{base}/editions?before=2"),
        "",
        Value::Null,
    )
    .await;
    assert_eq!(last["items"][0]["revision"], 1);
    assert_eq!(last["has_more"], false);
    assert_eq!(
        ok(&app, "GET", "/api/v1/briefings?q=%25_", "", Value::Null).await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        ok(&app, "GET", "/api/v1/briefings?q=x%25_", "", Value::Null).await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
