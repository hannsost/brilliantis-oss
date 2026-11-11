use crate::{
    profile::{initialize_workspace, Profile},
    storage,
};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Row, SqlitePool,
};
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};
type BackupResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;
async fn open(path: &Path, readonly: bool) -> BackupResult<SqlitePool> {
    Ok(SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .read_only(readonly)
                .create_if_missing(false)
                .foreign_keys(true)
                .busy_timeout(std::time::Duration::from_secs(10)),
        )
        .await?)
}
fn staging(destination: &Path) -> BackupResult<PathBuf> {
    if destination.exists() {
        return Err("Zieldatei besteht bereits. Neues Ziel verwenden.".into());
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let path = parent.join(format!(".brilliantis-{}.sqlite", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(&path)?;
    Ok(path)
}
fn publish(staging: &Path, destination: &Path) -> BackupResult<()> {
    std::fs::File::open(staging)?.sync_all()?;
    std::fs::hard_link(staging, destination).map_err(|e| {
        if e.kind() == ErrorKind::AlreadyExists {
            "Ziel besteht bereits.".to_string()
        } else {
            e.to_string()
        }
    })?;
    std::fs::remove_file(staging)?;
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}
pub async fn verify(pool: &SqlitePool) -> BackupResult<()> {
    let rows: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(pool)
        .await?;
    if rows != ["ok"] {
        return Err("SQLite-Integritätsprüfung fehlgeschlagen.".into());
    }
    let fk = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(pool)
        .await?;
    if !fk.is_empty() {
        return Err("Fremdschlüsselprüfung fehlgeschlagen.".into());
    }
    let table: Option<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='table' AND name='metadata'")
            .fetch_optional(pool)
            .await?;
    if table.is_none() {
        return Err("Keine Brilliantis-Datenbank.".into());
    }
    let schema: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name='schema_migrations'",
    )
    .fetch_optional(pool)
    .await?;
    if schema.is_some() {
        let applied =
            sqlx::query("SELECT version,checksum FROM schema_migrations ORDER BY version")
                .fetch_all(pool)
                .await?;
        for (index, row) in applied.iter().enumerate() {
            let version: i64 = row.get("version");
            let expected =
                storage::migration_checksum(version).ok_or("Unbekanntes Datenbankschema.")?;
            if version != index as i64 + 1 || row.get::<String, _>("checksum") != expected {
                return Err("Schema-Prüfsummen oder Reihenfolge ungültig.".into());
            }
        }
    }
    Ok(())
}
pub async fn snapshot(source: &Path, destination: &Path) -> BackupResult<()> {
    let pool = open(source, true).await?;
    verify(&pool).await?;
    let temporary = staging(destination)?;
    let result = async {
        sqlx::query("VACUUM INTO ?")
            .bind(temporary.to_string_lossy().as_ref())
            .execute(&pool)
            .await?;
        let check = open(&temporary, true).await?;
        verify(&check).await?;
        check.close().await;
        publish(&temporary, destination)
    }
    .await;
    pool.close().await;
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}
pub async fn restore(
    source: &Path,
    destination: &Path,
    profile: &Profile,
    demo: bool,
) -> BackupResult<()> {
    let source_pool = open(source, true).await?;
    verify(&source_pool).await?;
    let temporary = staging(destination)?;
    let result=async{sqlx::query("VACUUM INTO ?").bind(temporary.to_string_lossy().as_ref()).execute(&source_pool).await?;let pool=open(&temporary,false).await?;storage::migrate(&pool).await?;initialize_workspace(&pool,demo,profile).await?;let mut tx=pool.begin().await?;sqlx::query("DELETE FROM sessions").execute(&mut *tx).await?;sqlx::query("DELETE FROM oidc_flows").execute(&mut *tx).await?;sqlx::query("UPDATE api_tokens SET revoked=1").execute(&mut *tx).await?;sqlx::query("UPDATE machine_tokens SET revoked=1").execute(&mut *tx).await?;sqlx::query("UPDATE integration_jobs SET status='uncertain',error_code='restored_backup',lease_owner=NULL,lease_until=NULL WHERE status IN ('queued','retry','running')").execute(&mut *tx).await?;sqlx::query("INSERT INTO security_events(actor,action,target) VALUES('operator','backup.restore','workspace')").execute(&mut *tx).await?;tx.commit().await?;verify(&pool).await?;pool.close().await;publish(&temporary,destination)}.await;
    source_pool.close().await;
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}
