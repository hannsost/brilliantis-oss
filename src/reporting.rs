use crate::{
    auth::Principal, command_headers, conflict, finance, invalid, AppState, Error, Result,
};
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    Extension, Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Period {
    pub year: Option<i32>,
}
fn year_range(year: i32) -> Result<(String, String)> {
    if !(1900..=9999).contains(&year) {
        return Err(invalid("Jahr ungültig."));
    }
    Ok((format!("{year}-01-01"), format!("{year}-12-31")))
}
fn account_label(account: &str, name: Option<&str>) -> String {
    if let Some(name) = name {
        return match account.split(':').next().unwrap_or("") {
            "receivables" => format!("Forderungen · {name}"),
            "payables" => format!("Verbindlichkeiten · {name}"),
            "asset" => name.to_owned(),
            _ => account.to_owned(),
        };
    }
    match account {
        "revenue" => "Umsatzerlöse".into(),
        "expense" => "Betriebliche Aufwendungen".into(),
        "clearing" => "Zahlungsverrechnung".into(),
        "expense:bank_fees" => "Bank- und Zahlungsgebühren".into(),
        "expense:adjustment" => "Aufwandskorrekturen".into(),
        "revenue:adjustment" => "Erlöskorrekturen".into(),
        "revenue:rounding" => "Zahlbetragsrundung · Erlöse".into(),
        "expense:rounding" => "Zahlbetragsrundung · Aufwendungen".into(),
        "asset:other" => "Sonstiges Vermögen".into(),
        "liability:other" => "Sonstige Verbindlichkeiten".into(),
        "equity:opening" => "Eigenkapital · Eröffnung".into(),
        "equity:contribution" => "Einlagen / Entnahmen".into(),
        _ => {
            if let Some((kind, rate)) = account.split_once(':') {
                if let Ok(rate) = rate.parse::<i64>() {
                    if let Some(label) = match kind {
                        "vat_input" => Some("Vorsteuer"),
                        "vat_output" => Some("Umsatzsteuer"),
                        _ => None,
                    } {
                        return format!("{label} · {}.{:02} %", rate / 100, rate % 100);
                    }
                }
            }
            account.to_owned()
        }
    }
}
pub async fn report(
    State(state): State<AppState>,
    Query(query): Query<Period>,
) -> Result<Json<Value>> {
    let year = match query.year {
        Some(y) => y,
        None => sqlx::query_scalar::<_, String>("SELECT strftime('%Y','now')")
            .fetch_one(&state.pool)
            .await?
            .parse()
            .unwrap_or(2026),
    };
    let (start, end) = year_range(year)?;
    // A read transaction gives every widget the same accounting snapshot.
    let mut tx = state.pool.begin().await?;
    let rows=sqlx::query("SELECT l.account,(SELECT json_extract(r.body,'$.name') FROM records r WHERE r.kind=CASE WHEN l.account LIKE 'asset:%' THEN 'bank_account' ELSE 'customer' END AND r.id=substr(l.account,instr(l.account,':')+1)) account_name,SUM(l.debit_cents) debit,SUM(l.credit_cents) credit,SUM(CASE WHEN e.booking_date<? THEN l.debit_cents-l.credit_cents ELSE 0 END) opening,SUM(CASE WHEN e.booking_date>=? THEN l.debit_cents ELSE 0 END) period_debit,SUM(CASE WHEN e.booking_date>=? THEN l.credit_cents ELSE 0 END) period_credit FROM journal_lines l JOIN journal_entries e ON e.id=l.entry_id WHERE e.posted=1 AND e.booking_date<=? GROUP BY l.account ORDER BY l.account").bind(&start).bind(&start).bind(&start).bind(&end).fetch_all(&mut *tx).await?;
    let mut accounts = Vec::new();
    let mut revenue = 0i64;
    let mut expense = 0i64;
    let mut liquidity = 0i64;
    let mut vat_output = 0i64;
    let mut vat_input = 0i64;
    let mut total = 0i64;
    for row in rows {
        let account: String = row.get("account");
        let debit: i64 = row.get("debit");
        let credit: i64 = row.get("credit");
        let pd: i64 = row.get("period_debit");
        let pc: i64 = row.get("period_credit");
        let balance = debit - credit;
        total += balance;
        if account == "revenue" || account.starts_with("revenue:") {
            revenue += pc - pd;
        }
        if account == "expense" || account.starts_with("expense:") {
            expense += pd - pc;
        }
        let name: Option<String> = row.get("account_name");
        if account.starts_with("asset:") && name.is_some() {
            liquidity += balance;
        }
        if account.starts_with("vat_output:") {
            vat_output += pc - pd;
        }
        if account.starts_with("vat_input:") {
            vat_input += pd - pc;
        }
        accounts.push(json!({"account":account,"label":account_label(&account,name.as_deref()),"opening_cents":row.get::<i64,_>("opening"),"debit_cents":pd,"credit_cents":pc,"balance_cents":balance}));
    }
    let monthly=sqlx::query("SELECT substr(e.booking_date,1,7) month,SUM(CASE WHEN l.account='revenue' OR l.account LIKE 'revenue:%' THEN l.credit_cents-l.debit_cents ELSE 0 END) revenue,SUM(CASE WHEN l.account='expense' OR l.account LIKE 'expense:%' THEN l.debit_cents-l.credit_cents ELSE 0 END) expense FROM journal_entries e JOIN journal_lines l ON l.entry_id=e.id WHERE e.posted=1 AND e.booking_date BETWEEN ? AND ? GROUP BY month ORDER BY month").bind(&start).bind(&end).fetch_all(&mut *tx).await?;
    let monthly:Vec<Value>=monthly.iter().map(|r|json!({"month":r.get::<String,_>("month"),"revenue_cents":r.get::<i64,_>("revenue"),"expense_cents":r.get::<i64,_>("expense")})).collect();
    // Operational open items are explicitly current, separate from the year-end trial balance.
    let open=sqlx::query("SELECT json_extract(body,'$.direction') direction, CASE WHEN julianday('now')-julianday(json_extract(body,'$.due_date'))<1 THEN 'current' WHEN julianday('now')-julianday(json_extract(body,'$.due_date'))<=30 THEN '1_30' WHEN julianday('now')-julianday(json_extract(body,'$.due_date'))<=60 THEN '31_60' WHEN julianday('now')-julianday(json_extract(body,'$.due_date'))<=90 THEN '61_90' ELSE 'over_90' END bucket,COUNT(*) count,SUM(json_extract(body,'$.gross_cents')-COALESCE(json_extract(body,'$.credited_cents'),0)-COALESCE(json_extract(body,'$.paid_cents'),0)) amount FROM records WHERE kind='invoice' AND json_extract(body,'$.status') IN ('issued','partially_paid') GROUP BY direction,bucket").fetch_all(&mut *tx).await?;
    let aging:Vec<Value>=open.iter().map(|r|json!({"direction":r.get::<String,_>("direction"),"bucket":r.get::<String,_>("bucket"),"count":r.get::<i64,_>("count"),"amount_cents":r.get::<i64,_>("amount")})).collect();
    let receivables: i64 = aging
        .iter()
        .filter(|v| v["direction"] == "outgoing")
        .map(|v| v["amount_cents"].as_i64().unwrap())
        .sum();
    let payables: i64 = aging
        .iter()
        .filter(|v| v["direction"] == "incoming")
        .map(|v| v["amount_cents"].as_i64().unwrap())
        .sum();
    let overdue: i64 = aging
        .iter()
        .filter(|v| v["direction"] == "outgoing" && v["bucket"] != "current")
        .map(|v| v["amount_cents"].as_i64().unwrap())
        .sum();
    let unallocated:i64=sqlx::query_scalar("SELECT COUNT(*) FROM records WHERE kind='bank_transaction' AND json_extract(body,'$.status')!='allocated'").fetch_one(&mut *tx).await?;
    let drafts:i64=sqlx::query_scalar("SELECT COUNT(*) FROM records WHERE kind='invoice' AND json_extract(body,'$.status')='draft' AND json_extract(body,'$.issue_date') BETWEEN ? AND ?").bind(&start).bind(&end).fetch_one(&mut *tx).await?;
    let refunds:i64=sqlx::query_scalar("SELECT COALESCE(SUM(json_extract(body,'$.refund_due_cents')),0) FROM records WHERE kind='invoice' AND json_extract(body,'$.status')='cancelled'").fetch_one(&mut *tx).await?;
    let closed: Vec<String> =
        sqlx::query_scalar("SELECT period FROM closed_periods WHERE period LIKE ? ORDER BY period")
            .bind(format!("{year}-%"))
            .fetch_all(&mut *tx)
            .await?;
    let config: Option<String> =
        sqlx::query_scalar("SELECT value FROM metadata WHERE key='kpi_thresholds'")
            .fetch_optional(&mut *tx)
            .await?;
    let thresholds: Value = config
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(json!({"minimum_liquidity_cents":0,"maximum_overdue_cents":0}));
    let generated: String = sqlx::query_scalar("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')")
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut alerts = Vec::new();
    if liquidity < thresholds["minimum_liquidity_cents"].as_i64().unwrap_or(0) {
        alerts.push(json!({"code":"liquidity","severity":"warning","message":"Gebuchter Kontenbestand liegt unter dem Zielwert."}));
    }
    if overdue > thresholds["maximum_overdue_cents"].as_i64().unwrap_or(0) {
        alerts.push(json!({"code":"overdue","severity":"warning","message":"Überfällige Forderungen überschreiten den Zielwert."}));
    }
    if unallocated > 0 {
        alerts.push(json!({"code":"unallocated","severity":"info","message":"Kontoumsätze benötigen eine Zuordnung oder Sachkontobuchung."}));
    }
    if drafts > 0 {
        alerts.push(json!({"code":"draft_invoices","severity":"info","message":"Rechnungsentwürfe dieses Jahres sind noch nicht gebucht."}));
    }
    if refunds > 0 {
        alerts.push(json!({"code":"refunds","severity":"warning","message":"Stornierte Rechnungen enthalten noch auszugleichende Zahlungen."}));
    }
    Ok(Json(
        json!({"year":year,"period":{"from":start,"to":end},"generated_at":generated,"currency":"EUR","basis":"posted_journal","metrics":{"revenue_cents":revenue,"expense_cents":expense,"result_cents":revenue-expense,"liquidity_cents":liquidity,"receivables_current_cents":receivables,"payables_current_cents":payables,"overdue_current_cents":overdue,"vat_output_cents":vat_output,"vat_input_cents":vat_input,"vat_difference_cents":vat_output-vat_input,"unallocated_transactions":unallocated,"draft_invoices":drafts,"refunds_due_current_cents":refunds,"trial_balance_difference_cents":total},"accounts":accounts,"monthly":monthly,"aging_current":aging,"closed_periods":closed,"alerts":alerts,"thresholds":thresholds,"readiness":{"balanced":total==0,"periods_closed":closed.len(),"tax_filing":false,"statutory_annual_accounts":false}}),
    ))
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JournalPage {
    #[serde(default)]
    after: i64,
    year: i32,
}
pub async fn journal_list(
    State(state): State<AppState>,
    Query(input): Query<JournalPage>,
) -> Result<Json<Value>> {
    let (start, end) = year_range(input.year)?;
    if input.after < 0 {
        return Err(invalid("Cursor ungültig."));
    }
    let rows=sqlx::query("SELECT rowid cursor,* FROM journal_entries WHERE posted=1 AND rowid>? AND booking_date BETWEEN ? AND ? ORDER BY rowid LIMIT 101").bind(input.after).bind(start).bind(end).fetch_all(&state.pool).await?;
    let has_more = rows.len() > 100;
    let mut items = Vec::new();
    let mut cursor = input.after;
    for row in rows.iter().take(100) {
        cursor = row.get("cursor");
        let id: String = row.get("id");
        let lines=sqlx::query("SELECT account,debit_cents,credit_cents FROM journal_lines WHERE entry_id=? ORDER BY position").bind(&id).fetch_all(&state.pool).await?;
        items.push(json!({"id":id,"source_key":row.get::<String,_>("source_key"),"booking_date":row.get::<String,_>("booking_date"),"description":row.get::<String,_>("description"),"lines":lines.iter().map(|l|json!({"account":l.get::<String,_>("account"),"debit_cents":l.get::<i64,_>("debit_cents"),"credit_cents":l.get::<i64,_>("credit_cents")})).collect::<Vec<_>>()}));
    }
    Ok(Json(
        json!({"items":items,"next_cursor":cursor,"has_more":has_more}),
    ))
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Adjustment {
    booking_date: String,
    description: String,
    lines: Vec<Posting>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Posting {
    account: String,
    debit_cents: i64,
    credit_cents: i64,
}
pub fn account_allowed(account: &str) -> bool {
    account.len() <= 100
        && account
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b":_-".contains(&c))
        && [
            "asset:",
            "liability:",
            "equity:",
            "expense:",
            "revenue:",
            "vat_input:",
            "vat_output:",
        ]
        .iter()
        .any(|p| account.starts_with(p))
}
pub async fn adjustment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let value: Adjustment =
        serde_json::from_value(input.clone()).map_err(|_| invalid("Buchungsfelder ungültig."))?;
    if value.description.trim().is_empty()
        || value.description.len() > 1000
        || value.lines.len() < 2
        || value.lines.len() > 200
        || value.lines.iter().any(|l| {
            !account_allowed(&l.account)
                || l.debit_cents > 100_000_000_000
                || l.credit_cents > 100_000_000_000
        })
    {
        return Err(invalid(
            "Buchungstext oder Kontenzeilen ungültig. Nebenbücher über ihre Fachaktionen buchen.",
        ));
    }
    let (key, actor) = command_headers(&headers)?;
    let request = json!({"command":"journal.adjust","input":input,"actor":actor}).to_string();
    let mut tx = state.pool.begin().await?;
    if let Some(replay) = replay(&mut tx, &key, &request).await? {
        return Ok(Json(replay));
    }
    let id = finance::journal(
        &mut tx,
        &format!("adjustment:{key}"),
        &value.booking_date,
        &value.description,
        value
            .lines
            .into_iter()
            .map(|l| (l.account, l.debit_cents, l.credit_cents))
            .collect(),
    )
    .await?;
    let response = json!({"id":id,"status":"posted"});
    receipt(
        &mut tx,
        &key,
        &request,
        &response,
        &actor,
        "journal.adjust",
        &id,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(response))
}
pub async fn close_period(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    #[derive(Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    struct Input {
        period: String,
        closed: bool,
        reason: String,
    }
    let value: Input =
        serde_json::from_value(input.clone()).map_err(|_| invalid("Abschlussfelder ungültig."))?;
    crate::business::date(&format!("{}-01", value.period))?;
    if value.reason.trim().is_empty() || value.reason.len() > 1000 {
        return Err(invalid("Begründung angeben."));
    }
    let (key, actor) = command_headers(&headers)?;
    let request = json!({"command":"period.close","input":input,"actor":actor}).to_string();
    let mut tx = state.pool.begin().await?;
    if let Some(replay) = replay(&mut tx, &key, &request).await? {
        return Ok(Json(replay));
    }
    if value.closed {
        let current: String = sqlx::query_scalar("SELECT strftime('%Y-%m','now')")
            .fetch_one(&mut *tx)
            .await?;
        if value.period >= current {
            return Err(invalid("Nur abgeschlossene Kalendermonate sperren."));
        }
        sqlx::query("INSERT OR IGNORE INTO closed_periods(period,actor) VALUES(?,?)")
            .bind(&value.period)
            .bind(&actor)
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query("DELETE FROM closed_periods WHERE period=?")
            .bind(&value.period)
            .execute(&mut *tx)
            .await?;
    }
    let response = json!({"period":value.period,"closed":value.closed});
    receipt(
        &mut tx,
        &key,
        &request,
        &response,
        &actor,
        if value.closed {
            "period.close"
        } else {
            "period.reopen"
        },
        &value.period,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(response))
}
pub async fn thresholds(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    #[derive(Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    struct Input {
        minimum_liquidity_cents: i64,
        maximum_overdue_cents: i64,
    }
    let value: Input = serde_json::from_value(input.clone())
        .map_err(|_| invalid("Kennzahlengrenzen ungültig."))?;
    if !(-100_000_000_000..=100_000_000_000).contains(&value.minimum_liquidity_cents)
        || !(0..=100_000_000_000).contains(&value.maximum_overdue_cents)
    {
        return Err(invalid("Kennzahlengrenzen außerhalb des Wertebereichs."));
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO metadata(key,value) VALUES('kpi_thresholds',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value").bind(input.to_string()).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO security_events(actor,action,target) VALUES(?,'thresholds.update',?)")
        .bind(p.actor)
        .bind(input.to_string())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(input))
}
pub async fn replay(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &str,
    request: &str,
) -> Result<Option<Value>> {
    if let Some(r) = sqlx::query("SELECT request,response FROM commands WHERE idempotency_key=?")
        .bind(key)
        .fetch_optional(&mut **tx)
        .await?
    {
        if r.get::<String, _>("request") != request {
            return Err(conflict("Idempotency-Key bereits belegt."));
        }
        return Ok(Some(
            serde_json::from_str(r.get("response"))
                .map_err(|_| invalid("Befehlsbeleg ungültig."))?,
        ));
    }
    Ok(None)
}
pub async fn receipt(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &str,
    request: &str,
    response: &Value,
    actor: &str,
    action: &str,
    target: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO commands(idempotency_key,request,response) VALUES(?,?,?)")
        .bind(key)
        .bind(request)
        .bind(response.to_string())
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO security_events(actor,action,target) VALUES(?,?,?)")
        .bind(actor)
        .bind(action)
        .bind(target)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
fn csv(value: &str) -> String {
    let value = if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{value}")
    } else {
        value.to_string()
    };
    format!(
        "\"{}\"",
        value.replace('"', "\"\"").replace(['\r', '\n'], " ")
    )
}
pub async fn export(
    State(state): State<AppState>,
    Query(input): Query<Period>,
) -> Result<axum::response::Response> {
    use axum::response::IntoResponse;
    let (start, end) = year_range(input.year.ok_or_else(|| invalid("Jahr angeben."))?)?;
    let rows=sqlx::query("SELECT e.id,e.source_key,e.booking_date,e.description,l.position,l.account,l.debit_cents,l.credit_cents FROM journal_entries e JOIN journal_lines l ON e.id=l.entry_id WHERE e.posted=1 AND e.booking_date BETWEEN ? AND ? ORDER BY e.booking_date,e.id,l.position LIMIT 100001").bind(start).bind(end).fetch_all(&state.pool).await?;
    if rows.len() > 100000 {
        return Err(Error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "export_limit",
            "Jahresexport überschreitet 100.000 Zeilen. Über die Journal-API paginiert abrufen."
                .into(),
        ));
    }
    let mut result =
        String::from("Buchung;Quelle;Datum;Text;Position;Konto;Soll_Cent;Haben_Cent;Waehrung\r\n");
    for r in rows {
        result.push_str(&format!(
            "{};{};{};{};{};{};{};{};EUR\r\n",
            csv(r.get("id")),
            csv(r.get("source_key")),
            r.get::<String, _>("booking_date"),
            csv(r.get("description")),
            r.get::<i64, _>("position"),
            csv(r.get("account")),
            r.get::<i64, _>("debit_cents"),
            r.get::<i64, _>("credit_cents")
        ));
    }
    Ok((
        [
            ("content-type", "text/csv; charset=utf-8"),
            (
                "content-disposition",
                "attachment; filename=brilliantis-journal.csv",
            ),
        ],
        result,
    )
        .into_response())
}
