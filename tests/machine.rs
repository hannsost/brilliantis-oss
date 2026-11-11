#![cfg(unix)]
use axum::{body::Body, http::Request};
use brilliantis::{
    auth, backup, connect, machine,
    profile::{initialize_workspace, Profile},
    router, AppState,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::PathBuf,
    process::{Command, Output},
};
use tower::ServiceExt;

struct Fixture {
    root: PathBuf,
    pool: sqlx::SqlitePool,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
impl Fixture {
    async fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("brilliantis-machine-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let pool = connect(root.join("workspace.sqlite").to_str().unwrap())
            .await
            .unwrap();
        initialize_workspace(&pool, false, &Profile::standalone())
            .await
            .unwrap();
        Self { root, pool }
    }
    fn args(&self, action: &str, generation: &str) -> Vec<String> {
        let mut args = vec![
            "machine".into(),
            action.into(),
            "--database".into(),
            self.root.join("workspace.sqlite").to_str().unwrap().into(),
            "--name".into(),
            "website".into(),
            "--generation".into(),
            generation.into(),
        ];
        if action != "revoke" {
            args.extend([
                "--scopes".into(),
                "crm:write".into(),
                "--ttl-seconds".into(),
                "3600".into(),
                "--output".into(),
                self.root
                    .join(format!("{generation}.json"))
                    .to_str()
                    .unwrap()
                    .into(),
            ]);
        }
        args
    }
    fn run(&self, args: &[String]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_brilliantis"))
            .args(args)
            .env("RUST_LOG", "trace")
            .output()
            .unwrap()
    }
    fn token(&self, generation: &str) -> String {
        let value: Value = serde_json::from_slice(
            &fs::read(self.root.join(format!("{generation}.json"))).unwrap(),
        )
        .unwrap();
        value["token"].as_str().unwrap().into()
    }
    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
    fn app(&self) -> axum::Router {
        router(AppState {
            pool: self.pool.clone(),
            oidc: None,
            demo: false,
            port: 4317,
            profile: Profile::standalone(),
            security: auth::Security::new("http://127.0.0.1:4317", false).unwrap(),
            connectors: brilliantis::connectors::Hub::empty(),
        })
    }
}
async fn call(f: &Fixture, token: &str, method: &str, path: &str, body: Value) -> (u16, Value) {
    let response = f
        .app()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("host", "127.0.0.1:4317")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .header("idempotency-key", "machine-inquiry-1")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let raw = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

#[tokio::test]
async fn cli_idempotency_human_isolation_and_scoped_website_replay_across_renewal() {
    let f = Fixture::new().await;
    sqlx::query("INSERT INTO users(id,email,name,role,password_hash,version) VALUES('human','leon@example.invalid','website','viewer','unchanged',17)").execute(&f.pool).await.unwrap();
    let args = f.args("provision", "initial");
    let result = f.run(&args);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let token = f.token("initial");
    assert!(!String::from_utf8_lossy(&result.stdout).contains(&token));
    assert!(!String::from_utf8_lossy(&result.stderr).contains(&token));
    let path = f.root.join("initial.json");
    let before = fs::read(&path).unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(f.run(&args).status.success());
    assert!(before == fs::read(&path).unwrap());
    assert_eq!(modified, fs::metadata(path).unwrap().modified().unwrap());
    assert_eq!(f.count("machine_identities").await, 1);
    assert_eq!(f.count("machine_tokens").await, 1);
    assert_eq!(f.count("security_events").await, 1);
    assert_eq!(f.count("users").await, 1);
    let preserved: (String, String, i64) =
        sqlx::query_as("SELECT role,password_hash,version FROM users WHERE id='human'")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(preserved, ("viewer".into(), "unchanged".into(), 17));
    let hash: String = sqlx::query_scalar("SELECT token_hash FROM machine_tokens")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(hash != token && hash == brilliantis::storage::hash(&token));
    for (method, path) in [
        ("GET", "/api/v1/finance/report"),
        ("GET", "/api/v1/commerce/catalog"),
        ("POST", "/api/v1/access/tokens"),
        ("GET", "/api/v1/access/users"),
        ("GET", "/api/v1/connections"),
        ("GET", "/api/v1/jobs"),
        ("POST", "/api/v1/commands"),
    ] {
        assert_eq!(
            call(&f, &token, method, path, json!({})).await.0,
            403,
            "{path}"
        );
    }
    let body = json!({"id":"machine-inquiry-1","created_at":"2026-10-08T12:00:00Z","name":"Fixture","email":"fixture@example.invalid","company":"Fixture","topic":"hosting","message":"Hosting request","consent":true});
    let first = call(&f, &token, "POST", "/api/v1/import/website", body.clone()).await;
    assert_eq!(first.0, 200, "{}", first.1);
    let mut renew = f.args("renew", "next");
    renew.extend(["--from-generation".into(), "initial".into()]);
    assert!(f.run(&renew).status.success());
    assert!(f.run(&renew).status.success());
    let next = f.token("next");
    assert!(next != token);
    assert_eq!(
        call(&f, &token, "POST", "/api/v1/import/website", body.clone())
            .await
            .0,
        401
    );
    assert_eq!(
        call(&f, &next, "POST", "/api/v1/import/website", body.clone()).await,
        first
    );
    let mut changed = body;
    changed["message"] = json!("different");
    assert_eq!(
        call(&f, &next, "POST", "/api/v1/import/website", changed)
            .await
            .0,
        409
    );
    let actor: String =
        sqlx::query_scalar("SELECT actor_label FROM events WHERE event_type='website.import'")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(actor, "machine:website");
    assert_eq!(f.count("machine_tokens").await, 2);
    assert!(!f.run(&args).status.success());
}

#[tokio::test]
async fn revoke_expiry_explicit_renewal_and_restore_fail_closed() {
    let f = Fixture::new().await;
    assert!(f.run(&f.args("provision", "initial")).status.success());
    let initial = f.token("initial");
    let revoke = f.args("revoke", "initial");
    assert!(f.run(&revoke).status.success());
    assert!(f.run(&revoke).status.success());
    assert!(machine::identify(&f.pool, &initial).await.is_err());
    assert!(!f.run(&f.args("provision", "initial")).status.success());
    let mut renew = f.args("renew", "second");
    renew.extend(["--from-generation".into(), "wrong".into()]);
    assert!(!f.run(&renew).status.success());
    *renew.last_mut().unwrap() = "initial".into();
    let scopes = renew.iter().position(|s| s == "--scopes").unwrap() + 1;
    renew[scopes] = "commerce:read,commerce:write".into();
    assert!(f.run(&renew).status.success());
    let second = f.token("second");
    assert_eq!(
        call(&f, &second, "GET", "/api/v1/commerce/catalog", Value::Null)
            .await
            .0,
        200
    );
    assert_eq!(
        call(&f, &second, "POST", "/api/v1/import/website", json!({}))
            .await
            .0,
        403
    );
    assert_eq!(
        call(&f, &second, "POST", "/api/v1/commerce/catalog", json!({}))
            .await
            .0,
        403
    );
    let restored = f.root.join("restored.sqlite");
    backup::restore(
        &f.root.join("workspace.sqlite"),
        &restored,
        &Profile::standalone(),
        false,
    )
    .await
    .unwrap();
    let pool = connect(restored.to_str().unwrap()).await.unwrap();
    assert!(machine::identify(&pool, &second).await.is_err());
    assert!(machine::identify(&f.pool, &second).await.is_ok());
    pool.close().await;
    sqlx::query("UPDATE machine_tokens SET expires_at=0 WHERE generation='second'")
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(machine::identify(&f.pool, &second).await.is_err());
    assert!(!f.run(&renew).status.success());
    let mut third = f.args("renew", "third");
    third.extend(["--from-generation".into(), "second".into()]);
    assert!(f.run(&third).status.success());
}

#[tokio::test]
async fn output_refuses_existing_files_links_and_unsafe_permissions_without_rotation() {
    let f = Fixture::new().await;
    let args = f.args("provision", "initial");
    let path = f.root.join("initial.json");
    fs::write(&path, "untouched").unwrap();
    assert!(!f.run(&args).status.success());
    assert_eq!(fs::read_to_string(&path).unwrap(), "untouched");
    assert_eq!(f.count("machine_tokens").await, 0);
    assert_eq!(f.count("machine_identities").await, 0);
    fs::remove_file(&path).unwrap();
    symlink(f.root.join("missing"), &path).unwrap();
    assert!(!f.run(&args).status.success());
    assert!(!f.root.join("missing").exists());
    fs::remove_file(&path).unwrap();
    let alias = f.root.join("alias");
    symlink(&f.root, &alias).unwrap();
    let mut linked = args.clone();
    *linked.last_mut().unwrap() = alias.join("initial.json").to_str().unwrap().into();
    assert!(!f.run(&linked).status.success());
    fs::set_permissions(&f.root, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!f.run(&args).status.success());
    fs::set_permissions(&f.root, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(f.run(&args).status.success());
    let original = f.token("initial");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!f.run(&args).status.success());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(&path, f.root.join("hardlink")).unwrap();
    assert!(!f.run(&args).status.success());
    fs::remove_file(f.root.join("hardlink")).unwrap();
    let mut renew = f.args("renew", "next");
    renew.extend(["--from-generation".into(), "initial".into()]);
    fs::write(f.root.join("next.json"), "untouched").unwrap();
    assert!(!f.run(&renew).status.success());
    assert!(machine::identify(&f.pool, &original).await.is_ok());
    fs::remove_file(&path).unwrap();
    assert!(!f.run(&args).status.success());
    assert_eq!(f.count("machine_tokens").await, 1);
}

#[tokio::test]
async fn concurrent_provisioning_has_one_token_and_one_output() {
    let f = Fixture::new().await;
    let args = f.args("provision", "initial");
    let launch = |args: Vec<String>| {
        std::thread::spawn(move || {
            Command::new(env!("CARGO_BIN_EXE_brilliantis"))
                .args(args)
                .output()
                .unwrap()
        })
    };
    let a = launch(args.clone());
    let b = launch(args);
    assert!(a.join().unwrap().status.success());
    assert!(b.join().unwrap().status.success());
    assert_eq!(f.count("machine_tokens").await, 1);
    assert_eq!(f.count("security_events").await, 1);
}

#[tokio::test]
async fn cli_rejects_scope_expansion_parameter_drift_and_non_workspace_databases() {
    let f = Fixture::new().await;
    for (flag, value) in [
        ("--scopes", "*"),
        ("--scopes", "admin"),
        ("--scopes", "finance:write"),
        ("--scopes", "crm:read"),
        ("--scopes", ""),
        ("--ttl-seconds", "0"),
        ("--ttl-seconds", "7776001"),
        ("--output", "relative.json"),
    ] {
        let mut args = f.args("provision", "initial");
        let position = args.iter().position(|s| s == flag).unwrap() + 1;
        args[position] = value.into();
        assert!(!f.run(&args).status.success(), "{flag}");
    }
    assert_eq!(f.count("machine_tokens").await, 0);
    sqlx::query("UPDATE metadata SET value='demo' WHERE key='mode'")
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(!f.run(&f.args("provision", "initial")).status.success());
    sqlx::query("UPDATE metadata SET value='workspace' WHERE key='mode'")
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(f.run(&f.args("provision", "initial")).status.success());
    let mut changed = f.args("provision", "initial");
    let pos = changed.iter().position(|s| s == "--scopes").unwrap() + 1;
    changed[pos] = "crm:write,commerce:read".into();
    assert!(!f.run(&changed).status.success());
    assert!(!f.run(&f.args("provision", "another")).status.success());
    assert_eq!(f.count("machine_tokens").await, 1);
    sqlx::query("DELETE FROM schema_migrations WHERE version=?")
        .bind(brilliantis::storage::SCHEMA_VERSION)
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(!f.run(&f.args("revoke", "initial")).status.success());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT MAX(version) FROM schema_migrations")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        brilliantis::storage::SCHEMA_VERSION - 1
    );
}

#[tokio::test]
async fn billing_machine_is_separate_from_importer_and_has_no_finance_or_admin_access() {
    let f = Fixture::new().await;
    let old_args = f.args("provision", "old");
    assert!(f.run(&old_args).status.success());
    let old = f.token("old");
    assert_eq!(
        call(
            &f,
            &old,
            "POST",
            "/api/v1/commerce/orders/missing/billing",
            json!({})
        )
        .await
        .0,
        403
    );
    let mut args = f.args("renew", "billing");
    let index = args.iter().position(|a| a == "--scopes").unwrap();
    args[index + 1] = "commerce:read,commerce:billing".into();
    args.extend(["--from-generation".into(), "old".into()]);
    let result = f.run(&args);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let token = f.token("billing");
    for (method, path) in [
        ("POST", "/api/v1/import/website"),
        ("POST", "/api/v1/commerce/catalog"),
        ("POST", "/api/v1/commerce/customers"),
        ("POST", "/api/v1/commerce/orders/missing/invoice"),
        ("POST", "/api/v1/commerce/orders/missing/provisioning"),
        ("GET", "/api/v1/finance/report"),
        ("POST", "/api/v1/records/bank-transactions"),
        ("POST", "/api/v1/access/tokens"),
    ] {
        assert_eq!(
            call(&f, &token, method, path, json!({})).await.0,
            403,
            "{path}"
        );
    }
    assert_eq!(
        call(&f, &token, "GET", "/api/v1/commerce/catalog", Value::Null)
            .await
            .0,
        200
    );
    assert_eq!(call(&f,&token,"POST","/api/v1/commerce/orders/missing/billing",json!({"version":1,"provider":"stripe","provider_account_id":"acct_test","provider_event_id":"evt_test","operation":"bind","customer_id":"missing","provider_customer_id":"cus_test","provider_subscription_id":"","checkout_reference":"cs_test","started_on":"2026-01-01"})).await.0,404);
}

#[tokio::test]
async fn operator_catalog_cli_is_versioned_idempotent_and_rejects_wrong_database_mode() {
    let f = Fixture::new().await;
    let input = f.root.join("catalog.json");
    fs::write(&input,json!({"version":0,"revision":"test-r1","items":[{"id":"bundle","name":"Bundle","kind":"service","auto_quote":true,"charges":[],"dimensions":[{"key":"sites","min":0,"max":2,"step":1,"charge":{"description":"Website","cadence":"once","unit_price_cents":10000,"tax_basis_points":1900}}]}]}).to_string()).unwrap();
    let args = vec![
        "publish-commerce-catalog".into(),
        "--database".into(),
        f.root.join("workspace.sqlite").to_str().unwrap().into(),
        "--input-file".into(),
        input.to_str().unwrap().into(),
        "--expected-version".into(),
        "0".into(),
    ];
    let result = f.run(&args);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(f.run(&args).status.success());
    assert_eq!(f.count("commerce_catalog").await, 1);
    assert_eq!(f.count("commerce_events").await, 1);
    let mut changed: Value = serde_json::from_slice(&fs::read(&input).unwrap()).unwrap();
    changed["items"][0]["name"] = json!("Changed");
    fs::write(&input, changed.to_string()).unwrap();
    assert!(!f.run(&args).status.success());
    assert_eq!(f.count("commerce_catalog").await, 1);
    sqlx::query("UPDATE metadata SET value='demo' WHERE key='mode'")
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(!f.run(&args).status.success());
}
