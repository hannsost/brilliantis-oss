use brilliantis::connectors::{plans, Configuration, Connection, Hub, SecretReference};
use serde_json::{json, Value};
fn connection(provider: &str) -> Connection {
    Connection {
        id: "test".into(),
        name: "Test".into(),
        provider: provider.into(),
        base_url: "https://provider.example.invalid".into(),
        secret: SecretReference::File {
            path: "/unused-test-secret".into(),
        },
        settings: json!({"api_version":"2026-04","account_id":"123"}),
        operations: plans::operations(provider)
            .unwrap()
            .iter()
            .map(|v| v.to_string())
            .collect(),
    }
}
#[test]
fn every_advertised_operation_builds_an_explicit_plan() {
    let input = json!({"payment_id":"pay_1","customer_id":"customer_1","subscription_id":"sub_1","order_id":"order_1","capture_id":"capture_1","account_id":"account_1","session_id":"session_1","message_id":"message_1","draft_id":"draft_1","media_id":"media_1","creation_id":"creation_1","publish_id":"publish_1","topic_id":"123","ticket_id":"1","client_id":"123","fiscal_year_id":"20250101","legal_entity_id":"123","guid":"testguid","amount_cents":12345,"description":"Test payment","redirect_url":"https://example.invalid/return","webhook_url":"https://example.invalid/webhook","reference":"test-reference","name":"Name","email":"name@example.invalid","start_date":"2025-01-01","end_date":"2025-01-31","site_url":"https://example.invalid/","raw_base64url":"dGVzdA","video_url":"https://example.invalid/video.mp4","code":"code_1","country":"DE","payload":{}});
    for provider in [
        "mollie",
        "stripe",
        "paypal",
        "enable_banking",
        "shopware",
        "woocommerce",
        "shopify",
        "gmail",
        "search_console",
        "instagram",
        "tiktok",
        "discourse",
        "sap_business_one",
        "sap_s4",
        "datev_accounting",
        "peppol_storecove",
        "solveit",
    ] {
        let c = connection(provider);
        for op in plans::operations(provider).unwrap() {
            let result = plans::build(&c, op, &input);
            assert!(result.is_ok(), "{provider} {op}: {:?}", result.err());
            let plan = result.unwrap();
            assert_eq!(plan.url.host_str(), Some("provider.example.invalid"));
            if plan.scope.ends_with(":write") && plan.idempotency_header.is_none() {
                assert!(!plan.safe_retry, "{provider} {op}");
            }
        }
    }
}
#[test]
fn providers_use_native_money_encoding_and_scopes() {
    let input = json!({"amount_cents":12345,"description":"Subscription","redirect_url":"https://example.invalid","webhook_url":"https://example.invalid","reference":"invoice-1"});
    let mollie = plans::build(&connection("mollie"), "payments.create", &input).unwrap();
    assert_eq!(mollie.body["amount"]["value"], "123.45");
    assert_eq!(mollie.scope, "finance:write");
    assert!(mollie.safe_retry);
    let stripe = plans::build(&connection("stripe"), "payments.create", &input).unwrap();
    assert_eq!(stripe.body["amount"], "12345");
    assert!(stripe.form);
    assert_eq!(
        plans::build(&connection("sap_business_one"), "invoices.list", &json!({}))
            .unwrap()
            .scope,
        "finance:read"
    );
    let hub = Hub::new(
        Configuration {
            connections: vec![connection("mollie")],
        },
        false,
    )
    .unwrap();
    assert!(hub
        .plan("test", "payments.get", &json!({"payment_id":"../../other"}))
        .is_err());
    assert!(hub
        .plan("test", "payments.list", &json!({"password":"test-only"}))
        .is_err());
}
#[tokio::test]
async fn http_transport_uses_authorization_idempotency_and_redacts_results() {
    use axum::{http::HeaderMap, routing::any, Json, Router};
    use std::sync::{Arc, Mutex};
    let captured = Arc::new(Mutex::new(Vec::new()));
    let captured2 = captured.clone();
    let app=Router::new().route("/v2/payments",any(move|headers:HeaderMap,Json(body):Json<Value>|{let c=captured2.clone();async move{c.lock().unwrap().push((headers,body));Json(json!({"id":"pay_test","status":"open","access_token":"must-be-redacted","nested":{"private_key_pem":"must-be-redacted"}}))}}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let secret =
        std::env::temp_dir().join(format!("brilliantis-connector-{}", uuid::Uuid::new_v4()));
    std::fs::write(&secret, "local-fixture-only").unwrap();
    let mut c = connection("mollie");
    c.base_url = format!("http://{address}");
    c.secret = SecretReference::File {
        path: secret.clone(),
    };
    let hub = Hub::new(
        Configuration {
            connections: vec![c],
        },
        true,
    )
    .unwrap();
    let input = json!({"amount_cents":1001,"description":"Test","redirect_url":"https://example.invalid","webhook_url":"https://example.invalid","reference":"ref-1"});
    let response = hub
        .execute("test", "payments.create", &input, "stable-key")
        .await
        .unwrap();
    assert_eq!(response["data"]["id"], "pay_test");
    assert!(!response.to_string().contains("must-be-redacted"));
    let requests = captured.lock().unwrap();
    assert_eq!(requests[0].0["authorization"], "Bearer local-fixture-only");
    assert_eq!(requests[0].0["idempotency-key"], "stable-key");
    assert_eq!(requests[0].1["amount"]["value"], "10.01");
    drop(requests);
    server.abort();
    std::fs::remove_file(secret).unwrap();
}
#[test]
fn keycloak_mapping_checks_issuer_audience_expiry_and_active_state() {
    let valid = json!({"active":true,"iss":"https://identity.example.invalid/realms/test","aud":["account","brilliantis"],"sub":"subject","exp":brilliantis::auth::now()+60});
    assert!(brilliantis::oidc::validate(
        &valid,
        "https://identity.example.invalid/realms/test",
        "brilliantis"
    )
    .is_ok());
    for (key, value) in [
        ("active", json!(false)),
        ("aud", json!("other")),
        ("exp", json!(0)),
        ("iss", json!("https://other.invalid")),
    ] {
        let mut claims = valid.clone();
        claims[key] = value;
        assert!(brilliantis::oidc::validate(
            &claims,
            "https://identity.example.invalid/realms/test",
            "brilliantis"
        )
        .is_err());
    }
}
