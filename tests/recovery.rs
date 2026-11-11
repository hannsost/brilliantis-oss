use brilliantis::{
    backup, connect,
    profile::{initialize_workspace, Profile},
};
#[tokio::test]
async fn consistent_backup_restore_preserves_data_and_never_overwrites() {
    let dir = std::env::temp_dir().join(format!("brilliantis-recovery-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let source = dir.join("source.sqlite");
    let copy = dir.join("backup.sqlite");
    let restored = dir.join("restored.sqlite");
    let pool = connect(source.to_str().unwrap()).await.unwrap();
    initialize_workspace(&pool, false, &Profile::standalone())
        .await
        .unwrap();
    sqlx::query("INSERT INTO metadata(key,value) VALUES('test-marker','preserved')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("INSERT INTO briefings(id,version,body) VALUES('brief',2,'{\"status\":\"ready\"}'); INSERT INTO briefing_editions(briefing_id,revision,snapshot,text,sha256,actor) VALUES('brief',1,'{}','preserved edition','fixture-hash','fixture'); INSERT INTO briefing_handovers(briefing_id,revision,actor,note) VALUES('brief',1,'fixture','Manual test handoff');")
        .execute(&pool).await.unwrap();
    backup::snapshot(&source, &copy).await.unwrap();
    assert!(backup::snapshot(&source, &copy).await.is_err());
    sqlx::query("UPDATE metadata SET value='later' WHERE key='test-marker'")
        .execute(&pool)
        .await
        .unwrap();
    backup::restore(&copy, &restored, &Profile::standalone(), false)
        .await
        .unwrap();
    assert!(
        backup::restore(&copy, &source, &Profile::standalone(), false)
            .await
            .is_err()
    );
    let r = connect(restored.to_str().unwrap()).await.unwrap();
    let marker: String = sqlx::query_scalar("SELECT value FROM metadata WHERE key='test-marker'")
        .fetch_one(&r)
        .await
        .unwrap();
    assert_eq!(marker, "preserved");
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT text FROM briefing_editions WHERE briefing_id='brief'"
        )
        .fetch_one(&r)
        .await
        .unwrap(),
        "preserved edition"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT note FROM briefing_handovers WHERE briefing_id='brief'"
        )
        .fetch_one(&r)
        .await
        .unwrap(),
        "Manual test handoff"
    );
    assert!(sqlx::query("UPDATE briefing_editions SET text='changed'")
        .execute(&r)
        .await
        .is_err());
    assert!(sqlx::query("DELETE FROM briefing_editions")
        .execute(&r)
        .await
        .is_err());
    backup::verify(&r).await.unwrap();
    r.close().await;
    pool.close().await;
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn briefing_migration_preserves_schema7_business_data() {
    let pool = connect(":memory:").await.unwrap();
    initialize_workspace(&pool, false, &Profile::standalone())
        .await
        .unwrap();
    // Reconstitute the previously shipped schema before applying the additive migration.
    sqlx::raw_sql("DROP TRIGGER commerce_recurring_invoice_changed; DROP TABLE commerce_billing_refunds; DROP TABLE commerce_billing_payments; DROP TABLE commerce_billing_periods; DROP TABLE commerce_billing_invoices; DROP TABLE commerce_billing_events; DROP TABLE commerce_billing_bindings; DROP TABLE commerce_billing_accounts; DELETE FROM schema_migrations WHERE version=9; DROP TABLE briefing_handovers; DROP TABLE briefing_editions; DROP TABLE briefings; DELETE FROM schema_migrations WHERE version=8;")
        .execute(&pool).await.unwrap();
    let before: Vec<(String, String, String)> =
        sqlx::query_as("SELECT kind,id,body FROM records ORDER BY kind,id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(!before.is_empty());
    brilliantis::storage::migrate(&pool).await.unwrap();
    let after: Vec<(String, String, String)> =
        sqlx::query_as("SELECT kind,id,body FROM records ORDER BY kind,id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM briefings")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    backup::verify(&pool).await.unwrap();
}
#[tokio::test]
async fn migration_refuses_future_or_altered_schema() {
    let pool = connect(":memory:").await.unwrap();
    sqlx::query("UPDATE schema_migrations SET checksum='altered' WHERE version=2")
        .execute(&pool)
        .await
        .unwrap();
    assert!(brilliantis::storage::migrate(&pool).await.is_err());
    let pool = connect(":memory:").await.unwrap();
    sqlx::query("INSERT INTO schema_migrations(version,checksum) VALUES(999,'unknown')")
        .execute(&pool)
        .await
        .unwrap();
    assert!(brilliantis::storage::migrate(&pool).await.is_err());
}

#[tokio::test]
async fn restore_includes_live_wal_and_revokes_access_without_mutating_source() {
    let dir = std::env::temp_dir().join(format!("brilliantis-wal-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let source = dir.join("source.sqlite");
    let destination = dir.join("restored.sqlite");
    let pool = connect(source.to_str().unwrap()).await.unwrap();
    initialize_workspace(&pool, false, &Profile::standalone())
        .await
        .unwrap();
    sqlx::query("PRAGMA wal_autocheckpoint=0")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("INSERT INTO metadata(key,value) VALUES('wal-only','committed'); INSERT INTO users(id,email,name,role) VALUES('fixture-user','fixture@example.invalid','Fixture','sales'); INSERT INTO sessions(token_hash,user_id,csrf,created_at,expires_at,last_seen) VALUES('fixture-session-hash','fixture-user','fixture-csrf',1,9999999999,1); INSERT INTO api_tokens(id,user_id,name,token_hash,scopes,created_at,expires_at) VALUES('fixture-token','fixture-user','Fixture','fixture-token-hash','[\"crm:read\"]',1,9999999999); INSERT INTO integration_jobs(id,idempotency_key,request,connection_id,operation,payload,plan,actor,user_id,required_scope,safe_retry,status,next_attempt,created_at,updated_at) VALUES('fixture-job','fixture-key','{}','unused','unused','{}','{}','fixture','fixture-user','crm:read',1,'queued',1,1,1);")
        .execute(&pool).await.unwrap();
    assert!(
        std::fs::metadata(dir.join("source.sqlite-wal"))
            .unwrap()
            .len()
            > 0
    );
    sqlx::query("INSERT INTO oidc_flows(state_hash,browser_hash,purpose,issuer,client_id,redirect_uri,verifier,nonce_hash,created_at,expires_at) VALUES('test-state','test-browser','login','https://example.invalid','test','https://example.invalid/callback','test-verifier','test-nonce',1,9999999999)").execute(&pool).await.unwrap();
    backup::restore(&source, &destination, &Profile::standalone(), false)
        .await
        .unwrap();
    let restored = connect(destination.to_str().unwrap()).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM oidc_flows")
            .fetch_one(&restored)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM oidc_flows")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT value FROM metadata WHERE key='wal-only'")
            .fetch_one(&restored)
            .await
            .unwrap(),
        "committed"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sessions")
            .fetch_one(&restored)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT revoked FROM api_tokens")
            .fetch_one(&restored)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM integration_jobs")
            .fetch_one(&restored)
            .await
            .unwrap(),
        "uncertain"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT revoked FROM api_tokens")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM integration_jobs")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "queued"
    );
    restored.close().await;
    pool.close().await;
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn billing_schema8_backup_upgrades_additively_and_restores_with_version_assertions() {
    let dir = std::env::temp_dir().join(format!(
        "brilliantis-billing-migrate-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir(&dir).unwrap();
    let source = dir.join("source.sqlite");
    let snapshot = dir.join("schema8.sqlite");
    let restored = dir.join("restored.sqlite");
    let pool = connect(source.to_str().unwrap()).await.unwrap();
    initialize_workspace(&pool, false, &Profile::standalone())
        .await
        .unwrap();
    sqlx::raw_sql("DROP TRIGGER commerce_recurring_invoice_changed; DROP TABLE commerce_billing_refunds; DROP TABLE commerce_billing_payments; DROP TABLE commerce_billing_periods; DROP TABLE commerce_billing_invoices; DROP TABLE commerce_billing_events; DROP TABLE commerce_billing_bindings; DROP TABLE commerce_billing_accounts; DELETE FROM schema_migrations WHERE version=9;").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO commerce_invoices(order_id,invoice_id) VALUES('existing-order','existing-invoice')").execute(&pool).await.unwrap();
    let before: Vec<(String, String, String)> =
        sqlx::query_as("SELECT kind,id,body FROM records ORDER BY kind,id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT MAX(version) FROM schema_migrations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        8
    );
    backup::snapshot(&source, &snapshot).await.unwrap();
    brilliantis::storage::migrate(&pool).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT MAX(version) FROM schema_migrations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        9
    );
    assert_eq!(
        sqlx::query_as::<_, (String, String, String)>(
            "SELECT kind,id,body FROM records ORDER BY kind,id"
        )
        .fetch_all(&pool)
        .await
        .unwrap(),
        before
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM commerce_invoices")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    backup::verify(&pool).await.unwrap();
    backup::restore(&snapshot, &restored, &Profile::standalone(), false)
        .await
        .unwrap();
    let r = connect(restored.to_str().unwrap()).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT MAX(version) FROM schema_migrations")
            .fetch_one(&r)
            .await
            .unwrap(),
        9
    );
    assert_eq!(
        sqlx::query_as::<_, (String, String, String)>(
            "SELECT kind,id,body FROM records ORDER BY kind,id"
        )
        .fetch_all(&r)
        .await
        .unwrap(),
        before
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT invoice_id FROM commerce_invoices WHERE order_id='existing-order'"
        )
        .fetch_one(&r)
        .await
        .unwrap(),
        "existing-invoice"
    );
    backup::verify(&r).await.unwrap();
    r.close().await;
    pool.close().await;
    std::fs::remove_dir_all(dir).unwrap();
}
