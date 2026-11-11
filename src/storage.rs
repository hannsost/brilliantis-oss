use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/001_init.sql")),
    (2, include_str!("../migrations/002_workspace.sql")),
    (3, include_str!("../migrations/003_jobs.sql")),
    (4, include_str!("../migrations/004_finance.sql")),
    (5, include_str!("../migrations/005_commerce.sql")),
    (6, include_str!("../migrations/006_browser_oidc.sql")),
    (7, include_str!("../migrations/007_machines.sql")),
    (8, include_str!("../migrations/008_briefings.sql")),
    (9, include_str!("../migrations/009_billing.sql")),
];
pub const SCHEMA_VERSION: i64 = 9;

pub fn migration_checksum(version: i64) -> Option<String> {
    MIGRATIONS
        .iter()
        .find(|(v, _)| *v == version)
        .map(|(_, source)| hash(source))
}

pub fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

pub async fn migrate(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS schema_migrations(version INTEGER PRIMARY KEY, checksum TEXT NOT NULL, applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')))").execute(&mut *tx).await?;
    let applied = sqlx::query("SELECT version,checksum FROM schema_migrations ORDER BY version")
        .fetch_all(&mut *tx)
        .await?;
    for (index, row) in applied.iter().enumerate() {
        if row.get::<i64, _>("version") != index as i64 + 1 {
            return Err(sqlx::Error::Protocol(
                "Schema-Migrationsfolge ist unvollständig.".into(),
            ));
        }
        let version: i64 = row.get("version");
        let Some((_, source)) = MIGRATIONS.iter().find(|(v, _)| *v == version) else {
            return Err(sqlx::Error::Protocol(
                "Datenbankschema ist neuer als dieses Binary.".into(),
            ));
        };
        if row.get::<String, _>("checksum") != hash(source) {
            return Err(sqlx::Error::Protocol(
                "Prüfsumme einer angewendeten Migration stimmt nicht.".into(),
            ));
        }
    }
    for (version, source) in MIGRATIONS {
        if applied
            .iter()
            .any(|r| r.get::<i64, _>("version") == *version)
        {
            continue;
        }
        // v1 uses CREATE IF NOT EXISTS and adopts the original local prototype.
        sqlx::raw_sql(source).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO schema_migrations(version,checksum) VALUES(?,?)")
            .bind(version)
            .bind(hash(source))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}
