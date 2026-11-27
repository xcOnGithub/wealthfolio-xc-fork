//! Profile-local statement observations, independent of recalculated valuations.
use crate::{
    db::{get_connection, DbPool, WriteHandle},
    errors::StorageError,
};
use async_trait::async_trait;
use chrono::NaiveDate;
use diesel::{
    prelude::*,
    sql_query,
    sql_types::{Nullable, Text},
};
use std::{collections::BTreeMap, sync::Arc};
use wealthfolio_core::{
    errors::Result,
    portfolio::statements::{
        daily::{reviewed_chain, DailyStatementWrite, StoredDailyStatement},
        invalid, StatementRepository, StatementSnapshot, StatementWrite, StoredStatement,
    },
};

pub struct StatementStore {
    pool: Arc<DbPool>,
    writer: WriteHandle,
}
impl StatementStore {
    pub fn new(pool: Arc<DbPool>, writer: WriteHandle) -> Self {
        Self { pool, writer }
    }
}
#[derive(QueryableByName)]
struct Row {
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=Text)]
    revision: String,
}
fn read(conn: &mut SqliteConnection, account: &str) -> Result<Vec<StoredStatement>> {
    let rows = sql_query(
        "SELECT payload, revision FROM account_statement_history \
         WHERE account_id = ? ORDER BY statement_date",
    )
    .bind::<Text, _>(account)
    .load::<Row>(conn)
    .map_err(StorageError::from)?;
    rows.into_iter()
        .map(|row| {
            Ok(StoredStatement {
                snapshot: serde_json::from_str(&row.payload)?,
                revision: row.revision,
            })
        })
        .collect()
}

fn write_snapshot(
    conn: &mut SqliteConnection,
    account: &str,
    snapshot: &StatementSnapshot,
) -> Result<()> {
    sql_query(
        "INSERT INTO account_statement_history(account_id,statement_date,payload,revision)
         VALUES (?,?,?,?) ON CONFLICT(account_id,statement_date) DO UPDATE
         SET payload=excluded.payload, revision=excluded.revision",
    )
    .bind::<Text, _>(account)
    .bind::<Text, _>(snapshot.date.to_string())
    .bind::<Text, _>(serde_json::to_string(snapshot)?)
    .bind::<Text, _>(uuid::Uuid::new_v4().to_string())
    .execute(conn)
    .map_err(StorageError::from)?;
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DailyPayload {
    #[serde(default)]
    closing_asset_ids: Vec<String>,
    version: u32,
    interval: wealthfolio_core::portfolio::statements::reconstruction::StatementInterval,
}

// Store matching metadata separately so an older app can still read daily facts.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct SecurityReview {
    revision: String,
    activity_security_keys: BTreeMap<String, String>,
    review_order: u64,
}

#[derive(QueryableByName)]
struct DailyRow {
    #[diesel(embed)]
    facts: Row,
    #[diesel(sql_type = Nullable<Text>)]
    security_review: Option<String>,
}

pub(crate) enum DailyStatementsForBasis {
    Empty,
    Decoded(Vec<StoredDailyStatement>),
    Unverifiable,
}

fn decode_daily_row(row: DailyRow) -> Result<StoredDailyStatement> {
    let facts: DailyPayload = serde_json::from_str(&row.facts.payload)?;
    if facts.version != 1 {
        return Err(invalid());
    }
    let matches = row
        .security_review
        .as_deref()
        .map(serde_json::from_str::<SecurityReview>)
        .transpose()?
        // An older app may replace the facts and revision without
        // writing match metadata. Discard evidence tied to the old facts.
        .filter(|review| review.revision == row.facts.revision)
        .unwrap_or_default();
    Ok(StoredDailyStatement {
        version: facts.version,
        closing_asset_ids: facts.closing_asset_ids,
        activity_security_keys: matches.activity_security_keys,
        review_order: matches.review_order,
        priced: None,
        live_handoff: Default::default(),
        interval: facts.interval,
        revision: row.facts.revision,
    })
}

pub(crate) fn read_daily(
    conn: &mut SqliteConnection,
    account: &str,
) -> Result<Vec<StoredDailyStatement>> {
    let rows = sql_query("SELECT payload, revision, security_review FROM account_statement_reconstructions WHERE account_id=? ORDER BY through_date")
        .bind::<Text,_>(account).load::<DailyRow>(conn).map_err(StorageError::from)?;
    rows.into_iter().map(decode_daily_row).collect()
}

pub(crate) fn read_daily_for_basis(
    conn: &mut SqliteConnection,
    account: &str,
) -> Result<DailyStatementsForBasis> {
    let rows = sql_query("SELECT payload, revision, security_review FROM account_statement_reconstructions WHERE account_id=? ORDER BY through_date")
        .bind::<Text, _>(account)
        .load::<DailyRow>(conn)
        .map_err(StorageError::from)?;
    if rows.is_empty() {
        return Ok(DailyStatementsForBasis::Empty);
    }
    let mut decoded = Vec::with_capacity(rows.len());
    for row in rows {
        let Ok(stmt) = decode_daily_row(row) else {
            return Ok(DailyStatementsForBasis::Unverifiable);
        };
        decoded.push(stmt);
    }
    Ok(DailyStatementsForBasis::Decoded(decoded))
}

fn json_decimal(value: &serde_json::Value) -> Option<rust_decimal::Decimal> {
    match value {
        serde_json::Value::String(s) => s.parse().ok(),
        serde_json::Value::Number(n) => n.to_string().parse().ok(),
        _ => None,
    }
}

fn extract_provider_security_trade_dates(
    conn: &mut SqliteConnection,
    connection_id: &str,
    external_id: &str,
    account_currency: &str,
    source_history: &serde_json::Value,
) -> Result<wealthfolio_core::portfolio::statements::ProviderTradeHistory> {
    use wealthfolio_core::portfolio::statements::{
        daily::{
            is_provider_tx_basis_changing, pair_provider_trade_events,
            provider_tx_requires_security_key, RawProviderTx,
        },
        ProviderTradeEvent, ProviderTradeHistory,
    };

    #[derive(serde::Deserialize)]
    struct TxView {
        investment_transaction_id: String,
        #[serde(default)]
        account_id: String,
        security_id: Option<String>,
        cancel_transaction_id: Option<String>,
        date: NaiveDate,
        #[serde(rename = "type", default)]
        transaction_type: String,
        #[serde(default)]
        subtype: String,
        #[serde(default)]
        quantity: serde_json::Value,
    }
    #[derive(serde::Deserialize)]
    struct SecurityView {
        security_id: String,
        ticker_symbol: Option<String>,
        iso_currency_code: Option<String>,
    }

    let parse_opt_date = |key: &str| -> std::result::Result<Option<NaiveDate>, ()> {
        match source_history.get(key) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(s)) => s.parse::<NaiveDate>().map(Some).map_err(|_| ()),
            Some(_) => Err(()),
        }
    };
    let as_of = match source_history.get("asOf") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => match chrono::DateTime::parse_from_rfc3339(s) {
            Ok(dt) => Some(dt.with_timezone(&chrono::Utc)),
            Err(_) => return Ok(ProviderTradeHistory::incomplete()),
        },
        Some(_) => return Ok(ProviderTradeHistory::incomplete()),
    };
    let (Ok(history_start), Ok(history_end), Ok(coverage_start)) = (
        parse_opt_date("historyStart"),
        parse_opt_date("historyEnd"),
        parse_opt_date("coverageStart"),
    ) else {
        return Ok(ProviderTradeHistory::incomplete());
    };
    let has_coverage_meta = source_history.get("asOf").is_some()
        || source_history.get("historyStart").is_some()
        || source_history.get("historyEnd").is_some()
        || source_history.get("coverageStart").is_some();

    let raw_from = history_start
        .or(coverage_start)
        .or_else(|| as_of.and_then(|ts| ts.date_naive().checked_sub_days(chrono::Days::new(730))));
    // Never fabricate coverage beyond the UTC date actually queried from Plaid.
    let raw_through = history_end.or_else(|| as_of.map(|ts| ts.date_naive()));
    let (coverage_from, coverage_through) = match (raw_from, raw_through) {
        (Some(s), Some(e)) if s > e => (None, None),
        pair => pair,
    };

    let Some(tx_field) = source_history.get("transactions") else {
        return Ok(if has_coverage_meta {
            ProviderTradeHistory::incomplete()
        } else {
            ProviderTradeHistory::default()
        });
    };
    let Some(tx_values) = tx_field.as_array() else {
        return Ok(ProviderTradeHistory::incomplete());
    };

    let mut history = ProviderTradeHistory {
        coverage_from,
        coverage_through,
        events: BTreeMap::new(),
        verification_incomplete: false,
    };
    if tx_values.is_empty() {
        return Ok(history);
    }
    let mut txs = Vec::with_capacity(tx_values.len());
    for v in tx_values {
        let Ok(tx) = serde_json::from_value::<TxView>(v.clone()) else {
            return Ok(ProviderTradeHistory::incomplete());
        };
        if !tx.quantity.is_null() && json_decimal(&tx.quantity).is_none() {
            return Ok(ProviderTradeHistory::incomplete());
        }
        if tx.account_id.is_empty() || tx.account_id == external_id {
            txs.push(tx);
        }
    }
    if txs.is_empty() {
        return Ok(history);
    }
    let raw_txs: Vec<RawProviderTx<'_>> = txs
        .iter()
        .map(|tx| {
            let qty_nonzero = json_decimal(&tx.quantity).is_some_and(|q| !q.is_zero());
            RawProviderTx {
                id: tx.investment_transaction_id.as_str(),
                security_key: tx.security_id.as_deref(),
                date: tx.date,
                cancel_target: tx.cancel_transaction_id.as_deref(),
                basis_changing: is_provider_tx_basis_changing(
                    &tx.transaction_type,
                    &tx.subtype,
                    qty_nonzero,
                ),
                requires_security_key: provider_tx_requires_security_key(
                    &tx.transaction_type,
                    &tx.subtype,
                    qty_nonzero,
                ),
            }
        })
        .collect();
    let paired = pair_provider_trade_events(&raw_txs);
    if paired.has_unattributed_event {
        return Ok(ProviderTradeHistory::incomplete());
    }
    if paired.events.is_empty() {
        return Ok(history);
    }

    let mut sec_to_assets: BTreeMap<String, Vec<String>> = BTreeMap::new();
    match source_history.get("assetIdsBySecurity") {
        None | Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::Object(asset_map)) => {
            for (sec_id, val) in asset_map {
                let Some(asset_id) = val.as_str() else {
                    return Ok(ProviderTradeHistory::incomplete());
                };
                if !asset_id.is_empty() {
                    sec_to_assets
                        .entry(sec_id.clone())
                        .or_default()
                        .push(asset_id.to_owned());
                }
            }
        }
        Some(_) => return Ok(ProviderTradeHistory::incomplete()),
    }
    match source_history.get("basisBySecurity") {
        None | Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::Object(basis_map)) => {
            for (sec_id, entry) in basis_map {
                if let Some(asset_id) = entry
                    .get("assetId")
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    let list = sec_to_assets.entry(sec_id.clone()).or_default();
                    if !list.iter().any(|existing| existing == asset_id) {
                        list.push(asset_id.to_owned());
                    }
                }
            }
        }
        Some(_) => return Ok(ProviderTradeHistory::incomplete()),
    }
    let unresolved_secs: Vec<&str> = paired
        .events
        .keys()
        .copied()
        .filter(|sec_id| !sec_to_assets.contains_key(*sec_id))
        .collect();
    if !unresolved_secs.is_empty() {
        let mut securities: BTreeMap<String, (String, String)> = BTreeMap::new();
        if let Some(sec_field) = source_history.get("securities") {
            let Some(sec_values) = sec_field.as_array() else {
                return Ok(ProviderTradeHistory::incomplete());
            };
            for v in sec_values {
                let Ok(s) = serde_json::from_value::<SecurityView>(v.clone()) else {
                    return Ok(ProviderTradeHistory::incomplete());
                };
                let Some(ticker) = s
                    .ticker_symbol
                    .map(|t| t.trim().to_ascii_uppercase())
                    .filter(|t| !t.is_empty())
                else {
                    continue;
                };
                let ccy = s
                    .iso_currency_code
                    .filter(|c| !c.is_empty())
                    .unwrap_or_else(|| account_currency.to_owned());
                securities.insert(s.security_id, (ticker, ccy));
            }
        }
        #[derive(QueryableByName)]
        struct AssetRow {
            #[diesel(sql_type = Text)]
            id: String,
            #[diesel(sql_type = Nullable<Text>)]
            display_code: Option<String>,
            #[diesel(sql_type = Nullable<Text>)]
            instrument_symbol: Option<String>,
            #[diesel(sql_type = Text)]
            quote_ccy: String,
        }
        let asset_rows: Vec<AssetRow> = sql_query(
            "SELECT id, display_code, instrument_symbol, quote_ccy FROM assets WHERE kind = 'INVESTMENT'",
        )
        .load(conn)
        .map_err(StorageError::from)?;
        for sec_id in unresolved_secs {
            let opaque_id = wealthfolio_connect::plaid::ingest::stable_id(connection_id, sec_id);
            let ticker_match = securities.get(sec_id);
            for asset in &asset_rows {
                let matches_opaque = asset.id == opaque_id;
                let matches_ticker = ticker_match.is_some_and(|(ticker, ccy)| {
                    let sym = asset
                        .instrument_symbol
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .or(asset.display_code.as_deref().filter(|s| !s.is_empty()));
                    sym.is_some_and(|s| s.eq_ignore_ascii_case(ticker))
                        && (ccy.is_empty() || asset.quote_ccy.eq_ignore_ascii_case(ccy))
                });
                if matches_opaque || matches_ticker {
                    let entry = sec_to_assets.entry(sec_id.to_owned()).or_default();
                    if !entry.contains(&asset.id) {
                        entry.push(asset.id.clone());
                    }
                }
            }
        }
    }
    let mut by_asset: BTreeMap<String, Vec<ProviderTradeEvent>> = BTreeMap::new();
    for (sec_id, events) in paired.events {
        if let Some(asset_ids) = sec_to_assets.get(sec_id) {
            for asset_id in asset_ids {
                by_asset
                    .entry(asset_id.clone())
                    .or_default()
                    .extend(events.iter().copied());
            }
        }
    }
    for events in by_asset.values_mut() {
        events.sort_unstable();
    }
    history.events = by_asset;
    Ok(history)
}

pub(crate) fn read_provider_security_trade_dates(
    conn: &mut SqliteConnection,
    account: &str,
) -> Result<wealthfolio_core::portfolio::statements::ProviderTradeHistory> {
    #[derive(QueryableByName)]
    struct PlaidBody {
        #[diesel(sql_type = Text)]
        connection_id: String,
        #[diesel(sql_type = Text)]
        external_id: String,
        #[diesel(sql_type = Text)]
        body: String,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct MappingView {
        #[serde(default)]
        currency: String,
        #[serde(default)]
        source_history: serde_json::Value,
    }
    let Some(row) = sql_query(
        "SELECT connection_id, external_id, body FROM plaid_accounts WHERE account_id = ?",
    )
    .bind::<Text, _>(account)
    .get_result::<PlaidBody>(conn)
    .optional()
    .map_err(StorageError::from)?
    else {
        return Ok(wealthfolio_core::portfolio::statements::ProviderTradeHistory::default());
    };
    let Ok(mapping) = serde_json::from_str::<MappingView>(&row.body) else {
        return Ok(wealthfolio_core::portfolio::statements::ProviderTradeHistory::incomplete());
    };
    extract_provider_security_trade_dates(
        conn,
        &row.connection_id,
        &row.external_id,
        &mapping.currency,
        &mapping.source_history,
    )
}

fn decode_observed_snapshot_row(
    row: crate::portfolio::snapshot::AccountStateSnapshotDB,
) -> std::result::Result<wealthfolio_core::portfolio::snapshot::AccountStateSnapshot, StorageError>
{
    use std::collections::HashMap;
    use wealthfolio_core::portfolio::snapshot::{AccountStateSnapshot, Position};

    if !matches!(
        row.source.as_str(),
        "BROKER_IMPORTED" | "MANUAL_ENTRY" | "CSV_IMPORT"
    ) {
        return Err(StorageError::SerializationError(
            "Invalid observed snapshot source".into(),
        ));
    }
    serde_json::from_str::<HashMap<String, Position>>(&row.positions).map_err(|_| {
        StorageError::SerializationError("Invalid observed snapshot positions".into())
    })?;
    AccountStateSnapshot::try_from(row)
        .map_err(|_| StorageError::SerializationError("Invalid observed snapshot row".into()))
}

fn read_observed_snapshots(
    conn: &mut SqliteConnection,
    account: &str,
) -> Result<Vec<wealthfolio_core::portfolio::snapshot::AccountStateSnapshot>> {
    use crate::portfolio::snapshot::AccountStateSnapshotDB;
    use crate::schema::holdings_snapshots;

    let rows: Vec<AccountStateSnapshotDB> = holdings_snapshots::table
        .filter(holdings_snapshots::account_id.eq(account))
        .filter(holdings_snapshots::source.ne("CALCULATED"))
        .order(holdings_snapshots::snapshot_date.asc())
        .load(conn)
        .map_err(StorageError::from)?;
    rows.into_iter()
        .map(|row| decode_observed_snapshot_row(row).map_err(Into::into))
        .collect()
}

fn read_observed_snapshots_for_basis(
    conn: &mut SqliteConnection,
    account: &str,
) -> Result<Option<Vec<wealthfolio_core::portfolio::snapshot::AccountStateSnapshot>>> {
    use crate::portfolio::snapshot::AccountStateSnapshotDB;
    use crate::schema::holdings_snapshots;

    let rows: Vec<AccountStateSnapshotDB> = holdings_snapshots::table
        .filter(holdings_snapshots::account_id.eq(account))
        .filter(holdings_snapshots::source.ne("CALCULATED"))
        .order(holdings_snapshots::snapshot_date.asc())
        .load(conn)
        .map_err(StorageError::from)?;
    let mut snapshots = Vec::with_capacity(rows.len());
    for row in rows {
        let Ok(decoded) = decode_observed_snapshot_row(row) else {
            return Ok(None);
        };
        snapshots.push(decoded);
    }
    Ok(Some(snapshots))
}

fn earliest_affected_broker_date(
    conn: &mut SqliteConnection,
    account: &str,
) -> Result<Option<NaiveDate>> {
    use crate::schema::holdings_snapshots;

    let rows: Vec<(String, String)> = holdings_snapshots::table
        .filter(holdings_snapshots::account_id.eq(account))
        .filter(holdings_snapshots::source.ne("CALCULATED"))
        .select((
            holdings_snapshots::snapshot_date,
            holdings_snapshots::source,
        ))
        .load(conn)
        .map_err(StorageError::from)?;
    let mut earliest: Option<NaiveDate> = None;
    for (raw_date, raw_source) in rows {
        if matches!(raw_source.as_str(), "MANUAL_ENTRY" | "CSV_IMPORT") {
            continue;
        }
        let date = NaiveDate::parse_from_str(&raw_date, "%Y-%m-%d")
            .unwrap_or(wealthfolio_core::portfolio::projection::GENESIS);
        earliest = Some(earliest.map_or(date, |cur| cur.min(date)));
    }
    Ok(earliest)
}

fn min_opt_date(a: Option<NaiveDate>, b: Option<NaiveDate>) -> Option<NaiveDate> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

#[derive(PartialEq, Eq)]
struct EnrichedPositionBasis {
    key: String,
    asset_id: String,
    total_cost_basis: rust_decimal::Decimal,
    average_cost: rust_decimal::Decimal,
    cost_basis_account: Option<rust_decimal::Decimal>,
    cost_basis_base: Option<rust_decimal::Decimal>,
    cost_basis_status: Option<wealthfolio_core::portfolio::economic_events::BasisStatus>,
}

type AccountBrokerBasisState = BTreeMap<NaiveDate, Vec<EnrichedPositionBasis>>;

enum CapturedBasisState {
    NoStatements,
    Unverifiable(Option<NaiveDate>),
    SnapshotBasis(AccountBrokerBasisState),
}

fn compute_broker_basis_state_with_trades(
    conn: &mut SqliteConnection,
    account: &str,
    stmts: &[StoredDailyStatement],
    provider_trades: &wealthfolio_core::portfolio::statements::ProviderTradeHistory,
) -> Result<CapturedBasisState> {
    use wealthfolio_core::portfolio::snapshot::SnapshotSource;

    let Some(mut snapshots) = read_observed_snapshots_for_basis(conn, account)? else {
        return Ok(CapturedBasisState::Unverifiable(
            earliest_affected_broker_date(conn, account)?,
        ));
    };
    let asset_ids: std::collections::BTreeSet<String> = snapshots
        .iter()
        .flat_map(|s| s.positions.values().map(|p| p.asset_id.clone()))
        .collect();
    let splits = if asset_ids.is_empty() {
        Vec::new()
    } else {
        let asset_vec: Vec<String> = asset_ids.into_iter().collect();
        let mut split_rows = Vec::new();
        for chunk in crate::utils::chunk_for_sqlite(&asset_vec) {
            let rows: Vec<crate::activities::ActivityDB> = crate::schema::activities::table
                .filter(crate::schema::activities::asset_id.eq_any(chunk))
                .filter(
                    diesel::dsl::sql::<diesel::sql_types::Text>(
                        &crate::activities::effective_type_sql("activities"),
                    )
                    .eq("SPLIT"),
                )
                .load(conn)
                .map_err(StorageError::from)?;
            split_rows.extend(rows);
        }
        let activities: Vec<wealthfolio_core::activities::Activity> =
            split_rows.into_iter().map(Into::into).collect();
        let timezone: String = crate::schema::app_settings::table
            .filter(
                crate::schema::app_settings::setting_key
                    .eq(wealthfolio_core::settings::SETTING_TIMEZONE),
            )
            .select(crate::schema::app_settings::setting_value)
            .first::<String>(conn)
            .optional()
            .map_err(StorageError::from)?
            .unwrap_or_default();
        wealthfolio_core::portfolio::statements::daily::split_events_for_activities(
            &activities,
            &timezone,
        )
    };
    wealthfolio_core::portfolio::statements::daily::apply_statement_basis_to_snapshots(
        &mut snapshots,
        stmts,
        &splits,
        provider_trades,
    );
    let mut state: AccountBrokerBasisState = BTreeMap::new();
    for s in snapshots {
        if s.source != SnapshotSource::BrokerImported {
            continue;
        }
        let entry = state.entry(s.snapshot_date).or_default();
        for (key, pos) in s.positions {
            entry.push(EnrichedPositionBasis {
                key,
                asset_id: pos.asset_id,
                total_cost_basis: pos.total_cost_basis,
                average_cost: pos.average_cost,
                cost_basis_account: pos.cost_basis_account,
                cost_basis_base: pos.cost_basis_base,
                cost_basis_status: pos.cost_basis_status,
            });
        }
    }
    for positions in state.values_mut() {
        positions.sort_by(|a, b| (&a.key, &a.asset_id).cmp(&(&b.key, &b.asset_id)));
    }
    Ok(CapturedBasisState::SnapshotBasis(state))
}

fn earliest_changed_broker_date(
    before: &CapturedBasisState,
    after: &CapturedBasisState,
) -> Option<NaiveDate> {
    match (before, after) {
        (CapturedBasisState::NoStatements, _) | (_, CapturedBasisState::NoStatements) => None,
        (CapturedBasisState::Unverifiable(b), CapturedBasisState::Unverifiable(a)) => {
            min_opt_date(*b, *a)
        }
        (CapturedBasisState::Unverifiable(u), CapturedBasisState::SnapshotBasis(s))
        | (CapturedBasisState::SnapshotBasis(s), CapturedBasisState::Unverifiable(u)) => {
            min_opt_date(*u, s.keys().next().copied())
        }
        (CapturedBasisState::SnapshotBasis(b), CapturedBasisState::SnapshotBasis(a)) => {
            let changed_in_a = a
                .iter()
                .find(|(date, after_pos)| b.get(date) != Some(after_pos))
                .map(|(date, _)| *date);
            let changed_in_b = b
                .iter()
                .find(|(date, before_pos)| a.get(date) != Some(before_pos))
                .map(|(date, _)| *date);
            min_opt_date(changed_in_a, changed_in_b)
        }
    }
}

fn capture_account_basis_state(
    conn: &mut SqliteConnection,
    account: &str,
) -> Result<CapturedBasisState> {
    match read_daily_for_basis(conn, account)? {
        DailyStatementsForBasis::Empty => Ok(CapturedBasisState::NoStatements),
        DailyStatementsForBasis::Unverifiable => Ok(CapturedBasisState::Unverifiable(
            earliest_affected_broker_date(conn, account)?,
        )),
        DailyStatementsForBasis::Decoded(stmts) => {
            let trades = read_provider_security_trade_dates(conn, account)?;
            compute_broker_basis_state_with_trades(conn, account, &stmts, &trades)
        }
    }
}

/// Captures statement-enriched broker snapshot basis before a snapshot table
/// mutation and marks `projection_state` dirty back to the earliest affected
/// `BrokerImported` snapshot when a later supporting observation changes.
#[derive(Default)]
pub(crate) struct StatementBasisInvalidationGuard {
    before_by_account: BTreeMap<String, CapturedBasisState>,
}

impl StatementBasisInvalidationGuard {
    pub(crate) fn capture_before(
        &mut self,
        conn: &mut SqliteConnection,
        account: &str,
    ) -> Result<()> {
        if account.is_empty() || self.before_by_account.contains_key(account) {
            return Ok(());
        }
        let state = capture_account_basis_state(conn, account)?;
        self.before_by_account.insert(account.to_owned(), state);
        Ok(())
    }

    pub(crate) fn invalidate_changed(self, conn: &mut SqliteConnection) -> Result<()> {
        for (account, before) in self.before_by_account {
            if matches!(before, CapturedBasisState::NoStatements) {
                continue;
            }
            let after = capture_account_basis_state(conn, &account)?;
            if let Some(dirty_from) = earliest_changed_broker_date(&before, &after) {
                crate::portfolio::projection::invalidate_account_tx(conn, &account, dirty_from)?;
            }
        }
        Ok(())
    }
}

/// Atomically invalidates `projection_state` back to the earliest changed
/// provider trade date or affected `BrokerImported` snapshot when a `HOLDINGS`-mode
/// Plaid commit updates stored investment transactions on an account with reviewed
/// daily statements.
pub(crate) fn invalidate_holdings_if_provider_trades_changed_tx(
    conn: &mut SqliteConnection,
    mapping: &wealthfolio_connect::plaid::state::Mapping,
) -> Result<()> {
    let stmts_state = read_daily_for_basis(conn, &mapping.account_id)?;
    if matches!(stmts_state, DailyStatementsForBasis::Empty) {
        return Ok(());
    }
    let before = read_provider_security_trade_dates(conn, &mapping.account_id)?;
    let after = extract_provider_security_trade_dates(
        conn,
        &mapping.connection_id,
        &mapping.external_id,
        &mapping.currency,
        &mapping.source_history,
    )?;
    if before == after {
        return Ok(());
    }
    let mut earliest: Option<NaiveDate> = None;
    let (before_basis, after_basis) = match &stmts_state {
        DailyStatementsForBasis::Empty => unreachable!(),
        DailyStatementsForBasis::Unverifiable => {
            let affected = earliest_affected_broker_date(conn, &mapping.account_id)?;
            (
                CapturedBasisState::Unverifiable(affected),
                CapturedBasisState::Unverifiable(affected),
            )
        }
        DailyStatementsForBasis::Decoded(stmts) => (
            compute_broker_basis_state_with_trades(conn, &mapping.account_id, stmts, &before)?,
            compute_broker_basis_state_with_trades(conn, &mapping.account_id, stmts, &after)?,
        ),
    };
    if let Some(broker_dirty) = earliest_changed_broker_date(&before_basis, &after_basis) {
        earliest = Some(broker_dirty);
    }
    let empty = Vec::new();
    for asset_id in before.events.keys().chain(after.events.keys()) {
        let old = before.events.get(asset_id).unwrap_or(&empty);
        let new = after.events.get(asset_id).unwrap_or(&empty);
        if old == new {
            continue;
        }
        for ev in old.iter().chain(new.iter()) {
            let mut consider = |d: NaiveDate| {
                earliest = Some(earliest.map_or(d, |cur| cur.min(d)));
            };
            consider(ev.date);
            if let Some(cancel_date) = ev.cancelled_on {
                consider(cancel_date);
            }
        }
    }
    if before.verification_incomplete
        || after.verification_incomplete
        || before.coverage_from != after.coverage_from
    {
        earliest = Some(wealthfolio_core::portfolio::projection::GENESIS);
    }
    if before.coverage_through != after.coverage_through {
        let cov_dirty = match (before.coverage_through, after.coverage_through) {
            (Some(a), Some(b)) => a
                .min(b)
                .succ_opt()
                .unwrap_or(wealthfolio_core::portfolio::projection::GENESIS),
            _ => wealthfolio_core::portfolio::projection::GENESIS,
        };
        earliest = Some(earliest.map_or(cov_dirty, |cur| cur.min(cov_dirty)));
    }
    let dirty_from = earliest.unwrap_or(wealthfolio_core::portfolio::projection::GENESIS);
    crate::portfolio::projection::invalidate_account_tx(conn, &mapping.account_id, dirty_from)
}

#[derive(QueryableByName)]
struct StatementYearCountRow {
    #[diesel(sql_type = Text)]
    account_id: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    statement_count: i64,
}

#[async_trait]
impl StatementRepository for StatementStore {
    fn list(&self, account: &str) -> Result<Vec<StoredStatement>> {
        let mut conn = get_connection(&self.pool)?;
        read(&mut conn, account)
    }

    fn count_by_year(
        &self,
        account_ids: &[String],
        year: i32,
    ) -> Result<std::collections::HashMap<String, usize>> {
        if account_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let mut conn = get_connection(&self.pool)?;
        let start = format!("{year:04}-01-01");
        let end = format!("{year:04}-12-31");
        let mut counts = std::collections::HashMap::new();
        for chunk in crate::utils::chunk_for_sqlite(account_ids) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let query = format!(
                "SELECT account_id, COUNT(*) AS statement_count \
                 FROM account_statement_history \
                 WHERE account_id IN ({placeholders}) \
                   AND statement_date >= ? AND statement_date <= ? \
                 GROUP BY account_id"
            );
            let mut bound = sql_query(query).into_boxed();
            for account_id in chunk {
                bound = bound.bind::<Text, _>(account_id);
            }
            let rows = bound
                .bind::<Text, _>(&start)
                .bind::<Text, _>(&end)
                .load::<StatementYearCountRow>(&mut conn)
                .map_err(StorageError::from)?;
            for row in rows {
                if row.statement_count > 0 {
                    counts.insert(row.account_id, row.statement_count as usize);
                }
            }
        }
        Ok(counts)
    }

    fn list_daily(&self, account: &str) -> Result<Vec<StoredDailyStatement>> {
        let mut conn = get_connection(&self.pool)?;
        read_daily(&mut conn, account)
    }

    fn provider_security_trade_dates(
        &self,
        account: &str,
    ) -> Result<wealthfolio_core::portfolio::statements::ProviderTradeHistory> {
        let mut conn = get_connection(&self.pool)?;
        read_provider_security_trade_dates(&mut conn, account)
    }

    fn observed_snapshots(
        &self,
        account: &str,
    ) -> Result<Vec<wealthfolio_core::portfolio::snapshot::AccountStateSnapshot>> {
        let mut conn = get_connection(&self.pool)?;
        read_observed_snapshots(&mut conn, account)
    }
    async fn save_daily(&self, account: &str, writes: Vec<DailyStatementWrite>) -> Result<()> {
        if writes.is_empty()
            || writes.len() > wealthfolio_core::portfolio::statements::MAX_STATEMENT_BATCH
        {
            return Err(invalid());
        }
        let account = account.to_owned();
        self.writer.exec_tx(move |tx| {
            let conn = tx.conn();
            #[derive(QueryableByName)]
            struct Scope {
                #[diesel(sql_type=Text)] account_type: String,
                #[diesel(sql_type=Text)] currency: String,
                #[diesel(sql_type=Text)] tracking_mode: String,
            }
            let scope = sql_query("SELECT account_type,currency,tracking_mode FROM accounts WHERE id=?")
                .bind::<Text,_>(&account).get_result::<Scope>(conn).map_err(StorageError::from)?;
            if scope.account_type != wealthfolio_core::accounts::account_types::SECURITIES { return Err(invalid()); }
            let mut dates = std::collections::BTreeSet::new();
            for write in &writes {
                // Structural/account checks belong at persistence too. The service
                // owns user-timezone future-date admission, not SQLite's clock.
                write.validate(&scope.currency, NaiveDate::MAX)?;
                if !dates.insert(write.interval.through) { return Err(invalid()); }
            }
            let existing = read_daily(conn, &account)?;
            reviewed_chain(&existing, &writes)?;
            let snapshots = read(conn, &account)?;
            let review_order = existing.iter().map(|s| s.review_order).max().unwrap_or(0)
                .checked_add(1).ok_or_else(invalid)?;
            let mut mutated = false;
            for write in writes {
                let previous = existing.iter().find(|s| s.interval.through == write.interval.through);
                let snapshot = snapshots.iter().find(|s|s.snapshot.date == write.snapshot.date);
                let same_interval = previous.is_some_and(|s| write.matches_daily(s));
                let same_snapshot =
                    snapshot.is_some_and(|s| s.snapshot.same_financial_content(&write.snapshot));
                if same_interval && same_snapshot { continue; }
                if previous.map(|s| &s.revision) != write.expected_revision.as_ref()
                    || snapshot.map(|s| &s.revision) != write.expected_snapshot_revision.as_ref() { return Err(invalid()); }
                mutated = true;
                // Monthly review and delete also protect the attached daily facts.
                write_snapshot(conn, &account, &write.snapshot)?;
                let payload = DailyPayload {
                    version: 1,
                    closing_asset_ids: write.closing_asset_ids,
                    interval: write.interval,
                };
                let matches = SecurityReview {
                    revision: uuid::Uuid::new_v4().to_string(),
                    activity_security_keys: write.activity_security_keys.unwrap_or_default(),
                    review_order,
                };
                sql_query("INSERT INTO account_statement_reconstructions(account_id,through_date,payload,revision,security_review) VALUES (?,?,?,?,?) ON CONFLICT(account_id,through_date) DO UPDATE SET payload=excluded.payload,revision=excluded.revision,security_review=excluded.security_review")
                    .bind::<Text,_>(&account).bind::<Text,_>(payload.interval.through.to_string())
                    .bind::<Text,_>(serde_json::to_string(&payload)?).bind::<Text,_>(&matches.revision)
                    .bind::<Text,_>(serde_json::to_string(&matches)?)
                    .execute(conn).map_err(StorageError::from)?;
            }
            if mutated && scope.tracking_mode == "HOLDINGS" {
                crate::portfolio::projection::invalidate_account_tx(
                    conn,
                    &account,
                    wealthfolio_core::portfolio::projection::GENESIS,
                )?;
            }
            Ok(())
        }).await
    }
    async fn save(&self, account: &str, writes: Vec<StatementWrite>) -> Result<()> {
        let account = account.to_owned();
        self.writer
            .exec_tx(move |tx| {
                let conn = tx.conn();
                let existing = read(conn, &account)?;
                let mut mutated = false;
                for write in writes {
                    let old = existing
                        .iter()
                        .find(|s| s.snapshot.date == write.snapshot.date);
                    if old.is_some_and(|s| s.snapshot.same_financial_content(&write.snapshot)) {
                        continue;
                    }
                    if old.map(|s| &s.revision) != write.expected_revision.as_ref() {
                        return Err(invalid());
                    }
                    mutated = true;
                    write_snapshot(conn, &account, &write.snapshot)?;
                }
                if mutated {
                    invalidate_holdings_projection_tx(conn, &account)?;
                }
                Ok(())
            })
            .await
    }

    async fn delete(&self, account: &str, date: NaiveDate, revision: &str) -> Result<()> {
        let (account, revision) = (account.to_owned(), revision.to_owned());
        self.writer
            .exec_tx(move |tx| {
                let conn = tx.conn();
                let count = sql_query(
                    "DELETE FROM account_statement_history \
                 WHERE account_id=? AND statement_date=? AND revision=?",
                )
                .bind::<Text, _>(&account)
                .bind::<Text, _>(date.to_string())
                .bind::<Text, _>(revision)
                .execute(conn)
                .map_err(StorageError::from)?;
                if count != 1 {
                    return Err(invalid());
                }
                invalidate_holdings_projection_tx(conn, &account)?;
                Ok(())
            })
            .await
    }
}

fn invalidate_holdings_projection_tx(conn: &mut SqliteConnection, account: &str) -> Result<()> {
    #[derive(QueryableByName)]
    struct Mode {
        #[diesel(sql_type = Text)]
        tracking_mode: String,
    }
    let is_holdings = sql_query("SELECT tracking_mode FROM accounts WHERE id = ?")
        .bind::<Text, _>(account)
        .get_result::<Mode>(conn)
        .optional()
        .map_err(StorageError::from)?
        .is_some_and(|row| row.tracking_mode == "HOLDINGS");
    if is_holdings {
        crate::portfolio::projection::invalidate_account_tx(
            conn,
            account,
            wealthfolio_core::portfolio::projection::GENESIS,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::RwLock;
    use wealthfolio_core::portfolio::{
        statements::{StatementService, StatementSnapshot},
        valuation::ValuationRepositoryTrait,
    };
    struct Rig {
        _dir: tempfile::TempDir,
        pool: Arc<DbPool>,
        writer: WriteHandle,
        store: Arc<StatementStore>,
        service: StatementService,
    }
    impl Rig {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db =
                crate::db::DbAccess::plaintext(dir.path().join("statements.db").to_string_lossy());
            db.prepare().unwrap();
            db.run_migrations().unwrap();
            let pool = db.create_pool().unwrap();
            let writer = crate::db::write_actor::spawn_writer(pool.as_ref().clone()).unwrap();
            sql_query("INSERT INTO accounts(id,name,account_type,currency,is_default,is_active,tracking_mode,is_archived) VALUES ('a','Synthetic','SECURITIES','USD',0,1,'TRANSACTIONS',0),('b','Other','SECURITIES','USD',0,1,'HOLDINGS',0)").execute(&mut pool.get().unwrap()).unwrap();
            let store = Arc::new(StatementStore::new(pool.clone(), writer.clone()));
            let service = StatementService::new(
                store.clone(),
                Arc::new(crate::accounts::AccountRepository::new(
                    pool.clone(),
                    writer.clone(),
                )),
                Arc::new(crate::portfolio::valuation::ValuationRepository::new(
                    pool.clone(),
                    writer.clone(),
                )),
                Arc::new(RwLock::new("UTC".into())),
            );
            Self {
                _dir: dir,
                pool,
                writer,
                store,
                service,
            }
        }
    }
    fn write(date: &str, total: &str) -> StatementWrite {
        StatementWrite {
            snapshot: StatementSnapshot {
                date: date.parse().unwrap(),
                currency: "USD".into(),
                total_value: total.into(),
                cash_value: None,
                positions: vec![],
                positions_complete: false,
                format: "merrill-cma-monthly-v1".into(),
                source_page: 1,
            },
            expected_revision: None,
        }
    }
    #[tokio::test]
    async fn statement_history_review_is_required_and_account_scope_is_enforced() {
        let r = Rig::new();
        assert!(r
            .service
            .save("a".into(), vec![write("2025-01-31", "100")], false)
            .await
            .is_err());
        assert!(r
            .service
            .save("missing".into(), vec![write("2025-01-31", "100")], true)
            .await
            .is_err());
        assert!(r
            .service
            .save("a".into(), vec![write("2025-01-31", "100"); 6], true)
            .await
            .is_err());
        assert!(r
            .service
            .save("a".into(), vec![write("2025-01-31", "100"); 2], true)
            .await
            .is_err());
        r.service
            .save("a".into(), vec![write("2025-01-31", "100")], true)
            .await
            .unwrap();
        assert!(r.service.list("b".into()).await.unwrap().is_empty());
        let saved = r.store.list("a").unwrap();
        assert!(r
            .service
            .delete(
                "b".into(),
                saved[0].snapshot.date,
                saved[0].revision.clone()
            )
            .await
            .is_err());
        let other = Rig::new();
        assert!(other.store.list("a").unwrap().is_empty());
    }
    #[tokio::test]
    async fn statement_history_batch_conflicts_roll_back_and_replacements_compare_revision() {
        let r = Rig::new();
        let original = write("2025-01-31", "100");
        r.service
            .save("a".into(), vec![original.clone()], true)
            .await
            .unwrap();
        let saved = r.store.list("a").unwrap();
        r.service
            .save("a".into(), vec![original], true)
            .await
            .unwrap();
        assert_eq!(r.store.list("a").unwrap()[0].revision, saved[0].revision);
        assert!(r
            .service
            .save(
                "a".into(),
                vec![write("2025-02-28", "110"), write("2025-01-31", "90")],
                true
            )
            .await
            .is_err());
        assert_eq!(r.store.list("a").unwrap().len(), 1);
        let mut updated = write("2025-01-31", "90");
        updated.expected_revision = Some(saved[0].revision.clone());
        r.service
            .save("a".into(), vec![updated.clone()], true)
            .await
            .unwrap();
        let current = r.store.list("a").unwrap();
        assert_eq!(current[0].snapshot.total_value, "90");
        assert!(r
            .service
            .delete(
                "a".into(),
                saved[0].snapshot.date,
                saved[0].revision.clone()
            )
            .await
            .is_err());
        updated.snapshot.total_value = "80".into();
        assert!(r
            .service
            .save("a".into(), vec![updated], true)
            .await
            .is_err());
        r.service
            .delete(
                "a".into(),
                current[0].snapshot.date,
                current[0].revision.clone(),
            )
            .await
            .unwrap();
        assert!(r.store.list("a").unwrap().is_empty());
    }
    #[tokio::test]
    async fn statement_history_survives_projection_replacement_and_account_deletion_cascades() {
        let r = Rig::new();
        r.service
            .save(
                "a".into(),
                vec![write("2025-01-31", "100"), write("2025-02-28", "110")],
                true,
            )
            .await
            .unwrap();
        let valuations =
            crate::portfolio::valuation::ValuationRepository::new(r.pool.clone(), r.writer.clone());
        valuations
            .replace_valuations_for_account("a", None, &[])
            .await
            .unwrap();
        let points = r
            .service
            .history(
                "a".into(),
                "2025-01-01".parse().unwrap(),
                "2025-03-01".parse().unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(points.first().unwrap().value, "100");
        assert_eq!(points.last().unwrap().value, "110");
        sql_query("UPDATE accounts SET tracking_mode='HOLDINGS' WHERE id='a'")
            .execute(&mut r.pool.get().unwrap())
            .unwrap();
        assert_eq!(r.store.list("a").unwrap().len(), 2);
        r.writer
            .exec(|conn| {
                sql_query("DELETE FROM accounts WHERE id='a'")
                    .execute(conn)
                    .map_err(StorageError::from)?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(r.store.list("a").unwrap().is_empty());
    }
    mod daily_tests;

    #[tokio::test]
    async fn statement_count_by_year_groups_within_calendar_year() {
        let r = Rig::new();
        r.service
            .save(
                "a".into(),
                vec![
                    write("2024-12-31", "95"),
                    write("2025-01-31", "100"),
                    write("2025-06-30", "110"),
                ],
                true,
            )
            .await
            .unwrap();
        let counts_2025 = r.store.count_by_year(&["a".into()], 2025).unwrap();
        assert_eq!(counts_2025.get("a"), Some(&2));
        let counts_2024 = r.store.count_by_year(&["a".into()], 2024).unwrap();
        assert_eq!(counts_2024.get("a"), Some(&1));
        let counts_2026 = r.store.count_by_year(&["a".into()], 2026).unwrap();
        assert!(counts_2026.is_empty());
    }

    #[tokio::test]
    async fn statement_history_accepts_multi_month_bundle_batches_up_to_limit() {
        let r = Rig::new();
        let mut writes: Vec<_> = (1..=12)
            .map(|month| {
                let mut w = write(&format!("2024-{month:02}-28"), "100");
                w.snapshot.source_page = (month as u32) * 15;
                w
            })
            .collect();
        r.service
            .save("a".into(), writes.clone(), true)
            .await
            .unwrap();
        assert_eq!(r.store.list("a").unwrap().len(), 12);
        let too_many: Vec<_> = (0..=wealthfolio_core::portfolio::statements::MAX_STATEMENT_BATCH)
            .map(|i| {
                let date = NaiveDate::from_ymd_opt(2020, 1, 1)
                    .unwrap()
                    .checked_add_days(chrono::Days::new(i as u64))
                    .unwrap();
                write(&date.to_string(), "100")
            })
            .collect();
        assert!(r.service.save("a".into(), too_many, true).await.is_err());
        writes[0].snapshot.source_page =
            wealthfolio_core::portfolio::statements::MAX_STATEMENT_SOURCE_PAGE + 1;
        assert!(r
            .service
            .save("a".into(), vec![writes[0].clone()], true)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn holdings_statement_mutations_atomically_mark_projection_dirty() {
        use std::future::Future;
        use wealthfolio_core::portfolio::projection::{MarkerScope, ProjectionStoreTrait, GENESIS};
        use wealthfolio_core::portfolio::statements::reconstruction::{
            StatementDatePolicy, StatementInterval,
        };

        let r = Rig::new();
        let projections =
            crate::portfolio::projection::ProjectionStore::new(r.pool.clone(), r.writer.clone());
        let clear_markers = || {
            sql_query("DELETE FROM projection_state")
                .execute(&mut r.pool.get().unwrap())
                .unwrap();
        };
        let has_dirty_account =
            |id: &str| {
                projections.pending_markers().unwrap().iter().any(|m| {
                    m.scope == MarkerScope::Account(id.to_owned()) && m.dirty_from == GENESIS
                })
            };

        // TRANSACTIONS-mode account ('a') statement save/delete does not dirty projection_state.
        clear_markers();
        r.service
            .save("a".into(), vec![write("2025-01-31", "100")], true)
            .await
            .unwrap();
        assert!(!has_dirty_account("a"));
        let saved_a = r.store.list("a").unwrap().remove(0);
        r.service
            .delete("a".into(), saved_a.snapshot.date, saved_a.revision)
            .await
            .unwrap();
        assert!(!has_dirty_account("a"));

        // HOLDINGS-mode account ('b'): save_daily marks dirty in the same SQLite transaction,
        // even when the caller drops the future right after queueing the write.
        clear_markers();
        let mut daily_snapshot = write("2025-01-31", "100").snapshot;
        daily_snapshot.cash_value = Some("100".into());
        daily_snapshot.positions_complete = true;
        let daily_write = DailyStatementWrite {
            closing_asset_ids: vec![],
            activity_security_keys: None,
            expected_revision: None,
            expected_snapshot_revision: None,
            snapshot: daily_snapshot,
            interval: StatementInterval {
                date_policy: StatementDatePolicy::Settlement,
                from: "2025-01-01".parse().unwrap(),
                through: "2025-01-31".parse().unwrap(),
                currency: "USD".into(),
                opening_cash: "100".into(),
                closing_cash: "100".into(),
                closing_value: "100".into(),
                closing_positions_complete: true,
                activity_complete: true,
                closing_positions: vec![],
                events: vec![],
            },
        };
        {
            let fut = r.store.save_daily("b", vec![daily_write.clone()]);
            let mut fut = std::pin::pin!(fut);
            let waker = std::task::Waker::noop();
            let mut cx = std::task::Context::from_waker(waker);
            if let std::task::Poll::Ready(res) = fut.as_mut().poll(&mut cx) {
                res.unwrap();
            }
            // Drop `fut` immediately: writer commits queued tx, and projection_state must be dirty.
        }
        // Barrier on the writer actor ensures the queued transaction has committed.
        r.writer.exec(|_| Ok(())).await.unwrap();
        assert_eq!(r.store.list_daily("b").unwrap().len(), 1);
        assert!(has_dirty_account("b"));

        // Idempotent re-save with identical content does not re-dirty projection_state.
        clear_markers();
        r.store.save_daily("b", vec![daily_write]).await.unwrap();
        assert!(!has_dirty_account("b"));

        // Monthly replacement on 'b' atomically marks projection_state dirty.
        let saved_b = r.store.list("b").unwrap().remove(0);
        let mut replacement = write("2025-01-31", "120");
        replacement.expected_revision = Some(saved_b.revision);
        r.store.save("b", vec![replacement]).await.unwrap();
        assert!(has_dirty_account("b"));

        // Delete on 'b' atomically marks projection_state dirty.
        clear_markers();
        let updated_b = r.store.list("b").unwrap().remove(0);
        r.store
            .delete("b", updated_b.snapshot.date, &updated_b.revision)
            .await
            .unwrap();
        assert!(has_dirty_account("b"));
    }

    #[tokio::test]
    async fn later_supporting_snapshot_mutations_invalidate_earlier_broker_valuations() {
        use rust_decimal::Decimal;
        use std::collections::HashMap;
        use wealthfolio_core::{
            fx::FxService,
            portfolio::{
                coordinator::{
                    CoordinatorDeps, FactSources, PortfolioCoordinator, PortfolioJobRequest,
                    SilentObserver, WindowCadence,
                },
                economic_events::BasisStatus,
                projection::{MarkerScope, ProjectionStoreTrait},
                snapshot::{
                    AccountStateSnapshot, Position, SnapshotRepositoryTrait, SnapshotService,
                    SnapshotSource,
                },
                statements::{
                    reconstruction::{ClosingPosition, StatementDatePolicy, StatementInterval},
                    StatementPosition,
                },
            },
            quotes::{MarketSyncMode, Quote, QuoteService, QuoteServiceTrait, QuoteStore},
            secrets::SecretStore,
            sync::{SyncEntity, SyncOperation},
        };

        struct EmptySecrets;
        impl SecretStore for EmptySecrets {
            fn set_secret(&self, _: &str, _: &str) -> Result<()> {
                Ok(())
            }
            fn get_secret(&self, _: &str) -> Result<Option<String>> {
                Ok(None)
            }
            fn delete_secret(&self, _: &str) -> Result<()> {
                Ok(())
            }
        }

        let r = Rig::new();
        sql_query(
            "INSERT INTO assets(id,kind,display_code,instrument_symbol,instrument_type,is_active,quote_mode,quote_ccy,created_at,updated_at) \
             VALUES ('AMD','INVESTMENT','AMD','AMD','EQUITY',1,'MANUAL','USD',datetime('now'),datetime('now'))",
        )
        .execute(&mut r.pool.get().unwrap())
        .unwrap();

        let timezone = Arc::new(RwLock::new("UTC".to_string()));
        let base_currency = Arc::new(RwLock::new("USD".to_string()));
        let accounts = Arc::new(crate::accounts::AccountRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        ));
        let activities = Arc::new(crate::activities::ActivityRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        ));
        let assets = Arc::new(crate::assets::AssetRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        ));
        let market = Arc::new(crate::market_data::MarketDataRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        ));
        let quotes: Arc<dyn QuoteServiceTrait> = Arc::new(
            QuoteService::new_with_custom_provider(
                market.clone(),
                Arc::new(crate::market_data::QuoteSyncStateRepository::new(
                    r.pool.clone(),
                    r.writer.clone(),
                )),
                market.clone(),
                assets.clone(),
                activities.clone(),
                Arc::new(EmptySecrets),
                None,
            )
            .await
            .unwrap(),
        );
        let fx_rates = Arc::new(crate::fx::FxRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        ));
        let fx = Arc::new(FxService::new(fx_rates.clone()));
        let snapshots = Arc::new(crate::portfolio::snapshot::SnapshotRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        ));
        let projections = Arc::new(crate::portfolio::projection::ProjectionStore::new(
            r.pool.clone(),
            r.writer.clone(),
        ));
        let lots = Arc::new(crate::lots::LotsRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        ));
        let valuations = Arc::new(crate::portfolio::valuation::ValuationRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        ));
        let snapshot_service = Arc::new(
            SnapshotService::new(
                timezone.clone(),
                accounts.clone(),
                snapshots.clone(),
                activities.clone(),
            )
            .with_statement_repository(r.store.clone()),
        );
        let coordinator = PortfolioCoordinator::new(CoordinatorDeps {
            base_currency,
            timezone,
            sources: FactSources {
                accounts,
                activities,
                assets,
                quotes,
                fx_rates,
                snapshots: snapshots.clone(),
                projections: projections.clone(),
                statements: Some(r.store.clone()),
            },
            fx_service: fx,
            snapshot_service,
            projections: projections.clone(),
            lots,
            window_cadence: WindowCadence::Days(30),
        });

        let mut quote_rows = Vec::new();
        let mut day_cursor: NaiveDate = "2025-10-31".parse().unwrap();
        let end_day: NaiveDate = "2025-11-30".parse().unwrap();
        while day_cursor <= end_day {
            let ds = day_cursor.format("%Y-%m-%d").to_string();
            quote_rows.push(Quote {
                id: format!("{ds}_AMD"),
                asset_id: "AMD".into(),
                timestamp: day_cursor.and_hms_opt(16, 0, 0).unwrap().and_utc(),
                open: Decimal::from(50),
                high: Decimal::from(50),
                low: Decimal::from(50),
                close: Decimal::from(50),
                adjclose: Decimal::from(50),
                volume: Decimal::ZERO,
                currency: "USD".into(),
                data_source: "MANUAL".into(),
                created_at: chrono::Utc::now(),
                notes: None,
            });
            day_cursor = day_cursor.succ_opt().unwrap();
        }
        market.upsert_quotes(&quote_rows).await.unwrap();

        let mk_daily = |from: &str, through: &str| DailyStatementWrite {
            closing_asset_ids: vec!["AMD".into()],
            activity_security_keys: None,
            expected_revision: None,
            expected_snapshot_revision: None,
            snapshot: StatementSnapshot {
                date: through.parse().unwrap(),
                currency: "USD".into(),
                total_value: "350".into(),
                cash_value: Some("0".into()),
                positions: vec![StatementPosition {
                    label: "AMD".into(),
                    quantity: "7".into(),
                    market_value: Some("350".into()),
                    cost_basis: Some("280".into()),
                }],
                positions_complete: true,
                format: "merrill-cma-monthly-v1".into(),
                source_page: 1,
            },
            interval: StatementInterval {
                date_policy: StatementDatePolicy::Settlement,
                from: from.parse().unwrap(),
                through: through.parse().unwrap(),
                currency: "USD".into(),
                opening_cash: "0".into(),
                closing_cash: "0".into(),
                closing_value: "350".into(),
                closing_positions_complete: true,
                activity_complete: true,
                closing_positions: vec![ClosingPosition {
                    asset_id: "AMD".into(),
                    quantity: "7".into(),
                    market_value: "350".into(),
                    cost_basis: Some("280".into()),
                }],
                events: vec![],
            },
        };
        r.store
            .save_daily(
                "b",
                vec![
                    mk_daily("2025-10-01", "2025-10-31"),
                    mk_daily("2025-11-01", "2025-11-30"),
                ],
            )
            .await
            .unwrap();

        let nov24: NaiveDate = "2025-11-24".parse().unwrap();
        let nov26: NaiveDate = "2025-11-26".parse().unwrap();
        let mk_snap = |date: NaiveDate,
                       source: SnapshotSource,
                       basis: Decimal,
                       status: BasisStatus| AccountStateSnapshot {
            id: AccountStateSnapshot::stable_id("b", date),
            account_id: "b".into(),
            snapshot_date: date,
            currency: "USD".into(),
            positions: HashMap::from([(
                "AMD".into(),
                Position {
                    id: format!("b_AMD_{date}"),
                    account_id: "b".into(),
                    asset_id: "AMD".into(),
                    quantity: Decimal::from(7),
                    average_cost: basis.checked_div(Decimal::from(7)).unwrap_or(Decimal::ZERO),
                    total_cost_basis: basis,
                    currency: "USD".into(),
                    cost_basis_account: (status == BasisStatus::Complete).then_some(basis),
                    cost_basis_status: Some(status),
                    ..Default::default()
                },
            )]),
            cost_basis: basis,
            source,
            ..Default::default()
        };

        snapshots
            .save_or_update_snapshot(&mk_snap(
                nov24,
                SnapshotSource::BrokerImported,
                Decimal::ZERO,
                BasisStatus::Unknown,
            ))
            .await
            .unwrap();

        let run_incremental = || async {
            coordinator
                .run_job(
                    PortfolioJobRequest {
                        account_ids: Some(vec!["b".into()]),
                        market_sync: MarketSyncMode::None,
                        force_full: false,
                    },
                    &SilentObserver,
                )
                .await
                .unwrap();
        };
        let dirty_for_b = || {
            projections
                .pending_markers()
                .unwrap()
                .into_iter()
                .find(|m| m.scope == MarkerScope::Account("b".into()))
                .map(|m| m.dirty_from)
        };
        let nov24_valuation = || {
            valuations
                .get_historical_valuations("b", Some(nov24), Some(nov24))
                .unwrap()
                .remove(0)
        };

        run_incremental().await;
        let v0 = nov24_valuation();
        assert_eq!(v0.basis_status, BasisStatus::Complete);
        assert_eq!(v0.cost_basis, Decimal::from(280));
        assert_eq!(dirty_for_b(), None);

        // 1. Adding a Nov 26 manual observation with conflicting known basis ($210)
        // invalidates back to Nov 24 so incremental rebuild withholds Nov 24 basis.
        let manual_conflict = mk_snap(
            nov26,
            SnapshotSource::ManualEntry,
            Decimal::from(210),
            BasisStatus::Complete,
        );
        snapshots
            .save_or_update_snapshot(&manual_conflict)
            .await
            .unwrap();
        assert_eq!(dirty_for_b(), Some(nov24));
        run_incremental().await;
        assert_eq!(nov24_valuation().basis_status, BasisStatus::Unknown);

        // 2. Updating the Nov 26 manual observation to $280 invalidates back to Nov 24
        // and restores Complete $280 basis on Nov 24.
        let manual_match = mk_snap(
            nov26,
            SnapshotSource::ManualEntry,
            Decimal::from(280),
            BasisStatus::Complete,
        );
        snapshots
            .save_snapshots(std::slice::from_ref(&manual_match))
            .await
            .unwrap();
        assert_eq!(dirty_for_b(), Some(nov24));
        run_incremental().await;
        let v_restored = nov24_valuation();
        assert_eq!(v_restored.basis_status, BasisStatus::Complete);
        assert_eq!(v_restored.cost_basis, Decimal::from(280));

        // 3. Re-introducing the conflict and then deleting the Nov 26 observation
        // (both via SnapshotRepository and via AppSyncRepository) invalidates Nov 24.
        snapshots
            .save_or_update_snapshot(&manual_conflict)
            .await
            .unwrap();
        run_incremental().await;
        assert_eq!(nov24_valuation().basis_status, BasisStatus::Unknown);

        snapshots
            .delete_snapshot_for_account_by_id("b", &manual_conflict.id)
            .await
            .unwrap();
        assert_eq!(dirty_for_b(), Some(nov24));
        run_incremental().await;
        assert_eq!(nov24_valuation().basis_status, BasisStatus::Complete);

        // 4. Device-sync remote upsert + delete on Nov 27 also invalidates Nov 24.
        let nov27: NaiveDate = "2025-11-27".parse().unwrap();
        let synced_conflict = mk_snap(
            nov27,
            SnapshotSource::ManualEntry,
            Decimal::from(210),
            BasisStatus::Complete,
        );
        let app_sync =
            crate::sync::app_sync::AppSyncRepository::new(r.pool.clone(), r.writer.clone());
        let db_row =
            crate::portfolio::snapshot::AccountStateSnapshotDB::from(synced_conflict.clone());
        let payload = serde_json::json!({
            "id": db_row.id,
            "account_id": db_row.account_id,
            "snapshot_date": db_row.snapshot_date,
            "currency": db_row.currency,
            "positions": db_row.positions,
            "cash_balances": db_row.cash_balances,
            "cost_basis": db_row.cost_basis,
            "net_contribution": db_row.net_contribution,
            "calculated_at": db_row.calculated_at,
            "net_contribution_base": db_row.net_contribution_base,
            "cash_total_account_currency": db_row.cash_total_account_currency,
            "cash_total_base_currency": db_row.cash_total_base_currency,
            "source": db_row.source,
        });
        let ts1 = (chrono::Utc::now() + chrono::TimeDelta::seconds(10)).to_rfc3339();
        let ts2 = (chrono::Utc::now() + chrono::TimeDelta::seconds(20)).to_rfc3339();
        app_sync
            .apply_remote_events_lww_batch(vec![(
                SyncEntity::Snapshot,
                synced_conflict.id.clone(),
                SyncOperation::Create,
                "evt-1".into(),
                ts1,
                1,
                payload,
            )])
            .await
            .unwrap();
        assert_eq!(dirty_for_b(), Some(nov24));
        run_incremental().await;
        assert_eq!(nov24_valuation().basis_status, BasisStatus::Unknown);

        app_sync
            .apply_remote_events_lww_batch(vec![(
                SyncEntity::Snapshot,
                synced_conflict.id.clone(),
                SyncOperation::Delete,
                "evt-2".into(),
                ts2,
                2,
                serde_json::json!({}),
            )])
            .await
            .unwrap();
        assert_eq!(dirty_for_b(), Some(nov24));
        run_incremental().await;
        assert_eq!(nov24_valuation().basis_status, BasisStatus::Complete);
    }

    #[tokio::test]
    async fn unreadable_statements_sync_batches_and_effective_splits_work_end_to_end() {
        use rust_decimal::Decimal;
        use std::collections::HashMap;
        use std::sync::{Arc, RwLock};
        use wealthfolio_core::{
            assets::AssetClassificationService,
            fx::{FxService, FxServiceTrait},
            portfolio::{
                coordinator::{
                    CoordinatorDeps, FactSources, PortfolioCoordinator, PortfolioJobRequest,
                    SilentObserver, WindowCadence,
                },
                economic_events::BasisStatus,
                holdings::{
                    HoldingType, HoldingsService, HoldingsServiceTrait, HoldingsValuationService,
                },
                projection::{MarkerScope, ProjectionStoreTrait},
                snapshot::{
                    AccountStateSnapshot, Position, SnapshotRepositoryTrait, SnapshotService,
                    SnapshotServiceTrait, SnapshotSource,
                },
                statements::{
                    daily::DailyStatementWrite,
                    reconstruction::{ClosingPosition, StatementDatePolicy, StatementInterval},
                    StatementPosition, StatementSnapshot,
                },
            },
            quotes::{MarketSyncMode, Quote, QuoteService, QuoteServiceTrait, QuoteStore},
            secrets::SecretStore,
            sync::{SyncEntity, SyncOperation},
            taxonomies::TaxonomyService,
        };

        struct EmptySecrets;
        impl SecretStore for EmptySecrets {
            fn set_secret(&self, _: &str, _: &str) -> Result<()> {
                Ok(())
            }
            fn get_secret(&self, _: &str) -> Result<Option<String>> {
                Ok(None)
            }
            fn delete_secret(&self, _: &str) -> Result<()> {
                Ok(())
            }
        }

        let r = Rig::new();
        sql_query(
            "INSERT INTO assets(id,kind,display_code,instrument_symbol,instrument_type,is_active,quote_mode,quote_ccy,created_at,updated_at) \
             VALUES ('AMD','INVESTMENT','AMD','AMD','EQUITY',1,'MANUAL','USD',datetime('now'),datetime('now'))",
        )
        .execute(&mut r.pool.get().unwrap())
        .unwrap();

        let base_currency = Arc::new(RwLock::new("USD".to_string()));
        let timezone = Arc::new(RwLock::new("UTC".to_string()));
        let accounts = Arc::new(crate::accounts::AccountRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        ));
        let activities = Arc::new(crate::activities::ActivityRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        ));
        let assets = Arc::new(crate::assets::AssetRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        ));
        let market = Arc::new(crate::market_data::MarketDataRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        ));
        let quotes: Arc<dyn QuoteServiceTrait> = Arc::new(
            QuoteService::new_with_custom_provider(
                market.clone(),
                Arc::new(crate::market_data::QuoteSyncStateRepository::new(
                    r.pool.clone(),
                    r.writer.clone(),
                )),
                market.clone(),
                assets.clone(),
                activities.clone(),
                Arc::new(EmptySecrets),
                None,
            )
            .await
            .unwrap(),
        );
        let fx_rates = Arc::new(crate::fx::FxRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        ));
        let fx = Arc::new(FxService::new(fx_rates.clone()));
        fx.initialize().unwrap();
        let snapshots = Arc::new(crate::portfolio::snapshot::SnapshotRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        ));
        let projections = Arc::new(crate::portfolio::projection::ProjectionStore::new(
            r.pool.clone(),
            r.writer.clone(),
        ));
        let lots = Arc::new(crate::lots::LotsRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        ));
        let valuations = Arc::new(crate::portfolio::valuation::ValuationRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        ));
        let asset_service = Arc::new(
            wealthfolio_core::assets::AssetService::new(assets.clone(), quotes.clone()).unwrap(),
        );
        let snapshot_service = Arc::new(
            SnapshotService::new(
                timezone.clone(),
                accounts.clone(),
                snapshots.clone(),
                activities.clone(),
            )
            .with_statement_repository(r.store.clone()),
        );
        let holdings_service = HoldingsService::new_with_timezone(
            asset_service,
            snapshot_service.clone(),
            Arc::new(HoldingsValuationService::new_with_timezone(
                fx.clone(),
                quotes.clone(),
                timezone.clone(),
            )),
            Arc::new(AssetClassificationService::new(Arc::new(
                TaxonomyService::new(Arc::new(crate::taxonomies::TaxonomyRepository::new(
                    r.pool.clone(),
                    r.writer.clone(),
                ))),
            ))),
            timezone.clone(),
        )
        .with_lot_repository(lots.clone())
        .with_projection_store(projections.clone());
        let coordinator = PortfolioCoordinator::new(CoordinatorDeps {
            base_currency,
            timezone,
            sources: FactSources {
                accounts,
                activities: activities.clone(),
                assets,
                quotes,
                fx_rates,
                snapshots: snapshots.clone(),
                projections: projections.clone(),
                statements: Some(r.store.clone()),
            },
            fx_service: fx,
            snapshot_service: snapshot_service.clone(),
            projections: projections.clone(),
            lots,
            window_cadence: WindowCadence::Days(30),
        });

        let today = chrono::Utc::now().date_naive();
        let stmt_close = today - chrono::TimeDelta::days(10);
        let split_day = today - chrono::TimeDelta::days(6);
        let broker_day = today - chrono::TimeDelta::days(3);
        let stmt_from = stmt_close - chrono::TimeDelta::days(20);

        let mut quote_rows = Vec::new();
        let mut cursor = stmt_close;
        while cursor <= today {
            let ds = cursor.format("%Y-%m-%d").to_string();
            quote_rows.push(Quote {
                id: format!("{ds}_AMD"),
                asset_id: "AMD".into(),
                timestamp: cursor.and_hms_opt(16, 0, 0).unwrap().and_utc(),
                open: Decimal::from(50),
                high: Decimal::from(50),
                low: Decimal::from(50),
                close: Decimal::from(50),
                adjclose: Decimal::from(50),
                volume: Decimal::ZERO,
                currency: "USD".into(),
                data_source: "MANUAL".into(),
                created_at: chrono::Utc::now(),
                notes: None,
            });
            cursor = cursor.succ_opt().unwrap();
        }
        market.upsert_quotes(&quote_rows).await.unwrap();

        let mk_daily = |from: NaiveDate, through: NaiveDate| DailyStatementWrite {
            closing_asset_ids: vec!["AMD".into()],
            activity_security_keys: None,
            expected_revision: None,
            expected_snapshot_revision: None,
            snapshot: StatementSnapshot {
                date: through,
                currency: "USD".into(),
                total_value: "350".into(),
                cash_value: Some("0".into()),
                positions: vec![StatementPosition {
                    label: "AMD".into(),
                    quantity: "7".into(),
                    market_value: Some("350".into()),
                    cost_basis: Some("280".into()),
                }],
                positions_complete: true,
                format: "merrill-cma-monthly-v1".into(),
                source_page: 1,
            },
            interval: StatementInterval {
                date_policy: StatementDatePolicy::Settlement,
                from,
                through,
                currency: "USD".into(),
                opening_cash: "0".into(),
                closing_cash: "0".into(),
                closing_value: "350".into(),
                closing_positions_complete: true,
                activity_complete: true,
                closing_positions: vec![ClosingPosition {
                    asset_id: "AMD".into(),
                    quantity: "7".into(),
                    market_value: "350".into(),
                    cost_basis: Some("280".into()),
                }],
                events: vec![],
            },
        };
        r.store
            .save_daily(
                "b",
                vec![
                    mk_daily(stmt_from, stmt_close),
                    mk_daily(stmt_close.succ_opt().unwrap(), today),
                ],
            )
            .await
            .unwrap();

        let mk_snap = |date: NaiveDate,
                       source: SnapshotSource,
                       basis: Decimal,
                       status: BasisStatus| AccountStateSnapshot {
            id: AccountStateSnapshot::stable_id("b", date),
            account_id: "b".into(),
            snapshot_date: date,
            currency: "USD".into(),
            positions: HashMap::from([(
                "AMD".into(),
                Position {
                    id: format!("b_AMD_{date}"),
                    account_id: "b".into(),
                    asset_id: "AMD".into(),
                    quantity: Decimal::from(7),
                    average_cost: basis.checked_div(Decimal::from(7)).unwrap_or(Decimal::ZERO),
                    total_cost_basis: basis,
                    currency: "USD".into(),
                    cost_basis_account: (status == BasisStatus::Complete).then_some(basis),
                    cost_basis_status: Some(status),
                    ..Default::default()
                },
            )]),
            cost_basis: basis,
            source,
            ..Default::default()
        };
        snapshots
            .save_or_update_snapshot(&mk_snap(
                broker_day,
                SnapshotSource::BrokerImported,
                Decimal::ZERO,
                BasisStatus::Unknown,
            ))
            .await
            .unwrap();

        // 1. Effective split override (" SPLIT " on an ADJUSTMENT in another account)
        // blocks current holdings statement basis enrichment just like the engine.
        let security_basis = || async {
            holdings_service
                .get_holdings("b", "USD")
                .await
                .unwrap()
                .into_iter()
                .find(|h| h.holding_type == HoldingType::Security)
                .unwrap()
                .cost_basis
                .and_then(|m| m.local)
        };
        assert_eq!(security_basis().await, Some(Decimal::from(280)));

        let split_ts = split_day
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .to_rfc3339();
        {
            let mut conn = r.pool.get().unwrap();
            sql_query(
                "INSERT INTO activities(id,account_id,asset_id,activity_type,activity_type_override,activity_date,quantity,unit_price,currency,fee,status,created_at,updated_at) \
                 VALUES ('split-ovr','a','AMD','ADJUSTMENT',' SPLIT ',?,'2','0','USD','0','POSTED',datetime('now'),datetime('now'))",
            )
            .bind::<Text, _>(&split_ts)
            .execute(&mut conn)
            .unwrap();
        }
        assert_eq!(security_basis().await, None);
        {
            let mut conn = r.pool.get().unwrap();
            sql_query("DELETE FROM activities WHERE id = 'split-ovr'")
                .execute(&mut conn)
                .unwrap();
        }
        assert_eq!(security_basis().await, Some(Decimal::from(280)));

        let run_incremental = || async {
            coordinator
                .run_job(
                    PortfolioJobRequest {
                        account_ids: Some(vec!["b".into()]),
                        market_sync: MarketSyncMode::None,
                        force_full: false,
                    },
                    &SilentObserver,
                )
                .await
                .unwrap();
        };
        let dirty_for_b = || -> Option<NaiveDate> {
            projections
                .pending_markers()
                .unwrap()
                .into_iter()
                .find(|m| m.scope == MarkerScope::Account("b".into()))
                .map(|m| m.dirty_from)
        };
        run_incremental().await;
        assert_eq!(dirty_for_b(), None);

        // 2. Device-sync batch of 20 snapshot changes on account "b" applies cleanly
        // and invalidates broker_day once at the end of the batch.
        let app_sync =
            crate::sync::app_sync::AppSyncRepository::new(r.pool.clone(), r.writer.clone());
        let obs_day = broker_day.succ_opt().unwrap();
        let mut batch = Vec::new();
        for idx in 0..20_i64 {
            let basis = if idx == 19 {
                Decimal::from(210)
            } else {
                Decimal::from(280)
            };
            let snap = mk_snap(
                obs_day,
                SnapshotSource::ManualEntry,
                basis,
                BasisStatus::Complete,
            );
            let db_row = crate::portfolio::snapshot::AccountStateSnapshotDB::from(snap.clone());
            let payload = serde_json::json!({
                "id": db_row.id,
                "account_id": db_row.account_id,
                "snapshot_date": db_row.snapshot_date,
                "currency": db_row.currency,
                "positions": db_row.positions,
                "cash_balances": db_row.cash_balances,
                "cost_basis": db_row.cost_basis,
                "net_contribution": db_row.net_contribution,
                "calculated_at": db_row.calculated_at,
                "net_contribution_base": db_row.net_contribution_base,
                "cash_total_account_currency": db_row.cash_total_account_currency,
                "cash_total_base_currency": db_row.cash_total_base_currency,
                "source": db_row.source,
            });
            let ts = (chrono::Utc::now() + chrono::TimeDelta::seconds(idx + 1)).to_rfc3339();
            batch.push((
                SyncEntity::Snapshot,
                snap.id,
                if idx == 0 {
                    SyncOperation::Create
                } else {
                    SyncOperation::Update
                },
                format!("batch-evt-{idx}"),
                ts,
                idx + 1,
                payload,
            ));
        }
        assert_eq!(
            app_sync.apply_remote_events_lww_batch(batch).await.unwrap(),
            20
        );
        assert_eq!(dirty_for_b(), Some(broker_day));
        run_incremental().await;
        assert_eq!(
            valuations
                .get_historical_valuations("b", Some(broker_day), Some(broker_day))
                .unwrap()[0]
                .basis_status,
            BasisStatus::Unknown
        );

        // 3. Corrupting a stored statement payload (or unsupported version) must NOT
        // reject snapshot saves, deletes, or device-sync replay; it marks broker_day
        // dirty and keeps fallback unverified, while real SQLite errors still surface.
        {
            let mut conn = r.pool.get().unwrap();
            sql_query(
                "UPDATE account_statement_reconstructions SET payload = '{bad-json' WHERE account_id = 'b'",
            )
            .execute(&mut conn)
            .unwrap();
        }
        snapshots
            .delete_snapshot_for_account_by_id("b", &AccountStateSnapshot::stable_id("b", obs_day))
            .await
            .unwrap();
        assert_eq!(dirty_for_b(), Some(broker_day));
        assert_eq!(security_basis().await, None);
        assert_eq!(
            snapshot_service
                .get_holdings_keyframes("b", None, None)
                .unwrap()[0]
                .positions["AMD"]
                .basis_status(),
            BasisStatus::Unknown
        );

        run_incremental().await;
        assert_eq!(dirty_for_b(), None);

        let extra_day = obs_day.succ_opt().unwrap();
        let valid_csv = mk_snap(
            extra_day,
            SnapshotSource::CsvImport,
            Decimal::from(280),
            BasisStatus::Complete,
        );
        snapshots.save_or_update_snapshot(&valid_csv).await.unwrap();
        assert_eq!(dirty_for_b(), Some(broker_day));

        run_incremental().await;
        snapshots
            .delete_snapshot_for_account_by_id("b", &valid_csv.id)
            .await
            .unwrap();
        assert_eq!(dirty_for_b(), Some(broker_day));

        // Unsupported statement version (version: 2) also keeps device-sync writes working.
        {
            let mut conn = r.pool.get().unwrap();
            sql_query(
                "UPDATE account_statement_reconstructions SET payload = '{\"version\":2,\"closingAssetIds\":[],\"interval\":{\"datePolicy\":\"settlement\",\"from\":\"2025-10-01\",\"through\":\"2025-10-31\",\"currency\":\"USD\",\"openingCash\":\"0\",\"closingCash\":\"0\",\"closingValue\":\"0\",\"closingPositionsComplete\":true,\"activityComplete\":true,\"closingPositions\":[],\"events\":[]}}' WHERE account_id = 'b'",
            )
            .execute(&mut conn)
            .unwrap();
        }
        run_incremental().await;
        assert_eq!(dirty_for_b(), None);
        let db_csv = crate::portfolio::snapshot::AccountStateSnapshotDB::from(valid_csv.clone());
        let csv_payload = serde_json::json!({
            "id": db_csv.id,
            "account_id": db_csv.account_id,
            "snapshot_date": db_csv.snapshot_date,
            "currency": db_csv.currency,
            "positions": db_csv.positions,
            "cash_balances": db_csv.cash_balances,
            "cost_basis": db_csv.cost_basis,
            "net_contribution": db_csv.net_contribution,
            "calculated_at": db_csv.calculated_at,
            "net_contribution_base": db_csv.net_contribution_base,
            "cash_total_account_currency": db_csv.cash_total_account_currency,
            "cash_total_base_currency": db_csv.cash_total_base_currency,
            "source": db_csv.source,
        });
        assert_eq!(
            app_sync
                .apply_remote_events_lww_batch(vec![(
                    SyncEntity::Snapshot,
                    valid_csv.id.clone(),
                    SyncOperation::Create,
                    "corrupt-stmt-sync-1".into(),
                    (chrono::Utc::now() + chrono::TimeDelta::seconds(100)).to_rfc3339(),
                    100,
                    csv_payload,
                )])
                .await
                .unwrap(),
            1
        );
        assert_eq!(dirty_for_b(), Some(broker_day));

        // Real database failures still surface rather than being masked as Unverifiable.
        {
            let mut conn = r.pool.get().unwrap();
            sql_query("DROP TABLE account_statement_reconstructions")
                .execute(&mut conn)
                .unwrap();
        }
        assert!(snapshots.save_or_update_snapshot(&valid_csv).await.is_err());
    }
}
