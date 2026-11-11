use crate::{
    connect,
    profile::{initialize_workspace, Profile},
    router, seed_demo, AppState,
};
use std::{net::Ipv4Addr, path::PathBuf};

pub async fn run(profile: Profile) -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("machine") {
        // No tracing subscriber or HTTP server in the secret-writing operator CLI.
        return crate::machine::cli(profile, std::env::args().skip(2).collect()).await;
    }
    if std::env::args().nth(1).as_deref() == Some("publish-commerce-catalog") {
        return crate::commerce::operator::cli(profile, std::env::args().skip(2).collect()).await;
    }
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "brilliantis=info".into()),
        )
        .with_target(false)
        .try_init();
    let mut oidc_file: Option<PathBuf> = None;
    let mut backup_path: Option<PathBuf> = None;
    let mut restore_path: Option<PathBuf> = None;
    let mut init_if_empty = false;
    let mut port: u16 = 4317;
    let mut demo = false;
    let mut connectors_file: Option<PathBuf> = None;
    let mut bind: std::net::IpAddr = Ipv4Addr::LOCALHOST.into();
    let mut public_url: Option<String> = None;
    let mut admin: Option<String> = None;
    let mut password_file: Option<PathBuf> = None;
    let mut healthcheck = false;
    let mut database: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--demo" => demo = true,
            "--oidc-file" => {
                oidc_file = Some(args.next().ok_or("--oidc-file benötigt einen Pfad")?.into())
            }
            "--backup" => {
                backup_path = Some(
                    args.next()
                        .ok_or("--backup benötigt einen Zielpfad")?
                        .into(),
                )
            }
            "--restore" => {
                restore_path = Some(
                    args.next()
                        .ok_or("--restore benötigt einen Quellpfad")?
                        .into(),
                )
            }
            "--init-if-empty" => init_if_empty = true,
            "--connectors-file" => {
                connectors_file = Some(
                    args.next()
                        .ok_or("--connectors-file benötigt einen Pfad")?
                        .into(),
                )
            }
            "--bind" => {
                bind = args
                    .next()
                    .ok_or("--bind benötigt eine IP-Adresse")?
                    .parse()?
            }
            "--public-url" => {
                public_url = Some(args.next().ok_or("--public-url benötigt einen Origin")?)
            }
            "--init-admin" => {
                admin = Some(
                    args.next()
                        .ok_or("--init-admin benötigt eine E-Mail-Adresse")?,
                )
            }
            "--password-file" => {
                password_file = Some(
                    args.next()
                        .ok_or("--password-file benötigt einen Pfad")?
                        .into(),
                )
            }
            "--profile" => {
                let requested = args.next().ok_or("--profile benötigt einen Wert")?;
                if requested != profile.id() {
                    return Err(format!("Dieses Binary enthält das Profil {}. Für {requested} das passende Distributionspaket bauen.", profile.id()).into());
                }
            }
            "--healthcheck" => healthcheck = true,
            "--port" => port = args.next().ok_or("--port benötigt einen Wert")?.parse()?,
            "--database" => {
                database = Some(args.next().ok_or("--database benötigt einen Pfad")?.into())
            }
            "--help" | "-h" => {
                println!("brilliantis [--demo] [--port 4317] [--database PFAD] [--bind IP --public-url https://HOST] [--connectors-file PFAD] [--oidc-file PFAD]\nEinrichtung: --init-admin EMAIL --password-file PFAD [--init-if-empty]\nSicherung: --backup ZIEL | --restore QUELLE (neuer --database Pfad)\nStatus: --healthcheck. Demo ausschließlich an Loopback.");
                println!("Katalog: brilliantis publish-commerce-catalog --database PATH --input-file PATH --expected-version N");
                println!("Maschinenzugänge: brilliantis machine provision|renew|revoke --help");
                return Ok(());
            }
            _ => return Err(format!("Unbekanntes Argument: {arg}").into()),
        }
    }
    if healthcheck {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse()?,
            std::time::Duration::from_secs(2),
        )?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        stream.write_all(
            format!("GET /healthz HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )?;
        let mut response = String::new();
        stream.take(4096).read_to_string(&mut response)?;
        if !response.starts_with("HTTP/1.1 200") {
            return Err("Healthcheck fehlgeschlagen".into());
        }
        return Ok(());
    }
    let oidc = if let Some(file) = oidc_file {
        Some(crate::oidc::Oidc::new(serde_json::from_str(
            &std::fs::read_to_string(file)?,
        )?)?)
    } else {
        None
    };
    if demo && oidc.is_some() {
        return Err("OIDC gehört in eine geschützte Arbeitsinstanz.".into());
    }
    let connectors = if let Some(file) = connectors_file {
        let config: crate::connectors::Configuration =
            serde_json::from_str(&std::fs::read_to_string(file)?)?;
        crate::connectors::Hub::new(config, false)?
    } else {
        crate::connectors::Hub::empty()
    };
    let database = database.unwrap_or_else(|| profile.database_path(demo));
    if let Some(parent) = database.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    if backup_path.is_some() && restore_path.is_some() {
        return Err("Backup und Restore getrennt ausführen.".into());
    }
    if let Some(path) = backup_path {
        crate::backup::snapshot(&database, &path).await?;
        println!("Konsistente Sicherung erstellt und geprüft.");
        return Ok(());
    }
    if let Some(path) = restore_path {
        crate::backup::restore(&path, &database, &profile, demo).await?;
        println!("Wiederherstellung geprüft. Sitzungen und API-Zugänge widerrufen; offene Integrationsaufträge erfordern Prüfung.");
        return Ok(());
    }
    if let Some(address) = admin {
        if demo {
            return Err("Benutzerkonten gehören in eine Arbeitsinstanz.".into());
        }
        let password_path = password_file.ok_or("--init-admin benötigt --password-file")?;
        let password = std::fs::read_to_string(password_path)?
            .trim_end_matches(['\r', '\n'])
            .to_string();
        let pool = connect(&database.to_string_lossy()).await?;
        initialize_workspace(&pool, false, &profile).await?;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&pool)
            .await?;
        if init_if_empty && count > 0 {
            println!("Arbeitsinstanz ist bereits eingerichtet.");
            return Ok(());
        }
        crate::auth::bootstrap(&pool, &address, password).await?;
        println!("Administrator eingerichtet. Zugangsdaten wurden nicht ausgegeben.");
        return Ok(());
    }
    if demo && !bind.is_loopback() {
        return Err("Demo darf nur an Loopback binden.".into());
    }
    if !bind.is_loopback() && public_url.is_none() {
        return Err("Netzwerkbetrieb benötigt --public-url https://…".into());
    }
    // Bind before any data changes: an occupied port cannot seed a database.
    let listener = tokio::net::TcpListener::bind((bind, port)).await?;
    let port = listener.local_addr()?.port();
    let security = crate::auth::Security::new(
        &public_url.unwrap_or_else(|| format!("http://127.0.0.1:{port}")),
        demo,
    )?;
    if !bind.is_loopback() && !security.secure {
        return Err("Netzwerkbetrieb benötigt einen HTTPS-Origin.".into());
    }
    let pool = connect(&database.to_string_lossy()).await?;
    initialize_workspace(&pool, demo, &profile).await?;
    if let Some(oidc) = &oidc {
        oidc.bootstrap_identity(&pool).await?;
    }
    let requested = if demo { "demo" } else { "workspace" };
    if demo {
        seed_demo(&pool, &profile).await?;
        crate::demo_business::seed(&pool, &profile).await?;
    }
    println!(
        "Brilliantis · {} · {requested} · {}",
        security.origin,
        profile.id()
    );
    let worker_pool = pool.clone();
    let worker_hub = connectors.clone();
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(async move {
        loop {
            tokio::select! { _=stop_rx.changed()=>break, _=tokio::time::sleep(std::time::Duration::from_secs(1))=>{} }
            if let Err(error) = crate::jobs::run_one(&worker_pool, &worker_hub).await {
                tracing::error!(code = error.1, "Integration worker failed");
            }
        }
    });
    axum::serve(
        listener,
        router(AppState {
            pool,
            demo,
            port,
            profile,
            security,
            connectors,
            oidc,
        }),
    )
    .with_graceful_shutdown(async {
        #[cfg(unix)]
        {
            if let Ok(mut terminate) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
            } else {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
    })
    .await?;
    let _ = stop_tx.send(true);
    let _ = worker.await;
    Ok(())
}
