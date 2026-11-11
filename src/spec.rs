use schemars::{generate::SchemaSettings, JsonSchema};
use serde_json::{json, Value};
fn schema<T: JsonSchema>() -> Value {
    serde_json::to_value(
        SchemaSettings::draft2020_12()
            .with(|s| {
                s.inline_subschemas = true;
                s.meta_schema = None;
            })
            .into_generator()
            .into_root_schema_for::<T>(),
    )
    .unwrap()
}
fn reference(name: &str) -> Value {
    json!({"$ref":format!("#/components/schemas/{name}")})
}
fn operation(summary: &str, body: Option<Value>, idempotent: bool) -> Value {
    let mut v = json!({"summary":summary,"responses":{"200":{"description":"Ergebnis","content":{"application/json":{"schema":{"type":"object"}}}},"401":{"description":"Anmeldung fehlt oder abgelaufen"},"403":{"description":"Rechte, Origin oder CSRF-Prüfung"},"404":{"description":"Datensatz fehlt"},"409":{"description":"Versions-, Status- oder Idempotenzkonflikt"},"422":{"description":"Fachliche Validierung fehlgeschlagen"},"428":{"description":"Idempotency-Key fehlt"}}});
    if let Some(body) = body {
        v["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":body}}});
    }
    if idempotent {
        v["parameters"] = json!([{"name":"Idempotency-Key","in":"header","required":true,"schema":{"type":"string","minLength":1,"maxLength":120},"description":"Bei Wiederholung denselben Body und Key verwenden. Erfolgreiche Fachbefehle werden atomar protokolliert."}]);
    }
    v
}
fn parameters(path: &str) -> Vec<Value> {
    path.split('/').filter(|s|s.starts_with('{')).map(|s|json!({"name":s.trim_matches(['{','}']),"in":"path","required":true,"schema":{"type":"string"}})).collect()
}
pub fn openapi(profile: &crate::profile::Profile) -> Value {
    let mut schemas = json!({});
    macro_rules! add {
        ($ty:ty,$name:expr) => {
            schemas[$name] = schema::<$ty>();
        };
    }
    add!(crate::commerce::CatalogInput, "CommerceCatalog");
    add!(crate::commerce::CustomerInput, "CommerceCustomer");
    add!(crate::commerce::CustomerUpdate, "CommerceCustomerUpdate");
    add!(crate::commerce::OfferInput, "CommerceOffer");
    add!(crate::commerce::AcceptInput, "CommerceAccept");
    add!(crate::commerce::PriceInput, "CommercePrice");
    add!(crate::commerce::ProvisionInput, "CommerceProvision");
    add!(crate::commerce::InvoiceInput, "CommerceInvoice");
    add!(crate::commerce::billing::BillingInput, "CommerceBilling");
    add!(crate::LeadInput, "LeadInput");
    add!(crate::CampaignInput, "CampaignInput");
    add!(crate::CreatorInput, "CreatorInput");
    add!(crate::Update, "Update");
    add!(crate::WebsiteInput, "WebsiteInput");
    add!(crate::business::CustomerInput, "CustomerInput");
    add!(crate::business::ProductInput, "ProductInput");
    add!(crate::business::QuoteInput, "QuoteInput");
    add!(crate::business::Organization, "Organization");
    add!(crate::finance::InvoiceInput, "InvoiceInput");
    add!(crate::finance::AccountInput, "AccountInput");
    add!(crate::finance::TransactionInput, "TransactionInput");
    add!(crate::marketing::ChannelAccount, "ChannelAccount");
    add!(crate::marketing::Content, "Content");
    add!(crate::marketing::Collaboration, "Collaboration");
    add!(crate::marketing::Keyword, "Keyword");
    add!(crate::marketing::Order, "Order");
    add!(crate::briefings::BriefInput, "BriefInput");
    add!(crate::briefings::HandoffInput, "BriefHandoff");
    add!(crate::briefings::VersionInput, "BriefVersion");
    add!(crate::oidc::browser::IdentityInput, "OidcIdentityInput");
    add!(crate::auth::Login, "Login");
    add!(crate::auth::NewUser, "NewUser");
    add!(crate::auth::UserUpdate, "UserUpdate");
    add!(crate::auth::TokenInput, "TokenInput");
    add!(crate::auth::PasswordChange, "PasswordChange");
    add!(crate::jobs::JobInput, "JobInput");
    add!(crate::reporting::Adjustment, "Adjustment");
    let examples: Vec<Value> = profile
        .catalog()
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].clone())
        .collect();
    for s in ["LeadInput", "CampaignInput"] {
        schemas[s]["properties"]["product_id"]["examples"] = json!(examples);
        schemas[s]["properties"]["product_id"]["description"]=json!("Aktive Produkt-ID aus /api/v1/records/products. Beispiele sind Distributionsvorlagen, keine abgeschlossene Werteliste.");
    }
    let resources = [
        ("leads", "LeadInput"),
        ("campaigns", "CampaignInput"),
        ("creators", "CreatorInput"),
        ("customers", "CustomerInput"),
        ("products", "ProductInput"),
        ("quotes", "QuoteInput"),
        ("invoices", "InvoiceInput"),
        ("bank-accounts", "AccountInput"),
        ("bank-transactions", "TransactionInput"),
        ("orders", "Order"),
        ("channel-accounts", "ChannelAccount"),
        ("content", "Content"),
        ("collaborations", "Collaboration"),
        ("seo-keywords", "Keyword"),
    ];
    let mut paths = json!({});
    for (path, name, summary) in [
        (
            "catalog",
            "CommerceCatalog",
            "admin: unveränderliche Katalogrevision veröffentlichen",
        ),
        (
            "customers/{id}/update",
            "CommerceCustomerUpdate",
            "commerce:write: Kundendaten unter beiden Versionsbedingungen korrigieren",
        ),
        (
            "customers",
            "CommerceCustomer",
            "commerce:write: externen Kunden registrieren",
        ),
        (
            "offers",
            "CommerceOffer",
            "commerce:write: serverseitig validiertes Angebot anfordern",
        ),
        (
            "offers/{id}/price",
            "CommercePrice",
            "admin: ausgegebenes individuelles Projektangebot zuordnen",
        ),
        (
            "offers/{id}/accept",
            "CommerceAccept",
            "commerce:write: Angebot atomar annehmen; genau ein Auftrag",
        ),
        (
            "orders/{id}/provisioning",
            "CommerceProvision",
            "commerce:fulfill: Bereitstellung versioniert fortschreiben",
        ),
        (
            "orders/{id}/billing",
            "CommerceBilling",
            "commerce:billing: geprüfte Provider-Belege atomar abgleichen",
        ),
        (
            "orders/{id}/invoice",
            "CommerceInvoice",
            "finance:write: Erstrechnung aus angenommenen Positionen buchen",
        ),
    ] {
        let path = format!("/api/v1/commerce/{path}");
        paths[&path] = json!({"parameters":parameters(&path),"post":operation(summary,Some(reference(name)),true)});
    }
    for path in [
        "catalog",
        "customers/{id}",
        "offers/{id}",
        "orders/{id}",
        "customers/lookup",
        "offers/lookup",
        "orders/lookup",
        "events",
    ] {
        let full = format!("/api/v1/commerce/{path}");
        paths[&full]["get"] = operation(
            "commerce:read: Workspace-Vertrag brilliantis.commerce.v1 lesen",
            None,
            false,
        );
        paths[&full]["parameters"] = json!(parameters(&full));
        if path.ends_with("lookup") {
            paths[&full]["get"]["parameters"] = json!([{"name":"source","in":"query","required":true,"schema":{"type":"string"}},{"name":"external_id","in":"query","required":true,"schema":{"type":"string"}}]);
        }
        if path == "events" {
            paths[&full]["get"]["parameters"] = json!([{"name":"after","in":"query","schema":{"type":"integer","minimum":0,"default":0}},{"name":"limit","in":"query","schema":{"type":"integer","minimum":1,"maximum":200,"default":100}}]);
        }
    }
    paths["/api/v1/auth/oidc/login"] = json!({"get":{"summary":"Start Keycloak authorization code login with PKCE/S256, state and nonce","security":[],"responses":{"303":{"description":"Redirect to configured Keycloak authorization endpoint; browser-bound HttpOnly flow cookie"}}}});
    for path in [
        "/api/v1/auth/oidc/callback",
        "/api/v1/auth/oidc/logout/callback",
    ] {
        paths[path] = json!({"get":{"summary":"One-use browser-bound OIDC return; no arbitrary redirect targets","security":[],"responses":{"303":{"description":"Return to application; login failure never grants a session"}}}});
    }
    paths["/api/v1/access/users/{id}/oidc"] = json!({"parameters":parameters("/{id}"),"get":operation("admin: inspect current issuer/subject bindings and user version",None,false),"put":operation("admin: explicitly bind issuer and subject; optimistic user version; revoke existing sessions/tokens",Some(reference("OidcIdentityInput")),false)});
    let page_params = json!([{"name":"after","in":"query","schema":{"type":"integer","minimum":0,"default":0}},{"name":"limit","in":"query","schema":{"type":"integer","minimum":1,"maximum":200,"default":100}},{"name":"q","in":"query","schema":{"type":"string","maxLength":160}},{"name":"archived","in":"query","schema":{"type":"boolean","default":false}}]);
    paths["/api/v1/briefings"] = json!({"get":operation("crm:read: Briefings durchsuchen; items, next_cursor, has_more",None,false),"post":operation("crm:write: Grundbriefing oder Ergänzung anlegen",Some(reference("BriefInput")),true)});
    paths["/api/v1/briefings"]["get"]["parameters"] = page_params.clone();
    paths["/api/v1/briefings/{id}"] = json!({"parameters":parameters("/{id}"),"get":operation("crm:read: Briefingentwurf und aktuelle Version",None,false),"put":operation("crm:write: Entwurf ersetzen; bestehende Downloadfassungen bleiben unverändert",Some(json!({"type":"object","additionalProperties":false,"required":["version","fields"],"properties":{"version":{"type":"integer","minimum":1},"fields":reference("BriefInput")}})),true)});
    for action in ["release", "handoff", "archive", "restore"] {
        let path = format!("/api/v1/briefings/{{id}}/{action}");
        paths[&path] = json!({"parameters":parameters(&path),"post":operation(match action {"release"=>"crm:write: unveränderliche TXT-Fassung bereitstellen; kein Versand", "handoff"=>"crm:write: tatsächliche manuelle Übergabe einer revision einmalig protokollieren; kein Bot-Aktivierungsnachweis",_=>"crm:write: Briefing archivieren/wiederherstellen"},Some(reference(if action=="handoff" {"BriefHandoff"} else {"BriefVersion"})),true)});
    }
    paths["/api/v1/briefings/{id}/editions"] = json!({"parameters":parameters("/{id}"),"get":operation("crm:read: neueste 30 Fassungen mit SHA256 und getrenntem Übergabevermerk; items/has_more/next_cursor",None,false)});
    paths["/api/v1/briefings/{id}/editions"]["get"]["parameters"] = json!([{"name":"before","in":"query","schema":{"type":"integer","minimum":0,"default":0},"description":"Revision exklusiv; 0 für neueste Fassungen"}]);
    paths["/api/v1/briefings/{id}/editions/{revision}/download"] = json!({"parameters":parameters("/{id}/{revision}"),"get":{"summary":"crm:read: reproduzierbares UTF-8-TXT als Attachment; keine Statusänderung","responses":{"200":{"description":"Unveränderliche Downloadfassung","content":{"text/plain":{"schema":{"type":"string"}}}},"401":{"description":"Anmeldung erforderlich"},"403":{"description":"crm:read fehlt"},"404":{"description":"Fassung fehlt"}}}});
    for (resource, input) in resources {
        let path = format!("/api/v1/records/{resource}");
        paths[&path] = json!({"get":operation("Seite lesen; items, next_cursor und has_more",None,false),"post":operation("Datensatz anlegen",Some(reference(input)),true)});
        paths[&path]["get"]["parameters"] = page_params.clone();
        let path = format!("{path}/{{id}}");
        paths[&path] = json!({"parameters":parameters(&path),"get":operation("Datensatz mit Version lesen",None,false),"put":operation("Vollständige bearbeitbare Felder ersetzen; ausgegebene Belege sind unveränderlich",Some(json!({"type":"object","additionalProperties":false,"required":["version","fields"],"properties":{"version":{"type":"integer","minimum":1},"fields":reference(input)}})),true)});
        if ["bank-accounts", "bank-transactions", "orders"].contains(&resource) {
            paths[&path].as_object_mut().unwrap().remove("put");
        }
    }
    for resource in ["contracts", "credit-notes"] {
        let path = format!("/api/v1/records/{resource}");
        paths[&path] = json!({"get":operation("Abgeleitete Belege lesen",None,false)});
        paths[&path]["get"]["parameters"] = page_params.clone();
        let path = format!("{path}/{{id}}");
        paths[&path] =
            json!({"parameters":parameters(&path),"get":operation("Beleg lesen",None,false)});
    }
    let action = json!({"type":"object","required":["version"],"properties":{"version":{"type":"integer","minimum":1},"status":{"type":"string"},"invoice_id":{"type":"string"},"amount_cents":{"type":"integer","minimum":1},"account":{"type":"string"},"booking_date":{"type":"string","format":"date"},"publication_url":{"type":"string","format":"uri"}},"description":"archive/restore: Stammdaten; send/accept/reject: Angebot; cancel: Vertrag oder Rechnung (mit booking_date); issue: Rechnung; allocate/refund: Kontoumsatz mit invoice_id und amount_cents; categorize: Kontoumsatz mit account und amount_cents; status: Content/Bestellung/Kooperation. published benötigt publication_url."});
    paths["/api/v1/records/{resource}/{id}/{action}"] = json!({"parameters":parameters("/{resource}/{id}/{action}"),"post":operation("Versionierte Fachaktion",Some(action),true)});
    for (path, summary) in [
        ("/healthz", "Prozessstatus"),
        ("/readyz", "Datenbankbereitschaft"),
        (
            "/api/v1/dashboard",
            "CRM-Kennzahlen über alle aktiven Datensätze; Vorschau maximal 200 pro Art",
        ),
        (
            "/api/v1/catalog",
            "Erste 200 aktive Produkte; größere Kataloge über /records/products paginieren",
        ),
        (
            "/api/v1/events",
            "Ereigniscursor: after oder latest=true; maximal 100 Einträge",
        ),
        (
            "/api/v1/agent/context",
            "Fähigkeiten, Rechte und nächste Aktionen",
        ),
        ("/api/v1/openapi.json", "API-Vertrag"),
        (
            "/api/v1/connections",
            "Konfigurierte Provider und Operationen",
        ),
        (
            "/api/v1/jobs",
            "Letzte 100 berechtigte Integrationsaufträge",
        ),
        ("/api/v1/access/users", "Benutzerkonten; Administration"),
        (
            "/api/v1/access/tokens",
            "Eigene Agentenzugänge ohne Tokenwerte",
        ),
        ("/api/v1/auth/session", "Sitzungsstatus und CSRF-Token"),
        (
            "/api/v1/settings/organization",
            "Unternehmensdaten und Version",
        ),
        (
            "/api/v1/finance/report",
            "Bericht: year, Journalwerte und aktuelle offene Posten",
        ),
        (
            "/api/v1/finance/journal",
            "Journal: year und after; 100 Buchungen pro Seite",
        ),
        (
            "/api/v1/finance/export.csv",
            "CSV-Journal nach year; bis 100.000 Zeilen, sonst Fehler",
        ),
    ] {
        paths[path] = json!({"get":operation(summary,None,false)});
    }
    for (path, method, input, idempotent, summary) in [
        (
            "/api/v1/auth/login",
            "post",
            "Login",
            false,
            "Lokale Anmeldung; Origin erforderlich",
        ),
        (
            "/api/v1/auth/password",
            "post",
            "PasswordChange",
            false,
            "Eigenes Passwort ändern; eigene Sitzungen und Token widerrufen",
        ),
        (
            "/api/v1/access/users",
            "post",
            "NewUser",
            false,
            "Benutzerkonto mit lokaler Rolle und optionalem Keycloak-Subject anlegen",
        ),
        (
            "/api/v1/access/users/{id}",
            "put",
            "UserUpdate",
            false,
            "Rolle, Name und Deaktivierung; aktuelle Version erforderlich",
        ),
        (
            "/api/v1/access/tokens",
            "post",
            "TokenInput",
            false,
            "Agentenzugang einmalig ausgeben; nur aus Browsersitzung",
        ),
        (
            "/api/v1/connections/{id}/jobs",
            "post",
            "JobInput",
            true,
            "Dauerhaften externen Auftrag anlegen; Operation und input",
        ),
        (
            "/api/v1/finance/journal",
            "post",
            "Adjustment",
            true,
            "Ausgeglichene Sachkontobuchung festschreiben",
        ),
        (
            "/api/v1/import/website",
            "post",
            "WebsiteInput",
            true,
            "Website-Anfrage dedupliziert importieren",
        ),
    ] {
        if paths.get(path).is_none() {
            paths[path] = json!({});
        }
        paths[path][method] = operation(summary, Some(reference(input)), idempotent);
        paths[path]["parameters"] = json!(parameters(path));
    }
    for (path, summary) in [
        ("/api/v1/auth/logout", "Abmelden"),
        (
            "/api/v1/access/tokens/{id}/revoke",
            "Eigenen Agentenzugang widerrufen",
        ),
        (
            "/api/v1/jobs/{id}/cancel",
            "Eigenen wartenden Auftrag abbrechen",
        ),
    ] {
        paths[path] = json!({"parameters":parameters(path),"post":operation(summary,None,false)});
    }
    paths["/api/v1/settings/organization"]["put"] = operation(
        "Unternehmensdaten versioniert ersetzen",
        Some(
            json!({"type":"object","required":["version","fields"],"properties":{"version":{"type":"integer","minimum":0},"fields":reference("Organization")}}),
        ),
        true,
    );
    paths["/api/v1/finance/periods"] = json!({"post":operation("Kalendermonat begründet sperren oder wieder öffnen",Some(json!({"type":"object","additionalProperties":false,"required":["period","closed","reason"],"properties":{"period":{"type":"string","pattern":"^[0-9]{4}-[0-9]{2}$"},"closed":{"type":"boolean"},"reason":{"type":"string","minLength":1}}})),true)});
    paths["/api/v1/finance/thresholds"] = json!({"put":operation("Grenzwerte der Kennzahlenüberwachung setzen",Some(json!({"type":"object","additionalProperties":false,"required":["minimum_liquidity_cents","maximum_overdue_cents"],"properties":{"minimum_liquidity_cents":{"type":"integer"},"maximum_overdue_cents":{"type":"integer","minimum":0}}})),false)});
    for (resource, input) in [
        ("leads", "LeadInput"),
        ("campaigns", "CampaignInput"),
        ("creators", "CreatorInput"),
    ] {
        let path = format!("/api/v1/{resource}");
        paths[&path] = json!({"get":operation("Kompatible paginierte Liste",None,false),"post":operation("Anlegen",Some(reference(input)),true)});
        paths[&path]["get"]["parameters"] = page_params.clone();
        let path = format!("{path}/{{id}}");
        paths[&path] = json!({"parameters":parameters(&path),"patch":operation("Status ändern",Some(reference("Update")),true)});
    }
    for (path, mime, summary) in [
        (
            "/api/v1/documents/{kind}/{id}/print",
            "text/html",
            "Druckansicht ausgegebener quote/contract/invoice/credit_note",
        ),
        (
            "/api/v1/invoices/{id}/ubl.xml",
            "application/xml",
            "UBL für XRechnung 3.0; ausdrücklich ohne Schema-/Schematronnachweis",
        ),
    ] {
        paths[path] = json!({"parameters":parameters(path),"get":{"summary":summary,"responses":{"200":{"description":"Artefakt","content":{mime:{"schema":{"type":"string"}}}}}}});
    }
    paths["/api/v1/invoices/{id}/cii.xml"] = paths["/api/v1/invoices/{id}/ubl.xml"].clone();
    paths["/api/v1/invoices/{id}/cii.xml"]["get"]["summary"] =
        json!("CII EN16931 für ZUGFeRD-PDF/A-3-Assembler; ohne Schema-/Schematronnachweis");
    for path in [
        "/healthz",
        "/readyz",
        "/api/v1/auth/login",
        "/api/v1/auth/session",
    ] {
        for op in paths[path].as_object_mut().unwrap().values_mut() {
            if op.is_object() {
                op["security"] = json!([]);
            }
        }
    }
    for path in [
        "/api/v1/finance/report",
        "/api/v1/finance/journal",
        "/api/v1/finance/export.csv",
    ] {
        paths[path]["get"]["parameters"] = json!([{"name":"year","in":"query","required":path!="/api/v1/finance/report","schema":{"type":"integer","minimum":1900,"maximum":9999}}]);
    }
    paths["/api/v1/finance/journal"]["get"]["parameters"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"after","in":"query","schema":{"type":"integer","minimum":0}}));
    paths["/api/v1/events"]["get"]["parameters"] = json!([{"name":"after","in":"query","schema":{"type":"integer","minimum":0}},{"name":"latest","in":"query","schema":{"type":"boolean"}}]);
    paths["/api/v1/catalog"]["get"]["responses"]["200"]["content"]["application/json"]["schema"] =
        json!({"type":"array","items":{"type":"object"}});
    json!({"openapi":"3.1.0","info":{"title":"Brilliantis API","version":env!("CARGO_PKG_VERSION"),"license":{"name":"MIT","identifier":"MIT"},"description":"Ein isolierter Unternehmensarbeitsbereich pro Installation. Agenten verwenden Bearer-Token mit Scopes, optional Keycloak-Introspection. Browser verwenden HttpOnly-Sitzung und bei Änderungen Origin plus X-CSRF-Token. X-Brilliantis-Client ist nur in Loopback-Demos eine frei gewählte Urheberangabe; im Arbeitsbetrieb kommt sie aus der verifizierten Identität. Geld in EUR-Cent, Menge in Tausendsteln, Steuer in Basispunkten. Listen sind begrenzt und paginiert. Fehler schreiben keine Fachänderung. Externe Aufträge liefern bei unklarem Ausgang uncertain und werden ohne Idempotenzvertrag nicht blind wiederholt."},"servers":[{"url":"/"}],"security":[{"BearerAuth":[]},{"BrowserSession":[]}],"paths":paths,"components":{"securitySchemes":{"BearerAuth":{"type":"http","scheme":"bearer"},"BrowserSession":{"type":"apiKey","in":"cookie","name":"__Host-brilliantis_session","description":"Lokal über HTTP: brilliantis_session. Bei Änderungen zusätzlich X-CSRF-Token aus /auth/session und Origin senden."}},"schemas":schemas}})
}
