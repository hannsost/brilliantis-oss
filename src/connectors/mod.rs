use crate::{invalid, Error, Result};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::process::Command;

pub mod plans;

#[derive(Clone, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case", deny_unknown_fields)]
pub enum SecretReference {
    File {
        path: PathBuf,
    },
    Bitwarden {
        secret_id: String,
        project_id: String,
        machine_token_file: PathBuf,
        server_url: String,
    },
}
impl SecretReference {
    pub(crate) async fn resolve(&self) -> Result<String> {
        match self {
            Self::File { path } => read_secret(path).await,
            Self::Bitwarden {
                secret_id,
                project_id,
                machine_token_file,
                server_url,
            } => {
                uuid::Uuid::parse_str(secret_id)
                    .map_err(|_| invalid("Bitwarden Secret-ID ungültig."))?;
                uuid::Uuid::parse_str(project_id)
                    .map_err(|_| invalid("Bitwarden Projekt-ID ungültig."))?;
                let server =
                    url::Url::parse(server_url).map_err(|_| invalid("Bitwarden-URL ungültig."))?;
                if server.scheme() != "https" {
                    return Err(invalid("Bitwarden benötigt HTTPS."));
                }
                let token = read_secret(machine_token_file).await?;
                let mut command = Command::new("bws");
                command
                    .args([
                        "secret", "get", secret_id, "--output", "json", "--color", "no",
                    ])
                    .env_clear()
                    .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                    .env("BWS_ACCESS_TOKEN", token)
                    .env("BWS_SERVER_URL", server_url)
                    .stdin(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .kill_on_drop(true);
                let output = tokio::time::timeout(Duration::from_secs(30), command.output())
                    .await
                    .map_err(|_| connector_error("secret_timeout"))?
                    .map_err(|_| connector_error("secret_unavailable"))?;
                if !output.status.success() || output.stdout.len() > 65536 {
                    return Err(connector_error("secret_unavailable"));
                }
                let data: Value = serde_json::from_slice(&output.stdout)
                    .map_err(|_| connector_error("secret_invalid"))?;
                if data
                    .get("projectId")
                    .or_else(|| data.get("project_id"))
                    .and_then(Value::as_str)
                    != Some(project_id.as_str())
                {
                    return Err(connector_error("secret_project_mismatch"));
                }
                data["value"]
                    .as_str()
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| connector_error("secret_invalid"))
            }
        }
    }
}
async fn read_secret(path: &std::path::Path) -> Result<String> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|_| connector_error("secret_unavailable"))?;
    if !metadata.is_file() || metadata.len() > 65536 {
        return Err(connector_error("secret_invalid"));
    }
    let value = tokio::fs::read_to_string(path)
        .await
        .map_err(|_| connector_error("secret_unavailable"))?;
    if value.trim().is_empty() {
        return Err(connector_error("secret_invalid"));
    }
    Ok(value.trim_end_matches(['\r', '\n']).to_owned())
}
pub fn connector_error(code: &'static str) -> Error {
    Error(
        StatusCode::BAD_GATEWAY,
        code,
        "Verbindung fehlgeschlagen. Anschlussstatus und Anbieterzugang prüfen.".into(),
    )
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub base_url: String,
    pub secret: SecretReference,
    #[serde(default)]
    pub settings: Value,
    pub operations: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub connections: Vec<Connection>,
}
#[derive(Clone)]
pub struct Hub {
    client: reqwest::Client,
    connections: Arc<BTreeMap<String, Connection>>,
}
impl Hub {
    pub fn empty() -> Self {
        Self::new(
            Configuration {
                connections: Vec::new(),
            },
            false,
        )
        .expect("valid empty connector hub")
    }
    pub fn new(config: Configuration, allow_local_http: bool) -> Result<Self> {
        let mut connections = BTreeMap::new();
        for connection in config.connections {
            if connection.id.is_empty()
                || connection.id.len() > 80
                || !connection
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                || connections.contains_key(&connection.id)
            {
                return Err(invalid("Verbindungs-ID ungültig oder doppelt."));
            }
            let url = url::Url::parse(&connection.base_url)
                .map_err(|_| invalid("Anschluss-URL ungültig."))?;
            let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
            if (url.scheme() != "https" && !(allow_local_http && local && url.scheme() == "http"))
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(invalid(
                    "Anschluss benötigt eine HTTPS-Basis-URL ohne Zugangsdaten oder Query.",
                ));
            }
            let available = plans::operations(&connection.provider)?;
            if connection.operations.is_empty()
                || connection
                    .operations
                    .iter()
                    .any(|op| !available.contains(&op.as_str()))
            {
                return Err(invalid("Operation wird vom Adapter nicht unterstützt."));
            }
            connections.insert(connection.id.clone(), connection);
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(5))
            .user_agent("Brilliantis/0.2")
            .build()
            .map_err(|_| connector_error("client_configuration"))?;
        Ok(Self {
            client,
            connections: Arc::new(connections),
        })
    }
    pub fn catalog(&self) -> Value {
        json!(self.connections.values().map(|c|json!({"id":c.id,"name":c.name,"provider":c.provider,"operations":c.operations,"status":"configured_unverified"})).collect::<Vec<_>>())
    }
    pub fn plan(&self, id: &str, operation: &str, input: &Value) -> Result<plans::Plan> {
        let connection = self
            .connections
            .get(id)
            .ok_or_else(|| invalid("Verbindung ist nicht eingerichtet."))?;
        if !connection.operations.iter().any(|v| v == operation) {
            return Err(invalid(
                "Operation ist für diese Verbindung nicht freigeschaltet.",
            ));
        }
        if contains_secret(input) {
            return Err(invalid(
                "Zugangsdaten gehören in den Secret-Provider, nicht in einen Integrationsauftrag.",
            ));
        }
        plans::build(connection, operation, input)
    }
    pub async fn execute(
        &self,
        id: &str,
        operation: &str,
        input: &Value,
        key: &str,
    ) -> Result<Value> {
        let connection = self
            .connections
            .get(id)
            .ok_or_else(|| invalid("Verbindung ist nicht eingerichtet."))?;
        let plan = self.plan(id, operation, input)?;
        let raw = connection.secret.resolve().await?;
        let credentials: Value =
            serde_json::from_str(&raw).unwrap_or_else(|_| json!({"access_token":raw}));
        let mut request = self.client.request(plan.method.clone(), plan.url.clone());
        if ["gmail", "search_console"].contains(&connection.provider.as_str())
            && credentials.get("refresh_token").is_some()
        {
            let endpoint = connection.settings["token_endpoint"]
                .as_str()
                .unwrap_or("https://oauth2.googleapis.com/token");
            let endpoint =
                url::Url::parse(endpoint).map_err(|_| invalid("OAuth-Token-Endpunkt ungültig."))?;
            if endpoint.scheme() != "https"
                || !endpoint.username().is_empty()
                || endpoint.password().is_some()
            {
                return Err(invalid("OAuth-Token-Endpunkt benötigt HTTPS."));
            }
            let response = self
                .client
                .post(endpoint)
                .form(&[
                    ("grant_type", "refresh_token"),
                    ("client_id", credential(&credentials, "client_id")?),
                    ("client_secret", credential(&credentials, "client_secret")?),
                    ("refresh_token", credential(&credentials, "refresh_token")?),
                ])
                .send()
                .await
                .map_err(|_| connector_error("authentication_network"))?;
            let response = self.read_response(response).await?;
            request = request.bearer_auth(credential(&response, "access_token")?);
        } else if connection.provider == "paypal" || connection.provider == "shopware" {
            let mut url = url::Url::parse(&connection.base_url).unwrap();
            {
                let mut segments = url
                    .path_segments_mut()
                    .map_err(|_| invalid("URL ungültig."))?;
                segments.pop_if_empty();
                for part in if connection.provider == "paypal" {
                    vec!["v1", "oauth2", "token"]
                } else {
                    vec!["api", "oauth", "token"]
                } {
                    segments.push(part);
                }
            }
            let client_id = credential(&credentials, "client_id")?;
            let client_secret = credential(&credentials, "client_secret")?;
            let token_request = if connection.provider == "paypal" {
                self.client
                    .post(url)
                    .basic_auth(client_id, Some(client_secret))
                    .form(&[("grant_type", "client_credentials")])
            } else {
                self.client.post(url).json(&json!({"grant_type":"client_credentials","client_id":client_id,"client_secret":client_secret}))
            };
            let token_response = self
                .read_response(
                    token_request
                        .send()
                        .await
                        .map_err(|_| connector_error("authentication_network"))?,
                )
                .await?;
            request = request.bearer_auth(credential(&token_response, "access_token")?);
        } else if connection.provider == "sap_business_one" {
            let mut url = url::Url::parse(&connection.base_url).unwrap();
            url.path_segments_mut()
                .unwrap()
                .pop_if_empty()
                .push("Login");
            let response=self.client.post(url).json(&json!({"CompanyDB":credential(&credentials,"company_db")?,"UserName":credential(&credentials,"username")?,"Password":credential(&credentials,"password")?})).send().await.map_err(|_|connector_error("authentication_network"))?;
            let route = response
                .headers()
                .get_all(reqwest::header::SET_COOKIE)
                .iter()
                .filter_map(|h| h.to_str().ok())
                .filter_map(|h| h.split(';').next())
                .find(|h| h.starts_with("ROUTEID="))
                .map(str::to_owned);
            let data = self.read_response(response).await?;
            let session = credential(&data, "SessionId")?;
            if !session
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            {
                return Err(connector_error("authentication_response"));
            }
            let route = route
                .filter(|v| {
                    v.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"=-_.".contains(&b))
                })
                .map(|v| format!("; {v}"))
                .unwrap_or_default();
            request = request.header("Cookie", format!("B1SESSION={session}{route}"));
        } else if connection.provider == "enable_banking" {
            let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
            header.kid = Some(credential(&credentials, "application_id")?.to_owned());
            let time = crate::auth::now();
            let claims = json!({"iss":"enablebanking.com","aud":"api.enablebanking.com","iat":time,"exp":time+300});
            let key = jsonwebtoken::EncodingKey::from_rsa_pem(
                credential(&credentials, "private_key_pem")?.as_bytes(),
            )
            .map_err(|_| connector_error("authentication_key"))?;
            let jwt = jsonwebtoken::encode(&header, &claims, &key)
                .map_err(|_| connector_error("authentication_key"))?;
            request = request.bearer_auth(jwt);
        } else if connection.provider == "woocommerce" {
            request = request.basic_auth(
                credential(&credentials, "consumer_key")?,
                Some(credential(&credentials, "consumer_secret")?),
            );
        } else if connection.provider == "shopify" {
            request = request.header(
                "X-Shopify-Access-Token",
                credential(&credentials, "access_token")?,
            );
        } else if connection.provider == "discourse" {
            request = request
                .header("Api-Key", credential(&credentials, "api_key")?)
                .header("Api-Username", credential(&credentials, "username")?);
        } else if credentials.get("username").is_some() {
            request = request.basic_auth(
                credential(&credentials, "username")?,
                Some(credential(&credentials, "password")?),
            );
        } else {
            request = request.bearer_auth(credential(&credentials, "access_token")?);
        }
        if let Some(header) = plan.idempotency_header {
            request = request.header(header, key);
        }
        if connection.provider == "stripe" {
            if let Some(version) = connection.settings["api_version"].as_str() {
                request = request.header("Stripe-Version", version);
            }
        }
        if plan.form {
            request = request.form(
                plan.body
                    .as_object()
                    .ok_or_else(|| invalid("Formulardaten fehlen."))?,
            );
        } else if !plan.body.is_null() {
            request = request.json(&plan.body);
        }
        let response = request.send().await.map_err(|_| {
            connector_error(if plan.safe_retry {
                "provider_network"
            } else {
                "delivery_uncertain"
            })
        })?;
        let mut value = self.read_response(response).await?;
        if value.get("errors").is_some()
            || value["error"]["code"].as_str().is_some_and(|v| v != "ok")
        {
            return Err(connector_error("provider_business_error"));
        }
        redact(&mut value);
        Ok(json!({"connection_id":id,"operation":operation,"data":value}))
    }
    async fn read_response(&self, mut response: reqwest::Response) -> Result<Value> {
        let status = response.status();
        if !status.is_success() {
            return Err(connector_error(match status.as_u16() {
                401 | 403 => "provider_authorization",
                409 => "provider_conflict",
                429 => "provider_rate_limit",
                500..=599 => "provider_unavailable",
                _ => "provider_rejected",
            }));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| connector_error("provider_response"))?
        {
            if body.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err(connector_error("provider_response_limit"));
            }
            body.extend_from_slice(&chunk);
        }
        if body.is_empty() {
            return Ok(json!({}));
        }
        serde_json::from_slice(&body).map_err(|_| connector_error("provider_response_format"))
    }
}
fn credential<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| connector_error("secret_fields"))
}
fn secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "password",
        "access_token",
        "refresh_token",
        "client_secret",
        "private_key",
        "private_key_pem",
        "bws_access_token",
        "consumer_key",
        "authorization",
        "api_key",
        "consumer_secret",
        "sessionid",
    ]
    .iter()
    .any(|s| key.contains(s))
}
fn contains_secret(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(k, v)| secret_key(k) || contains_secret(v)),
        Value::Array(list) => list.iter().any(contains_secret),
        _ => false,
    }
}
fn redact(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|k, _| !secret_key(k));
            for item in map.values_mut() {
                redact(item);
            }
        }
        Value::Array(list) => {
            for item in list {
                redact(item);
            }
        }
        _ => {}
    }
}

pub fn reject_secrets(value: &Value) -> Result<()> {
    if contains_secret(value) {
        Err(invalid(
            "Zugangsdaten ausschließlich über Secret-Referenzen verwalten.",
        ))
    } else {
        Ok(())
    }
}
