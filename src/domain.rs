use crate::{conflict, invalid, new_id, profile::Profile, Error, Result};
use axum::http::StatusCode;
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};

pub fn field(value: &Value, name: &str, min: usize, max: usize) -> Result<String> {
    let s = value[name]
        .as_str()
        .ok_or_else(|| invalid(&format!("{name}: Text erwartet.")))?
        .trim();
    if s.chars().count() < min
        || s.chars().count() > max
        || s.chars().any(|c| c.is_control() && c != '\n')
    {
        return Err(invalid(&format!(
            "{name}: {min} bis {max} Zeichen erwartet."
        )));
    }
    Ok(s.to_string())
}
fn money(value: &Value, name: &str) -> Result<i64> {
    value[name]
        .as_i64()
        .filter(|n| (0..=100_000_000_000).contains(n))
        .ok_or_else(|| {
            invalid(&format!(
                "{name}: ganzzahlige Cent zwischen 0 und 100000000000 erwartet."
            ))
        })
}
fn product(value: &Value, profile: &Profile) -> Result<String> {
    let id = field(value, "product_id", 1, 60)?;
    let _ = profile;
    Ok(id)
}
pub fn email(value: &Value) -> Result<String> {
    let mail = field(value, "email", 0, 254)?;
    if !mail.is_empty()
        && (mail.matches('@').count() != 1
            || mail.chars().any(char::is_whitespace)
            || !mail.split('@').nth(1).unwrap_or("").contains('.')
            || mail.starts_with('@')
            || mail.ends_with('.'))
    {
        return Err(invalid("Ungültige E-Mail-Adresse."));
    }
    Ok(mail)
}
fn validate_lead(value: &Value, profile: &Profile) -> Result<Value> {
    let source = field(value, "source", 1, 60)?;
    if !["manual", "website", "referral", "campaign", "partner"].contains(&source.as_str()) {
        return Err(invalid("Unbekannte Lead-Quelle."));
    }
    Ok(
        json!({"company":field(value,"company",1,160)?,"contact":field(value,"contact",0,120)?,"email":email(value)?,"title":field(value,"title",1,200)?,"product_id":product(value, profile)?,"source":source,"amount_cents":money(value,"amount_cents")?,"next_action":field(value,"next_action",1,500)?,"status":"new","currency":"EUR"}),
    )
}

/// A successful command, audit event and replay record commit together.
pub async fn execute(
    pool: &SqlitePool,
    profile: &Profile,
    key: &str,
    actor: &str,
    command: &str,
    id: Option<&str>,
    input: Value,
) -> Result<Value> {
    let mut tx = pool.begin().await?;
    // Portal offers must create their order and contract in the same transaction.
    // The commerce service calls execute_in directly after checking its own version.
    if command == "quote.accept" {
        let attached: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM commerce_records WHERE kind='offer' AND json_extract(body,'$.quote.id')=?")
            .bind(id).fetch_one(&mut *tx).await?;
        if attached > 0 {
            return Err(conflict("Portal-Angebot über /api/v1/commerce/offers/{id}/accept annehmen, damit Auftrag und Vertrag gemeinsam entstehen."));
        }
    }
    let response = execute_in(&mut tx, profile, key, actor, command, id, input).await?;
    tx.commit().await?;
    Ok(response)
}

/// Compose business commands atomically with the enclosing commerce transaction.
pub(crate) async fn execute_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    profile: &Profile,
    key: &str,
    actor: &str,
    command: &str,
    id: Option<&str>,
    input: Value,
) -> Result<Value> {
    crate::connectors::reject_secrets(&input)?;
    if command.ends_with(".create")
        && ["lead", "campaign", "creator"].contains(&command.split('.').next().unwrap_or(""))
    {
        validate_edit(command.split('.').next().unwrap(), &input)?;
    }
    if command.ends_with(".edit")
        && input.as_object().is_none_or(|m| {
            m.keys()
                .any(|k| !["version", "fields"].contains(&k.as_str()))
        })
    {
        return Err(invalid(
            "Bearbeitung benötigt ausschließlich version und fields.",
        ));
    }
    let request = json!({"command":command,"id":id,"actor":actor,"input":input}).to_string();
    let stored: Option<String> =
        sqlx::query_scalar("SELECT value FROM metadata WHERE key='profile'")
            .fetch_optional(&mut **tx)
            .await?;
    if stored.as_deref() != Some(profile.id()) {
        return Err(conflict(
            "Befehl und Datenbank gehören nicht zum selben initialisierten Profil.",
        ));
    }
    if let Some(row) = sqlx::query("SELECT request,response FROM commands WHERE idempotency_key=?")
        .bind(key)
        .fetch_optional(&mut **tx)
        .await?
    {
        if row.get::<String, _>("request") != request {
            return Err(conflict(
                "Idempotency-Key wurde bereits für einen anderen Befehl verwendet.",
            ));
        }
        return serde_json::from_str(row.get("response"))
            .map_err(|_| invalid("Gespeicherte Antwort ungültig."));
    }
    let kind = if command == "website.import" {
        "lead"
    } else {
        command.split('.').next().unwrap_or("")
    };
    if ![
        "lead",
        "campaign",
        "creator",
        "product",
        "customer",
        "quote",
        "contract",
        "invoice",
        "credit_note",
        "bank_account",
        "bank_transaction",
        "channel_account",
        "content",
        "collaboration",
        "seo_keyword",
        "order",
    ]
    .contains(&kind)
    {
        return Err(invalid("Unbekannter Objekttyp."));
    }
    if input.get("product_id").is_some() {
        let product_id = field(&input, "product_id", 1, 100)?;
        let exists: Option<i64> =
            sqlx::query_scalar("SELECT archived FROM records WHERE kind='product' AND id=?")
                .bind(&product_id)
                .fetch_optional(&mut **tx)
                .await?;
        if exists != Some(0) && !(exists.is_none() && profile.accepts_product(&product_id)) {
            return Err(invalid("Produkt ist unbekannt oder archiviert."));
        }
    }
    let entity_id = id.map(str::to_owned).unwrap_or_else(new_id);
    let mut external_ref = None;
    let (mut body, version, summary) = match command {
        "lead.create" => {
            let v = validate_lead(&input, profile)?;
            let summary = format!("Lead angelegt: {}", v["company"].as_str().unwrap());
            (v, 1, summary)
        }
        "campaign.create" => {
            let channel = field(&input, "channel", 1, 40)?;
            if ![
                "email",
                "content",
                "partner",
                "event",
                "seo",
                "social",
                "forum",
                "influencer",
            ]
            .contains(&channel.as_str())
            {
                return Err(invalid("Unbekannter Kampagnenkanal."));
            }
            let name = field(&input, "name", 1, 160)?;
            let v = json!({"name":name,"product_id":product(&input, profile)?,"audience":field(&input,"audience",1,500)?,"objective":field(&input,"objective",1,1000)?,"channel":channel,"budget_cents":money(&input,"budget_cents")?,"status":"draft","currency":"EUR"});
            (v, 1, format!("Kampagnenentwurf angelegt: {name}"))
        }
        "creator.create" => {
            let name = field(&input, "name", 1, 160)?;
            let category = field(&input, "kind", 1, 30)?;
            if !["brand", "virtual", "external"].contains(&category.as_str()) {
                return Err(invalid("Unbekannte Profilart."));
            }
            let channels = input["channels"]
                .as_array()
                .filter(|a| !a.is_empty() && a.len() <= 7)
                .ok_or_else(|| invalid("Mindestens einen Kanal angeben."))?;
            let allowed = [
                "instagram",
                "tiktok",
                "youtube",
                "linkedin",
                "forum",
                "blog",
                "other",
            ];
            for (i, channel) in channels.iter().enumerate() {
                if !allowed.contains(&channel.as_str().unwrap_or(""))
                    || channels[..i].contains(channel)
                {
                    return Err(invalid("Unbekannter oder doppelter Kanal."));
                }
            }
            (
                json!({"name":name,"kind":category,"niche":field(&input,"niche",1,500)?,"channels":channels,"handles":field(&input,"handles",0,1000)?,"email":email(&input)?,"budget_cents":money(&input,"budget_cents")?,"notes":field(&input,"notes",0,2000)?,"status":"planned","currency":"EUR"}),
                1,
                format!("Influencer-Profil angelegt: {name}"),
            )
        }
        "website.import" => {
            let ext = field(&input, "id", 1, 120)?;
            if input["consent"] != true {
                return Err(invalid(
                    "Der Import erwartet die Verarbeitungszustimmung zur Anfrage.",
                ));
            }
            let exists: Option<String> = sqlx::query_scalar(
                "SELECT entity_id FROM external_refs WHERE system='website' AND external_id=?",
            )
            .bind(&ext)
            .fetch_optional(&mut **tx)
            .await?;
            if exists.is_some() {
                return Err(conflict("Diese Website-Anfrage wurde bereits importiert."));
            }
            let company = field(&input, "company", 0, 160)?;
            let name = field(&input, "name", 1, 120)?;
            let topic = field(&input, "topic", 1, 100)?;
            let created = field(&input, "created_at", 1, 60)?;
            let hosting = input.get("hosting").filter(|v| !v.is_null());
            if hosting.is_some_and(|v| !v.is_object()) {
                return Err(invalid("hosting muss ein Objekt sein."));
            }
            let source = json!({"company":if company.is_empty(){name.clone()}else{company},"contact":name,"email":email(&input)?,"title":format!("Website-Anfrage: {topic}"),"product_id":profile.website_product(hosting.is_some()),"source":"website","amount_cents":0,"next_action":"Bedarf prüfen und verbindliches Angebot kalkulieren"});
            let mut v = validate_lead(&source, profile)?;
            v["inquiry"] = json!({"external_id":ext,"created_at":created,"message":field(&input,"message",1,5000)?,"hosting_snapshot":hosting,"pricing_status":"unverified_example","marketing_consent":false});
            external_ref = Some(ext);
            (v, 1, "Website-Anfrage lokal importiert".to_string())
        }
        "lead.update" | "campaign.update" | "creator.update" => {
            let row = sqlx::query(
                "SELECT body,version FROM records WHERE kind=? AND id=? AND archived=0",
            )
            .bind(kind)
            .bind(&entity_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| {
                Error(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    "Eintrag nicht gefunden.".into(),
                )
            })?;
            let version = row.get::<i64, _>("version");
            if input["version"].as_i64() != Some(version) {
                return Err(conflict("Der Eintrag wurde geändert. Aktuelle Version laden und Änderung erneut prüfen."));
            }
            let mut body: Value = serde_json::from_str(row.get("body"))
                .map_err(|_| invalid("Datenbankeintrag ungültig."))?;
            let old = body["status"].as_str().unwrap_or("").to_string();
            let status = field(&input, "status", 1, 30)?;
            let allowed: &[&str] = match (kind, old.as_str()) {
                ("lead", "new") => &["new", "qualified", "lost"],
                ("lead", "qualified") => &["qualified", "new", "proposal", "lost"],
                ("lead", "proposal") => &["proposal", "qualified", "won", "lost"],
                ("lead", "won") => &["won"],
                ("lead", "lost") => &["lost"],
                ("campaign", "draft") => &["draft", "planned"],
                ("campaign", "planned") => &["planned", "draft", "paused"],
                ("campaign", "paused") => &["paused", "planned", "draft"],
                ("creator", "planned") => &["planned", "briefing", "paused"],
                ("creator", "briefing") => &["briefing", "collaborating", "paused"],
                ("creator", "collaborating") => &["collaborating", "paused"],
                ("creator", "paused") => &["paused", "planned", "briefing"],
                _ => &[],
            };
            if !allowed.contains(&status.as_str()) {
                return Err(invalid("Dieser Statuswechsel ist nicht möglich."));
            }
            body["status"] = json!(status);
            if !input["next_action"].is_null() {
                if kind != "lead" {
                    return Err(invalid("next_action gehört zu einem Lead."));
                }
                body["next_action"] = json!(field(&input, "next_action", 1, 500)?);
            }
            (
                body,
                version + 1,
                format!(
                    "{}: {old} → {status}",
                    match kind {
                        "lead" => "Pipeline",
                        "creator" => "Influencer",
                        _ => "Kampagnenplanung",
                    }
                ),
            )
        }
        _ if ["invoice", "credit_note", "bank_account", "bank_transaction"].contains(&kind) => {
            crate::finance::prepare(tx, command, &entity_id, &input).await?
        }
        _ if [
            "channel_account",
            "content",
            "collaboration",
            "seo_keyword",
            "order",
        ]
        .contains(&kind) =>
        {
            crate::marketing::prepare(tx, command, &entity_id, &input).await?
        }
        _ => crate::business::prepare(tx, command, &entity_id, &input).await?,
    };
    if (command.ends_with(".create") || command.ends_with(".edit") || command == "website.import")
        && body["product_id"].is_string()
    {
        let product_id = body["product_id"].as_str().unwrap();
        let archived: Option<i64> =
            sqlx::query_scalar("SELECT archived FROM records WHERE kind='product' AND id=?")
                .bind(product_id)
                .fetch_optional(&mut **tx)
                .await?;
        if archived != Some(0)
            && !(archived.is_none()
                && profile
                    .legacy_product_ids()
                    .iter()
                    .any(|id| id == product_id))
        {
            return Err(invalid("Produkt ist unbekannt oder archiviert."));
        }
    }
    if version == 1 {
        sqlx::query("INSERT INTO records(kind,id,version,body) VALUES(?,?,?,?)")
            .bind(kind)
            .bind(&entity_id)
            .bind(version)
            .bind(body.to_string())
            .execute(&mut **tx)
            .await?;
    } else {
        sqlx::query("UPDATE records SET version=?,body=?,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE kind=? AND id=?").bind(version).bind(body.to_string()).bind(kind).bind(&entity_id).execute(&mut **tx).await?;
    }
    sqlx::query("UPDATE records SET archived=? WHERE kind=? AND id=?")
        .bind(body["archived"] == true)
        .bind(kind)
        .bind(&entity_id)
        .execute(&mut **tx)
        .await?;
    crate::business::persist_lines(tx, kind, &entity_id, &body).await?;
    if let Some(ext) = external_ref {
        sqlx::query(
            "INSERT INTO external_refs(system,external_id,entity_id) VALUES('website',?,?)",
        )
        .bind(ext)
        .bind(&entity_id)
        .execute(&mut **tx)
        .await?;
    }
    let event_id = new_id();
    sqlx::query("INSERT INTO events(id,event_type,entity_id,entity_version,actor_label,summary) VALUES(?,?,?,?,?,?)").bind(&event_id).bind(command).bind(&entity_id).bind(version).bind(actor).bind(summary).execute(&mut **tx).await?;
    let timestamps = sqlx::query("SELECT created_at,updated_at FROM records WHERE kind=? AND id=?")
        .bind(kind)
        .bind(&entity_id)
        .fetch_one(&mut **tx)
        .await?;
    body["id"] = json!(entity_id);
    body["version"] = json!(version);
    body["created_at"] = json!(timestamps.get::<String, _>("created_at"));
    body["updated_at"] = json!(timestamps.get::<String, _>("updated_at"));
    let response = json!({"record":body,"event_id":event_id});
    sqlx::query("INSERT INTO commands(idempotency_key,request,response) VALUES(?,?,?)")
        .bind(key)
        .bind(request)
        .bind(response.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(response)
}

/// Stable fixture keys preserve existing demo records and all user edits.
pub async fn seed_demo(pool: &SqlitePool, profile: &Profile) -> Result<()> {
    for fixture in profile.demo() {
        let existing: Option<String> =
            sqlx::query_scalar("SELECT response FROM commands WHERE idempotency_key=?")
                .bind(&fixture.key)
                .fetch_optional(pool)
                .await?;
        let response = if let Some(stored) = existing {
            serde_json::from_str(&stored).map_err(|_| invalid("Demo-Beleg ist ungültig."))?
        } else {
            execute(
                pool,
                profile,
                &fixture.key,
                "demo.seed",
                &fixture.command,
                None,
                fixture.input.clone(),
            )
            .await?
        };
        let id = response["record"]["id"]
            .as_str()
            .ok_or_else(|| invalid("Demo-ID fehlt."))?;
        for (index, step) in fixture.steps.iter().enumerate() {
            execute(
                pool,
                profile,
                &format!("{}-step-{index}", fixture.key),
                "demo.seed",
                "lead.update",
                Some(id),
                json!({"version":index+1,"status":step,"next_action":null}),
            )
            .await?;
        }
    }
    Ok(())
}

pub fn validate_edit(kind: &str, value: &Value) -> Result<Value> {
    match kind {
        "lead" => {
            serde_json::from_value::<crate::LeadInput>(value.clone())
                .map_err(|_| invalid("Ungültige Lead-Felder."))?;
            validate_lead(value, &Profile::standalone())
        }
        "campaign" => {
            serde_json::from_value::<crate::CampaignInput>(value.clone())
                .map_err(|_| invalid("Ungültige Kampagnenfelder."))?;
            for (key, min, max) in [
                ("name", 1, 160),
                ("product_id", 1, 100),
                ("audience", 1, 500),
                ("objective", 1, 1000),
            ] {
                field(value, key, min, max)?;
            }
            money(value, "budget_cents")?;
            if ![
                "email",
                "content",
                "partner",
                "event",
                "seo",
                "social",
                "forum",
                "influencer",
            ]
            .contains(&value["channel"].as_str().unwrap_or(""))
            {
                return Err(invalid("Ungültiger Kanal."));
            }
            let mut body = value.clone();
            body["currency"] = json!("EUR");
            Ok(body)
        }
        "creator" => {
            serde_json::from_value::<crate::CreatorInput>(value.clone())
                .map_err(|_| invalid("Ungültige Influencer-Felder."))?;
            for (key, min, max) in [
                ("name", 1, 160),
                ("niche", 1, 500),
                ("handles", 0, 1000),
                ("notes", 0, 2000),
            ] {
                field(value, key, min, max)?;
            }
            email(value)?;
            money(value, "budget_cents")?;
            if !["brand", "virtual", "external"].contains(&value["kind"].as_str().unwrap_or("")) {
                return Err(invalid("Ungültige Profilart."));
            }
            let channels = value["channels"]
                .as_array()
                .ok_or_else(|| invalid("Kanäle fehlen."))?;
            if channels.is_empty()
                || channels.len() > 7
                || channels.iter().enumerate().any(|(i, c)| {
                    channels[..i].contains(c)
                        || ![
                            "instagram",
                            "tiktok",
                            "youtube",
                            "linkedin",
                            "forum",
                            "blog",
                            "other",
                        ]
                        .contains(&c.as_str().unwrap_or(""))
                })
            {
                return Err(invalid("Ungültige Kanäle."));
            }
            let mut body = value.clone();
            body["currency"] = json!("EUR");
            Ok(body)
        }
        _ => Err(invalid("Bearbeitung nicht unterstützt.")),
    }
}
