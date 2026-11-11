use crate::{domain, profile::Profile, Result};
use serde_json::{json, Value};
use sqlx::SqlitePool;
async fn command(
    pool: &SqlitePool,
    profile: &Profile,
    key: &str,
    command: &str,
    id: Option<&str>,
    input: Value,
) -> Result<Value> {
    Ok(domain::execute(
        pool,
        profile,
        &format!("demo-business-v2:{key}"),
        "demo.business",
        command,
        id,
        input,
    )
    .await?["record"]
        .clone())
}
pub async fn seed(pool: &SqlitePool, profile: &Profile) -> Result<()> {
    let organization = json!({"name":if profile.id()=="neurofunken"{"NeuroFunken · Demo-Unternehmen"}else{"Brilliantis · Demo-Unternehmen"},"address":"Beispielstraße 12","postal_code":"10115","city":"Berlin","country":"DE","email":"office@example.invalid","phone":"+49 30 000000","vat_id":"DE000000000","tax_number":"","registration":"Fiktive Beispieldaten","iban":"DE89370400440532013000","payment_terms":"Zahlbar innerhalb von 14 Tagen."});
    sqlx::query("INSERT OR IGNORE INTO organization(id,version,body) VALUES(1,1,?)")
        .bind(organization.to_string())
        .execute(pool)
        .await?;
    let customer=command(pool,profile,"customer","customer.create",None,json!({"name":"Nordlicht Studio (Demo)","email":"studio@example.invalid","contact":"Alex Beispiel","address":"Musterweg 8","postal_code":"20095","city":"Hamburg","country":"DE","vat_id":"","notes":"Fiktiver Kunde für den vollständigen Geschäftsablauf."})).await?;
    let bank = command(
        pool,
        profile,
        "account",
        "bank_account.create",
        None,
        json!({"name":"Geschäftskonto (Demo)","iban":"","kind":"bank","connection_id":""}),
    )
    .await?;
    for (month, price, paid) in [
        (1, 89000, true),
        (3, 119000, true),
        (5, 149000, true),
        (7, 189000, true),
        (9, 229000, false),
    ] {
        let key = format!("invoice-{month}");
        let invoice=command(pool,profile,&key,"invoice.create",None,json!({"customer_id":customer["id"],"title":format!("Managed Services {month:02}/2026 (Demo)"),"issue_date":format!("2026-{month:02}-01"),"service_date":format!("2026-{month:02}-01"),"due_date":format!("2026-{month:02}-15"),"direction":"outgoing","external_number":"","buyer_reference":"DEMO-REFERENZ","notes":"Demonstration – kein echter Rechnungsbeleg.","lines":[{"description":"Betrieb, Support und Weiterentwicklung (Demo)","quantity_milli":1000,"unit_price_cents":price,"tax_basis_points":1900}]})).await?;
        let issued = command(
            pool,
            profile,
            &format!("issue-{month}"),
            "invoice.issue",
            invoice["id"].as_str(),
            json!({"version":1}),
        )
        .await?;
        if paid {
            let payment=command(pool,profile,&format!("payment-{month}"),"bank_transaction.create",None,json!({"account_id":bank["id"],"external_id":format!("DEMO-2026-{month:02}"),"booking_date":format!("2026-{month:02}-12"),"amount_cents":issued["gross_cents"],"description":"Rechnungsausgleich (Demo)","counterparty":"Nordlicht Studio (Demo)"})).await?;
            command(pool,profile,&format!("allocation-{month}"),"bank_transaction.allocate",payment["id"].as_str(),json!({"version":1,"invoice_id":invoice["id"],"amount_cents":issued["gross_cents"]})).await?;
        }
        let fee=command(pool,profile,&format!("fee-{month}"),"bank_transaction.create",None,json!({"account_id":bank["id"],"external_id":format!("DEMO-FEE-2026-{month:02}"),"booking_date":format!("2026-{month:02}-28"),"amount_cents":-1900,"description":"Kontoführung (Demo)","counterparty":"Demo-Bank"})).await?;
        command(
            pool,
            profile,
            &format!("fee-category-{month}"),
            "bank_transaction.categorize",
            fee["id"].as_str(),
            json!({"version":1,"account":"expense:bank_fees","amount_cents":1900}),
        )
        .await?;
    }
    let quote=command(pool,profile,"quote","quote.create",None,json!({"customer_id":customer["id"],"title":"Nächster Ausbauschritt (Demo)","valid_until":"2027-12-31","notes":"Demo-Angebot zur Planung einer Erweiterung.","lines":[{"description":"Einrichtung und Migration (Demo)","quantity_milli":1000,"unit_price_cents":125000,"tax_basis_points":1900}]})).await?;
    command(
        pool,
        profile,
        "quote-send",
        "quote.send",
        quote["id"].as_str(),
        json!({"version":1}),
    )
    .await?;
    let account=command(pool,profile,"channel","channel_account.create",None,json!({"name":"Brilliantis Werkstatt (Demo)","platform":"instagram","handle":"@brilliantis.demo","identity":"brand","creator_id":"","connection_id":"","notes":"Fiktives Markenprofil. Kein externes Konto verbunden."})).await?;
    command(pool,profile,"content","content.create",None,json!({"title":"Ein Blick hinter die Kulissen (Demo)","channel_account_id":account["id"],"campaign_id":"","planned_date":"2026-10-15","body":"So wird aus einer Kundenanfrage ein übersichtlicher Geschäftsprozess. Wir zeigen die Arbeit an Brilliantis.","disclosure":"Eigenwerbung · NeuroFunken","media_url":""})).await?;
    command(pool,profile,"seo","seo_keyword.create",None,json!({"name":"Open Source ERP für kleine Teams (Demo)","campaign_id":"","target_url":"https://example.invalid/brilliantis","intent":"Produktvergleich und selbst hosten","notes":"Produktseite, verständliche Funktionsübersicht und Installationsanleitung vorbereiten."})).await?;
    Ok(())
}
