use crate::{conflict, invalid, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use std::{path::PathBuf, sync::Arc};

/// Version 1 is the public composition contract. Provider implementations can
/// grow behind this boundary without the core depending on a distribution.
#[derive(Clone, Debug)]
pub struct Profile(Arc<Descriptor>, Arc<Vec<DemoCommand>>);

#[derive(Debug, Deserialize, Serialize)]
struct Descriptor {
    schema_version: u32,
    id: String,
    label: String,
    workspace: String,
    initials: String,
    tagline: String,
    description: String,
    orchestrator: Option<String>,
    agent_note: String,
    catalog: Vec<Value>,
    integrations: Vec<Value>,
    showcase: Vec<String>,
    website_products: WebsiteProducts,
    legacy_product_ids: Vec<String>,
    data_paths: DataPaths,
}
#[derive(Debug, Deserialize, Serialize)]
struct WebsiteProducts {
    default: String,
    hosting: String,
}
#[derive(Debug, Deserialize, Serialize)]
struct DataPaths {
    demo: String,
    workspace: String,
    legacy_workspace: Option<String>,
}
#[derive(Debug, Deserialize)]
pub struct DemoCommand {
    pub key: String,
    pub command: String,
    pub input: Value,
    #[serde(default)]
    pub steps: Vec<String>,
}

impl Profile {
    pub fn standalone() -> Self {
        Self::load(
            include_str!("../distributions/standalone/profile.json"),
            include_str!("../distributions/standalone/demo.json"),
        )
        .expect("valid bundled standalone profile")
    }

    pub fn load(source: &str, demo_source: &str) -> Result<Self> {
        let descriptor: Descriptor = serde_json::from_str(source)
            .map_err(|_| invalid("Ungültiger Distributionsvertrag."))?;
        if descriptor.schema_version != 1
            || descriptor.id.is_empty()
            || !descriptor
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'-')
        {
            return Err(invalid(
                "Distributionsvertrag oder Profil-ID wird nicht unterstützt.",
            ));
        }
        let demo: Vec<DemoCommand> =
            serde_json::from_str(demo_source).map_err(|_| invalid("Ungültige Demo-Vorlagen."))?;
        let profile = Self(Arc::new(descriptor), Arc::new(demo));
        let products: Vec<&str> = profile
            .0
            .catalog
            .iter()
            .filter_map(|p| p["id"].as_str())
            .collect();
        if products.len() != profile.0.catalog.len()
            || products.is_empty()
            || products
                .iter()
                .enumerate()
                .any(|(i, p)| products[..i].contains(p))
            || !products.contains(&profile.website_product(false))
            || !products.contains(&profile.website_product(true))
        {
            return Err(invalid(
                "Produktkatalog und Importzuordnung passen nicht zusammen.",
            ));
        }
        for (i, fixture) in profile.demo().iter().enumerate() {
            if fixture.key.is_empty()
                || profile.demo()[..i].iter().any(|f| f.key == fixture.key)
                || !["lead.create", "campaign.create", "creator.create"]
                    .contains(&fixture.command.as_str())
                || (!fixture.steps.is_empty() && fixture.command != "lead.create")
            {
                return Err(invalid("Ungültiger oder doppelter Demo-Befehl."));
            }
        }
        Ok(profile)
    }
    pub fn id(&self) -> &str {
        &self.0.id
    }
    pub fn descriptor(&self) -> Value {
        let mut value = serde_json::to_value(&*self.0).expect("serializable descriptor");
        // Local paths and seed templates are not needed by the GUI or agent context.
        value.as_object_mut().unwrap().remove("data_paths");
        value
    }
    pub fn catalog(&self) -> Value {
        json!(self.0.catalog)
    }
    pub fn legacy_product_ids(&self) -> &[String] {
        &self.0.legacy_product_ids
    }
    pub fn accepts_product(&self, id: &str) -> bool {
        self.0.catalog.iter().any(|p| p["id"] == id)
            || self.0.legacy_product_ids.iter().any(|value| value == id)
    }
    pub fn website_product(&self, hosting: bool) -> &str {
        if hosting {
            &self.0.website_products.hosting
        } else {
            &self.0.website_products.default
        }
    }
    pub fn database_path(&self, demo: bool) -> PathBuf {
        let paths = &self.0.data_paths;
        if demo {
            return PathBuf::from(&paths.demo);
        }
        if let Some(legacy) = &paths.legacy_workspace {
            if std::path::Path::new(legacy).exists() {
                return PathBuf::from(legacy);
            }
        }
        PathBuf::from(&paths.workspace)
    }
    pub fn demo(&self) -> &[DemoCommand] {
        &self.1
    }
    pub fn integrations(&self) -> Value {
        let mut items: Vec<Value> = serde_json::from_str(include_str!("connectors.json"))
            .expect("valid embedded connector catalog");
        for integration in &self.0.integrations {
            items.retain(|item| item["id"] != integration["id"]);
            items.push(integration.clone());
        }
        json!(items)
    }
}

/// Bind a database to one mode and distribution. A profile change is a migration,
/// never an implicit reinterpretation of products or stored command receipts.
pub async fn initialize_workspace(pool: &SqlitePool, demo: bool, profile: &Profile) -> Result<()> {
    let mut tx = pool.begin().await?;
    let mode: Option<String> = sqlx::query_scalar("SELECT value FROM metadata WHERE key='mode'")
        .fetch_optional(&mut *tx)
        .await?;
    let stored: Option<String> =
        sqlx::query_scalar("SELECT value FROM metadata WHERE key='profile'")
            .fetch_optional(&mut *tx)
            .await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM records")
        .fetch_one(&mut *tx)
        .await?;
    let legacy = stored.is_none() && (mode.is_some() || count > 0);
    let existing = stored
        .as_deref()
        .or(if legacy { Some("neurofunken") } else { None });
    if existing.is_some_and(|value| value != profile.id()) {
        return Err(conflict("Datenbank gehört zu einem anderen Profil. Frühere Orbit-Daten mit --profile neurofunken öffnen; für Standalone eine eigene Datenbank verwenden."));
    }
    let requested = if demo { "demo" } else { "workspace" };
    if mode.as_deref().is_some_and(|value| value != requested) {
        return Err(conflict(
            "Datenbankmodus passt nicht: Demo und Arbeitsdaten getrennt starten.",
        ));
    }
    for (key, value) in [("mode", requested), ("profile", profile.id())] {
        sqlx::query("INSERT OR IGNORE INTO metadata(key,value) VALUES(?,?)")
            .bind(key)
            .bind(value)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("INSERT OR IGNORE INTO metadata(key,value) VALUES('workspace_id',?)")
        .bind(crate::new_id())
        .execute(&mut *tx)
        .await?;
    for mut product in profile.catalog().as_array().unwrap().clone() {
        let id = product["id"].as_str().unwrap().to_owned();
        product.as_object_mut().unwrap().remove("id");
        product["price_cents"] = json!(0);
        product["tax_basis_points"] = json!(1900);
        product["unit"] = json!("Leistung");
        product["currency"] = json!("EUR");
        product["status"] = json!("active");
        sqlx::query("INSERT OR IGNORE INTO records(kind,id,version,body) VALUES('product',?,1,?)")
            .bind(id)
            .bind(product.to_string())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}
