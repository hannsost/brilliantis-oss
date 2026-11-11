//! Keycloak browser login and agent introspection. Explicit issuer/subject mappings own access.
pub mod browser;
use crate::{
    auth::{self, Principal},
    connectors::SecretReference,
    invalid, Result,
};
use serde::Deserialize;
use serde_json::Value;
use sqlx::{Row, SqlitePool};
use std::sync::Arc;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub issuer: String,
    pub audience: String,
    pub client_id: String,
    pub client_secret: SecretReference,
    #[serde(default)]
    pub browser_login: bool,
    #[serde(default = "login_label")]
    pub login_label: String,
    #[serde(default)]
    pub bootstrap_admin: Option<browser::BootstrapAdmin>,
    /// Optional private enterprise CA; TLS verification is always enabled.
    #[serde(default)]
    pub ca_certificate_file: Option<std::path::PathBuf>,
}
fn login_label() -> String {
    "Mit Single Sign-On anmelden".into()
}
#[derive(Clone)]
pub struct Oidc {
    config: Arc<Configuration>,
    client: reqwest::Client,
    gate: Arc<tokio::sync::Semaphore>,
}
impl Oidc {
    pub fn new(config: Configuration) -> Result<Self> {
        let url = url::Url::parse(&config.issuer).map_err(|_| invalid("OIDC-Issuer ungültig."))?;
        if url.scheme() != "https"
            || url.query().is_some()
            || url.fragment().is_some()
            || url.password().is_some()
            || !url.username().is_empty()
            || config.audience.is_empty()
            || config.client_id.is_empty()
            || config.issuer.ends_with('/')
            || config.issuer.len() > 2048
            || config.client_id.len() > 200
            || config.login_label.trim().is_empty()
            || config.login_label.len() > 120
        {
            return Err(invalid(
                "OIDC benötigt einen HTTPS-Issuer, Client und Audience.",
            ));
        }
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(10));
        if let Some(path) = &config.ca_certificate_file {
            let bytes =
                std::fs::read(path).map_err(|_| invalid("OIDC-CA konnte nicht gelesen werden."))?;
            let certificate = reqwest::Certificate::from_pem(&bytes)
                .map_err(|_| invalid("OIDC-CA ist ungültig."))?;
            builder = builder.add_root_certificate(certificate);
        }
        let client = builder
            .build()
            .map_err(|_| invalid("OIDC-Client konnte nicht eingerichtet werden."))?;
        Ok(Self {
            config: Arc::new(config),
            client,
            gate: Arc::new(tokio::sync::Semaphore::new(8)),
        })
    }
    pub fn issuer(&self) -> &str {
        &self.config.issuer
    }
    pub fn browser_enabled(&self) -> bool {
        self.config.browser_login
    }
    pub fn login_descriptor(&self) -> Value {
        serde_json::json!({"enabled":self.browser_enabled(),"label":self.config.login_label,"login_url":"/api/v1/auth/oidc/login"})
    }
    pub async fn identify(&self, pool: &SqlitePool, token: &str) -> Result<Principal> {
        if token.len() > 16000 || token.len() < 16 {
            return Err(auth::unauthorized());
        }
        let _permit = self
            .gate
            .acquire()
            .await
            .map_err(|_| auth::unauthorized())?;
        let secret = self.config.client_secret.resolve().await?;
        let response = self
            .client
            .post(format!(
                "{}/protocol/openid-connect/token/introspect",
                self.config.issuer.trim_end_matches('/')
            ))
            .basic_auth(&self.config.client_id, Some(secret))
            .form(&[("token", token), ("token_type_hint", "access_token")])
            .send()
            .await
            .map_err(|_| auth::unauthorized())?;
        if !response.status().is_success() || response.content_length().is_some_and(|n| n > 65536) {
            return Err(auth::unauthorized());
        }
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| auth::unauthorized())? {
            if bytes.len() + chunk.len() > 65536 {
                return Err(auth::unauthorized());
            }
            bytes.extend_from_slice(&chunk);
        }
        let claims: Value = serde_json::from_slice(&bytes).map_err(|_| auth::unauthorized())?;
        let subject = validate(&claims, &self.config.issuer, &self.config.audience)?;
        let row = sqlx::query("SELECT u.id,u.name,u.role FROM oidc_identities i JOIN users u ON u.id=i.user_id WHERE i.issuer=? AND i.subject=? AND u.disabled=0")
            .bind(&self.config.issuer)
            .bind(subject)
            .fetch_optional(pool)
            .await?
            .ok_or_else(auth::unauthorized)?;
        let id: String = row.get("id");
        Ok(Principal {
            actor: format!("user:{id}/oidc"),
            id,
            name: row.get("name"),
            role: row.get("role"),
            scopes: vec!["*".into()],
            session_hash: None,
            csrf: None,
        })
    }
}
pub fn validate<'a>(claims: &'a Value, issuer: &str, audience: &str) -> Result<&'a str> {
    let matches_aud = claims["aud"].as_str() == Some(audience)
        || claims["aud"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(audience)));
    if claims["active"] != true
        || claims["iss"].as_str() != Some(issuer)
        || !matches_aud
        || !claims["exp"].as_i64().is_some_and(|t| t > auth::now())
        || claims["nbf"].as_i64().is_some_and(|t| t > auth::now() + 30)
    {
        return Err(auth::unauthorized());
    }
    claims["sub"]
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= 200)
        .ok_or_else(auth::unauthorized)
}
