//! Explicit local catalog publication. Never borrows a human session or widens a service token.
use super::*;
use crate::profile::Profile;

pub async fn cli(
    profile: Profile,
    args: Vec<String>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    if args == ["--help"] {
        println!("brilliantis publish-commerce-catalog --database PATH --input-file PATH --expected-version N\nBestehende Arbeitsinstanz; aktuelles Schema/Profil. Eingabe: CommerceCatalog JSON. Keine Zugangsdaten erforderlich oder ausgegeben.");
        return Ok(());
    }
    let mut fields = BTreeMap::new();
    let mut args = args.into_iter();
    while let Some(key) = args.next() {
        if !["--database", "--input-file", "--expected-version"].contains(&key.as_str())
            || fields.contains_key(&key)
        {
            return Err(invalid("Unbekanntes oder doppeltes Katalogargument.").into());
        }
        fields.insert(
            key,
            args.next()
                .ok_or_else(|| invalid("Katalogargument benötigt einen Wert."))?,
        );
    }
    let take = |key: &str| {
        fields
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| invalid("database, input-file und expected-version sind erforderlich."))
    };
    let expected: i64 = take("--expected-version")?
        .parse()
        .map_err(|_| invalid("Katalogversion ungültig."))?;
    if expected < 0 {
        return Err(invalid("Katalogversion ungültig.").into());
    }
    let raw = std::fs::read(take("--input-file")?)?;
    if raw.len() > 1024 * 1024 {
        return Err(invalid("Katalogdatei zu groß.").into());
    }
    let input: Value = serde_json::from_slice(&raw)
        .map_err(|_| invalid("Katalogdatei benötigt gültiges JSON."))?;
    crate::connectors::reject_secrets(&input)?;
    let p: CatalogInput = parse(&input)?;
    if p.version != expected {
        return Err(conflict("Dateiversion entspricht nicht expected-version.").into());
    }
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(take("--database")?)
                .create_if_missing(false)
                .foreign_keys(true)
                .busy_timeout(std::time::Duration::from_secs(5)),
        )
        .await
        .map_err(|_| invalid("Bestehende Arbeitsdatenbank konnte nicht geöffnet werden."))?;
    crate::backup::verify(&pool)
        .await
        .map_err(|_| invalid("Arbeitsdatenbankprüfung fehlgeschlagen."))?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let schema: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(version),0) FROM schema_migrations")
        .fetch_one(&mut *tx)
        .await?;
    let mode: Option<String> = sqlx::query_scalar("SELECT value FROM metadata WHERE key='mode'")
        .fetch_optional(&mut *tx)
        .await?;
    let stored: Option<String> =
        sqlx::query_scalar("SELECT value FROM metadata WHERE key='profile'")
            .fetch_optional(&mut *tx)
            .await?;
    if schema != crate::storage::SCHEMA_VERSION
        || mode.as_deref() != Some("workspace")
        || stored.as_deref() != Some(profile.id())
    {
        return Err(invalid("Zuerst Arbeitsinstanz mit passendem Profil und aktuellem Binary starten. Demo oder altes Schema nicht verändert.").into());
    }
    let current: Option<String> = sqlx::query_scalar(
        "SELECT c.body FROM commerce_catalog c JOIN commerce_state s ON s.revision=c.revision",
    )
    .fetch_optional(&mut *tx)
    .await?;
    let expected_response = json!({"contract":CONTRACT,"version":expected+1,"revision":p.revision,"currency":"EUR","items":p.items});
    if current.as_deref().map(decode).transpose()?.as_ref() == Some(&expected_response) {
        println!("Katalogrevision bereits identisch veröffentlicht; keine Änderung.");
        return Ok(());
    }
    let response = catalog_in(&mut tx, &input, "operator.cli").await?;
    sqlx::query("INSERT INTO security_events(actor,action,target) VALUES('operator.cli','commerce.catalog.publish',?)").bind(response["revision"].as_str()).execute(&mut *tx).await?;
    tx.commit().await?;
    pool.close().await;
    println!(
        "Katalog veröffentlicht: Revision {}, Version {}.",
        response["revision"].as_str().unwrap(),
        response["version"]
    );
    Ok(())
}
