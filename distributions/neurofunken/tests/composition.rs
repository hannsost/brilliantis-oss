use brilliantis::{
    connect, domain,
    profile::{initialize_workspace, Profile},
    records, seed_demo,
};
use serde_json::json;

#[tokio::test]
async fn neurofunken_composes_the_core_and_preserves_seeded_user_work() {
    let profile = brilliantis_neurofunken::profile().unwrap();
    assert_eq!(profile.id(), "neurofunken");
    assert_eq!(profile.website_product(true), "hosting");
    assert_eq!(profile.website_product(false), "engineering");
    assert!(profile.accepts_product("brilliantis"));
    assert!(profile.accepts_product("orbit"));
    assert!(!profile.accepts_product("service"));
    for id in ["solveit", "symphony", "keycloak", "bitwarden"] {
        assert!(profile
            .integrations()
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["id"] == id && i["status"] == "planned"));
    }
    let pool = connect(":memory:").await.unwrap();
    initialize_workspace(&pool, true, &profile).await.unwrap();
    seed_demo(&pool, &profile).await.unwrap();
    let before = records(&pool, "lead").await.unwrap();
    assert_eq!(before.len(), 6);
    let record = before.iter().find(|r| r["status"] == "new").unwrap();
    domain::execute(
        &pool,
        &profile,
        "user-edit",
        "test",
        "lead.update",
        record["id"].as_str(),
        json!({"version":1,"status":"qualified","next_action":"Eigene Notiz"}),
    )
    .await
    .unwrap();
    seed_demo(&pool, &profile).await.unwrap();
    let after = records(&pool, "lead").await.unwrap();
    assert_eq!(after.len(), 6);
    assert_eq!(
        after.iter().find(|r| r["id"] == record["id"]).unwrap()["next_action"],
        "Eigene Notiz"
    );
    assert_eq!(records(&pool, "creator").await.unwrap().len(), 2);
    assert!(initialize_workspace(&pool, true, &Profile::standalone())
        .await
        .is_err());
}

#[tokio::test]
async fn legacy_workspace_requires_neurofunken_and_keeps_command_receipts() {
    let pool = connect(":memory:").await.unwrap();
    let profile = brilliantis_neurofunken::profile().unwrap();
    initialize_workspace(&pool, true, &profile).await.unwrap();
    seed_demo(&pool, &profile).await.unwrap();
    let before = records(&pool, "lead").await.unwrap();
    sqlx::query("DELETE FROM metadata WHERE key='profile'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(initialize_workspace(&pool, true, &Profile::standalone())
        .await
        .is_err());
    initialize_workspace(&pool, true, &profile).await.unwrap();
    seed_demo(&pool, &profile).await.unwrap();
    assert_eq!(before, records(&pool, "lead").await.unwrap());
}
