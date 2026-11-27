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

pub(crate) fn read_daily(
    conn: &mut SqliteConnection,
    account: &str,
) -> Result<Vec<StoredDailyStatement>> {
    let rows = sql_query("SELECT payload, revision, security_review FROM account_statement_reconstructions WHERE account_id=? ORDER BY through_date")
        .bind::<Text,_>(account).load::<DailyRow>(conn).map_err(StorageError::from)?;
    rows.into_iter()
        .map(|row| {
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
        })
        .collect()
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

    let coverage_from = history_start
        .or(coverage_start)
        .or_else(|| as_of.and_then(|ts| ts.date_naive().checked_sub_days(chrono::Days::new(730))));
    // Never fabricate coverage beyond the UTC date actually queried from Plaid.
    let coverage_through = history_end.or_else(|| as_of.map(|ts| ts.date_naive()));

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
    if let Some(basis_map) = source_history
        .get("basisBySecurity")
        .and_then(serde_json::Value::as_object)
    {
        for (sec_id, entry) in basis_map {
            if let Some(asset_id) = entry.get("assetId").and_then(serde_json::Value::as_str) {
                sec_to_assets
                    .entry(sec_id.clone())
                    .or_default()
                    .push(asset_id.to_owned());
            }
        }
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

fn read_observed_snapshots(
    conn: &mut SqliteConnection,
    account: &str,
) -> Result<Vec<wealthfolio_core::portfolio::snapshot::AccountStateSnapshot>> {
    use crate::portfolio::snapshot::AccountStateSnapshotDB;
    use crate::schema::holdings_snapshots;
    use std::collections::HashMap;
    use wealthfolio_core::portfolio::snapshot::{AccountStateSnapshot, Position};

    let rows: Vec<AccountStateSnapshotDB> = holdings_snapshots::table
        .filter(holdings_snapshots::account_id.eq(account))
        .filter(holdings_snapshots::source.ne("CALCULATED"))
        .order(holdings_snapshots::snapshot_date.asc())
        .load(conn)
        .map_err(StorageError::from)?;
    let mut snapshots = Vec::with_capacity(rows.len());
    for row in rows {
        if !matches!(
            row.source.as_str(),
            "BROKER_IMPORTED" | "MANUAL_ENTRY" | "CSV_IMPORT"
        ) {
            return Err(StorageError::SerializationError(
                "Invalid observed snapshot source".into(),
            )
            .into());
        }
        serde_json::from_str::<HashMap<String, Position>>(&row.positions).map_err(|_| {
            StorageError::SerializationError("Invalid observed snapshot positions".into())
        })?;
        let decoded = AccountStateSnapshot::try_from(row).map_err(|_| {
            StorageError::SerializationError("Invalid observed snapshot row".into())
        })?;
        snapshots.push(decoded);
    }
    Ok(snapshots)
}

/// Atomically invalidates `projection_state` back to the earliest changed
/// provider trade date when a `HOLDINGS`-mode Plaid commit adds or updates
/// stored investment transactions on an account with reviewed daily statements.
pub(crate) fn invalidate_holdings_if_provider_trades_changed_tx(
    conn: &mut SqliteConnection,
    mapping: &wealthfolio_connect::plaid::state::Mapping,
) -> Result<()> {
    if read_daily(conn, &mapping.account_id)?.is_empty() {
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
}
