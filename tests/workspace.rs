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
#[tokio::test]
async fn quote_acceptance_snapshots_and_contract_are_atomic_and_unique() {
    let (pool, app) = setup().await;
    let c = configured(&app).await;
    let quote=create(&app,"quotes","q",json!({"customer_id":c["id"],"title":"Projekt","valid_until":"2099-12-31","notes":"","lines":lines()})).await;
    assert_eq!(quote["gross_cents"], 23800);
    let sent = action(&app, "quotes", &quote, "send", "send").await;
    assert_eq!(sent["number"], "AN-00000001");
    let mut changed = customer();
    changed["name"] = json!("Umbenannt");
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("/api/v1/records/customers/{}", c["id"].as_str().unwrap()),
            "editcustomer",
            json!({"version":1,"fields":changed})
        )
        .await
        .0,
        200
    );
    let accepted = action(&app, "quotes", &sent, "accept", "accept").await;
    assert_eq!(accepted["customer"]["name"], "Beispiel & Partner");
    assert_eq!(
        accepted,
        action(&app, "quotes", &sent, "accept", "accept").await
    );
    assert_eq!(
        brilliantis::records(&pool, "contract").await.unwrap().len(),
        1
    );
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("/api/v1/records/quotes/{}", quote["id"].as_str().unwrap()),
            "immutable",
            json!({"version":3,"fields":{}})
        )
        .await
        .0,
        409
    );
}
#[tokio::test]
async fn invoice_payment_cancellation_and_report_reconcile() {
    let (pool, app) = setup().await;
    let c = configured(&app).await;
    let invoice=create(&app,"invoices","invoice",json!({"customer_id":c["id"],"title":"Leistung","issue_date":"2025-02-01","due_date":"2025-02-15","service_date":"2025-01-31","direction":"outgoing","external_number":"","notes":"","lines":lines()})).await;
    let issued = action(&app, "invoices", &invoice, "issue", "issue").await;
    assert_eq!(issued["gross_cents"], 23800);
    let account = create(
        &app,
        "bank-accounts",
        "bank",
        json!({"name":"Bank","iban":"","kind":"bank","connection_id":""}),
    )
    .await;
    let txinput = json!({"account_id":account["id"],"external_id":"BANK-1","booking_date":"2025-02-20","amount_cents":23800,"description":"Zahlung","counterparty":"Partner"});
    let tx = create(&app, "bank-transactions", "tx", txinput.clone()).await;
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/records/bank-transactions",
            "duplicate",
            txinput
        )
        .await
        .0,
        409
    );
    let path = format!(
        "/api/v1/records/bank-transactions/{}/allocate",
        tx["id"].as_str().unwrap()
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &path,
            "too-much",
            json!({"version":1,"invoice_id":issued["id"],"amount_cents":23801})
        )
        .await
        .0,
        422
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &path,
            "allocate",
            json!({"version":1,"invoice_id":issued["id"],"amount_cents":23800})
        )
        .await
        .0,
        200
    );
    let (code, r) = call(
        &app,
        "GET",
        "/api/v1/finance/report?year=2025",
        "",
        Value::Null,
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(r["metrics"]["revenue_cents"], 20000);
    assert_eq!(r["metrics"]["vat_output_cents"], 3800);
    assert_eq!(r["metrics"]["liquidity_cents"], 23800);
    assert_eq!(r["metrics"]["receivables_current_cents"], 0);
    assert_eq!(r["metrics"]["trial_balance_difference_cents"], 0);
    let postedid = issued["journal_id"].as_str().unwrap();
    assert!(sqlx::query(
        "UPDATE journal_lines SET debit_cents=1 WHERE entry_id=? AND debit_cents>0"
    )
    .bind(postedid)
    .execute(&pool)
    .await
    .is_err());
    let invpath = format!(
        "/api/v1/records/invoices/{}",
        issued["id"].as_str().unwrap()
    );
    assert_eq!(
        call(
            &app,
            "PUT",
            &invpath,
            "immutable",
            json!({"version":3,"fields":{}})
        )
        .await
        .0,
        409
    );
    let (code, credit) = call(
        &app,
        "POST",
        &format!("{invpath}/cancel"),
        "cancel",
        json!({"version":3,"booking_date":"2025-03-01"}),
    )
    .await;
    assert_eq!(code, 200, "{credit}");
    assert_eq!(credit["record"]["refund_due_cents"], 23800);
    let refund = create(&app,"bank-transactions","refund-tx",json!({"account_id":account["id"],"external_id":"REFUND-1","booking_date":"2025-03-02","amount_cents":-23800,"description":"Erstattung","counterparty":"Partner"})).await;
    let refund_path = format!(
        "/api/v1/records/bank-transactions/{}/refund",
        refund["id"].as_str().unwrap()
    );
    let refund_body = json!({"version":1,"invoice_id":issued["id"],"amount_cents":23800});
    let (status, result) = call(&app, "POST", &refund_path, "refund", refund_body.clone()).await;
    assert_eq!(status, 200, "{result}");
    assert_eq!(
        call(&app, "POST", &refund_path, "refund", refund_body)
            .await
            .1,
        result
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &refund_path,
            "refund-again",
            json!({"version":2,"invoice_id":issued["id"],"amount_cents":1})
        )
        .await
        .0,
        422
    );
    let fee = create(&app,"bank-transactions","fee",json!({"account_id":account["id"],"external_id":"FEE-1","booking_date":"2025-03-03","amount_cents":-100,"description":"Gebühr","counterparty":"Bank"})).await;
    let fee_path = format!(
        "/api/v1/records/bank-transactions/{}/categorize",
        fee["id"].as_str().unwrap()
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &fee_path,
            "fee-wrong",
            json!({"version":1,"account":"revenue:sales","amount_cents":100})
        )
        .await
        .0,
        422
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &fee_path,
            "fee-correct",
            json!({"version":1,"account":"expense:bank_fees","amount_cents":100})
        )
        .await
        .0,
        200
    );

    let (_, r) = call(
        &app,
        "GET",
        "/api/v1/finance/report?year=2025",
        "",
        Value::Null,
    )
    .await;
    assert_eq!(r["metrics"]["revenue_cents"], 0);
    assert_eq!(r["metrics"]["vat_output_cents"], 0);
    assert_eq!(r["metrics"]["refunds_due_current_cents"], 0);
    assert_eq!(r["metrics"]["expense_cents"], 100);
    assert_eq!(r["metrics"]["liquidity_cents"], -100);
    assert_eq!(r["metrics"]["unallocated_transactions"], 0);
    assert_eq!(r["metrics"]["trial_balance_difference_cents"], 0);
    assert_eq!(
        brilliantis::records(&pool, "credit_note")
            .await
            .unwrap()
            .len(),
        1
    );
}
#[tokio::test]
async fn closed_period_blocks_posting_without_partial_records() {
    let (pool, app) = setup().await;
    configured(&app).await;
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/finance/periods",
            "close",
            json!({"period":"2025-02","closed":true,"reason":"Abgestimmt"})
        )
        .await
        .0,
        200
    );
    let account = create(
        &app,
        "bank-accounts",
        "bank",
        json!({"name":"Bank","iban":"","kind":"bank","connection_id":""}),
    )
    .await;
    let input = json!({"account_id":account["id"],"external_id":"1","booking_date":"2025-02-20","amount_cents":100,"description":"Zahlung","counterparty":""});
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/records/bank-transactions",
            "tx",
            input.clone()
        )
        .await
        .0,
        409
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM financial_sources")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/finance/periods",
            "reopen",
            json!({"period":"2025-02","closed":false,"reason":"Nachbuchung"})
        )
        .await
        .0,
        200
    );
    create(&app, "bank-transactions", "tx", input).await;
}
#[test]
fn vat_rounds_by_rate_and_rejects_overflow() {
    use brilliantis::business::{calculate, LineInput};
    let line = LineInput {
        description: "Kleinbetrag".into(),
        quantity_milli: 1000,
        unit_price_cents: 3,
        unit_gross_cents: None,
        tax_basis_points: 1900,
    };
    let (_, net, tax) = calculate(&[line.clone(), line.clone()]).unwrap();
    assert_eq!((net, tax), (6, 1));
    let mut enormous = line;
    enormous.quantity_milli = 1_000_000_000;
    enormous.unit_price_cents = 100_000_000_000;
    assert!(calculate(&[enormous]).is_err());
    assert!(brilliantis::business::date("2025-02-29").is_err());
    assert!(brilliantis::business::date("2024-02-29").is_ok());
}
#[tokio::test]
async fn catalog_edits_archive_and_search_use_persisted_records() {
    let (_, app) = setup().await;
    let p=create(&app,"products","p",json!({"name":"A_100%","category":"Service","description":"","billing":"monatlich","unit":"Monat","price_cents":12345,"tax_basis_points":1900})).await;
    let (_, d) = call(&app, "GET", "/api/v1/dashboard", "", Value::Null).await;
    assert!(d["catalog"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["id"] == p["id"]));
    let (_, found) = call(
        &app,
        "GET",
        "/api/v1/records/products?q=A_100%25",
        "",
        Value::Null,
    )
    .await;
    assert_eq!(found["items"].as_array().unwrap().len(), 1);
    action(&app, "products", &p, "archive", "archive").await;
    let (_, found) = call(
        &app,
        "GET",
        "/api/v1/records/products?archived=true",
        "",
        Value::Null,
    )
    .await;
    assert_eq!(found["items"].as_array().unwrap().len(), 1);
}
#[tokio::test]
async fn order_sources_deduplicate_and_content_tracks_publication() {
    let (_, app) = setup().await;
    let c = configured(&app).await;
    let input = json!({"name":"Shop order","customer_id":c["id"],"source":"shopware-main","external_id":"1001","ordered_date":"2025-01-01","notes":"","lines":lines()});
    let order = create(&app, "orders", "order", input.clone()).await;
    assert_eq!(
        call(&app, "POST", "/api/v1/records/orders", "order-again", input)
            .await
            .0,
        409
    );
    let path = format!(
        "/api/v1/records/orders/{}/status",
        order["id"].as_str().unwrap()
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &path,
            "confirm",
            json!({"version":1,"status":"confirmed"})
        )
        .await
        .0,
        200
    );
    let channel=create(&app,"channel-accounts","channel",json!({"name":"Marke","platform":"instagram","handle":"@example","identity":"brand","creator_id":"","connection_id":"","notes":""})).await;
    let content=create(&app,"content","content",json!({"title":"Einblick","campaign_id":"","channel_account_id":channel["id"],"body":"Unser Produkt","media_url":"https://example.invalid/media.jpg","disclosure":"Werbung","planned_date":"2025-01-01"})).await;
    let path = format!(
        "/api/v1/records/content/{}/status",
        content["id"].as_str().unwrap()
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &path,
            "plan",
            json!({"version":1,"status":"planned"})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &path,
            "published-missing",
            json!({"version":2,"status":"published"})
        )
        .await
        .0,
        422
    );
    assert_eq!(call(&app,"POST",&path,"published",json!({"version":2,"status":"published","publication_url":"https://example.invalid/post/1"})).await.0,200);
}

#[tokio::test]
async fn demo_business_reconciles_and_preserves_changes_on_restart() {
    let (pool, app) = setup().await;
    let profile = Profile::standalone();
    brilliantis::demo_business::seed(&pool, &profile)
        .await
        .unwrap();
    let customers = brilliantis::records(&pool, "customer").await.unwrap();
    let customer = &customers[0];
    let mut fields = customer.clone();
    for key in [
        "id",
        "version",
        "archived",
        "status",
        "created_at",
        "updated_at",
    ] {
        fields.as_object_mut().unwrap().remove(key);
    }
    fields["name"] = json!("Mein bearbeiteter Kunde");
    let (status, changed) = call(
        &app,
        "PUT",
        &format!(
            "/api/v1/records/customers/{}",
            customer["id"].as_str().unwrap()
        ),
        "demo-edit",
        json!({"version":customer["version"],"fields":fields}),
    )
    .await;
    assert_eq!(status, 200, "{changed}");
    let entries: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM journal_entries")
        .fetch_one(&pool)
        .await
        .unwrap();
    brilliantis::demo_business::seed(&pool, &profile)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM journal_entries")
            .fetch_one(&pool)
            .await
            .unwrap(),
        entries
    );
    let customers = brilliantis::records(&pool, "customer").await.unwrap();
    assert_eq!(customers[0]["name"], "Mein bearbeiteter Kunde");
    let (status, report) = call(
        &app,
        "GET",
        "/api/v1/finance/report?year=2026",
        "",
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(report["metrics"]["revenue_cents"], 775000);
    assert_eq!(report["metrics"]["expense_cents"], 9500);
    assert_eq!(report["metrics"]["liquidity_cents"], 640240);
    assert_eq!(report["metrics"]["receivables_current_cents"], 272510);
    assert_eq!(report["metrics"]["trial_balance_difference_cents"], 0);
    assert_eq!(report["metrics"]["unallocated_transactions"], 0);
    assert!(report["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["label"] == "Geschäftskonto (Demo)"));
    let (status, result) = call(&app,"POST","/api/v1/finance/journal","non-cash-asset",json!({"booking_date":"2026-01-01","description":"Sacheinlage","lines":[{"account":"asset:other","debit_cents":25000,"credit_cents":0},{"account":"equity:contribution","debit_cents":0,"credit_cents":25000}]})).await;
    assert_eq!(status, 200, "{result}");
    let (_, report) = call(
        &app,
        "GET",
        "/api/v1/finance/report?year=2026",
        "",
        Value::Null,
    )
    .await;
    assert_eq!(
        report["metrics"]["liquidity_cents"], 640240,
        "Non-cash assets must not inflate liquidity"
    );
}

#[tokio::test]
async fn invoice_xml_uses_snapshot_escaped_text_and_exact_totals() {
    let (pool, app) = setup().await;
    let mut org = organization();
    org["phone"] = json!("+49 30 123456");
    org["iban"] = json!("DE89370400440532013000");
    assert_eq!(
        call(
            &app,
            "PUT",
            "/api/v1/settings/organization",
            "org-xml",
            json!({"version":0,"fields":org})
        )
        .await
        .0,
        200
    );
    let c = create(&app, "customers", "xml-customer", customer()).await;
    let invoice = create(&app,"invoices","xml-invoice",json!({"customer_id":c["id"],"title":"Service <Premium> & Support","issue_date":"2026-01-01","due_date":"2026-01-15","service_date":"2026-01-01","direction":"outgoing","external_number":"","buyer_reference":"CLIENT-1","notes":"","lines":lines()})).await;
    let issued = action(&app, "invoices", &invoice, "issue", "xml-issue").await;
    let ubl = brilliantis::documents::ubl(&issued).unwrap();
    let cii = brilliantis::documents::cii(&issued).unwrap();
    let ubl_doc = roxmltree::Document::parse(&ubl).unwrap();
    let cii_doc = roxmltree::Document::parse(&cii).unwrap();
    assert_eq!(
        ubl_doc.root_element().tag_name().namespace(),
        Some("urn:oasis:names:specification:ubl:schema:xsd:Invoice-2")
    );
    assert_eq!(
        cii_doc.root_element().tag_name().namespace(),
        Some("urn:un:unece:uncefact:data:standard:CrossIndustryInvoice:100")
    );
    assert!(ubl.contains("Beispiel &amp; Partner"));
    assert!(cii.contains("Service &lt;Premium&gt; &amp; Support"));
    for (document, total) in [(&ubl_doc, "PayableAmount"), (&cii_doc, "DuePayableAmount")] {
        assert_eq!(
            document
                .descendants()
                .find(|n| n.has_tag_name(total))
                .unwrap()
                .text(),
            Some("238.00")
        );
    }
    // XML parsing is a structural test, not a claim of schema/Schematron conformance.
    let mut missing = issued.clone();
    missing["supplier"]["phone"] = json!("");
    assert!(brilliantis::documents::ubl(&missing).is_err());
    let mut missing_date = issued;
    missing_date.as_object_mut().unwrap().remove("issue_date");
    assert!(brilliantis::documents::cii(&missing_date).is_err());
    pool.close().await;
}

#[test]
fn agreed_gross_prices_preserve_tax_math_quantities_and_legacy_shape() {
    use brilliantis::business::{apply_rounding, calculate, rounding_cents, LineInput};
    for rate in [0, 700, 1900, 10000] {
        for gross in [1, 13, 1300, 1600, 3000, 49000, 55900, 99999999] {
            for quantity_milli in [1, 500, 1000, 2000, 20000] {
                let net = (gross * 10000 + (10000 + rate) / 2) / (10000 + rate);
                let line = LineInput {
                    description: "Agreed gross price".into(),
                    quantity_milli,
                    unit_price_cents: net,
                    unit_gross_cents: Some(gross),
                    tax_basis_points: rate,
                };
                let (lines, net, tax) = calculate(&[line.clone(), line.clone()]).unwrap();
                assert_eq!(tax, (net * rate + 5000) / 10000);
                let target = ((quantity_milli * gross + 500) / 1000) * 2;
                assert_eq!(net + tax + rounding_cents(&lines), target);
                let mut body =
                    json!({"lines":lines,"net_cents":net,"tax_cents":tax,"gross_cents":net+tax});
                apply_rounding(&mut body);
                assert_eq!(body["gross_cents"], target);
                let mut legacy = line;
                legacy.unit_gross_cents = None;
                let (legacy_lines, legacy_net, legacy_tax) =
                    calculate(&[legacy.clone(), legacy]).unwrap();
                assert_eq!((legacy_net, legacy_tax), (net, tax));
                assert!(legacy_lines
                    .iter()
                    .all(|l| l.get("unit_gross_cents").is_none()
                        && l.get("rounding_cents").is_none()
                        && l.get("gross_cents").is_none()));
                let mut legacy_body = json!({"lines":legacy_lines,"net_cents":net,"tax_cents":tax,"gross_cents":net+tax});
                let before = legacy_body.clone();
                apply_rounding(&mut legacy_body);
                assert_eq!(legacy_body, before);
            }
        }
    }
    let valid = LineInput {
        description: "Gross".into(),
        quantity_milli: 1000,
        unit_price_cents: 1092,
        unit_gross_cents: Some(1300),
        tax_basis_points: 1900,
    };
    for gross in [-1, 1301, 100_000_000_001] {
        let mut bad = valid.clone();
        bad.unit_gross_cents = Some(gross);
        assert!(calculate(&[bad]).is_err());
    }
    let mut enormous = valid;
    enormous.quantity_milli = 1_000_000_000;
    enormous.unit_gross_cents = Some(1_000_000);
    enormous.unit_price_cents = 840336;
    assert!(calculate(&[enormous]).is_err());
    assert!(serde_json::from_value::<LineInput>(json!({"description":"Injected rounding","quantity_milli":1000,"unit_price_cents":1092,"unit_gross_cents":1300,"tax_basis_points":1900,"rounding_cents":100})).is_err());
}
