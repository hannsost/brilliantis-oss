//! Explicit local operator access. Independent from all human accounts and sessions.
#[cfg(unix)]
mod file;
use crate::{
    auth::{self, Principal},
    conflict, invalid,
    profile::Profile,
    storage::hash,
    Result,
};
use serde_json::json;
use sqlx::{Row, SqlitePool};
use std::{collections::BTreeMap, path::PathBuf};

pub const SCOPES: &[&str] = &[
    "crm:write",
    "commerce:read",
    "commerce:write",
    "commerce:billing",
];

pub async fn identify(pool: &SqlitePool, token: &str) -> Result<Principal> {
    if !token.starts_with("brm_") || token.len() != 68 {
        return Err(auth::unauthorized());
    }
    let row = sqlx::query("SELECT t.machine,t.scopes FROM machine_tokens t JOIN machine_identities m ON m.name=t.machine AND m.current_generation=t.generation WHERE t.token_hash=? AND t.revoked=0 AND t.expires_at>?")
        .bind(hash(token)).bind(auth::now()).fetch_optional(pool).await?.ok_or_else(auth::unauthorized)?;
    let name: String = row.get("machine");
    let scopes: Vec<String> =
        serde_json::from_str(row.get("scopes")).map_err(|_| auth::unauthorized())?;
    if scopes.is_empty() || scopes.iter().any(|s| !SCOPES.contains(&s.as_str())) {
        return Err(auth::unauthorized());
    }
    Ok(Principal {
        id: format!("machine:{name}"),
        name: name.clone(),
        role: "machine".into(),
        actor: format!("machine:{name}"),
        scopes,
        session_hash: None,
        csrf: None,
    })
}

pub struct Issue {
    pub machine: String,
    pub generation: String,
    pub from_generation: Option<String>,
    pub scopes: Vec<String>,
    pub ttl_seconds: i64,
    pub output: PathBuf,
}

fn identifier(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return Err(invalid("Maschinenname und Generation benötigen 1–80 Zeichen aus a-z, 0-9, Unterstrich oder Bindestrich."));
    }
    Ok(())
}

#[cfg(unix)]
pub async fn issue(pool: &SqlitePool, mut input: Issue) -> Result<()> {
    identifier(&input.machine)?;
    identifier(&input.generation)?;
    if let Some(previous) = &input.from_generation {
        identifier(previous)?;
        if previous == &input.generation {
            return Err(invalid("Erneuerung benötigt eine neue Generation."));
        }
    }
    if !(60..=90 * 86400).contains(&input.ttl_seconds)
        || input.scopes.is_empty()
        || input.scopes.len() > SCOPES.len()
        || input.scopes.iter().any(|s| !SCOPES.contains(&s.as_str()))
    {
        return Err(invalid("Explizite Scopes crm:write, commerce:read/write/billing und Laufzeit 60–7776000 Sekunden erforderlich; keine Wildcards oder Administratorrechte."));
    }
    input.scopes.sort();
    input.scopes.dedup();
    let output = file::Output::open(&input.output)?;
    let request = json!({"from_generation":input.from_generation,"scopes":input.scopes,"ttl_seconds":input.ttl_seconds,"output":input.output}).to_string();
    // Serialize operators across processes before reading the current generation.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    if let Some(row) = sqlx::query("SELECT request,output_hash,revoked,expires_at FROM machine_tokens WHERE machine=? AND generation=?")
        .bind(&input.machine).bind(&input.generation).fetch_optional(&mut *tx).await? {
        if row.get::<String,_>("request") != request { return Err(conflict("Generation existiert mit anderen Parametern. Keine Änderung vorgenommen.")); }
        if row.get::<i64,_>("revoked") != 0 || row.get::<i64,_>("expires_at") <= auth::now() {
            return Err(conflict("Generation widerrufen oder abgelaufen. Neue Generation ausdrücklich mit renew anlegen."));
        }
        output.matches(row.get("output_hash"))?;
        return Ok(());
    }
    let current: Option<String> =
        sqlx::query_scalar("SELECT current_generation FROM machine_identities WHERE name=?")
            .bind(&input.machine)
            .fetch_optional(&mut *tx)
            .await?;
    match (&current, &input.from_generation) {
        (None,None) => { sqlx::query("INSERT INTO machine_identities(name,current_generation,created_at) VALUES(?,?,?)")
            .bind(&input.machine).bind(&input.generation).bind(auth::now()).execute(&mut *tx).await?; }
        (Some(current),Some(previous)) if current == previous => {
            sqlx::query("UPDATE machine_tokens SET revoked=1 WHERE machine=? AND revoked=0").bind(&input.machine).execute(&mut *tx).await?;
            sqlx::query("UPDATE machine_identities SET current_generation=? WHERE name=?").bind(&input.generation).bind(&input.machine).execute(&mut *tx).await?;
        }
        _ => return Err(conflict("Maschine existiert bereits oder Ausgangsgeneration stimmt nicht. Bestehenden Zugang nicht überschrieben; renew benötigt die aktuelle Generation.")),
    }
    let created = auth::now();
    let expires = created + input.ttl_seconds;
    let token = format!("brm_{}", auth::secret());
    let document = json!({"schema_version":1,"machine":input.machine,"generation":input.generation,"token":token,"scopes":input.scopes,"created_at":created,"expires_at":expires}).to_string()+"\n";
    sqlx::query("INSERT INTO machine_tokens(machine,generation,token_hash,scopes,request,output_hash,created_at,expires_at) VALUES(?,?,?,?,?,?,?,?)")
        .bind(&input.machine).bind(&input.generation).bind(hash(&token)).bind(json!(input.scopes).to_string()).bind(request).bind(hash(&document)).bind(created).bind(expires).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO security_events(actor,action,target) VALUES('operator.cli',?,?)")
        .bind(if input.from_generation.is_some() {
            "machine.renew"
        } else {
            "machine.provision"
        })
        .bind(format!("{}/{}", input.machine, input.generation))
        .execute(&mut *tx)
        .await?;
    // Publish and fsync the private file before activating its hash. On an IO/commit
    // failure leave any file for explicit operator recovery, never overwrite it.
    output.write_new(document.as_bytes())?;
    tx.commit().await?;
    Ok(())
}

#[cfg(not(unix))]
pub async fn issue(_pool: &SqlitePool, _input: Issue) -> Result<()> {
    Err(invalid(
        "Sichere Token-Dateiausgabe benötigt Unix-Dateirechte.",
    ))
}

pub async fn revoke(pool: &SqlitePool, machine: &str, generation: &str) -> Result<()> {
    identifier(machine)?;
    identifier(generation)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let revoked: Option<i64> =
        sqlx::query_scalar("SELECT revoked FROM machine_tokens WHERE machine=? AND generation=?")
            .bind(machine)
            .bind(generation)
            .fetch_optional(&mut *tx)
            .await?;
    match revoked {
        Some(1) => return Ok(()),
        Some(0) => {}
        _ => return Err(invalid("Maschine oder Generation nicht vorhanden.")),
    }
    sqlx::query("UPDATE machine_tokens SET revoked=1 WHERE machine=? AND generation=?")
        .bind(machine)
        .bind(generation)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO security_events(actor,action,target) VALUES('operator.cli','machine.revoke',?)").bind(format!("{machine}/{generation}")).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn cli(
    profile: Profile,
    args: Vec<String>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    if args.iter().any(|s| s == "--help") {
        println!("brilliantis machine provision --database PATH --name NAME --generation ID --scopes crm:write --ttl-seconds 2592000 --output /PRIVATE/NEW.json\nbrilliantis machine renew (gleiche Felder) --from-generation PREVIOUS\nbrilliantis machine revoke --database PATH --name NAME --generation ID\nExplizite Scopes: crm:write,commerce:read,commerce:write,commerce:billing. Laufzeit: 60–7776000 Sekunden. Ausgabe: private neue 0600-JSON-Datei; vorhandene Ziele werden nie überschrieben.");
        return Ok(());
    }
    let action = args.first().map(String::as_str).unwrap_or("");
    if !["provision", "renew", "revoke"].contains(&action) {
        return Err(invalid("Maschinenaktion provision, renew oder revoke erforderlich.").into());
    }
    let mut fields = BTreeMap::new();
    let (pairs, remainder) = args[1..].as_chunks::<2>();
    for pair in pairs {
        if ![
            "--database",
            "--name",
            "--generation",
            "--scopes",
            "--ttl-seconds",
            "--output",
            "--from-generation",
        ]
        .contains(&pair[0].as_str())
            || fields.insert(pair[0].as_str(), pair[1].as_str()).is_some()
        {
            return Err(invalid("Unbekannter oder doppelter Maschinenparameter.").into());
        }
    }
    if !remainder.is_empty() {
        return Err(invalid("Maschinenparameter benötigt einen Wert.").into());
    }
    let take = |key| {
        fields
            .get(key)
            .copied()
            .ok_or_else(|| invalid("Erforderlicher Maschinenparameter fehlt."))
    };
    let database = take("--database")?;
    let machine = take("--name")?;
    let generation = take("--generation")?;
    let input = if action == "revoke" {
        if fields.len() != 3 {
            return Err(
                invalid("Widerruf benötigt ausschließlich database, name und generation.").into(),
            );
        }
        None
    } else {
        if (action == "renew") != fields.contains_key("--from-generation") {
            return Err(invalid("Nur renew benötigt from-generation.").into());
        }
        Some(Issue {
            machine: machine.into(),
            generation: generation.into(),
            from_generation: fields.get("--from-generation").map(|s| s.to_string()),
            scopes: take("--scopes")?.split(',').map(String::from).collect(),
            ttl_seconds: take("--ttl-seconds")?
                .parse()
                .map_err(|_| invalid("Laufzeit ungültig."))?,
            output: take("--output")?.into(),
        })
    };
    // Do not create a workspace for a typo or migrate a database behind an old server.
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(database)
                .create_if_missing(false)
                .foreign_keys(true)
                .busy_timeout(std::time::Duration::from_secs(5)),
        )
        .await
        .map_err(|_| invalid("Bestehende Arbeitsdatenbank konnte nicht geöffnet werden."))?;
    crate::backup::verify(&pool)
        .await
        .map_err(|_| invalid("Arbeitsdatenbankprüfung fehlgeschlagen."))?;
    let schema: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(version),0) FROM schema_migrations")
        .fetch_one(&pool)
        .await
        .map_err(|_| invalid("Arbeitsdatenbankschema fehlt."))?;
    let mode: Option<String> = sqlx::query_scalar("SELECT value FROM metadata WHERE key='mode'")
        .fetch_optional(&pool)
        .await
        .map_err(|_| invalid("Arbeitsdatenbankmodus fehlt."))?;
    let stored: Option<String> =
        sqlx::query_scalar("SELECT value FROM metadata WHERE key='profile'")
            .fetch_optional(&pool)
            .await
            .map_err(|_| invalid("Arbeitsdatenbankprofil fehlt."))?;
    if schema != crate::storage::SCHEMA_VERSION
        || mode.as_deref() != Some("workspace")
        || stored.as_deref() != Some(profile.id())
    {
        return Err(invalid("Zuerst Arbeitsinstanz mit passendem Profil und aktuellem Binary starten. Demo oder altes Schema nicht verändert.").into());
    }
    if let Some(input) = input {
        issue(&pool, input).await?;
    } else {
        revoke(&pool, machine, generation).await?;
    }
    pool.close().await;
    println!("Maschinenzugang geprüft. Keine Zugangsdaten ausgegeben.");
    Ok(())
}
