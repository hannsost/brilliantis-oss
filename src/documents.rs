use crate::{business, domain::field, invalid, AppState, Result};
use axum::{
    extract::{Path, State},
    response::{Html, IntoResponse, Response},
};
use serde_json::Value;
use std::collections::BTreeMap;
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn text(v: &Value, key: &str) -> String {
    escape(v[key].as_str().unwrap_or(""))
}
fn amount(n: i64) -> String {
    format!(
        "{}{}.{:02}",
        if n < 0 { "-" } else { "" },
        n.unsigned_abs() / 100,
        n.unsigned_abs() % 100
    )
}
fn cents(v: &Value, key: &str) -> i64 {
    v[key].as_i64().unwrap_or(0)
}
fn groups(v: &Value) -> BTreeMap<i64, (i64, i64)> {
    let mut result = BTreeMap::new();
    for line in v["lines"].as_array().into_iter().flatten() {
        let g = result
            .entry(cents(line, "tax_basis_points"))
            .or_insert((0, 0));
        g.0 += cents(line, "net_cents");
        g.1 += cents(line, "tax_cents");
    }
    result
}
async fn document(state: &AppState, kind: &str, id: &str) -> Result<Value> {
    if !["quote", "contract", "invoice", "credit_note"].contains(&kind) {
        return Err(invalid("Dokumenttyp ungültig."));
    }
    let mut tx = state.pool.begin().await?;
    let v = business::record(&mut tx, kind, id).await?;
    if v["status"] == "draft" {
        return Err(invalid("Dokument zuerst ausgeben oder buchen."));
    }
    Ok(v)
}
pub async fn print(
    State(state): State<AppState>,
    Path((kind, id)): Path<(String, String)>,
) -> Result<Html<String>> {
    let v = document(&state, &kind, &id).await?;
    let supplier = &v["supplier"];
    let customer = &v["customer"];
    let title = match kind.as_str() {
        "quote" => "Angebot",
        "contract" => "Vertrag",
        "credit_note" => "Stornorechnung",
        _ => "Rechnung",
    };
    let party = |p: &Value| {
        format!(
            "<strong>{}</strong><br>{}<br>{} {}<br>{}",
            text(p, "name"),
            text(p, "address"),
            text(p, "postal_code"),
            text(p, "city"),
            text(p, "country")
        )
    };
    let lines = v["lines"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, l)| {
            format!(
                "<tr><td>{}</td><td>{}</td><td>{:.3}</td><td>{}</td><td>{}%</td><td>{}</td></tr>",
                i + 1,
                text(l, "description"),
                cents(l, "quantity_milli") as f64 / 1000.,
                amount(cents(l, "unit_price_cents")),
                amount(cents(l, "tax_basis_points")),
                amount(cents(l, "net_cents"))
            )
        })
        .collect::<String>();
    Ok(Html(format!("<!doctype html><html lang=\"de\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title} {number}</title><link rel=\"stylesheet\" href=\"/app.css\"></head><body class=\"document-page\"><article class=\"document-sheet\"><header class=\"invoice-brand\"><div><img src=\"/favicon.svg\" width=\"35\" height=\"35\" alt=\"\"><span>Brilliantis</span></div><strong>{seller}</strong></header><div class=\"invoice-parties\"><section><small>EMPFÄNGER</small><p>{buyer}</p></section><section><small>ABSENDER</small><p>{supplier}</p></section></div><h1>{title} {number}</h1><h2>{subject}</h2>{notice}<dl class=\"invoice-meta\"><div><dt>Belegdatum</dt><dd>{issued}</dd></div><div><dt>Leistungsdatum</dt><dd>{service}</dd></div><div><dt>Fällig / gültig bis</dt><dd>{due}</dd></div></dl><table class=\"invoice-lines\"><thead><tr><th>Pos.</th><th>Leistung</th><th>Menge</th><th>Einzelpreis</th><th>USt.</th><th>Netto EUR</th></tr></thead><tbody>{lines}</tbody></table><div class=\"document-totals\"><span>Netto<strong>{net} EUR</strong></span><span>Umsatzsteuer<strong>{tax} EUR</strong></span>{rounding}<span class=\"total\">Gesamt<strong>{gross} EUR</strong></span></div><p class=\"invoice-notes\">{notes}</p><p class=\"invoice-notes\">{terms}</p><div class=\"invoice-footer\"><span>{seller}<br>{registration}</span><span>{email}<br>IBAN: {iban}</span><span>USt-IdNr.: {vat}<br>Steuernummer: {tax_number}</span></div></article></body></html>",notice=if v["status"]=="cancelled"{"<p class=\"notice\">Dieser Beleg wurde storniert. Bitte den zugehörigen Korrekturbeleg beachten.</p>"}else{""},number=text(&v,"number"),seller=text(supplier,"name"),buyer=party(customer),supplier=party(supplier),subject=text(&v,"title"),issued=text(&v,"issue_date"),service=text(&v,"service_date"),due=if v["due_date"].is_string(){text(&v,"due_date")}else{text(&v,"valid_until")},net=amount(cents(&v,"net_cents")),tax=amount(cents(&v,"tax_cents")),gross=amount(cents(&v,"gross_cents")),rounding=if cents(&v,"rounding_cents")==0 {String::new()} else {format!("<span>Zahlbetragsrundung<strong>{} EUR</strong></span>",amount(cents(&v,"rounding_cents")))},notes=text(&v,"notes"),terms=text(supplier,"payment_terms"),registration=text(supplier,"registration"),email=text(supplier,"email"),iban=text(supplier,"iban"),vat=text(supplier,"vat_id"),tax_number=text(supplier,"tax_number"))))
}
/// UBL 2.1 XRechnung 3.0 draft artifact. A schema/Schematron validator must approve it before transport.
pub fn ubl(v: &Value) -> Result<String> {
    for key in ["issue_date", "service_date", "due_date"] {
        business::date(v[key].as_str().unwrap_or(""))?;
    }
    let supplier = &v["supplier"];
    let buyer = &v["customer"];
    for p in [supplier, buyer] {
        for k in ["name", "address", "postal_code", "city", "country", "email"] {
            field(p, k, 1, 500)?;
        }
    }
    field(supplier, "phone", 1, 100)?;
    field(supplier, "iban", 1, 40)?;
    field(v, "buyer_reference", 1, 200)?;
    if supplier["vat_id"].as_str().unwrap_or("").is_empty()
        && supplier["tax_number"].as_str().unwrap_or("").is_empty()
    {
        return Err(invalid(
            "USt-IdNr. oder Steuernummer des Ausstellers fehlt.",
        ));
    }
    if v["direction"] != "outgoing" {
        return Err(invalid("Ausgabe für eigene Ausgangsrechnungen vorgesehen."));
    }
    let address = |p: &Value| {
        format!("<cac:PostalAddress><cbc:StreetName>{}</cbc:StreetName><cbc:CityName>{}</cbc:CityName><cbc:PostalZone>{}</cbc:PostalZone><cac:Country><cbc:IdentificationCode>{}</cbc:IdentificationCode></cac:Country></cac:PostalAddress>",text(p,"address"),text(p,"city"),text(p,"postal_code"),text(p,"country"))
    };
    let party = |p: &Value, seller: bool| {
        let mut tax = String::new();
        for (key, scheme) in [("vat_id", "VAT"), ("tax_number", "FC")] {
            if !p[key].as_str().unwrap_or("").is_empty() {
                tax.push_str(&format!("<cac:PartyTaxScheme><cbc:CompanyID>{}</cbc:CompanyID><cac:TaxScheme><cbc:ID>{scheme}</cbc:ID></cac:TaxScheme></cac:PartyTaxScheme>",text(p,key)));
            }
        }
        format!("<cac:Party><cbc:EndpointID schemeID=\"EM\">{email}</cbc:EndpointID><cac:PartyName><cbc:Name>{name}</cbc:Name></cac:PartyName>{address}{tax}<cac:PartyLegalEntity><cbc:RegistrationName>{name}</cbc:RegistrationName></cac:PartyLegalEntity>{contact}</cac:Party>",email=text(p,"email"),name=text(p,"name"),address=address(p),contact=if seller{format!("<cac:Contact><cbc:Name>{}</cbc:Name><cbc:Telephone>{}</cbc:Telephone><cbc:ElectronicMail>{}</cbc:ElectronicMail></cac:Contact>",text(p,"name"),text(p,"phone"),text(p,"email"))}else{String::new()})
    };
    let mut tax = String::new();
    for (rate, (net, vat)) in groups(v) {
        tax.push_str(&format!("<cac:TaxSubtotal><cbc:TaxableAmount currencyID=\"EUR\">{}</cbc:TaxableAmount><cbc:TaxAmount currencyID=\"EUR\">{}</cbc:TaxAmount><cac:TaxCategory><cbc:ID>{}</cbc:ID><cbc:Percent>{}</cbc:Percent><cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme></cac:TaxCategory></cac:TaxSubtotal>",amount(net),amount(vat),if rate==0{"Z"}else{"S"},amount(rate)));
    }
    let mut lines = String::new();
    for (i, l) in v["lines"]
        .as_array()
        .ok_or_else(|| invalid("Positionen fehlen."))?
        .iter()
        .enumerate()
    {
        lines.push_str(&format!("<cac:InvoiceLine><cbc:ID>{}</cbc:ID><cbc:InvoicedQuantity unitCode=\"C62\">{:.3}</cbc:InvoicedQuantity><cbc:LineExtensionAmount currencyID=\"EUR\">{}</cbc:LineExtensionAmount><cac:Item><cbc:Name>{}</cbc:Name><cac:ClassifiedTaxCategory><cbc:ID>{}</cbc:ID><cbc:Percent>{}</cbc:Percent><cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme></cac:ClassifiedTaxCategory></cac:Item><cac:Price><cbc:PriceAmount currencyID=\"EUR\">{}</cbc:PriceAmount></cac:Price></cac:InvoiceLine>",i+1,cents(l,"quantity_milli")as f64/1000.,amount(cents(l,"net_cents")),text(l,"description"),if cents(l,"tax_basis_points")==0{"Z"}else{"S"},amount(cents(l,"tax_basis_points")),amount(cents(l,"unit_price_cents"))));
    }
    Ok(format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Invoice xmlns=\"urn:oasis:names:specification:ubl:schema:xsd:Invoice-2\" xmlns:cac=\"urn:oasis:names:specification:ubl:schema:xsd:CommonAggregateComponents-2\" xmlns:cbc=\"urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2\"><cbc:CustomizationID>urn:cen.eu:en16931:2017#compliant#urn:xeinkauf.de:kosit:xrechnung_3.0</cbc:CustomizationID><cbc:ProfileID>urn:fdc:peppol.eu:2017:poacc:billing:01:1.0</cbc:ProfileID><cbc:ID>{number}</cbc:ID><cbc:IssueDate>{issued}</cbc:IssueDate><cbc:DueDate>{due}</cbc:DueDate><cbc:InvoiceTypeCode>380</cbc:InvoiceTypeCode><cbc:Note>{notes}</cbc:Note><cbc:DocumentCurrencyCode>EUR</cbc:DocumentCurrencyCode><cbc:BuyerReference>{reference}</cbc:BuyerReference><cac:InvoicePeriod><cbc:StartDate>{service}</cbc:StartDate><cbc:EndDate>{service}</cbc:EndDate></cac:InvoicePeriod><cac:AccountingSupplierParty>{supplier}</cac:AccountingSupplierParty><cac:AccountingCustomerParty>{buyer}</cac:AccountingCustomerParty><cac:PaymentMeans><cbc:PaymentMeansCode>58</cbc:PaymentMeansCode><cbc:PaymentID>{number}</cbc:PaymentID><cac:PayeeFinancialAccount><cbc:ID>{iban}</cbc:ID></cac:PayeeFinancialAccount></cac:PaymentMeans><cac:PaymentTerms><cbc:Note>{terms}</cbc:Note></cac:PaymentTerms><cac:TaxTotal><cbc:TaxAmount currencyID=\"EUR\">{tax_total}</cbc:TaxAmount>{tax}</cac:TaxTotal><cac:LegalMonetaryTotal><cbc:LineExtensionAmount currencyID=\"EUR\">{net}</cbc:LineExtensionAmount><cbc:TaxExclusiveAmount currencyID=\"EUR\">{net}</cbc:TaxExclusiveAmount><cbc:TaxInclusiveAmount currencyID=\"EUR\">{tax_inclusive}</cbc:TaxInclusiveAmount>{rounding}<cbc:PayableAmount currencyID=\"EUR\">{gross}</cbc:PayableAmount></cac:LegalMonetaryTotal>{lines}</Invoice>",number=text(v,"number"),issued=text(v,"issue_date"),due=text(v,"due_date"),notes=if v["notes"].as_str().unwrap_or("").is_empty(){text(v,"title")}else{text(v,"notes")},reference=text(v,"buyer_reference"),service=text(v,"service_date"),supplier=party(supplier,true),buyer=party(buyer,false),iban=text(supplier,"iban"),terms=text(supplier,"payment_terms"),tax_total=amount(cents(v,"tax_cents")),net=amount(cents(v,"net_cents")),gross=amount(cents(v,"gross_cents")),tax_inclusive=amount(cents(v,"net_cents")+cents(v,"tax_cents")),rounding=if cents(v,"rounding_cents")==0 {String::new()} else {format!("<cbc:PayableRoundingAmount currencyID=\"EUR\">{}</cbc:PayableRoundingAmount>",amount(cents(v,"rounding_cents")))}))
}
pub async fn ubl_export(State(state): State<AppState>, Path(id): Path<String>) -> Result<Response> {
    let v = document(&state, "invoice", &id).await?;
    let xml = ubl(&v)?;
    roxmltree::Document::parse(&xml).map_err(|_| invalid("XML konnte nicht erzeugt werden."))?;
    Ok((
        [
            ("content-type", "application/xml; charset=utf-8"),
            (
                "content-disposition",
                "attachment; filename=xrechnung-unvalidated.xml",
            ),
            ("x-brilliantis-validation", "schema-and-schematron-pending"),
        ],
        xml,
    )
        .into_response())
}

/// CII D16B EN16931 XML for a separate PDF/A-3 assembler. This is not itself a ZUGFeRD PDF.
pub fn cii(v: &Value) -> Result<String> {
    let _ = ubl(v)?;
    let supplier = &v["supplier"];
    let buyer = &v["customer"];
    let datetime = |value: &str| {
        format!(
            "<udt:DateTimeString format=\"102\">{}</udt:DateTimeString>",
            escape(&value.replace('-', ""))
        )
    };
    let party = |p: &Value, seller: bool| {
        let mut tax = String::new();
        for (key, scheme) in [("vat_id", "VA"), ("tax_number", "FC")] {
            if !p[key].as_str().unwrap_or("").is_empty() {
                tax.push_str(&format!("<ram:SpecifiedTaxRegistration><ram:ID schemeID=\"{scheme}\">{}</ram:ID></ram:SpecifiedTaxRegistration>",text(p,key)));
            }
        }
        format!("<ram:Name>{}</ram:Name>{}<ram:PostalTradeAddress><ram:PostcodeCode>{}</ram:PostcodeCode><ram:LineOne>{}</ram:LineOne><ram:CityName>{}</ram:CityName><ram:CountryID>{}</ram:CountryID></ram:PostalTradeAddress><ram:URIUniversalCommunication><ram:URIID schemeID=\"EM\">{}</ram:URIID></ram:URIUniversalCommunication>{tax}",text(p,"name"),if seller{format!("<ram:DefinedTradeContact><ram:PersonName>{}</ram:PersonName><ram:TelephoneUniversalCommunication><ram:CompleteNumber>{}</ram:CompleteNumber></ram:TelephoneUniversalCommunication><ram:EmailURIUniversalCommunication><ram:URIID>{}</ram:URIID></ram:EmailURIUniversalCommunication></ram:DefinedTradeContact>",text(p,"name"),text(p,"phone"),text(p,"email"))}else{String::new()},text(p,"postal_code"),text(p,"address"),text(p,"city"),text(p,"country"),text(p,"email"))
    };
    let mut lines = String::new();
    for (i, l) in v["lines"].as_array().unwrap().iter().enumerate() {
        lines.push_str(&format!("<ram:IncludedSupplyChainTradeLineItem><ram:AssociatedDocumentLineDocument><ram:LineID>{}</ram:LineID></ram:AssociatedDocumentLineDocument><ram:SpecifiedTradeProduct><ram:Name>{}</ram:Name></ram:SpecifiedTradeProduct><ram:SpecifiedLineTradeAgreement><ram:NetPriceProductTradePrice><ram:ChargeAmount>{}</ram:ChargeAmount></ram:NetPriceProductTradePrice></ram:SpecifiedLineTradeAgreement><ram:SpecifiedLineTradeDelivery><ram:BilledQuantity unitCode=\"C62\">{:.3}</ram:BilledQuantity></ram:SpecifiedLineTradeDelivery><ram:SpecifiedLineTradeSettlement><ram:ApplicableTradeTax><ram:TypeCode>VAT</ram:TypeCode><ram:CategoryCode>{}</ram:CategoryCode><ram:RateApplicablePercent>{}</ram:RateApplicablePercent></ram:ApplicableTradeTax><ram:SpecifiedTradeSettlementLineMonetarySummation><ram:LineTotalAmount>{}</ram:LineTotalAmount></ram:SpecifiedTradeSettlementLineMonetarySummation></ram:SpecifiedLineTradeSettlement></ram:IncludedSupplyChainTradeLineItem>",i+1,text(l,"description"),amount(cents(l,"unit_price_cents")),cents(l,"quantity_milli")as f64/1000.,if cents(l,"tax_basis_points")==0{"Z"}else{"S"},amount(cents(l,"tax_basis_points")),amount(cents(l,"net_cents"))));
    }
    let mut taxes = String::new();
    for (rate, (net, tax)) in groups(v) {
        taxes.push_str(&format!("<ram:ApplicableTradeTax><ram:CalculatedAmount>{}</ram:CalculatedAmount><ram:TypeCode>VAT</ram:TypeCode><ram:BasisAmount>{}</ram:BasisAmount><ram:CategoryCode>{}</ram:CategoryCode><ram:RateApplicablePercent>{}</ram:RateApplicablePercent></ram:ApplicableTradeTax>",amount(tax),amount(net),if rate==0{"Z"}else{"S"},amount(rate)));
    }
    Ok(format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><rsm:CrossIndustryInvoice xmlns:rsm=\"urn:un:unece:uncefact:data:standard:CrossIndustryInvoice:100\" xmlns:ram=\"urn:un:unece:uncefact:data:standard:ReusableAggregateBusinessInformationEntity:100\" xmlns:udt=\"urn:un:unece:uncefact:data:standard:UnqualifiedDataType:100\"><rsm:ExchangedDocumentContext><ram:GuidelineSpecifiedDocumentContextParameter><ram:ID>urn:cen.eu:en16931:2017</ram:ID></ram:GuidelineSpecifiedDocumentContextParameter></rsm:ExchangedDocumentContext><rsm:ExchangedDocument><ram:ID>{number}</ram:ID><ram:TypeCode>380</ram:TypeCode><ram:IssueDateTime>{issued}</ram:IssueDateTime><ram:IncludedNote><ram:Content>{title}</ram:Content></ram:IncludedNote></rsm:ExchangedDocument><rsm:SupplyChainTradeTransaction>{lines}<ram:ApplicableHeaderTradeAgreement><ram:BuyerReference>{reference}</ram:BuyerReference><ram:SellerTradeParty>{supplier}</ram:SellerTradeParty><ram:BuyerTradeParty>{buyer}</ram:BuyerTradeParty></ram:ApplicableHeaderTradeAgreement><ram:ApplicableHeaderTradeDelivery><ram:ActualDeliverySupplyChainEvent><ram:OccurrenceDateTime>{service}</ram:OccurrenceDateTime></ram:ActualDeliverySupplyChainEvent></ram:ApplicableHeaderTradeDelivery><ram:ApplicableHeaderTradeSettlement><ram:PaymentReference>{number}</ram:PaymentReference><ram:InvoiceCurrencyCode>EUR</ram:InvoiceCurrencyCode><ram:SpecifiedTradeSettlementPaymentMeans><ram:TypeCode>58</ram:TypeCode><ram:PayeePartyCreditorFinancialAccount><ram:IBANID>{iban}</ram:IBANID></ram:PayeePartyCreditorFinancialAccount></ram:SpecifiedTradeSettlementPaymentMeans>{taxes}<ram:SpecifiedTradePaymentTerms><ram:Description>{terms}</ram:Description><ram:DueDateDateTime>{due}</ram:DueDateDateTime></ram:SpecifiedTradePaymentTerms><ram:SpecifiedTradeSettlementHeaderMonetarySummation><ram:LineTotalAmount>{net}</ram:LineTotalAmount><ram:ChargeTotalAmount>0.00</ram:ChargeTotalAmount><ram:AllowanceTotalAmount>0.00</ram:AllowanceTotalAmount><ram:TaxBasisTotalAmount>{net}</ram:TaxBasisTotalAmount><ram:TaxTotalAmount currencyID=\"EUR\">{tax}</ram:TaxTotalAmount>{rounding}<ram:GrandTotalAmount>{tax_inclusive}</ram:GrandTotalAmount><ram:DuePayableAmount>{gross}</ram:DuePayableAmount></ram:SpecifiedTradeSettlementHeaderMonetarySummation></ram:ApplicableHeaderTradeSettlement></rsm:SupplyChainTradeTransaction></rsm:CrossIndustryInvoice>",number=text(v,"number"),issued=datetime(v["issue_date"].as_str().unwrap()),title=text(v,"title"),reference=text(v,"buyer_reference"),supplier=party(supplier,true),buyer=party(buyer,false),service=datetime(v["service_date"].as_str().unwrap()),iban=text(supplier,"iban"),terms=text(supplier,"payment_terms"),due=datetime(v["due_date"].as_str().unwrap()),net=amount(cents(v,"net_cents")),tax=amount(cents(v,"tax_cents")),gross=amount(cents(v,"gross_cents")),tax_inclusive=amount(cents(v,"net_cents")+cents(v,"tax_cents")),rounding=if cents(v,"rounding_cents")==0 {String::new()} else {format!("<ram:RoundingAmount>{}</ram:RoundingAmount>",amount(cents(v,"rounding_cents")))}))
}
pub async fn cii_export(State(state): State<AppState>, Path(id): Path<String>) -> Result<Response> {
    let v = document(&state, "invoice", &id).await?;
    let xml = cii(&v)?;
    roxmltree::Document::parse(&xml).map_err(|_| invalid("XML konnte nicht erzeugt werden."))?;
    Ok((
        [
            ("content-type", "application/xml; charset=utf-8"),
            (
                "content-disposition",
                "attachment; filename=factur-x-unvalidated.xml",
            ),
            ("x-brilliantis-validation", "schema-and-schematron-pending"),
        ],
        xml,
    )
        .into_response())
}
