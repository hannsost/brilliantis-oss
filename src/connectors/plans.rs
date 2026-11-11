use super::Connection;
use crate::{business::date, invalid, Result};
use reqwest::Method;
use serde_json::{json, Value};

#[derive(Debug)]
pub struct Plan {
    pub method: Method,
    pub url: url::Url,
    pub body: Value,
    pub form: bool,
    pub safe_retry: bool,
    pub idempotency_header: Option<&'static str>,
    pub scope: &'static str,
}
pub fn operations(provider: &str) -> Result<&'static [&'static str]> {
    Ok(match provider {
        "mollie" => &[
            "payments.list",
            "payments.get",
            "payments.create",
            "payments.refund",
            "customers.list",
            "customers.create",
            "subscriptions.list",
        ],
        "stripe" => &[
            "payments.list",
            "payments.get",
            "payments.create",
            "payments.refund",
            "balance.list",
            "customers.list",
            "subscriptions.list",
        ],
        "paypal" => &[
            "transactions.list",
            "orders.create",
            "orders.get",
            "orders.capture",
            "payments.refund",
        ],
        "enable_banking" => &[
            "banks.list",
            "consent.start",
            "sessions.create",
            "sessions.get",
            "accounts.get",
            "balances.list",
            "transactions.list",
            "payments.create",
            "payments.get",
        ],
        "shopware" => &["orders.list", "products.list", "customers.list"],
        "woocommerce" => &[
            "orders.list",
            "orders.get",
            "products.list",
            "customers.list",
        ],
        "shopify" => &["orders.list", "products.list"],
        "gmail" => &[
            "messages.list",
            "messages.get",
            "drafts.create",
            "drafts.send",
        ],
        "search_console" => &["sites.list", "analytics.query"],
        "instagram" => &[
            "profile.get",
            "media.list",
            "media.create",
            "media.status",
            "media.publish",
        ],
        "tiktok" => &["creator.get", "video.draft", "video.status"],
        "discourse" => &["topics.list", "topic.get", "posts.create"],
        "sap_business_one" => &[
            "partners.list",
            "orders.list",
            "invoices.list",
            "orders.create",
        ],
        "sap_s4" => &["partners.list", "orders.list", "invoices.list"],
        "datev_accounting" => &[
            "clients.list",
            "fiscal_years.list",
            "accounts_receivable.list",
        ],
        "peppol_storecove" => &[
            "entities.get",
            "participants.discover",
            "documents.submit",
            "documents.status",
        ],
        "solveit" => &[
            "applications.list",
            "tickets.list",
            "tickets.get",
            "tickets.update",
            "tickets.message",
            "work.get",
        ],
        _ => return Err(invalid("Unbekannter Adapter.")),
    })
}
fn text<'a>(input: &'a Value, key: &str) -> Result<&'a str> {
    input[key]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 4000 && !s.chars().any(char::is_control))
        .ok_or_else(|| invalid(&format!("{key}: gültigen Text angeben.")))
}
fn id(input: &Value, key: &str) -> Result<String> {
    let value = text(input, key)?;
    if value.len() > 160
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
    {
        return Err(invalid("Ungültige externe ID."));
    }
    Ok(value.to_owned())
}
fn cents(input: &Value) -> Result<i64> {
    input["amount_cents"]
        .as_i64()
        .filter(|v| *v > 0 && *v <= 100_000_000_000)
        .ok_or_else(|| invalid("Betrag muss in positiven EUR-Cent angegeben werden."))
}
fn amount(input: &Value) -> Result<Value> {
    let c = cents(input)?;
    Ok(json!({"currency":"EUR","value":format!("{}.{:02}",c/100,c%100)}))
}
fn payload(input: &Value) -> Result<Value> {
    input
        .get("payload")
        .filter(|v| v.is_object())
        .cloned()
        .ok_or_else(|| invalid("Anbieterspezifisches payload-Objekt fehlt."))
}
fn required_version(connection: &Connection) -> Result<String> {
    let value = text(&connection.settings, "api_version")?;
    if !value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
    {
        return Err(invalid("API-Version ungültig."));
    }
    Ok(value.to_owned())
}
pub fn build(connection: &Connection, operation: &str, input: &Value) -> Result<Plan> {
    if !operations(&connection.provider)?.contains(&operation) {
        return Err(invalid("Unbekannte Adapteroperation."));
    }
    let mut url =
        url::Url::parse(&connection.base_url).map_err(|_| invalid("Basis-URL ungültig."))?;
    let mut path: Vec<String>;
    let mut method = Method::GET;
    let mut body = Value::Null;
    let mut query: Vec<(String, String)> = Vec::new();
    let mut form = false;
    let mut header = None;
    let mut retry = true;
    let mut scope = if operation.starts_with("invoices.")
        || operation.starts_with("billing_documents.")
        || [
            "mollie",
            "stripe",
            "paypal",
            "enable_banking",
            "datev_accounting",
            "peppol_storecove",
        ]
        .contains(&connection.provider.as_str())
    {
        "finance:read"
    } else {
        "crm:read"
    };
    macro_rules! route {($($part:expr),+)=>{{path=vec![$($part.to_string()),+];}};}
    macro_rules! write {
        ($value:expr) => {
            method = Method::POST;
            body = $value;
            scope = if scope.starts_with("finance") {
                "finance:write"
            } else {
                "crm:write"
            };
        };
    }
    match (connection.provider.as_str(), operation) {
        ("mollie", "payments.list") => {
            route!("v2", "payments");
            query.push(("limit".into(), "100".into()));
        }
        ("mollie", "payments.get") => route!("v2", "payments", id(input, "payment_id")?),
        ("mollie", "payments.create") => {
            route!("v2", "payments");
            write!(
                json!({"amount":amount(input)?,"description":text(input,"description")?,"redirectUrl":text(input,"redirect_url")?,"webhookUrl":text(input,"webhook_url")?,"metadata":{"brilliantis_reference":text(input,"reference")?}})
            );
            header = Some("Idempotency-Key");
            retry = true;
        }
        ("mollie", "payments.refund") => {
            route!("v2", "payments", id(input, "payment_id")?, "refunds");
            write!(json!({"amount":amount(input)?,"description":text(input,"description")?}));
            header = Some("Idempotency-Key");
            retry = true;
        }
        ("mollie", "customers.list") => route!("v2", "customers"),
        ("mollie", "customers.create") => {
            route!("v2", "customers");
            write!(json!({"name":text(input,"name")?,"email":text(input,"email")?}));
            header = Some("Idempotency-Key");
            retry = true;
        }
        ("mollie", "subscriptions.list") => route!(
            "v2",
            "customers",
            id(input, "customer_id")?,
            "subscriptions"
        ),
        ("stripe", "payments.list") => route!("v1", "payment_intents"),
        ("stripe", "payments.get") => route!("v1", "payment_intents", id(input, "payment_id")?),
        ("stripe", "payments.create") => {
            route!("v1", "payment_intents");
            write!(
                json!({"amount":cents(input)?.to_string(),"currency":"eur","description":text(input,"description")?,"metadata[brilliantis_reference]":text(input,"reference")?})
            );
            form = true;
            header = Some("Idempotency-Key");
            retry = true;
        }
        ("stripe", "payments.refund") => {
            route!("v1", "refunds");
            write!(
                json!({"payment_intent":id(input,"payment_id")?,"amount":cents(input)?.to_string()})
            );
            form = true;
            header = Some("Idempotency-Key");
            retry = true;
        }
        ("stripe", "balance.list") => route!("v1", "balance_transactions"),
        ("stripe", "customers.list") => route!("v1", "customers"),
        ("stripe", "subscriptions.list") => route!("v1", "subscriptions"),
        ("paypal", "transactions.list") => {
            route!("v1", "reporting", "transactions");
            query.extend([
                ("start_date".into(), text(input, "start_date")?.into()),
                ("end_date".into(), text(input, "end_date")?.into()),
                ("page_size".into(), "100".into()),
            ]);
        }
        ("paypal", "orders.create") => {
            route!("v2", "checkout", "orders");
            let a = amount(input)?;
            write!(
                json!({"intent":"CAPTURE","purchase_units":[{"reference_id":text(input,"reference")?,"description":text(input,"description")?,"amount":{"currency_code":"EUR","value":a["value"]}}]})
            );
            header = Some("PayPal-Request-Id");
            retry = true;
        }
        ("paypal", "orders.get") => route!("v2", "checkout", "orders", id(input, "order_id")?),
        ("paypal", "orders.capture") => {
            route!(
                "v2",
                "checkout",
                "orders",
                id(input, "order_id")?,
                "capture"
            );
            write!(json!({}));
            header = Some("PayPal-Request-Id");
            retry = true;
        }
        ("paypal", "payments.refund") => {
            route!(
                "v2",
                "payments",
                "captures",
                id(input, "capture_id")?,
                "refund"
            );
            let a = amount(input)?;
            write!(json!({"amount":{"currency_code":"EUR","value":a["value"]}}));
            header = Some("PayPal-Request-Id");
            retry = true;
        }
        ("enable_banking", "banks.list") => route!("aspsps"),
        ("enable_banking", "consent.start") => {
            route!("auth");
            write!(payload(input)?);
        }
        ("enable_banking", "sessions.create") => {
            route!("sessions");
            write!(json!({"code":text(input,"code")?}));
        }
        ("enable_banking", "sessions.get") => route!("sessions", id(input, "session_id")?),
        ("enable_banking", "accounts.get") => {
            route!("accounts", id(input, "account_id")?, "details")
        }
        ("enable_banking", "balances.list") => {
            route!("accounts", id(input, "account_id")?, "balances")
        }
        ("enable_banking", "transactions.list") => {
            route!("accounts", id(input, "account_id")?, "transactions")
        }
        ("enable_banking", "payments.create") => {
            route!("payments");
            write!(payload(input)?);
        }
        ("enable_banking", "payments.get") => route!("payments", id(input, "payment_id")?),
        ("shopware", op) => {
            let entity = match op {
                "orders.list" => "order",
                "products.list" => "product",
                _ => "customer",
            };
            route!("api", "search", entity);
            method = Method::POST;
            body = json!({"page":input["page"].as_u64().unwrap_or(1),"limit":100,"associations":if entity=="order"{json!({"lineItems":{},"orderCustomer":{},"currency":{},"transactions":{}})}else{json!({})}});
        }
        ("woocommerce", op) => {
            route!(
                "wp-json",
                "wc",
                "v3",
                if op.starts_with("orders") {
                    "orders"
                } else if op.starts_with("products") {
                    "products"
                } else {
                    "customers"
                }
            );
            if op == "orders.get" {
                path.push(id(input, "order_id")?);
            } else {
                query.extend([
                    ("per_page".into(), "100".into()),
                    (
                        "page".into(),
                        input["page"].as_u64().unwrap_or(1).to_string(),
                    ),
                ]);
            }
        }
        ("shopify", op) => {
            route!(
                "admin",
                "api",
                required_version(connection)?,
                "graphql.json"
            );
            method = Method::POST;
            body = if op == "orders.list" {
                json!({"query":"query Orders($after:String){orders(first:100,after:$after){nodes{id name createdAt displayFinancialStatus totalPriceSet{shopMoney{amount currencyCode}}} pageInfo{hasNextPage endCursor}}}","variables":{"after":input["cursor"]}})
            } else {
                json!({"query":"query Products($after:String){products(first:100,after:$after){nodes{id title status} pageInfo{hasNextPage endCursor}}}","variables":{"after":input["cursor"]}})
            };
        }
        ("gmail", "messages.list") => {
            route!("gmail", "v1", "users", "me", "messages");
            query.push(("maxResults".into(), "100".into()));
            if let Some(q) = input["q"].as_str() {
                query.push(("q".into(), q.into()));
            }
        }
        ("gmail", "messages.get") => route!(
            "gmail",
            "v1",
            "users",
            "me",
            "messages",
            id(input, "message_id")?
        ),
        ("gmail", "drafts.create") => {
            route!("gmail", "v1", "users", "me", "drafts");
            write!(json!({"message":{"raw":text(input,"raw_base64url")?}}));
        }
        ("gmail", "drafts.send") => {
            route!("gmail", "v1", "users", "me", "drafts", "send");
            write!(json!({"id":id(input,"draft_id")?}));
        }
        ("search_console", "sites.list") => route!("webmasters", "v3", "sites"),
        ("search_console", "analytics.query") => {
            route!(
                "webmasters",
                "v3",
                "sites",
                text(input, "site_url")?,
                "searchAnalytics",
                "query"
            );
            date(text(input, "start_date")?)?;
            date(text(input, "end_date")?)?;
            method = Method::POST;
            body = json!({"startDate":text(input,"start_date")?,"endDate":text(input,"end_date")?,"dimensions":["query","page"],"rowLimit":1000,"startRow":input["start_row"].as_u64().unwrap_or(0)});
        }
        ("instagram", op) => {
            let version = required_version(connection)?;
            match op {
                "profile.get" => route!(version, id(&connection.settings, "account_id")?),
                "media.list" => route!(version, id(&connection.settings, "account_id")?, "media"),
                "media.status" => {
                    route!(version, id(input, "creation_id")?);
                    query.push(("fields".into(), "status_code,status".into()));
                }
                "media.create" => {
                    route!(version, id(&connection.settings, "account_id")?, "media");
                    write!(payload(input)?);
                }
                _ => {
                    route!(
                        version,
                        id(&connection.settings, "account_id")?,
                        "media_publish"
                    );
                    write!(json!({"creation_id":id(input,"creation_id")?}));
                }
            }
        }
        ("tiktok", "creator.get") => {
            route!("v2", "post", "publish", "creator_info", "query");
            method = Method::POST;
            body = json!({});
        }
        ("tiktok", "video.draft") => {
            route!("v2", "post", "publish", "inbox", "video", "init");
            write!(
                json!({"source_info":{"source":"PULL_FROM_URL","video_url":text(input,"video_url")?}})
            );
        }
        ("tiktok", "video.status") => {
            route!("v2", "post", "publish", "status", "fetch");
            method = Method::POST;
            body = json!({"publish_id":text(input,"publish_id")?});
        }
        ("discourse", "topics.list") => route!("latest.json"),
        ("discourse", "topic.get") => route!("t", format!("{}.json", id(input, "topic_id")?)),
        ("discourse", "posts.create") => {
            route!("posts.json");
            write!(payload(input)?);
        }
        ("sap_business_one", op) => {
            route!(match op {
                "partners.list" => "BusinessPartners",
                "invoices.list" => "Invoices",
                _ => "Orders",
            });
            if op == "orders.create" {
                write!(payload(input)?);
            } else {
                query.push(("$top".into(), "100".into()));
            }
        }
        ("sap_s4", op) => {
            let (service, entity) = match op {
                "partners.list" => ("API_BUSINESS_PARTNER", "A_BusinessPartner"),
                "orders.list" => ("API_SALES_ORDER_SRV", "A_SalesOrder"),
                _ => ("API_BILLING_DOCUMENT_SRV", "A_BillingDocument"),
            };
            route!(service, entity);
            query.extend([
                ("$format".into(), "json".into()),
                ("$top".into(), "100".into()),
            ]);
        }
        ("datev_accounting", op) => {
            route!("clients");
            if op != "clients.list" {
                path.extend([id(input, "client_id")?, "fiscal-years".into()]);
            }
            if op == "accounts_receivable.list" {
                path.extend([id(input, "fiscal_year_id")?, "accounts-receivable".into()]);
            }
        }
        ("peppol_storecove", "entities.get") => {
            route!("legal_entities", id(input, "legal_entity_id")?)
        }
        ("peppol_storecove", "participants.discover") => {
            route!("discovery", "receives");
            method = Method::POST;
            body = payload(input)?;
        }
        ("peppol_storecove", "documents.submit") => {
            route!("document_submissions");
            write!(payload(input)?);
        }
        ("peppol_storecove", "documents.status") => {
            route!(
                "document_submissions",
                id(input, "guid")?,
                "evidence",
                "sending"
            )
        }
        ("solveit", op) => {
            route!(if op == "applications.list" {
                "applications"
            } else {
                "tickets"
            });
            if !op.ends_with(".list") {
                path.push(id(input, "ticket_id")?);
            }
            match op {
                "tickets.update" => {
                    method = Method::PATCH;
                    body = payload(input)?;
                    header = Some("Idempotency-Key");
                    scope = "crm:write";
                }
                "tickets.message" => {
                    path.push("messages".into());
                    write!(payload(input)?);
                    header = Some("Idempotency-Key");
                    retry = true;
                }
                "work.get" => path.push("work".into()),
                _ => {}
            }
        }
        _ => return Err(invalid("Adapteroperation ist nicht implementiert.")),
    }
    if let Some(cursor) = input["cursor"].as_str() {
        match connection.provider.as_str() {
            "mollie" => query.push(("from".into(), cursor.into())),
            "stripe" => query.push(("starting_after".into(), cursor.into())),
            "gmail" => query.push(("pageToken".into(), cursor.into())),
            "enable_banking" => query.push(("continuation_key".into(), cursor.into())),
            _ => {}
        }
    }
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| invalid("Basis-URL ungültig."))?;
        segments.pop_if_empty();
        for segment in path {
            segments.push(&segment);
        }
    }
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    if method != Method::GET && scope.ends_with(":write") && header.is_none() {
        retry = false;
    }
    Ok(Plan {
        method,
        url,
        body,
        form,
        safe_retry: retry,
        idempotency_header: header,
        scope,
    })
}
