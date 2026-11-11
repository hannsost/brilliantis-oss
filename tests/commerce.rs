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
                .uri(path)
                .method(method)
                .header("host", "127.0.0.1:4317")
                .header("content-type", "application/json")
                .header("x-brilliantis-client", "test.workspace")
                .header("idempotency-key", key)
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
    let data = response.into_body().collect().await.unwrap().to_bytes();
    (
        code,
        serde_json::from_slice(&data).unwrap_or(json!(String::from_utf8_lossy(&data))),
    )
}
fn customer() -> Value {
    json!({"name":"Beispiel & Partner","email":"partner@example.invalid","contact":"Alex","address":"Teststraße 1","postal_code":"10115","city":"Berlin","country":"DE","vat_id":"DE123456789","notes":""})
}
fn organization() -> Value {
    json!({"name":"Test GmbH","address":"Musterstraße 2","postal_code":"10115","city":"Berlin","country":"DE","email":"test@example.invalid","vat_id":"DE111111111","tax_number":"","registration":"","iban":"","payment_terms":"14 Tage netto"})
}
async fn create(app: &Router, resource: &str, key: &str, value: Value) -> Value {
    let (code, result) = call(
        app,
        "POST",
        &format!("/api/v1/records/{resource}"),
        key,
        value,
    )
    .await;
    assert_eq!(code, 200, "{result}");
    result["record"].clone()
}
async fn action(app: &Router, resource: &str, record: &Value, action: &str, key: &str) -> Value {
    let (code, result) = call(
        app,
        "POST",
        &format!(
            "/api/v1/records/{resource}/{}/{action}",
            record["id"].as_str().unwrap()
        ),
        key,
        json!({"version":record["version"]}),
    )
    .await;
    assert_eq!(code, 200, "{result}");
    result["record"].clone()
}
async fn configured(app: &Router) -> Value {
    assert_eq!(
        call(
            app,
            "PUT",
            "/api/v1/settings/organization",
            "org",
            json!({"version":0,"fields":organization()})
        )
        .await
        .0,
        200
    );
    create(app, "customers", "customer", customer()).await
}
fn lines() -> Value {
    json!([{"description":"Service","quantity_milli":2000,"unit_price_cents":10000,"tax_basis_points":1900}])
}

fn catalog(version: i64, revision: &str) -> Value {
    json!({"version":version,"revision":revision,"items":[
        {"id":"hosting","name":"Hosting","kind":"hosting","charges":[
            {"description":"Einrichtung","cadence":"once","unit_price_cents":5000,"tax_basis_points":1900},
            {"description":"Grundpreis","cadence":"monthly","unit_price_cents":1000,"tax_basis_points":1900}
        ],"dimensions":[{"key":"cpu","min":1,"max":8,"step":1,"charge":{"description":"vCPU","cadence":"monthly","unit_price_cents":300,"tax_basis_points":1900}}]},
        {"id":"website","name":"Individuelle Website","kind":"website","charges":[],"dimensions":[]}
    ]})
}
async fn write(app: &Router, path: &str, key: &str, input: Value) -> Value {
    let (code, result) = call(app, "POST", &format!("/api/v1/commerce/{path}"), key, input).await;
    assert_eq!(code, 200, "{result}");
    result["record"].clone()
}
async fn prepare(app: &Router) {
    configured(app).await;
    write(app, "catalog", "catalog", catalog(0, "r1")).await;
    write(
        app,
        "customers",
        "bind",
        json!({"source":"portal","external_id":"customer-1","customer":customer()}),
    )
    .await;
}
fn offer(external: &str) -> Value {
    json!({"source":"portal","external_id":external,"external_customer_id":"customer-1","catalog_revision":"r1","configuration_revision":"config-1","item_id":"hosting","quantity":1,"configuration":{"cpu":2}})
}
#[tokio::test]
async fn server_pricing_acceptance_invoice_and_actual_payment_form_one_chain() {
    let (pool, app) = setup().await;
    prepare(&app).await;
    let input = offer("checkout-1");
    let quote = write(&app, "offers", "quote", input.clone()).await;
    assert_eq!(quote["quote"]["net_cents"], 6600);
    assert_eq!(quote["quote"]["gross_cents"], 7854);
    assert_eq!(quote["billing_plan"][0]["cadence"], "once");
    assert_eq!(quote["billing_plan"][2]["cadence"], "monthly");
    assert_eq!(quote, write(&app, "offers", "quote", input.clone()).await);
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/commerce/offers",
            "other-key",
            input.clone()
        )
        .await
        .0,
        409
    );
    let accept = format!("offers/{}/accept", quote["id"].as_str().unwrap());
    assert_eq!(
        call(
            &app,
            "POST",
            &format!(
                "/api/v1/records/quotes/{}/accept",
                quote["quote"]["id"].as_str().unwrap()
            ),
            "bypass",
            json!({"version":quote["quote"]["version"]})
        )
        .await
        .0,
        409
    );
    let consent = json!({"version":1,"acceptance_reference":"portal:acceptance-1"});
    let (a, b) = tokio::join!(
        write(&app, &accept, "accept", consent.clone()),
        write(&app, &accept, "accept", consent.clone())
    );
    assert_eq!(a, b);
    let order = a;
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/commerce/{accept}"),
            "new-accept",
            consent
        )
        .await
        .0,
        409
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM records WHERE kind='order'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM records WHERE kind='contract'")
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
    let path = format!("orders/{}", order["id"].as_str().unwrap());
    let read = format!("/api/v1/commerce/{path}");
    assert_eq!(
        call(&app, "GET", &read, "", Value::Null).await.1["record"]["payment"]["status"],
        "not_invoiced"
    );
    let invoice_input = json!({"version":1,"issue_date":"2026-10-08","due_date":"2026-10-22","service_date":"2026-10-08"});
    sqlx::query("INSERT INTO closed_periods(period,actor) VALUES('2026-10','test')")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/commerce/{path}/invoice"),
            "invoice",
            invoice_input.clone()
        )
        .await
        .0,
        409
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM records WHERE kind='invoice'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM commerce_invoices")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    sqlx::query("DELETE FROM closed_periods")
        .execute(&pool)
        .await
        .unwrap();
    let invoiced = write(
        &app,
        &format!("{path}/invoice"),
        "invoice",
        invoice_input.clone(),
    )
    .await;
    assert_eq!(
        invoiced,
        write(&app, &format!("{path}/invoice"), "invoice", invoice_input).await
    );
    let current = call(&app, "GET", &read, "", Value::Null).await.1["record"].clone();
    assert_eq!(current["payment"]["status"], "issued");
    assert_eq!(current["payment"]["paid_cents"], 0);
    let account = create(
        &app,
        "bank-accounts",
        "account",
        json!({"name":"Bank","iban":"","kind":"bank","connection_id":""}),
    )
    .await;
    let transaction=create(&app,"bank-transactions","tx",json!({"account_id":account["id"],"external_id":"bank-1","booking_date":"2026-10-08","amount_cents":7854,"description":"Zahlung","counterparty":"Kunde"})).await;
    assert_eq!(
        call(
            &app,
            "POST",
            &format!(
                "/api/v1/records/bank-transactions/{}/allocate",
                transaction["id"].as_str().unwrap()
            ),
            "allocate",
            json!({"version":1,"invoice_id":invoiced["invoice_id"],"amount_cents":7854})
        )
        .await
        .0,
        200
    );
    let paid = call(&app, "GET", &read, "", Value::Null).await.1["record"].clone();
    assert_eq!(paid["payment"]["status"], "paid");
    assert_eq!(paid["payment"]["paid_cents"], 7854);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM commerce_events WHERE event_type='commerce.invoice.changed'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
    assert_eq!(paid["version"], 2); // Payment has its own invoice_version, independent of provisioning.
    let looked = call(
        &app,
        "GET",
        "/api/v1/commerce/orders/lookup?source=portal&external_id=checkout-1",
        "",
        Value::Null,
    )
    .await;
    assert_eq!(looked.1["record"], paid);
    let events = call(
        &app,
        "GET",
        "/api/v1/commerce/events?after=0&limit=2",
        "",
        Value::Null,
    )
    .await
    .1;
    assert_eq!(events["has_more"], true);
    assert_eq!(events["items"].as_array().unwrap().len(), 2);
}
#[tokio::test]
async fn stale_catalog_frontend_prices_invalid_configuration_and_partial_writes_are_rejected() {
    let (pool, app) = setup().await;
    prepare(&app).await;
    let mut p = offer("bad");
    p["price_cents"] = json!(1);
    assert_eq!(
        call(&app, "POST", "/api/v1/commerce/offers", "bad-price", p)
            .await
            .0,
        422
    );
    let mut p = offer("bad");
    p["configuration"]["cpu"] = json!(9);
    assert_eq!(
        call(&app, "POST", "/api/v1/commerce/offers", "bad-config", p)
            .await
            .0,
        422
    );
    write(&app, "catalog", "r2", catalog(1, "r2")).await;
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/commerce/offers",
            "stale",
            offer("stale")
        )
        .await
        .0,
        409
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/commerce/catalog",
            "reused",
            catalog(2, "r1")
        )
        .await
        .0,
        409
    );
    let mut p = offer("missing-org");
    p["catalog_revision"] = json!("r2");
    sqlx::query("DELETE FROM organization")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(&app, "POST", "/api/v1/commerce/offers", "missing-org", p)
            .await
            .0,
        422
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM records WHERE kind='quote'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM commerce_records WHERE kind='offer'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}
#[tokio::test]
async fn individual_projects_need_operator_quote_and_provisioning_recovers_visibly() {
    let (_, app) = setup().await;
    prepare(&app).await;
    let mut p = offer("project");
    p["item_id"] = json!("website");
    p["configuration"] = json!({});
    let request = write(&app, "offers", "request", p).await;
    assert_eq!(request["status"], "quote_requested");
    assert!(request["quote"].is_null());
    let path = format!("offers/{}", request["id"].as_str().unwrap());
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/commerce/{path}/accept"),
            "early",
            json!({"version":1,"acceptance_reference":"consent"})
        )
        .await
        .0,
        409
    );
    let q=create(&app,"quotes","manual",json!({"customer_id":request["customer_id"],"title":"Websiteprojekt","valid_until":"2099-12-31","notes":"Individuelles Projektangebot","lines":lines()})).await;
    let q = action(&app, "quotes", &q, "send", "manual-send").await;
    write(
        &app,
        &format!("{path}/price"),
        "price",
        json!({"version":1,"quote_id":q["id"],"quote_version":q["version"]}),
    )
    .await;
    let order = write(
        &app,
        &format!("{path}/accept"),
        "accepted",
        json!({"version":2,"acceptance_reference":"portal:project-consent"}),
    )
    .await;
    let path = format!("orders/{}/provisioning", order["id"].as_str().unwrap());
    let running = write(
        &app,
        &path,
        "running",
        json!({"version":1,"status":"running","external_reference":"worker:job-1","error_code":""}),
    )
    .await;
    assert_eq!(running["version"], 2);
    let failed=write(&app,&path,"failed",json!({"version":2,"status":"failed","external_reference":"worker:job-1","error_code":"upstream_timeout"})).await;
    assert_eq!(failed["provisioning"]["error_code"], "upstream_timeout");
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/commerce/{path}"),
            "stale",
            json!({"version":2,"status":"running","external_reference":"","error_code":""})
        )
        .await
        .0,
        409
    );
    write(
        &app,
        &path,
        "resume",
        json!({"version":3,"status":"running","external_reference":"worker:job-1","error_code":""}),
    )
    .await;
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/commerce/{path}"),
            "fake-ready",
            json!({"version":4,"status":"ready","external_reference":"","error_code":""})
        )
        .await
        .0,
        422
    );
    let ready=write(&app,&path,"ready",json!({"version":4,"status":"ready","external_reference":"worker:service-1","error_code":""})).await;
    assert_eq!(ready["version"], 5);
}

fn aggregate(version: i64, revision: &str) -> Value {
    json!({"version":version,"revision":revision,"items":[{"id":"bundle","name":"Website und Hosting","kind":"service","auto_quote":true,"charges":[],"dimensions":[
        {"key":"websites","min":0,"max":5,"step":1,"charge":{"description":"Website","cadence":"once","unit_price_cents":10000,"tax_basis_points":1900}},
        {"key":"hosting","min":0,"max":5,"step":1,"charge":{"description":"Hosting","cadence":"monthly","unit_price_cents":1000,"tax_basis_points":1900}},
        {"key":"domains","min":0,"max":5,"step":1,"charge":{"description":"Domain","cadence":"yearly","unit_price_cents":1000,"tax_basis_points":1900}}
    ]}]})
}
async fn billing_order(app: &Router) -> Value {
    configured(app).await;
    write(app, "catalog", "catalog", aggregate(0, "bundle-r1")).await;
    write(
        app,
        "customers",
        "bind",
        json!({"source":"portal","external_id":"customer-1","customer":customer()}),
    )
    .await;
    let offer = write(app,"offers","offer",json!({"source":"portal","external_id":"bundle-1","external_customer_id":"customer-1","catalog_revision":"bundle-r1","configuration_revision":"selection-r1","item_id":"bundle","quantity":1,"configuration":{"websites":1,"hosting":1,"domains":1}})).await;
    assert_eq!(offer["quote"]["gross_cents"], 14280);
    write(
        app,
        &format!("offers/{}/accept", offer["id"].as_str().unwrap()),
        "accept",
        json!({"version":offer["version"],"acceptance_reference":"checkout:test"}),
    )
    .await
}
fn billing_body(order: &Value, event: &str, evidence: Value) -> Value {
    let mut p = json!({"version":order["version"],"provider":"stripe","provider_account_id":"acct_test","provider_event_id":event});
    p.as_object_mut()
        .unwrap()
        .extend(evidence.as_object().unwrap().clone());
    p
}
async fn billing_call(app: &Router, order: &Value, key: &str, input: Value) -> (u16, Value) {
    call(
        app,
        "POST",
        &format!(
            "/api/v1/commerce/orders/{}/billing",
            order["id"].as_str().unwrap()
        ),
        key,
        input,
    )
    .await
}
async fn bill(app: &Router, order: &Value, event: &str, evidence: Value) -> Value {
    let key = format!("{event}:{}", evidence["operation"].as_str().unwrap());
    let (code, v) = billing_call(app, order, &key, billing_body(order, event, evidence)).await;
    assert_eq!(code, 200, "{v}");
    v["record"].clone()
}
fn binding_evidence(order: &Value) -> Value {
    json!({"operation":"bind","customer_id":order["customer_id"],"provider_customer_id":"cus_test","provider_subscription_id":"sub_test","checkout_reference":"cs_test","started_on":"2026-01-31"})
}
fn invoice_evidence(initial: bool, id: &str, status: &str, paid: i64) -> Value {
    let total = if initial { 14280 } else { 1190 };
    let periods = if initial {
        json!([{"cadence":"monthly","start":"2026-01-31","end":"2026-02-28"},{"cadence":"yearly","start":"2026-01-31","end":"2027-01-31"}])
    } else {
        json!([{"cadence":"monthly","start":"2026-02-28","end":"2026-03-31"}])
    };
    let issue = if initial { "2026-01-31" } else { "2026-02-28" };
    json!({"operation":"invoice","provider_invoice_id":id,"provider_customer_id":"cus_test","provider_subscription_id":"sub_test","initial":initial,"periods":periods,"issue_date":issue,"due_date":issue,"status":status,"currency":"EUR","total_cents":total,"paid_cents":paid,"payments":if paid>0 {json!([{"id":format!("pi_{id}"),"amount_cents":paid,"booking_date":issue}])} else {json!([])}})
}
fn refund_evidence(id: &str, amount: i64) -> Value {
    json!({"operation":"refund","provider_invoice_id":"in_first","provider_customer_id":"cus_test","provider_subscription_id":"sub_test","provider_refund_id":id,"payment_id":"pi_in_first","amount_cents":amount,"currency":"EUR","booking_date":"2026-03-01"})
}
async fn count(pool: &sqlx::SqlitePool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn billing_cycles_real_journal_dedupe_failed_then_partial_then_paid_and_late_events() {
    let (pool, app) = setup().await;
    let mut order = billing_order(&app).await;
    order = bill(&app, &order, "evt_checkout", binding_evidence(&order)).await;
    // Same event may bind AND invoice; both operations have separate event identity.
    let failed = invoice_evidence(true, "in_first", "payment_failed", 0);
    order = bill(&app, &order, "evt_checkout", failed.clone()).await;
    assert_eq!(order["billing"]["invoices"][0]["paid_cents"], 0);
    assert_eq!(count(&pool, "journal_entries").await, 1);
    let partial = invoice_evidence(true, "in_first", "open", 5000);
    order = bill(&app, &order, "evt_partial", partial.clone()).await;
    assert_eq!(order["billing"]["invoices"][0]["status"], "partially_paid");
    assert_eq!(count(&pool, "journal_entries").await, 3);
    let mut full = partial.clone();
    full["status"] = json!("paid");
    full["paid_cents"] = json!(14280);
    full["payments"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"pi_second","amount_cents":9280,"booking_date":"2026-02-01"}));
    order = bill(&app, &order, "evt_paid", full.clone()).await;
    assert_eq!(order["billing"]["invoices"][0]["status"], "paid");
    assert_eq!(count(&pool, "journal_entries").await, 5);
    let old_version = order["version"].clone();
    // Provider event replay with a different HTTP key ignores stale version, returns current state.
    let replay = billing_call(
        &app,
        &order,
        "new-http-key",
        billing_body(&json!({"version":1}), "evt_paid", full.clone()),
    )
    .await;
    assert_eq!(replay.0, 200, "{}", replay.1);
    assert_eq!(replay.1["record"]["version"], old_version);
    order = bill(&app, &order, "evt_equivalent", full.clone()).await;
    order = bill(&app, &order, "evt_old_failure", failed).await;
    assert_eq!(order["billing"]["invoices"][0]["provider_status"], "paid");
    assert_eq!(count(&pool, "journal_entries").await, 5);
    order = bill(
        &app,
        &order,
        "evt_recurring",
        invoice_evidence(false, "in_feb", "paid", 1190),
    )
    .await;
    assert_eq!(order["billing"]["invoices"].as_array().unwrap().len(), 2);
    assert_eq!(order["billing"]["invoices"][1]["gross_cents"], 1190); // once/yearly excluded
    assert_eq!(count(&pool, "journal_entries").await, 8);
    assert_eq!(count(&pool, "commerce_invoices").await, 1);
    assert_eq!(count(&pool, "commerce_billing_periods").await, 3);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COALESCE(SUM(debit_cents-credit_cents),0) FROM journal_lines WHERE account LIKE 'receivables:%'").fetch_one(&pool).await.unwrap(),0);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COALESCE(SUM(debit_cents-credit_cents),0) FROM journal_lines WHERE account='clearing'").fetch_one(&pool).await.unwrap(),0);
    brilliantis::backup::verify(&pool).await.unwrap();
}

#[tokio::test]
async fn billing_refunds_create_partial_credits_and_exact_full_journal_reversal() {
    let (pool, app) = setup().await;
    let mut order = billing_order(&app).await;
    order = bill(&app, &order, "evt_bind", binding_evidence(&order)).await;
    // A refund before its invoice is retryable, never a fabricated payment.
    assert_eq!(
        billing_call(
            &app,
            &order,
            "early",
            billing_body(&order, "evt_early", refund_evidence("re_early", 1))
        )
        .await
        .0,
        409
    );
    order = bill(
        &app,
        &order,
        "evt_paid",
        invoice_evidence(true, "in_first", "paid", 14280),
    )
    .await;
    order = bill(
        &app,
        &order,
        "evt_refund",
        refund_evidence("re_partial", 101),
    )
    .await;
    let inv = &order["billing"]["invoices"][0];
    assert_eq!(inv["paid_cents"], 14179);
    assert_eq!(inv["credited_cents"], 101);
    assert_eq!(inv["refunded_cents"], 101);
    assert_eq!(inv["refund_due_cents"], 0);
    assert_eq!(inv["status"], "paid");
    let before = count(&pool, "journal_entries").await;
    order = bill(
        &app,
        &order,
        "evt_other_refund",
        refund_evidence("re_partial", 101),
    )
    .await;
    assert_eq!(count(&pool, "journal_entries").await, before);
    assert_eq!(
        billing_call(
            &app,
            &order,
            "over",
            billing_body(&order, "evt_over", refund_evidence("re_over", 14280))
        )
        .await
        .0,
        422
    );
    order = bill(
        &app,
        &order,
        "evt_refund_final",
        refund_evidence("re_final", 14179),
    )
    .await;
    let inv = &order["billing"]["invoices"][0];
    assert_eq!(inv["paid_cents"], 0);
    assert_eq!(inv["refunded_cents"], 14280);
    assert_eq!(inv["credited_cents"], 14280);
    assert_eq!(inv["status"], "cancelled");
    assert_eq!(count(&pool, "payment_refunds").await, 2);
    let balances: Vec<(String, i64)> = sqlx::query_as(
        "SELECT account,SUM(debit_cents-credit_cents) FROM journal_lines GROUP BY account",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    for (account, balance) in balances {
        assert_eq!(balance, 0, "{account}");
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT SUM(json_extract(body,'$.tax_cents')) FROM records WHERE kind='credit_note'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        2280
    );
}

#[tokio::test]
async fn billing_validation_atomic_rollback_and_semantic_collisions() {
    let (pool, app) = setup().await;
    let mut order = billing_order(&app).await;
    let mut bad = binding_evidence(&order);
    bad["customer_id"] = json!("other");
    assert_eq!(
        billing_call(
            &app,
            &order,
            "badcustomer",
            billing_body(&order, "evt_bad", bad)
        )
        .await
        .0,
        422
    );
    order = bill(&app, &order, "evt_bind", binding_evidence(&order)).await;
    for (field, value) in [
        ("total_cents", json!(1)),
        ("currency", json!("USD")),
        ("paid_cents", json!(1)),
        ("provider_customer_id", json!("other")),
        ("provider_subscription_id", json!("other")),
        ("unexpected", json!(true)),
    ] {
        let mut evidence = invoice_evidence(true, "in_first", "paid", 14280);
        evidence[field] = value;
        let code = billing_call(
            &app,
            &order,
            &format!("invalid-{field}"),
            billing_body(&order, "evt_invalid", evidence),
        )
        .await
        .0;
        assert!([409, 422].contains(&code), "{field}: {code}");
    }
    let mut bad = invoice_evidence(true, "in_first", "paid", 14280);
    bad["periods"][0]["end"] = json!("2026-03-01");
    assert_eq!(
        billing_call(
            &app,
            &order,
            "badperiod",
            billing_body(&order, "evt_badperiod", bad)
        )
        .await
        .0,
        422
    );
    let mut bad = billing_body(
        &order,
        "evt_version",
        invoice_evidence(true, "in_first", "paid", 14280),
    );
    bad["version"] = json!(0);
    assert_eq!(billing_call(&app, &order, "badversion", bad).await.0, 409);
    sqlx::query("INSERT INTO closed_periods(period,actor) VALUES('2026-01','test')")
        .execute(&pool)
        .await
        .unwrap();
    let body = billing_body(
        &order,
        "evt_paid",
        invoice_evidence(true, "in_first", "paid", 14280),
    );
    assert_eq!(
        billing_call(&app, &order, "paid", body.clone()).await.0,
        409
    );
    assert_eq!(count(&pool, "commerce_billing_invoices").await, 0);
    assert_eq!(count(&pool, "journal_entries").await, 0);
    sqlx::query("DELETE FROM closed_periods")
        .execute(&pool)
        .await
        .unwrap();
    let r = billing_call(&app, &order, "paid", body).await;
    assert_eq!(r.0, 200, "{}", r.1);
    order = r.1["record"].clone();
    // A new provider ID cannot invoice the same initial/cadence period twice.
    assert_eq!(
        billing_call(
            &app,
            &order,
            "semantic-double",
            billing_body(
                &order,
                "evt_double",
                invoice_evidence(true, "in_duplicate", "paid", 14280)
            )
        )
        .await
        .0,
        409
    );
    let mut changed = invoice_evidence(true, "in_first", "paid", 14280);
    changed["due_date"] = json!("2026-02-01");
    assert_eq!(
        billing_call(
            &app,
            &order,
            "changed-invoice",
            billing_body(&order, "evt_changed", changed)
        )
        .await
        .0,
        409
    );
    let mut event_changed = invoice_evidence(true, "in_first", "open", 0);
    event_changed["status"] = json!("payment_failed");
    assert_eq!(
        billing_call(
            &app,
            &order,
            "changed-event",
            billing_body(&order, "evt_paid", event_changed)
        )
        .await
        .0,
        409
    );
    assert_eq!(count(&pool, "journal_entries").await, 3);
}

#[tokio::test]
async fn billing_subscription_cancel_is_independent_and_old_updates_do_not_reactivate() {
    let (_pool, app) = setup().await;
    let mut order = billing_order(&app).await;
    order = bill(&app, &order, "evt_bind", binding_evidence(&order)).await;
    let now = brilliantis::auth::now();
    let status = |s: &str, at: i64, cancel: bool| json!({"operation":"subscription","provider_customer_id":"cus_test","provider_subscription_id":"sub_test","status":s,"cancel_at_period_end":cancel,"observed_at":at});
    order = bill(
        &app,
        &order,
        "evt_cancel_request",
        status("active", now - 2, true),
    )
    .await;
    assert_eq!(
        order["billing"]["subscription"]["cancel_at_period_end"],
        true
    );
    order = bill(&app, &order, "evt_canceled", status("canceled", now, false)).await;
    order = bill(
        &app,
        &order,
        "evt_old_active",
        status("active", now - 1, false),
    )
    .await;
    assert_eq!(order["billing"]["subscription"]["status"], "canceled");
    assert_eq!(
        billing_call(
            &app,
            &order,
            "reactivate",
            billing_body(&order, "evt_reactivate", status("active", now + 1, false))
        )
        .await
        .0,
        409
    );
    // Historical paid invoices delivered after cancellation remain valid evidence.
    order = bill(
        &app,
        &order,
        "evt_delayed_invoice",
        invoice_evidence(true, "in_first", "paid", 14280),
    )
    .await;
    assert_eq!(order["billing"]["invoices"][0]["paid_cents"], 14280);
    let mut future = invoice_evidence(false, "in_future", "paid", 1190);
    future["periods"] = json!([{"cadence":"monthly","start":"2028-01-31","end":"2028-02-29"}]);
    future["issue_date"] = json!("2028-01-31");
    future["due_date"] = json!("2028-01-31");
    future["payments"][0]["booking_date"] = json!("2028-01-31");
    assert_eq!(
        billing_call(
            &app,
            &order,
            "future",
            billing_body(&order, "evt_future", future)
        )
        .await
        .0,
        409
    );
}

#[tokio::test]
async fn aggregate_auto_quote_opt_in_zero_total_and_unknown_dimensions() {
    let (_pool, app) = setup().await;
    configured(&app).await;
    let mut invalid_catalog = aggregate(0, "bad");
    invalid_catalog["items"][0]["auto_quote"] = json!(false);
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/commerce/catalog",
            "badcatalog",
            invalid_catalog
        )
        .await
        .0,
        422
    );
    write(&app, "catalog", "catalog", aggregate(0, "bundle-r1")).await;
    write(
        &app,
        "customers",
        "bind",
        json!({"source":"portal","external_id":"customer-1","customer":customer()}),
    )
    .await;
    let mut input = json!({"source":"portal","external_id":"bundle-1","external_customer_id":"customer-1","catalog_revision":"bundle-r1","configuration_revision":"selection-r1","item_id":"bundle","quantity":1,"configuration":{"websites":0,"hosting":0,"domains":0}});
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/commerce/offers",
            "zero",
            input.clone()
        )
        .await
        .0,
        422
    );
    input["configuration"]["websites"] = json!(1);
    input["configuration"]["custom_work"] = json!(1);
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/commerce/offers",
            "unknown",
            input.clone()
        )
        .await
        .0,
        422
    );
    input["configuration"]
        .as_object_mut()
        .unwrap()
        .remove("custom_work");
    let offer = write(&app, "offers", "once-only", input).await;
    assert_eq!(offer["quote"]["gross_cents"], 11900);
    assert_eq!(offer["status"], "offered");
}

#[tokio::test]
async fn scoped_customer_update_checks_both_versions_and_keeps_accepted_snapshot() {
    let (_pool, app) = setup().await;
    let order = billing_order(&app).await;
    let url = "/api/v1/commerce/customers/lookup?source=portal&external_id=customer-1";
    let current = call(&app, "GET", url, "", Value::Null).await.1["record"].clone();
    assert_eq!(current["customer"]["address"], "Teststraße 1");
    assert_eq!(current["customer_version"], 1);
    let mut address = customer();
    address["address"] = json!("Neue Straße 2");
    let p = json!({"version":current["version"],"customer_version":current["customer_version"],"customer":address});
    let path = format!("customers/{}/update", current["id"].as_str().unwrap());
    let updated = write(&app, &path, "customer-correction", p.clone()).await;
    assert_eq!(updated["customer"]["address"], "Neue Straße 2");
    assert_eq!(updated["customer_version"], 2);
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/commerce/{path}"),
            "stale",
            p
        )
        .await
        .0,
        409
    );
    // A separate operator core edit must also be detected by the scoped commerce writer.
    let corepath = format!(
        "/api/v1/records/customers/{}",
        current["customer_id"].as_str().unwrap()
    );
    assert_eq!(
        call(
            &app,
            "PUT",
            &corepath,
            "operator-edit",
            json!({"version":2,"fields":customer()})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/commerce/{path}"),
            "stale-core",
            json!({"version":2,"customer_version":2,"customer":customer()})
        )
        .await
        .0,
        409
    );
    let read = call(
        &app,
        "GET",
        &format!("/api/v1/commerce/orders/{}", order["id"].as_str().unwrap()),
        "",
        Value::Null,
    )
    .await
    .1["record"]
        .clone();
    assert_eq!(read["quote"], order["quote"]);
}

#[tokio::test]
async fn once_only_checkout_and_recurring_out_of_order_keep_unique_periods() {
    let (pool, app) = setup().await;
    let mut order = billing_order(&app).await;
    order = bill(&app, &order, "evt_bind", binding_evidence(&order)).await;
    order = bill(
        &app,
        &order,
        "evt_later_first",
        invoice_evidence(false, "in_feb", "paid", 1190),
    )
    .await;
    order = bill(
        &app,
        &order,
        "evt_first_later",
        invoice_evidence(true, "in_first", "paid", 14280),
    )
    .await;
    assert_eq!(order["billing"]["invoices"].as_array().unwrap().len(), 2);
    assert_eq!(
        billing_call(
            &app,
            &order,
            "duplicate-period",
            billing_body(
                &order,
                "evt_duplicate_period",
                invoice_evidence(false, "in_another", "paid", 1190)
            )
        )
        .await
        .0,
        409
    );
    assert_eq!(count(&pool, "commerce_billing_invoices").await, 2);
    let offer=write(&app,"offers","once-offer",json!({"source":"portal","external_id":"once-checkout","external_customer_id":"customer-1","catalog_revision":"bundle-r1","configuration_revision":"selection-once","item_id":"bundle","quantity":1,"configuration":{"websites":1,"hosting":0,"domains":0}})).await;
    let mut once = write(
        &app,
        &format!("offers/{}/accept", offer["id"].as_str().unwrap()),
        "once-accept",
        json!({"version":offer["version"],"acceptance_reference":"once"}),
    )
    .await;
    let mut bind = binding_evidence(&once);
    bind["provider_subscription_id"] = json!("");
    bind["checkout_reference"] = json!("cs_once");
    once = bill(&app, &once, "evt_once_bind", bind).await;
    let mut inv = invoice_evidence(true, "cs_once", "paid", 11900);
    inv["provider_subscription_id"] = json!("");
    inv["periods"] = json!([]);
    inv["total_cents"] = json!(11900);
    once = bill(&app, &once, "evt_once_paid", inv).await;
    assert_eq!(once["billing"]["invoices"][0]["paid_cents"], 11900);
    let mut capture = capture_evidence();
    capture["checkout_reference"] = json!("cs_once");
    capture["amount_cents"] = json!(11900);
    assert_eq!(
        billing_call(
            &app,
            &once,
            "invoiced-capture",
            billing_body(&once, "evt_invoiced_capture", capture)
        )
        .await
        .0,
        409
    );
}

#[tokio::test]
async fn partial_refund_then_remaining_payment_keeps_open_items_and_journal_consistent() {
    let (pool, app) = setup().await;
    let mut order = billing_order(&app).await;
    order = bill(&app, &order, "evt_bind", binding_evidence(&order)).await;
    let partial = invoice_evidence(true, "in_first", "open", 5000);
    order = bill(&app, &order, "evt_partial", partial.clone()).await;
    order = bill(&app, &order, "evt_refund", refund_evidence("re_part", 101)).await;
    let report = call(
        &app,
        "GET",
        "/api/v1/finance/report?year=2026",
        "",
        Value::Null,
    )
    .await;
    assert_eq!(report.0, 200);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT SUM(debit_cents-credit_cents) FROM journal_lines WHERE account LIKE 'receivables:%'").fetch_one(&pool).await.unwrap(),9280);
    let mut final_evidence = partial;
    final_evidence["paid_cents"] = json!(14280);
    final_evidence["status"] = json!("paid");
    final_evidence["payments"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"pi_balance","amount_cents":9280,"booking_date":"2026-03-02"}));
    order = bill(&app, &order, "evt_remaining", final_evidence).await;
    assert_eq!(order["billing"]["invoices"][0]["paid_cents"], 14179);
    assert_eq!(order["billing"]["invoices"][0]["status"], "paid");
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT SUM(debit_cents-credit_cents) FROM journal_lines WHERE account LIKE 'receivables:%'").fetch_one(&pool).await.unwrap(),0);
}

#[tokio::test]
async fn mixed_vat_small_partial_refunds_never_overcredit_and_finish_at_original_tax() {
    let (pool, app) = setup().await;
    configured(&app).await;
    let mut cat = aggregate(0, "bundle-r1");
    cat["items"][0]["dimensions"][0]["charge"]["tax_basis_points"] = json!(0);
    cat["items"][0]["dimensions"][1]["charge"]["tax_basis_points"] = json!(700);
    write(&app, "catalog", "catalog", cat).await;
    write(
        &app,
        "customers",
        "bind",
        json!({"source":"portal","external_id":"customer-1","customer":customer()}),
    )
    .await;
    let offer=write(&app,"offers","offer",json!({"source":"portal","external_id":"mixed","external_customer_id":"customer-1","catalog_revision":"bundle-r1","configuration_revision":"selection-r1","item_id":"bundle","quantity":1,"configuration":{"websites":1,"hosting":1,"domains":1}})).await;
    let mut order = write(
        &app,
        &format!("offers/{}/accept", offer["id"].as_str().unwrap()),
        "accept",
        json!({"version":offer["version"],"acceptance_reference":"mixed"}),
    )
    .await;
    assert_eq!(order["quote"]["gross_cents"], 12260);
    order = bill(&app, &order, "evt_bind", binding_evidence(&order)).await;
    let mut invoice = invoice_evidence(true, "in_first", "paid", 12260);
    invoice["total_cents"] = json!(12260);
    order = bill(&app, &order, "evt_paid", invoice).await;
    for (i, amount) in [1, 2, 123, 4000, 8134].iter().enumerate() {
        order = bill(
            &app,
            &order,
            &format!("evt_refund_{i}"),
            refund_evidence(&format!("re_{i}"), *amount),
        )
        .await;
    }
    assert_eq!(order["billing"]["invoices"][0]["credited_cents"], 12260);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT SUM(json_extract(body,'$.tax_cents')) FROM records WHERE kind='credit_note'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        260
    );
    let balances: Vec<i64> = sqlx::query_scalar(
        "SELECT SUM(debit_cents-credit_cents) FROM journal_lines GROUP BY account",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(balances.iter().all(|b| *b == 0));
    let raw: Vec<String> = sqlx::query_scalar("SELECT body FROM records WHERE kind='credit_note'")
        .fetch_all(&pool)
        .await
        .unwrap();
    for body in raw {
        let v: Value = serde_json::from_str(&body).unwrap();
        for line in v["lines"].as_array().unwrap() {
            assert!(line["net_cents"].as_i64().unwrap() >= 0);
            assert!(line["tax_cents"].as_i64().unwrap() >= 0);
        }
    }
}

#[tokio::test]
async fn provider_refunds_reuse_operator_correction_and_concurrent_events_allocate_once() {
    let (pool, app) = setup().await;
    let mut order = billing_order(&app).await;
    let mut malformed = binding_evidence(&order);
    malformed["started_on"] = json!("2026-01-+1");
    assert_eq!(
        billing_call(
            &app,
            &order,
            "bad-date",
            billing_body(&order, "evt_bad", malformed)
        )
        .await
        .0,
        422
    );
    order = bill(&app, &order, "evt_bind", binding_evidence(&order)).await;
    let input = billing_body(
        &order,
        "evt_paid",
        invoice_evidence(true, "in_first", "paid", 14280),
    );
    let (a, b) = tokio::join!(
        billing_call(&app, &order, "race-a", input.clone()),
        billing_call(&app, &order, "race-b", input)
    );
    assert_eq!(a.0, 200, "{}", a.1);
    assert_eq!(b.0, 200, "{}", b.1);
    assert_eq!(a.1, b.1);
    order = a.1["record"].clone();
    assert_eq!(count(&pool, "journal_entries").await, 3);
    let inv = &order["billing"]["invoices"][0];
    let path = format!(
        "/api/v1/records/invoices/{}/cancel",
        inv["invoice_id"].as_str().unwrap()
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &path,
            "operator-cancel",
            json!({"version":inv["invoice_version"],"booking_date":"2026-02-01"})
        )
        .await
        .0,
        200
    );
    order = bill(
        &app,
        &order,
        "evt_refund1",
        refund_evidence("re_first", 100),
    )
    .await;
    order = bill(
        &app,
        &order,
        "evt_refund2",
        refund_evidence("re_second", 14180),
    )
    .await;
    assert_eq!(order["billing"]["invoices"][0]["paid_cents"], 0);
    assert_eq!(order["billing"]["invoices"][0]["refunded_cents"], 14280);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM records WHERE kind='credit_note'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    let balances: Vec<i64> = sqlx::query_scalar(
        "SELECT SUM(debit_cents-credit_cents) FROM journal_lines GROUP BY account",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(balances.iter().all(|b| *b == 0));
}

#[tokio::test]
async fn void_uses_provider_correction_date_without_backdating_into_closed_issue_period() {
    let (pool, app) = setup().await;
    let mut order = billing_order(&app).await;
    order = bill(&app, &order, "evt_bind", binding_evidence(&order)).await;
    order = bill(
        &app,
        &order,
        "evt_open",
        invoice_evidence(true, "in_first", "open", 0),
    )
    .await;
    sqlx::query("INSERT INTO closed_periods(period,actor) VALUES('2026-01','test')")
        .execute(&pool)
        .await
        .unwrap();
    let mut void = invoice_evidence(true, "in_first", "void", 0);
    assert_eq!(
        billing_call(
            &app,
            &order,
            "undated-void",
            billing_body(&order, "evt_undated", void.clone())
        )
        .await
        .0,
        422
    );
    void["voided_on"] = json!("2026-02-01");
    order = bill(&app, &order, "evt_void", void).await;
    assert_eq!(order["billing"]["invoices"][0]["status"], "cancelled");
    assert_eq!(order["billing"]["invoices"][0]["provider_status"], "void");
    let dates: Vec<String> =
        sqlx::query_scalar("SELECT booking_date FROM journal_entries ORDER BY booking_date")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(dates, vec!["2026-01-31", "2026-02-01"]);
}

#[tokio::test]
async fn gross_catalog_survives_snapshots_renewals_exports_and_complete_refunds() {
    let (pool, app) = setup().await;
    let mut org = organization();
    org["phone"] = json!("+49 30 123456");
    org["iban"] = json!("DE89370400440532013000");
    assert_eq!(
        call(
            &app,
            "PUT",
            "/api/v1/settings/organization",
            "gross-org",
            json!({"version":0,"fields":org})
        )
        .await
        .0,
        200
    );
    let mut cat = aggregate(0, "gross-v1");
    for (index, gross) in [49000_i64, 1300, 1600].into_iter().enumerate() {
        let charge = &mut cat["items"][0]["dimensions"][index]["charge"];
        charge["unit_gross_cents"] = json!(gross);
        charge["unit_price_cents"] = json!((gross * 10000 + 5950) / 11900);
    }
    let mut invalid = cat.clone();
    invalid["items"][0]["dimensions"][1]["charge"]["unit_price_cents"] = json!(1093);
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/commerce/catalog",
            "invalid-gross",
            invalid
        )
        .await
        .0,
        422
    );
    write(&app, "catalog", "gross-cat", cat.clone()).await;
    write(
        &app,
        "customers",
        "gross-customer",
        json!({"source":"portal","external_id":"customer-1","customer":customer()}),
    )
    .await;
    let offer = write(&app,"offers","gross-offer",json!({"source":"portal","external_id":"gross-order","external_customer_id":"customer-1","catalog_revision":"gross-v1","configuration_revision":"selection-v1","item_id":"bundle","quantity":1,"configuration":{"websites":1,"hosting":1,"domains":1}})).await;
    assert_eq!(offer["quote"]["gross_cents"], 51900);
    assert_eq!(offer["quote"]["rounding_cents"], 1);
    let snapshot = offer["quote"].clone();
    cat["version"] = json!(1);
    cat["revision"] = json!("gross-v2");
    cat["items"][0]["dimensions"][0]["charge"]["unit_gross_cents"] = json!(50000);
    cat["items"][0]["dimensions"][0]["charge"]["unit_price_cents"] = json!(42017);
    write(&app, "catalog", "new-gross-cat", cat).await;
    let mut order = write(
        &app,
        &format!("offers/{}/accept", offer["id"].as_str().unwrap()),
        "gross-accept",
        json!({"version":offer["version"],"acceptance_reference":"gross-consent"}),
    )
    .await;
    assert_eq!(order["quote"]["lines"], snapshot["lines"]);
    assert_eq!(order["quote"]["gross_cents"], 51900);
    assert_eq!(order["billing_plan"][0]["line"]["unit_gross_cents"], 49000);
    let contract = brilliantis::records(&pool, "contract")
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(contract["gross_cents"], 51900);
    assert_eq!(contract["rounding_cents"], 1);
    order = bill(&app, &order, "gross-bind", binding_evidence(&order)).await;
    for (index, total) in [51900_i64, 1300, 1600].into_iter().enumerate() {
        let id = if index == 0 {
            "in_first"
        } else if index == 1 {
            "in_month"
        } else {
            "in_year"
        };
        let mut evidence = invoice_evidence(index == 0, id, "paid", total);
        evidence["total_cents"] = json!(total);
        if index == 2 {
            evidence["periods"] =
                json!([{"cadence":"yearly","start":"2027-01-31","end":"2028-01-31"}]);
            evidence["issue_date"] = json!("2027-01-31");
            evidence["due_date"] = json!("2027-01-31");
            evidence["payments"][0]["booking_date"] = json!("2027-01-31");
        }
        let mut wrong = evidence.clone();
        wrong["total_cents"] = json!(total + 1);
        assert_eq!(
            billing_call(
                &app,
                &order,
                &format!("wrong-{index}"),
                billing_body(&order, &format!("wrong-{index}"), wrong)
            )
            .await
            .0,
            422
        );
        order = bill(&app, &order, &format!("gross-paid-{index}"), evidence).await;
    }
    for invoice in order["billing"]["invoices"].as_array().unwrap() {
        assert!(invoice["rounding_cents"].is_i64());
    }
    let invoices = brilliantis::records(&pool, "invoice").await.unwrap();
    for mut inv in invoices {
        let net = inv["net_cents"].as_i64().unwrap();
        let tax = inv["tax_cents"].as_i64().unwrap();
        let gross = inv["gross_cents"].as_i64().unwrap();
        let rounding = inv["rounding_cents"].as_i64().unwrap();
        assert_eq!(tax, (net * 1900 + 5000) / 10000);
        assert_eq!(gross, net + tax + rounding);
        assert_eq!(rounding, if gross == 1600 { -1 } else { 1 });
        inv["buyer_reference"] = json!("GROSS-TEST");
        let ubl = brilliantis::documents::ubl(&inv).unwrap();
        let cii = brilliantis::documents::cii(&inv).unwrap();
        for (xml, total_tag, round_tag, tax_total_tag) in [
            (
                &ubl,
                "PayableAmount",
                "PayableRoundingAmount",
                "TaxInclusiveAmount",
            ),
            (
                &cii,
                "DuePayableAmount",
                "RoundingAmount",
                "GrandTotalAmount",
            ),
        ] {
            let doc = roxmltree::Document::parse(xml).unwrap();
            let value = |tag| {
                doc.descendants()
                    .find(|n| n.has_tag_name(tag))
                    .unwrap()
                    .text()
                    .unwrap()
                    .to_owned()
            };
            assert_eq!(
                value(total_tag),
                format!("{}.{:02}", gross / 100, gross % 100)
            );
            assert_eq!(
                value(tax_total_tag),
                format!("{}.{:02}", (net + tax) / 100, (net + tax) % 100)
            );
            assert_eq!(
                value(round_tag),
                if rounding < 0 { "-0.01" } else { "0.01" }
            );
        }
    }
    // Refund each invoice in parts; signed rounding must also reverse exactly.
    for (index, total) in [51900_i64, 1300, 1600].into_iter().enumerate() {
        for (part, amount) in [1_i64, 100, total - 101].into_iter().enumerate() {
            let mut evidence = refund_evidence(&format!("gross-refund-{index}-{part}"), amount);
            evidence["provider_invoice_id"] = json!(if index == 0 {
                "in_first"
            } else if index == 1 {
                "in_month"
            } else {
                "in_year"
            });
            evidence["payment_id"] = json!(if index == 0 {
                "pi_in_first"
            } else if index == 1 {
                "pi_in_month"
            } else {
                "pi_in_year"
            });
            evidence["booking_date"] = json!("2027-02-01");
            order = bill(
                &app,
                &order,
                &format!("gross-refund-{index}-{part}"),
                evidence,
            )
            .await;
        }
    }
    let balances: Vec<i64> = sqlx::query_scalar(
        "SELECT SUM(debit_cents-credit_cents) FROM journal_lines GROUP BY account",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(balances.iter().all(|v| *v == 0), "{balances:?}");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM journal_lines WHERE account='revenue:rounding'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        6
    );
    for inv in order["billing"]["invoices"].as_array().unwrap() {
        assert_eq!(inv["gross_cents"], inv["credited_cents"]);
        assert_eq!(inv["gross_cents"], inv["refunded_cents"]);
    }
}

#[tokio::test]
async fn old_net_offer_is_not_repriced_after_a_gross_catalog_revision() {
    let (pool, app) = setup().await;
    configured(&app).await;
    write(&app, "catalog", "net-cat", aggregate(0, "net-v1")).await;
    write(
        &app,
        "customers",
        "old-customer",
        json!({"source":"portal","external_id":"customer-1","customer":customer()}),
    )
    .await;
    let offer = write(&app,"offers","old-offer",json!({"source":"portal","external_id":"old-order","external_customer_id":"customer-1","catalog_revision":"net-v1","configuration_revision":"selection-v1","item_id":"bundle","quantity":1,"configuration":{"websites":1,"hosting":1,"domains":1}})).await;
    let snapshot = offer["quote"].clone();
    let mut cat = aggregate(1, "gross-v2");
    for dimension in cat["items"][0]["dimensions"].as_array_mut().unwrap() {
        dimension["charge"]["unit_gross_cents"] = json!(1300);
        dimension["charge"]["unit_price_cents"] = json!(1092);
    }
    write(&app, "catalog", "gross-cat", cat).await;
    let mut order = write(
        &app,
        &format!("offers/{}/accept", offer["id"].as_str().unwrap()),
        "old-accept",
        json!({"version":offer["version"],"acceptance_reference":"old-consent"}),
    )
    .await;
    assert_eq!(order["quote"]["lines"], snapshot["lines"]);
    assert_eq!(order["quote"]["gross_cents"], 14280);
    assert!(order["quote"].get("rounding_cents").is_none());
    order = bill(&app, &order, "old-bind", binding_evidence(&order)).await;
    order = bill(
        &app,
        &order,
        "old-paid",
        invoice_evidence(true, "in_first", "paid", 14280),
    )
    .await;
    let inv = brilliantis::records(&pool, "invoice")
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(inv["gross_cents"], 14280);
    assert!(inv.get("rounding_cents").is_none());
    assert!(order["payment"].get("rounding_cents").is_none());
    assert_eq!(inv["net_cents"], 12000);
    assert_eq!(inv["tax_cents"], 2280);
}

async fn capture_order(app: &Router, reference: &str) -> Value {
    let offer = write(app,"offers",&format!("offer-{reference}"),json!({"source":"portal","external_id":reference,"external_customer_id":"customer-1","catalog_revision":"capture-v1","configuration_revision":"selection-v1","item_id":"bundle","quantity":1,"configuration":{"websites":1,"hosting":0,"domains":0}})).await;
    write(
        app,
        &format!("offers/{}/accept", offer["id"].as_str().unwrap()),
        &format!("accept-{reference}"),
        json!({"version":offer["version"],"acceptance_reference":reference}),
    )
    .await
}
fn capture_evidence() -> Value {
    json!({"operation":"payment","provider_payment_id":"pi_capture","provider_charge_id":"ch_capture","provider_customer_id":"cus_test","checkout_reference":"cs_capture","issue_date":"2026-01-31","booking_date":"2026-01-31","currency":"EUR","amount_cents":1300})
}

#[tokio::test]
async fn once_capture_has_real_payment_references_and_refunds_without_provider_invoice() {
    let (pool, app) = setup().await;
    configured(&app).await;
    let ready = call(&app, "GET", "/readyz", "", Value::Null).await;
    assert_eq!(ready.1["commerce_payment_capture_version"], 1);
    let mut cat = aggregate(0, "capture-v1");
    cat["items"][0]["dimensions"][0]["charge"]["unit_price_cents"] = json!(1092);
    cat["items"][0]["dimensions"][0]["charge"]["unit_gross_cents"] = json!(1300);
    write(&app, "catalog", "capture-cat", cat).await;
    write(
        &app,
        "customers",
        "capture-customer",
        json!({"source":"portal","external_id":"customer-1","customer":customer()}),
    )
    .await;
    let mut order = capture_order(&app, "capture-order").await;
    let capture = capture_evidence();
    assert_eq!(
        billing_call(
            &app,
            &order,
            "capture-unbound",
            billing_body(&order, "evt_unbound", capture.clone())
        )
        .await
        .0,
        409
    );
    let mut bind = binding_evidence(&order);
    bind["provider_subscription_id"] = json!("");
    bind["checkout_reference"] = json!("cs_capture");
    order = bill(&app, &order, "evt_capture_bind", bind).await;
    for (index, (field, value, code)) in [
        ("amount_cents", json!(1299), 422),
        ("amount_cents", json!(0), 422),
        ("currency", json!("USD"), 422),
        ("provider_customer_id", json!("cus_other"), 409),
        ("checkout_reference", json!("cs_other"), 422),
        ("provider_payment_id", json!("in_fake"), 422),
        ("provider_charge_id", json!("pi_fake"), 422),
        ("booking_date", json!("2026-01-30"), 422),
    ]
    .into_iter()
    .enumerate()
    {
        let mut invalid = capture.clone();
        invalid[field] = value;
        assert_eq!(
            billing_call(
                &app,
                &order,
                &format!("invalid-capture-{index}"),
                billing_body(&order, &format!("evt_invalid_capture_{index}"), invalid)
            )
            .await
            .0,
            code
        );
    }
    assert_eq!(count(&pool, "commerce_invoices").await, 0);
    assert_eq!(count(&pool, "journal_entries").await, 0);
    order = bill(&app, &order, "evt_capture", capture.clone()).await;
    let inv = &order["billing"]["invoices"][0];
    assert_eq!(inv["provider_payment_id"], "pi_capture");
    assert_eq!(inv["provider_charge_id"], "ch_capture");
    assert!(inv.get("provider_invoice_id").is_none());
    assert_eq!(inv["paid_cents"], 1300);
    assert_eq!(inv["rounding_cents"], 1);
    assert_eq!(inv["status"], "paid");
    assert_eq!(count(&pool, "commerce_billing_invoices").await, 0);
    assert_eq!(count(&pool, "commerce_invoices").await, 1);
    assert_eq!(count(&pool, "commerce_billing_payments").await, 1);
    let journal_count = count(&pool, "journal_entries").await;
    let replay = billing_call(
        &app,
        &order,
        "capture-new-http-key",
        billing_body(&json!({"version":1}), "evt_capture", capture.clone()),
    )
    .await;
    assert_eq!(replay.0, 200, "{}", replay.1);
    assert_eq!(replay.1["record"]["version"], order["version"]);
    order = bill(&app, &order, "evt_capture_observed_again", capture.clone()).await;
    assert_eq!(count(&pool, "journal_entries").await, journal_count);
    let mut changed = capture.clone();
    changed["provider_charge_id"] = json!("ch_other");
    assert_eq!(
        billing_call(
            &app,
            &order,
            "capture-changed",
            billing_body(&order, "evt_capture_changed", changed)
        )
        .await
        .0,
        409
    );
    assert_eq!(
        billing_call(
            &app,
            &order,
            "capture-invoice-mix",
            billing_body(
                &order,
                "evt_capture_invoice",
                invoice_evidence(true, "in_unwanted", "paid", 1300)
            )
        )
        .await
        .0,
        409
    );
    let invoice = brilliantis::records(&pool, "invoice")
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert!(invoice["notes"].as_str().unwrap().contains("pi_capture"));
    assert!(invoice["notes"].as_str().unwrap().contains("ch_capture"));
    for (index, amount) in [100, 1200].into_iter().enumerate() {
        let refund = json!({"operation":"refund","provider_customer_id":"cus_test","provider_subscription_id":"","provider_refund_id":format!("re_capture_{index}"),"payment_id":"pi_capture","amount_cents":amount,"currency":"EUR","booking_date":"2026-02-01"});
        order = bill(
            &app,
            &order,
            &format!("evt_capture_refund_{index}"),
            refund.clone(),
        )
        .await;
        let after_refund = order["billing"]["invoices"].clone();
        let count_after = count(&pool, "journal_entries").await;
        order = bill(
            &app,
            &order,
            &format!("evt_capture_after_refund_{index}"),
            capture.clone(),
        )
        .await;
        assert_eq!(order["billing"]["invoices"], after_refund);
        order = bill(
            &app,
            &order,
            &format!("evt_capture_refund_replay_{index}"),
            refund,
        )
        .await;
        assert_eq!(count(&pool, "journal_entries").await, count_after);
    }
    assert_eq!(order["billing"]["invoices"][0]["refunded_cents"], 1300);
    assert_eq!(order["billing"]["invoices"][0]["status"], "cancelled");
    let read = call(
        &app,
        "GET",
        &format!("/api/v1/commerce/orders/{}", order["id"].as_str().unwrap()),
        "",
        Value::Null,
    )
    .await;
    assert_eq!(read.1["record"]["payment"]["status"], "cancelled");
    assert_eq!(read.1["record"]["payment"]["rounding_cents"], 1);
    let balances: Vec<i64> = sqlx::query_scalar(
        "SELECT SUM(debit_cents-credit_cents) FROM journal_lines GROUP BY account",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(balances.iter().all(|v| *v == 0), "{balances:?}");

    // The same payment or charge cannot be assigned to a second order, even after refund.
    let mut other = capture_order(&app, "other-order").await;
    let mut bind = binding_evidence(&other);
    bind["provider_subscription_id"] = json!("");
    bind["checkout_reference"] = json!("cs_other");
    other = bill(&app, &other, "evt_other_bind", bind).await;
    for (index, (pi, charge)) in [("pi_capture", "ch_other"), ("pi_other", "ch_capture")]
        .into_iter()
        .enumerate()
    {
        let mut reused = capture.clone();
        reused["checkout_reference"] = json!("cs_other");
        reused["provider_payment_id"] = json!(pi);
        reused["provider_charge_id"] = json!(charge);
        assert_eq!(
            billing_call(
                &app,
                &other,
                &format!("capture-reused-{index}"),
                billing_body(&other, &format!("evt_capture_reused_{index}"), reused)
            )
            .await
            .0,
            409
        );
    }
    let wrong_refund = json!({"operation":"refund","provider_customer_id":"cus_test","provider_subscription_id":"","provider_refund_id":"re_other","payment_id":"pi_capture","amount_cents":1,"currency":"EUR","booking_date":"2026-02-01"});
    assert_eq!(
        billing_call(
            &app,
            &other,
            "wrong-order-refund",
            billing_body(&other, "evt_wrong_refund", wrong_refund)
        )
        .await
        .0,
        409
    );
    assert_eq!(count(&pool, "commerce_invoices").await, 1);
    assert_eq!(count(&pool, "commerce_billing_payments").await, 1);
    brilliantis::backup::verify(&pool).await.unwrap();
}

#[tokio::test]
async fn recurring_orders_require_the_existing_invoice_evidence_path() {
    let (_, app) = setup().await;
    let mut order = billing_order(&app).await;
    order = bill(&app, &order, "evt_bind", binding_evidence(&order)).await;
    assert_eq!(
        billing_call(
            &app,
            &order,
            "recurring-capture",
            billing_body(&order, "evt_recurring_capture", capture_evidence())
        )
        .await
        .0,
        409
    );
}
