//! Reviewed facts and source selection. Reads use cached quotes only; the explicit
//! refresh operation is the sole provider boundary. No live ledger writes.
use super::{
    amount, invalid,
    pricing::{price_statement, PricedStatementInterval},
    reconstruction::{validate_statement_chain, ReconstructedInterval, StatementInterval},
    statement_values, StatementRepository, StatementService, StatementSnapshot, StatementValue,
    StoredStatement,
};
use crate::{
    assets::AssetRepositoryTrait,
    errors::{Error, Result, ValidationError},
    fx::{FxError, FxServiceTrait},
    portfolio::coordinator::blocking,
    quotes::QuoteServiceTrait,
    utils::time_utils::{parse_user_timezone_or_default, user_today},
};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone)]
pub struct PricingSources {
    pub assets: Arc<dyn AssetRepositoryTrait>,
    pub quotes: Arc<dyn QuoteServiceTrait>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DailyStatementWrite {
    /// Reviewed identity for each original closing row, before grouping lots.
    #[serde(default)]
    pub closing_asset_ids: Vec<String>,
    /// Browser-derived fingerprints keyed by security event occurrence.
    #[serde(default)]
    pub activity_security_keys: Option<BTreeMap<String, String>>,
    pub interval: StatementInterval,
    pub snapshot: StatementSnapshot,
    pub expected_revision: Option<String>,
    pub expected_snapshot_revision: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredDailyStatement {
    #[serde(default)]
    pub closing_asset_ids: Vec<String>,
    #[serde(default)]
    pub activity_security_keys: BTreeMap<String, String>,
    /// Account-local save order. Legacy reviews without this value have equal priority.
    #[serde(default)]
    pub review_order: u64,
    pub version: u32,
    pub interval: StatementInterval,
    pub revision: String,
    #[serde(default)]
    pub priced: Option<PricedStatementInterval>,
    #[serde(default)]
    pub live_handoff: LiveHandoff,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyStatementPreview {
    pub reconstructed: ReconstructedInterval,
    pub priced: PricedStatementInterval,
    /// No Plaid matching is required. Continuity with the live ledger has not
    /// been established by statement checkpoint reconciliation.
    pub live_handoff: LiveHandoff,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LiveHandoff {
    #[default]
    Unverified,
    Match,
    Mismatch,
}
pub fn review_error(error: super::reconstruction::ReconstructionError) -> crate::errors::Error {
    ValidationError::InvalidInput(error.to_string()).into()
}
impl PricingSources {
    pub fn price(&self, interval: &StatementInterval) -> Result<PricedStatementInterval> {
        let ids = asset_ids(interval);
        let assets = self.assets.list_by_asset_ids(&ids)?;
        // Preserve original timestamps: filled quote rows would conceal age.
        let mut quotes = self.quotes.get_quotes_in_range(
            &ids.iter().cloned().collect(),
            interval.from,
            interval.through,
        )?;
        quotes.extend(
            self.quotes
                .get_latest_quotes_as_of(&ids, interval.from)?
                .into_values(),
        );
        quotes.sort_by(|a, b| a.id.cmp(&b.id));
        quotes.dedup_by(|a, b| a.id == b.id);
        price_statement(interval, &assets, &quotes).map_err(review_error)
    }
}
fn asset_ids(interval: &StatementInterval) -> Vec<String> {
    interval
        .closing_positions
        .iter()
        .map(|p| p.asset_id.clone())
        .chain(interval.events.iter().filter_map(|e| e.asset_id.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
impl DailyStatementWrite {
    pub fn matches_daily(&self, stored: &StoredDailyStatement) -> bool {
        self.interval == stored.interval
            && self.closing_asset_ids == stored.closing_asset_ids
            && self
                .activity_security_keys
                .as_ref()
                .is_none_or(|keys| keys == &stored.activity_security_keys)
    }

    pub fn validate(&self, currency: &str, today: NaiveDate) -> Result<()> {
        if self.closing_asset_ids.len() > 1000
            || self.closing_asset_ids.iter().any(|id| {
                id.trim().is_empty() || id.len() > 128 || id.chars().any(char::is_control)
            })
        {
            return Err(invalid());
        }
        self.snapshot.validate(currency, today)?;
        let i = &self.interval;
        if let Some(keys) = &self.activity_security_keys {
            if keys.len() > i.events.len() {
                return Err(invalid());
            }
            let security_events: BTreeSet<_> = i
                .events
                .iter()
                .filter(|event| event.asset_id.is_some())
                .map(|event| event.source_id.as_str())
                .collect();
            if keys.iter().any(|(source, key)| {
                !security_events.contains(source.as_str())
                    || key.len() != 64
                    || !key
                        .bytes()
                        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            }) {
                return Err(invalid());
            }
        }
        if self.snapshot.date != i.through
            || self.snapshot.currency != i.currency
            || !self.snapshot.positions_complete
            || !i.closing_positions_complete
            || amount(&self.snapshot.total_value)? != amount(&i.closing_value)?
            || self
                .snapshot
                .cash_value
                .as_deref()
                .map(amount)
                .transpose()?
                != Some(amount(&i.closing_cash)?)
        {
            return Err(invalid());
        }
        let links = if self.closing_asset_ids.is_empty() {
            // Compatibility for a normalized single-row-per-security caller.
            i.closing_positions
                .iter()
                .map(|p| p.asset_id.clone())
                .collect()
        } else {
            self.closing_asset_ids.clone()
        };
        if links.len() != self.snapshot.positions.len() {
            return Err(invalid());
        }
        let mut grouped: BTreeMap<
            String,
            (
                rust_decimal::Decimal,
                rust_decimal::Decimal,
                Option<rust_decimal::Decimal>,
            ),
        > = BTreeMap::new();
        for (reported, id) in self.snapshot.positions.iter().zip(links) {
            let g = grouped.entry(id).or_insert((
                rust_decimal::Decimal::ZERO,
                rust_decimal::Decimal::ZERO,
                Some(rust_decimal::Decimal::ZERO),
            ));
            g.0 =
                g.0.checked_add(amount(&reported.quantity)?)
                    .ok_or_else(invalid)?;
            g.1 =
                g.1.checked_add(amount(
                    reported.market_value.as_deref().ok_or_else(invalid)?,
                )?)
                .ok_or_else(invalid)?;
            g.2 = match (g.2, reported.cost_basis.as_deref().map(amount).transpose()?) {
                (Some(a), Some(b)) => Some(a.checked_add(b).ok_or_else(invalid)?),
                _ => None,
            };
        }
        if grouped.len() != i.closing_positions.len() {
            return Err(invalid());
        }
        for linked in &i.closing_positions {
            let expected = grouped.get(&linked.asset_id).ok_or_else(invalid)?;
            if expected.0 != amount(&linked.quantity)?
                || expected.1 != amount(&linked.market_value)?
                || expected.2 != linked.cost_basis.as_deref().map(amount).transpose()?
            {
                return Err(invalid());
            }
        }
        super::reconstruction::reconstruct_statement(i).map_err(review_error)?;
        Ok(())
    }
}
/// Validate against the persisted set inside the same writer transaction.
pub fn reviewed_chain(
    existing: &[StoredDailyStatement],
    writes: &[DailyStatementWrite],
) -> Result<()> {
    let mut intervals: BTreeMap<_, _> = existing
        .iter()
        .map(|s| (s.interval.through, s.interval.clone()))
        .collect();
    for write in writes {
        intervals.insert(write.interval.through, write.interval.clone());
    }
    validate_statement_chain(&intervals.into_values().collect::<Vec<_>>()).map_err(review_error)?;
    Ok(())
}
impl StatementService {
    fn handoff(&self, account: &str, interval: &StatementInterval) -> Result<LiveHandoff> {
        let Some(snapshots) = &self.live_snapshots else {
            return Ok(LiveHandoff::Unverified);
        };
        let boundary = self
            .valuations
            .get_historical_valuations(account, None, None)?
            .into_iter()
            .map(|v| v.valuation_date)
            .min();
        let Some(day) = boundary.filter(|d| *d >= interval.from && *d <= interval.through) else {
            return Ok(LiveHandoff::Unverified);
        };
        // Compare end-of-day states on an actual common date. Never extrapolate a
        // statement across a gap or compare settlement positions on different days.
        let live = snapshots.get_holdings_keyframes(account, Some(day), Some(day))?;
        let Some(live) = live
            .iter()
            .find(|s| s.snapshot_date == day && s.currency == interval.currency)
        else {
            return Ok(LiveHandoff::Unverified);
        };
        let reconstructed =
            super::reconstruction::reconstruct_statement(interval).map_err(review_error)?;
        let state = reconstructed
            .days
            .iter()
            .find(|s| s.date == day)
            .ok_or_else(invalid)?;
        let quantities: BTreeMap<_, _> = live
            .positions
            .values()
            .filter(|p| !p.quantity.is_zero())
            .map(|p| (p.asset_id.clone(), p.quantity.normalize().to_string()))
            .collect();
        let matches = quantities == state.positions
            && (live.cash_total_account_currency - amount(&state.cash)?).abs()
                <= rust_decimal::Decimal::new(1, 2);
        Ok(if matches {
            LiveHandoff::Match
        } else {
            LiveHandoff::Mismatch
        })
    }
    fn validate_daily(&self, account: &str, writes: &[DailyStatementWrite]) -> Result<()> {
        if writes.is_empty() || writes.len() > super::MAX_STATEMENT_BATCH {
            return Err(invalid());
        }
        let a = self.accounts.get_by_id(account)?;
        if a.account_type != crate::accounts::account_types::SECURITIES {
            return Err(invalid());
        }
        let today = user_today(parse_user_timezone_or_default(
            &self.timezone.read().unwrap_or_else(|p| p.into_inner()),
        ));
        let mut dates = BTreeSet::new();
        for write in writes {
            write.validate(&a.currency, today)?;
            if !dates.insert(write.interval.through) {
                return Err(invalid());
            }
        }
        let daily = self.repository.list_daily(account)?;
        let monthly = self.repository.list(account)?;
        for write in writes {
            let previous = daily
                .iter()
                .find(|s| s.interval.through == write.interval.through);
            let snapshot = monthly
                .iter()
                .find(|s| s.snapshot.date == write.snapshot.date);
            if previous.is_some_and(|s| write.matches_daily(s))
                && snapshot.is_some_and(|s| s.snapshot.same_financial_content(&write.snapshot))
            {
                continue;
            }
            if previous.map(|s| &s.revision) != write.expected_revision.as_ref()
                || snapshot.map(|s| &s.revision) != write.expected_snapshot_revision.as_ref()
            {
                return Err(invalid());
            }
        }
        reviewed_chain(&daily, writes)
    }
    pub async fn list_daily(&self, account: String) -> Result<Vec<StoredDailyStatement>> {
        let this = self.clone();
        blocking(move || {
            this.accounts.get_by_id(&account)?;
            let mut stored = this.repository.list_daily(&account)?;
            for row in &mut stored {
                row.priced = this
                    .pricing
                    .as_ref()
                    .and_then(|p| p.price(&row.interval).ok());
                row.live_handoff = this.handoff(&account, &row.interval)?;
            }
            Ok(stored)
        })
        .await
    }
    pub async fn preview_daily(
        &self,
        account: String,
        writes: Vec<DailyStatementWrite>,
    ) -> Result<Vec<DailyStatementPreview>> {
        let this = self.clone();
        blocking(move || {
            this.validate_daily(&account, &writes)?;
            let pricing = this.pricing.as_ref().ok_or_else(invalid)?;
            writes
                .iter()
                .map(|w| {
                    Ok(DailyStatementPreview {
                        reconstructed: super::reconstruction::reconstruct_statement(&w.interval)
                            .map_err(review_error)?,
                        priced: pricing.price(&w.interval)?,
                        live_handoff: this.handoff(&account, &w.interval)?,
                    })
                })
                .collect()
        })
        .await
    }
    pub async fn save_daily(
        &self,
        account: String,
        writes: Vec<DailyStatementWrite>,
        confirmed: bool,
    ) -> Result<()> {
        if !confirmed {
            return Err(invalid());
        }
        // Offload history-sized validation and metadata/quote reads.
        self.preview_daily(account.clone(), writes.clone()).await?;
        self.repository.save_daily(&account, writes).await?;
        self.emit_holdings_changed(&account).await
    }
    /// User-requested bounded refresh through configured providers. A failure for
    /// one asset is reported without discarding successful coverage for others.
    pub async fn refresh_daily_quotes(
        &self,
        account: String,
        writes: Vec<DailyStatementWrite>,
    ) -> Result<usize> {
        self.preview_daily(account, writes.clone()).await?;
        let sources = self.pricing.as_ref().ok_or_else(invalid)?;
        let mut by_asset: BTreeMap<String, Vec<(NaiveDate, NaiveDate)>> = BTreeMap::new();
        for w in writes {
            for id in asset_ids(&w.interval) {
                by_asset
                    .entry(id)
                    .or_default()
                    .push((w.interval.from, w.interval.through));
            }
        }
        let mut ranges = Vec::new();
        for (id, mut spans) in by_asset {
            spans.sort_unstable();
            let mut merged: Vec<(NaiveDate, NaiveDate)> = Vec::new();
            for (from, through) in spans {
                if let Some(last) = merged.last_mut() {
                    if last.1.succ_opt().is_some_and(|next| from <= next) {
                        last.1 = last.1.max(through);
                        continue;
                    }
                }
                merged.push((from, through));
            }
            for (from, through) in merged {
                ranges.push((id.clone(), from, through));
            }
        }
        if ranges.len() > 100 {
            return Err(invalid());
        }
        let mut failures = 0;
        for (id, from, through) in ranges {
            let start = from
                .checked_sub_days(chrono::Days::new(7))
                .ok_or_else(invalid)?;
            match sources
                .quotes
                .fetch_quotes_for_symbol(&id, "USD", start, through)
                .await
            {
                Ok(quotes) => {
                    sources.quotes.bulk_upsert_quotes(quotes).await?;
                }
                Err(_) => failures += 1,
            }
        }
        Ok(failures)
    }
}
/// Identical precedence for account and combined net-worth reads. Reported
/// endpoints win; daily states never extrapolate across uncovered months or
/// reactivate inside the live source's ownership interval.
#[allow(clippy::too_many_arguments)]
pub fn history_values(
    repository: &dyn StatementRepository,
    statements: &[StoredStatement],
    pricing: Option<&PricingSources>,
    account: &str,
    start: NaiveDate,
    end: NaiveDate,
    boundary: Option<NaiveDate>,
    currency: &str,
    fx: Option<&dyn FxServiceTrait>,
) -> Result<Vec<StatementValue>> {
    let fallback = statement_values(statements, start, end, boundary, currency, fx)?;
    let mut values: BTreeMap<_, _> = fallback.into_iter().map(|v| (v.date, v)).collect();
    let Some(pricing) = pricing else {
        return Ok(values.into_values().collect());
    };
    let daily = repository.list_daily(account)?;
    for stored in daily {
        let interval = &stored.interval;
        if interval.through < start
            || interval.from > end
            || boundary.is_some_and(|b| interval.from >= b)
        {
            continue;
        }
        let Ok(priced) = pricing.price(interval) else {
            continue;
        };
        for day in priced.days {
            if day.date < start
                || day.date > end
                || boundary.is_some_and(|b| day.date >= b)
                || statements.iter().any(|s| s.snapshot.date == day.date)
            {
                continue;
            }
            // An incomplete day retains the explicit monthly fallback, never a partial sum.
            let Some(raw_value) = day.value else {
                continue;
            };
            let convert = |raw: &str| -> Result<String> {
                let v = amount(raw)?;
                Ok(if currency == interval.currency {
                    v
                } else {
                    fx.ok_or_else(invalid)?.convert_currency_as_of(
                        v,
                        &interval.currency,
                        currency,
                        day.date,
                    )?
                }
                .to_string())
            };
            let converted =
                convert(&raw_value).and_then(|value| convert(&day.cash).map(|cash| (value, cash)));
            let (value, cash_value) = match converted {
                Ok(converted) => converted,
                Err(Error::Fx(FxError::RateNotFound(_))) => continue,
                Err(err) => return Err(err),
            };
            values.insert(
                day.date,
                StatementValue {
                    date: day.date,
                    statement_date: interval.through,
                    value,
                    cash_value: Some(cash_value),
                    currency: currency.into(),
                    source: "statementDaily".into(),
                    quality: Some(day.quality),
                },
            );
        }
    }
    Ok(values.into_values().collect())
}

fn has_statement_security_event(
    statements: &[StoredDailyStatement],
    asset_id: &str,
    from_exclusive: NaiveDate,
    to_inclusive: NaiveDate,
) -> bool {
    use super::reconstruction::StatementEventKind;
    from_exclusive < to_inclusive
        && statements.iter().any(|s| {
            s.interval.events.iter().any(|e| {
                e.asset_id.as_deref() == Some(asset_id)
                    && matches!(
                        e.kind,
                        StatementEventKind::Buy
                            | StatementEventKind::Sell
                            | StatementEventKind::TransferIn
                            | StatementEventKind::TransferOut
                    )
                    && from_exclusive < e.date
                    && e.date <= to_inclusive
            })
        })
}

type ObservedHistoryEntry = (
    NaiveDate,
    BTreeMap<String, (rust_decimal::Decimal, Option<rust_decimal::Decimal>)>,
);

fn collect_observed_history(
    snapshots: &[crate::portfolio::snapshot::AccountStateSnapshot],
) -> Vec<ObservedHistoryEntry> {
    use crate::portfolio::snapshot::SnapshotSource;
    snapshots
        .iter()
        .filter(|s| s.source != SnapshotSource::Calculated)
        .map(|s| {
            let positions = s
                .positions
                .values()
                .filter(|p| !p.quantity.is_zero())
                .map(|p| (p.asset_id.clone(), (p.quantity, p.known_cost_basis())))
                .collect();
            (s.snapshot_date, positions)
        })
        .collect()
}

/// Returns true when a provider investment transaction record can change a
/// security position's quantity or cost basis (trades, non-zero quantity events,
/// security transfers, and return of capital).
pub fn is_provider_tx_basis_changing(
    transaction_type: &str,
    subtype: &str,
    quantity_nonzero: bool,
) -> bool {
    let is_cash_flow = matches!(
        subtype,
        "deposit" | "contribution" | "withdrawal" | "distribution"
    );
    quantity_nonzero
        || matches!(transaction_type, "buy" | "sell")
        || (transaction_type == "transfer" && !is_cash_flow)
        || subtype.eq_ignore_ascii_case("return of capital")
}

/// Returns true when a provider transaction record is inherently tied to a
/// security (non-zero quantity, buy/sell, or return of capital), so a missing
/// `security_key` must fail closed as an unattributed security event.
pub fn provider_tx_requires_security_key(
    transaction_type: &str,
    subtype: &str,
    quantity_nonzero: bool,
) -> bool {
    quantity_nonzero
        || matches!(transaction_type, "buy" | "sell")
        || subtype.eq_ignore_ascii_case("return of capital")
}

/// Updates the continuous provider trade history start date when a new fetch
/// covers `[fetch_start, fetch_end]`. Retains `prior_start` only when
/// `prior_end` abuts or overlaps `fetch_start` (`fetch_start <= prior_end + 1`).
pub fn merge_trade_coverage_start(
    prior_start: Option<NaiveDate>,
    prior_end: Option<NaiveDate>,
    fetch_start: NaiveDate,
) -> NaiveDate {
    match (prior_start, prior_end) {
        (Some(start), Some(end)) if end.succ_opt().is_some_and(|next| fetch_start <= next) => {
            start.min(fetch_start)
        }
        _ => fetch_start,
    }
}

/// Input record for pairing provider investment transactions with cancellations.
pub struct RawProviderTx<'a> {
    pub id: &'a str,
    pub security_key: Option<&'a str>,
    pub date: NaiveDate,
    pub cancel_target: Option<&'a str>,
    pub basis_changing: bool,
    pub requires_security_key: bool,
}

/// Result of pairing provider investment transactions with cancellations.
pub struct PairedProviderTrades<'a> {
    pub events: BTreeMap<&'a str, Vec<super::ProviderTradeEvent>>,
    /// True when a security-affecting transaction or dangling cancellation
    /// lacked a `security_key`; callers must fail closed.
    pub has_unattributed_event: bool,
}

/// Pairs valid cancellation records with their target transactions and returns
/// basis-changing [`super::ProviderTradeEvent`]s grouped by `security_key`.
/// Cancellations whose target is absent from `txs` are retained as uncancelled
/// events on the cancellation date.
pub fn pair_provider_trade_events<'a>(txs: &[RawProviderTx<'a>]) -> PairedProviderTrades<'a> {
    use std::collections::{HashMap, HashSet};
    let present_ids: HashSet<&str> = txs
        .iter()
        .map(|tx| tx.id)
        .filter(|id| !id.is_empty())
        .collect();
    let mut cancelled_on_by_target: HashMap<&str, NaiveDate> = HashMap::new();
    let mut paired_cancel_indices: HashSet<usize> = HashSet::new();
    for (idx, tx) in txs.iter().enumerate() {
        if let Some(target) = tx.cancel_target.filter(|t| !t.is_empty()) {
            if target != tx.id && present_ids.contains(target) {
                cancelled_on_by_target.insert(target, tx.date);
                paired_cancel_indices.insert(idx);
            }
        }
    }
    let mut events: BTreeMap<&'a str, Vec<super::ProviderTradeEvent>> = BTreeMap::new();
    let mut has_unattributed_event = false;
    for (idx, tx) in txs.iter().enumerate() {
        if paired_cancel_indices.contains(&idx) {
            continue;
        }
        let dangling_cancel = tx.cancel_target.is_some_and(|t| !t.is_empty());
        if !tx.basis_changing && !dangling_cancel {
            continue;
        }
        let Some(key) = tx.security_key else {
            if tx.requires_security_key || dangling_cancel {
                has_unattributed_event = true;
            }
            continue;
        };
        events
            .entry(key)
            .or_default()
            .push(super::ProviderTradeEvent {
                date: tx.date,
                cancelled_on: cancelled_on_by_target.get(tx.id).copied(),
            });
    }
    PairedProviderTrades {
        events,
        has_unattributed_event,
    }
}

/// Converts raw `SPLIT` activities into grouped engine [`wealthfolio_portfolio_engine::SplitEvent`]s.
pub fn split_events_for_activities(
    activities: &[crate::activities::Activity],
    timezone: &str,
) -> Vec<wealthfolio_portfolio_engine::SplitEvent> {
    let tz = parse_user_timezone_or_default(timezone);
    wealthfolio_portfolio_engine::group_splits(
        activities
            .iter()
            .map(crate::portfolio::coordinator::raw_activity)
            .filter_map(|raw| wealthfolio_portfolio_engine::SplitRow::from_raw(&raw, &tz)),
    )
}

enum ClosingLookup<'a> {
    Missing,
    Unique(&'a super::reconstruction::ClosingPosition),
    Duplicate,
}

fn lookup_closing_position<'a>(
    stmt: &'a StoredDailyStatement,
    asset_id: &str,
) -> ClosingLookup<'a> {
    let mut found = None;
    for cp in &stmt.interval.closing_positions {
        if cp.asset_id == asset_id {
            if found.is_some() {
                return ClosingLookup::Duplicate;
            }
            found = Some(cp);
        }
    }
    match found {
        Some(cp) => ClosingLookup::Unique(cp),
        None => ClosingLookup::Missing,
    }
}

/// Returns the inclusive `(from, through)` date range of observed snapshots
/// needed to verify statement basis fallback for `snapshot_date` in `currency`,
/// or `None` when no earlier statement applies (or `stmt_date == snapshot_date`).
pub fn statement_basis_history_window(
    statements: &[StoredDailyStatement],
    currency: &str,
    snapshot_date: NaiveDate,
) -> Option<(NaiveDate, NaiveDate)> {
    let stmt_date = statements
        .iter()
        .filter(|s| s.interval.through <= snapshot_date && s.interval.currency == currency)
        .map(|s| s.interval.through)
        .max()?;
    if stmt_date == snapshot_date {
        return None;
    }
    let mut upper = snapshot_date;
    if let Some(mut cursor) = statements.iter().find(|s| {
        s.interval.currency == currency
            && s.interval.from <= snapshot_date
            && snapshot_date <= s.interval.through
    }) {
        upper = cursor.interval.through;
        while let Some(next) = statements.iter().find(|s| {
            s.interval.currency == currency
                && cursor
                    .interval
                    .through
                    .succ_opt()
                    .is_some_and(|next_day| s.interval.from == next_day)
                && s.interval.through > cursor.interval.through
        }) {
            cursor = next;
            upper = cursor.interval.through;
        }
    }
    Some((stmt_date, upper))
}

fn apply_statement_basis_with_history(
    snapshot: &mut crate::portfolio::snapshot::AccountStateSnapshot,
    statements: &[StoredDailyStatement],
    observed_history: &[ObservedHistoryEntry],
    splits: &[wealthfolio_portfolio_engine::SplitEvent],
    provider_trades: &super::ProviderTradeHistory,
) -> bool {
    use crate::portfolio::{economic_events::BasisStatus, snapshot::SnapshotSource};
    use rust_decimal::Decimal;

    if provider_trades.verification_incomplete
        || snapshot.source != SnapshotSource::BrokerImported
        || !snapshot
            .positions
            .values()
            .any(|p| p.basis_status() == BasisStatus::Unknown)
    {
        return false;
    }
    let Some(latest) = statements
        .iter()
        .filter(|s| {
            s.interval.through <= snapshot.snapshot_date && s.interval.currency == snapshot.currency
        })
        .max_by_key(|s| s.interval.through)
    else {
        return false;
    };
    let stmt_date = latest.interval.through;
    let containing = (stmt_date < snapshot.snapshot_date)
        .then(|| {
            statements.iter().find(|s| {
                s.interval.currency == snapshot.currency
                    && s.interval.from <= snapshot.snapshot_date
                    && snapshot.snapshot_date <= s.interval.through
            })
        })
        .flatten();
    let covered_by_provider = provider_trades.covers_window(stmt_date, snapshot.snapshot_date);
    let covered_by_statement = containing.is_some_and(|next_stmt| {
        next_stmt.interval.activity_complete
            && stmt_date
                .succ_opt()
                .is_some_and(|next_day| next_stmt.interval.from <= next_day)
    });
    let interval_covered =
        stmt_date == snapshot.snapshot_date || covered_by_provider || covered_by_statement;
    if !interval_covered {
        return false;
    }
    let mut closing: BTreeMap<&str, Option<(Decimal, Decimal)>> = BTreeMap::new();
    for p in &latest.interval.closing_positions {
        let parsed = p.cost_basis.as_deref().and_then(|raw_basis| {
            let qty = amount(&p.quantity).ok()?;
            let basis = amount(raw_basis).ok()?;
            (qty > Decimal::ZERO && basis >= Decimal::ZERO).then_some((qty, basis))
        });
        closing
            .entry(p.asset_id.as_str())
            .and_modify(|slot| *slot = None)
            .or_insert(parsed);
    }
    if !closing.values().any(Option::is_some) {
        return false;
    }
    let mut updated = false;
    for pos in snapshot.positions.values_mut() {
        if pos.basis_status() != BasisStatus::Unknown
            || pos.quantity <= Decimal::ZERO
            || (!pos.currency.is_empty() && pos.currency != snapshot.currency)
        {
            continue;
        }
        let Some(&(stmt_qty, stmt_basis)) =
            closing.get(pos.asset_id.as_str()).and_then(Option::as_ref)
        else {
            continue;
        };
        let asset_id = wealthfolio_portfolio_engine::model::AssetId::new(&pos.asset_id);
        let has_event_in = |from_exclusive: NaiveDate, to_inclusive: NaiveDate| -> bool {
            has_statement_security_event(statements, &pos.asset_id, from_exclusive, to_inclusive)
                || provider_trades
                    .events
                    .get(&pos.asset_id)
                    .is_some_and(|events| {
                        events
                            .iter()
                            .any(|event| event.affects_window(from_exclusive, to_inclusive))
                    })
                || wealthfolio_portfolio_engine::split_quantity_factor(
                    splits,
                    &asset_id,
                    from_exclusive,
                    to_inclusive,
                ) != Some(Decimal::ONE)
        };
        let observed_mismatch_after =
            |from_exclusive: NaiveDate, to_inclusive: NaiveDate| -> bool {
                observed_history.iter().any(|(date, positions)| {
                    from_exclusive < *date
                        && *date <= to_inclusive
                        && positions.get(&pos.asset_id).is_none_or(|(qty, basis)| {
                            *qty != pos.quantity || basis.is_some_and(|known| known != stmt_basis)
                        })
                })
            };
        if stmt_qty != pos.quantity
            || has_event_in(stmt_date, snapshot.snapshot_date)
            || observed_history.iter().any(|(date, positions)| {
                stmt_date <= *date
                    && *date < snapshot.snapshot_date
                    && positions.get(&pos.asset_id).is_none_or(|(qty, basis)| {
                        *qty != pos.quantity || basis.is_some_and(|known| known != stmt_basis)
                    })
            })
        {
            continue;
        }
        if let Some(next_stmt) = containing {
            if !covered_by_provider {
                let confirmed_by_next = match lookup_closing_position(next_stmt, &pos.asset_id) {
                    ClosingLookup::Unique(cp) => {
                        amount(&cp.quantity).ok() == Some(pos.quantity)
                            && cp
                                .cost_basis
                                .as_deref()
                                .is_some_and(|raw| amount(raw).ok() == Some(stmt_basis))
                    }
                    ClosingLookup::Missing | ClosingLookup::Duplicate => false,
                };
                if has_event_in(snapshot.snapshot_date, next_stmt.interval.through)
                    || observed_mismatch_after(snapshot.snapshot_date, next_stmt.interval.through)
                    || !confirmed_by_next
                {
                    continue;
                }
            } else {
                let mut cursor = Some(next_stmt);
                let mut window_from = snapshot.snapshot_date;
                let mut conflicts_with_later = false;
                while let Some(stmt) = cursor {
                    let window_covered = stmt.interval.activity_complete
                        || provider_trades.covers_window(window_from, stmt.interval.through);
                    if !window_covered || has_event_in(window_from, stmt.interval.through) {
                        break;
                    }
                    if observed_mismatch_after(window_from, stmt.interval.through) {
                        conflicts_with_later = true;
                        break;
                    }
                    match lookup_closing_position(stmt, &pos.asset_id) {
                        ClosingLookup::Duplicate => {
                            conflicts_with_later = true;
                            break;
                        }
                        ClosingLookup::Missing => {
                            if stmt.interval.closing_positions_complete {
                                conflicts_with_later = true;
                                break;
                            }
                        }
                        ClosingLookup::Unique(cp) => {
                            if amount(&cp.quantity).ok() != Some(pos.quantity) {
                                conflicts_with_later = true;
                                break;
                            }
                            if let Some(raw) = cp.cost_basis.as_deref() {
                                if amount(raw).ok() != Some(stmt_basis) {
                                    conflicts_with_later = true;
                                }
                                break;
                            }
                        }
                    }
                    window_from = stmt.interval.through;
                    cursor = statements.iter().find(|s| {
                        s.interval.currency == snapshot.currency
                            && stmt
                                .interval
                                .through
                                .succ_opt()
                                .is_some_and(|next_day| s.interval.from == next_day)
                            && s.interval.through > stmt.interval.through
                    });
                }
                if conflicts_with_later {
                    continue;
                }
            }
        }
        let Some(avg_cost) = stmt_basis.checked_div(pos.quantity) else {
            continue;
        };
        pos.total_cost_basis = stmt_basis;
        pos.average_cost = avg_cost;
        pos.cost_basis_account = Some(stmt_basis);
        if snapshot.base_currency.as_deref() == Some(snapshot.currency.as_str()) {
            pos.cost_basis_base = Some(stmt_basis);
        } else {
            pos.cost_basis_base = None;
        }
        pos.cost_basis_status = Some(BasisStatus::Complete);
        updated = true;
    }
    if updated {
        let mut total_basis = Decimal::ZERO;
        for p in snapshot.positions.values() {
            if p.currency.is_empty() || p.currency == snapshot.currency {
                if let Some(known) = p.known_cost_basis() {
                    if let Some(sum) = total_basis.checked_add(known) {
                        total_basis = sum;
                    }
                }
            }
        }
        snapshot.cost_basis = total_basis;
    }
    updated
}

/// Populate missing (`BasisStatus::Unknown`) cost basis on a `BrokerImported`
/// snapshot from the latest reviewed daily statement on or before its date when
/// share quantity, split state, and stored provider history cover and confirm
/// the interval since statement close.
pub fn apply_statement_basis_to_snapshot(
    snapshot: &mut crate::portfolio::snapshot::AccountStateSnapshot,
    statements: &[StoredDailyStatement],
    observed_history: &[crate::portfolio::snapshot::AccountStateSnapshot],
    splits: &[wealthfolio_portfolio_engine::SplitEvent],
    provider_trades: &super::ProviderTradeHistory,
) -> bool {
    let history = collect_observed_history(observed_history);
    apply_statement_basis_with_history(snapshot, statements, &history, splits, provider_trades)
}

/// Populate missing (`BasisStatus::Unknown`) cost basis across an account's
/// observed snapshots using `snapshots` itself for intervening-quantity checks.
pub fn apply_statement_basis_to_snapshots(
    snapshots: &mut [crate::portfolio::snapshot::AccountStateSnapshot],
    statements: &[StoredDailyStatement],
    splits: &[wealthfolio_portfolio_engine::SplitEvent],
    provider_trades: &super::ProviderTradeHistory,
) {
    if statements.is_empty() {
        return;
    }
    let history = collect_observed_history(snapshots);
    for snapshot in snapshots {
        apply_statement_basis_with_history(snapshot, statements, &history, splits, provider_trades);
    }
}

/// Suggests an opening per-share unit cost at `opening_day` (`start_date - 1`)
/// from reviewed daily statements when share quantity, split state, observed
/// snapshots, and statement/provider events stayed unchanged across the covered
/// interval between `opening_day` and the statement's closing date. A statement
/// covering `opening_day` always takes precedence over older pre-gap statements.
#[allow(clippy::too_many_arguments)]
pub fn suggested_opening_unit_cost(
    statements: &[StoredDailyStatement],
    currency: &str,
    asset_id: &str,
    opening_day: NaiveDate,
    opening_quantity: rust_decimal::Decimal,
    observed_snapshots: &[crate::portfolio::snapshot::AccountStateSnapshot],
    splits: &[wealthfolio_portfolio_engine::SplitEvent],
    provider_trades: &[super::ProviderTradeEvent],
    earliest_trade_coverage: Option<NaiveDate>,
) -> Option<rust_decimal::Decimal> {
    use rust_decimal::Decimal;
    if opening_quantity <= Decimal::ZERO {
        return None;
    }
    let asset = wealthfolio_portfolio_engine::model::AssetId::new(asset_id);
    let observed_history = collect_observed_history(observed_snapshots);
    let statement_basis_and_unit_cost =
        |stmt: &StoredDailyStatement| -> Option<(Decimal, Decimal)> {
            if stmt.interval.currency != currency {
                return None;
            }
            let ClosingLookup::Unique(cp) = lookup_closing_position(stmt, asset_id) else {
                return None;
            };
            let qty = amount(&cp.quantity).ok()?;
            if qty != opening_quantity {
                return None;
            }
            let basis = amount(cp.cost_basis.as_deref()?).ok()?;
            if basis < Decimal::ZERO {
                return None;
            }
            let unit = basis.checked_div(opening_quantity)?.normalize();
            Some((basis, unit))
        };
    let has_event_in = |from_exclusive: NaiveDate, to_inclusive: NaiveDate| -> bool {
        has_statement_security_event(statements, asset_id, from_exclusive, to_inclusive)
            || provider_trades
                .iter()
                .any(|event| event.affects_window(from_exclusive, to_inclusive))
            || wealthfolio_portfolio_engine::split_quantity_factor(
                splits,
                &asset,
                from_exclusive,
                to_inclusive,
            ) != Some(Decimal::ONE)
    };
    let observed_consistent =
        |from_inclusive: NaiveDate, to_inclusive: NaiveDate, expected_basis: Decimal| -> bool {
            !observed_history.iter().any(|(date, positions)| {
                from_inclusive <= *date
                    && *date <= to_inclusive
                    && positions.get(asset_id).is_none_or(|(qty, basis)| {
                        *qty != opening_quantity
                            || basis.is_some_and(|known| known != expected_basis)
                    })
            })
        };
    let unchanged_between =
        |from_exclusive: NaiveDate, to_inclusive: NaiveDate, expected_basis: Decimal| -> bool {
            !has_event_in(from_exclusive, to_inclusive)
                && observed_consistent(from_exclusive, to_inclusive, expected_basis)
        };
    if let Some(containing) = statements.iter().find(|s| {
        s.interval.currency == currency
            && s.interval.from <= opening_day
            && opening_day <= s.interval.through
    }) {
        let (stmt_basis, unit) = statement_basis_and_unit_cost(containing)?;
        if containing.interval.through == opening_day {
            if !observed_consistent(opening_day, opening_day, stmt_basis) {
                return None;
            }
            return Some(unit);
        }
        if !containing.interval.activity_complete
            || !unchanged_between(opening_day, containing.interval.through, stmt_basis)
        {
            return None;
        }
        let mut window_start = containing.interval.from;
        let mut cursor = containing;
        while let Some(prev) = statements.iter().find(|s| {
            s.interval.currency == currency
                && s.interval
                    .through
                    .succ_opt()
                    .is_some_and(|next_day| next_day == cursor.interval.from)
                && s.interval.from < cursor.interval.from
        }) {
            if has_event_in(prev.interval.through, opening_day) {
                break;
            }
            window_start = prev.interval.through;
            match lookup_closing_position(prev, asset_id) {
                ClosingLookup::Duplicate => return None,
                ClosingLookup::Missing => {
                    if prev.interval.closing_positions_complete {
                        return None;
                    }
                }
                ClosingLookup::Unique(cp) => {
                    if amount(&cp.quantity).ok() != Some(opening_quantity) {
                        return None;
                    }
                    if let Some(raw) = cp.cost_basis.as_deref() {
                        if amount(raw).ok() != Some(stmt_basis) {
                            return None;
                        }
                        break;
                    }
                }
            }
            if !prev.interval.activity_complete {
                break;
            }
            window_start = prev.interval.from;
            cursor = prev;
        }
        for (obs_date, positions) in &observed_history {
            if window_start <= *obs_date
                && *obs_date < opening_day
                && !has_event_in(*obs_date, opening_day)
                && positions.get(asset_id).is_none_or(|(qty, basis)| {
                    *qty != opening_quantity || basis.is_some_and(|known| known != stmt_basis)
                })
            {
                return None;
            }
        }
        return Some(unit);
    }
    let latest_before = statements
        .iter()
        .filter(|s| s.interval.through < opening_day && s.interval.currency == currency)
        .max_by_key(|s| s.interval.through)?;
    let covered = earliest_trade_coverage.is_some_and(|earliest| {
        latest_before
            .interval
            .through
            .succ_opt()
            .is_some_and(|next_day| earliest <= next_day)
    });
    if !covered {
        return None;
    }
    let (stmt_basis, unit) = statement_basis_and_unit_cost(latest_before)?;
    if !unchanged_between(latest_before.interval.through, opening_day, stmt_basis) {
        return None;
    }
    Some(unit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portfolio::{
        economic_events::BasisStatus,
        snapshot::{AccountStateSnapshot, Position, SnapshotSource},
        statements::reconstruction::{
            ClosingPosition, StatementDatePolicy, StatementEvent, StatementEventKind,
        },
    };
    use rust_decimal::Decimal;
    use std::collections::HashMap;

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn stmt(
        from: &str,
        through: &str,
        qty: &str,
        basis: Option<&str>,
        events: Vec<StatementEvent>,
    ) -> StoredDailyStatement {
        StoredDailyStatement {
            closing_asset_ids: vec!["AMD".into()],
            activity_security_keys: BTreeMap::new(),
            review_order: 1,
            version: 1,
            revision: "r1".into(),
            priced: None,
            live_handoff: LiveHandoff::Unverified,
            interval: StatementInterval {
                date_policy: StatementDatePolicy::Settlement,
                from: d(from),
                through: d(through),
                currency: "USD".into(),
                opening_cash: "0".into(),
                closing_cash: "0".into(),
                closing_value: "350".into(),
                closing_positions_complete: true,
                activity_complete: true,
                closing_positions: vec![ClosingPosition {
                    asset_id: "AMD".into(),
                    quantity: qty.into(),
                    market_value: "350".into(),
                    cost_basis: basis.map(Into::into),
                }],
                events,
            },
        }
    }

    fn snap(
        date: &str,
        source: SnapshotSource,
        qty: i64,
        status: Option<BasisStatus>,
    ) -> AccountStateSnapshot {
        let p = Position {
            asset_id: "AMD".into(),
            currency: "USD".into(),
            quantity: Decimal::from(qty),
            cost_basis_status: status,
            ..Default::default()
        };
        AccountStateSnapshot {
            id: format!("s-{date}"),
            account_id: "a".into(),
            snapshot_date: d(date),
            currency: "USD".into(),
            positions: HashMap::from([("AMD".into(), p)]),
            source,
            ..Default::default()
        }
    }

    #[test]
    fn statement_basis_respects_source_intervening_snapshots_and_splits() {
        let statements = vec![stmt("2026-10-01", "2026-10-31", "7", Some("280"), vec![])];
        let covered = |events: BTreeMap<String, Vec<super::super::ProviderTradeEvent>>| {
            super::super::ProviderTradeHistory {
                coverage_from: Some(d("2026-11-01")),
                coverage_through: Some(d("2026-11-24")),
                events,
                verification_incomplete: false,
            }
        };
        let no_trades = covered(BTreeMap::new());

        // Missing or insufficient trade coverage withholds fallback across (stmt_date, snapshot_date],
        // whereas same-day close (stmt_date == snapshot_date) or coverage starting on stmt_date + 1 allows it.
        let mut uncovered_snap = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(!apply_statement_basis_to_snapshot(
            &mut uncovered_snap,
            &statements,
            &[],
            &[],
            &super::super::ProviderTradeHistory::default(),
        ));
        let gap_before_coverage = super::super::ProviderTradeHistory {
            coverage_from: Some(d("2026-11-02")),
            coverage_through: Some(d("2026-11-24")),
            events: BTreeMap::new(),
            verification_incomplete: false,
        };
        assert!(!apply_statement_basis_to_snapshot(
            &mut uncovered_snap,
            &statements,
            &[],
            &[],
            &gap_before_coverage,
        ));
        let mut same_day_close = snap(
            "2026-10-31",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(apply_statement_basis_to_snapshot(
            &mut same_day_close,
            &statements,
            &[],
            &[],
            &super::super::ProviderTradeHistory::default(),
        ));
        assert_eq!(
            same_day_close.positions["AMD"].total_cost_basis,
            Decimal::from(280)
        );

        // Manual snapshots are never mutated.
        let mut manual = snap(
            "2026-11-24",
            SnapshotSource::ManualEntry,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(!apply_statement_basis_to_snapshot(
            &mut manual,
            &statements,
            &[],
            &[],
            &no_trades,
        ));
        assert_eq!(manual.positions["AMD"].basis_status(), BasisStatus::Unknown);

        // Intervening observed quantity change (7 -> 5 -> 7) blocks fallback.
        let intervening = snap(
            "2026-11-10",
            SnapshotSource::BrokerImported,
            5,
            Some(BasisStatus::Unknown),
        );
        let mut round_trip = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(!apply_statement_basis_to_snapshot(
            &mut round_trip,
            &statements,
            &[intervening],
            &[],
            &no_trades,
        ));
        assert_eq!(
            round_trip.positions["AMD"].basis_status(),
            BasisStatus::Unknown
        );

        // Intervening observed snapshot with matching quantity but conflicting known
        // basis (including explicit $0) blocks fallback; matching known basis allows it.
        for conflicting_basis in [Decimal::from(300), Decimal::ZERO] {
            let mut conflicting = snap(
                "2026-11-10",
                SnapshotSource::BrokerImported,
                7,
                Some(BasisStatus::Complete),
            );
            conflicting
                .positions
                .get_mut("AMD")
                .unwrap()
                .total_cost_basis = conflicting_basis;
            let mut later = snap(
                "2026-11-24",
                SnapshotSource::BrokerImported,
                7,
                Some(BasisStatus::Unknown),
            );
            assert!(!apply_statement_basis_to_snapshot(
                &mut later,
                &statements,
                &[conflicting],
                &[],
                &no_trades,
            ));
            assert_eq!(later.positions["AMD"].basis_status(), BasisStatus::Unknown);
        }
        let mut matching = snap(
            "2026-11-10",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Complete),
        );
        matching.positions.get_mut("AMD").unwrap().total_cost_basis = Decimal::from(280);
        let mut later_matching = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(apply_statement_basis_to_snapshot(
            &mut later_matching,
            &statements,
            &[matching],
            &[],
            &no_trades,
        ));
        assert_eq!(
            later_matching.positions["AMD"].total_cost_basis,
            Decimal::from(280)
        );

        // Closing-day (date == stmt_date) conflicting known basis (including explicit $0)
        // withholds fallback, while closing-day Unknown or matching basis allows it.
        for conflicting_basis in [Decimal::ZERO, Decimal::from(300)] {
            let mut closing_day_conflict = snap(
                "2026-10-31",
                SnapshotSource::BrokerImported,
                7,
                Some(BasisStatus::Complete),
            );
            closing_day_conflict
                .positions
                .get_mut("AMD")
                .unwrap()
                .total_cost_basis = conflicting_basis;
            let mut later = snap(
                "2026-11-24",
                SnapshotSource::BrokerImported,
                7,
                Some(BasisStatus::Unknown),
            );
            assert!(!apply_statement_basis_to_snapshot(
                &mut later,
                &statements,
                &[closing_day_conflict],
                &[],
                &no_trades,
            ));
            assert_eq!(later.positions["AMD"].basis_status(), BasisStatus::Unknown);
        }
        // Closing-day (date == stmt_date) quantity mismatch or missing position also
        // withholds fallback, while matching quantity with Unknown basis allows it.
        let closing_day_qty_mismatch = snap(
            "2026-10-31",
            SnapshotSource::BrokerImported,
            5,
            Some(BasisStatus::Unknown),
        );
        let mut later_after_qty_mismatch = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(!apply_statement_basis_to_snapshot(
            &mut later_after_qty_mismatch,
            &statements,
            &[closing_day_qty_mismatch],
            &[],
            &no_trades,
        ));
        let mut closing_day_missing = snap(
            "2026-10-31",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        closing_day_missing.positions.clear();
        let mut later_after_missing_close = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(!apply_statement_basis_to_snapshot(
            &mut later_after_missing_close,
            &statements,
            &[closing_day_missing],
            &[],
            &no_trades,
        ));
        let closing_day_unknown = snap(
            "2026-10-31",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        let mut later_after_unknown_close = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(apply_statement_basis_to_snapshot(
            &mut later_after_unknown_close,
            &statements,
            &[closing_day_unknown],
            &[],
            &no_trades,
        ));
        assert_eq!(
            later_after_unknown_close.positions["AMD"].total_cost_basis,
            Decimal::from(280)
        );

        // Provider trades strictly after statement close through snapshot date block
        // fallback (e.g., same-day sell and repurchase), and cancellations straddling
        // statement close or snapshot date also block fallback. In-window cancelled
        // pairs and out-of-window trades do not.
        let ev = |date: &str, cancelled_on: Option<&str>| super::super::ProviderTradeEvent {
            date: d(date),
            cancelled_on: cancelled_on.map(d),
        };
        for blocking_events in [
            vec![ev("2026-11-10", None)],
            vec![ev("2026-10-20", Some("2026-11-05"))],
            vec![ev("2026-11-10", Some("2026-11-26"))],
        ] {
            let trades = covered(BTreeMap::from([("AMD".into(), blocking_events)]));
            let mut traded = snap(
                "2026-11-24",
                SnapshotSource::BrokerImported,
                7,
                Some(BasisStatus::Unknown),
            );
            assert!(!apply_statement_basis_to_snapshot(
                &mut traded,
                &statements,
                &[],
                &[],
                &trades,
            ));
            assert_eq!(traded.positions["AMD"].basis_status(), BasisStatus::Unknown);
        }
        let non_blocking_trades = covered(BTreeMap::from([(
            "AMD".into(),
            vec![
                ev("2026-10-31", None),
                ev("2026-11-25", None),
                ev("2026-11-10", Some("2026-11-12")),
            ],
        )]));
        let mut traded_out_of_window = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(apply_statement_basis_to_snapshot(
            &mut traded_out_of_window,
            &statements,
            &[],
            &[],
            &non_blocking_trades,
        ));
        assert_eq!(
            traded_out_of_window.positions["AMD"].total_cost_basis,
            Decimal::from(280)
        );

        // Split after statement close blocks fallback, while an unchanged
        // pre-split / non-split BrokerImported position receives closing basis.
        let splits = vec![wealthfolio_portfolio_engine::SplitEvent {
            asset: wealthfolio_portfolio_engine::model::AssetId::new("AMD"),
            split_date: d("2026-11-05"),
            ratio: Decimal::from(2),
        }];
        let mut post_split = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(!apply_statement_basis_to_snapshot(
            &mut post_split,
            &statements,
            &[],
            &splits,
            &no_trades,
        ));
        let mut unchanged = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        unchanged.base_currency = Some("CAD".into());
        unchanged.positions.get_mut("AMD").unwrap().cost_basis_base = Some(Decimal::ZERO);
        assert!(apply_statement_basis_to_snapshot(
            &mut unchanged,
            &statements,
            &[],
            &[],
            &no_trades,
        ));
        assert_eq!(
            unchanged.positions["AMD"].total_cost_basis,
            Decimal::from(280)
        );
        assert_eq!(unchanged.positions["AMD"].average_cost, Decimal::from(40));
        assert_eq!(unchanged.positions["AMD"].cost_basis_base, None);
        assert_eq!(
            unchanged.positions["AMD"].basis_status(),
            BasisStatus::Complete
        );

        // Incomplete verification (corrupt stored provider history) fails closed even on same-day close.
        let mut same_day_corrupt = snap(
            "2026-10-31",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(!apply_statement_basis_to_snapshot(
            &mut same_day_corrupt,
            &statements,
            &[],
            &[],
            &super::super::ProviderTradeHistory::incomplete(),
        ));

        // merge_trade_coverage_start retains prior start only when windows overlap or abut.
        assert_eq!(
            merge_trade_coverage_start(
                Some(d("2022-01-01")),
                Some(d("2024-01-01")),
                d("2024-01-02"),
            ),
            d("2022-01-01")
        );
        assert_eq!(
            merge_trade_coverage_start(
                Some(d("2022-01-01")),
                Some(d("2024-01-01")),
                d("2024-01-03"),
            ),
            d("2024-01-03")
        );

        // pair_provider_trade_events fails closed on unattributed security trades while
        // ignoring account-level cash transfers without a security_key.
        let cash_transfer_only = pair_provider_trade_events(&[RawProviderTx {
            id: "xfer1",
            security_key: None,
            date: d("2026-11-10"),
            cancel_target: None,
            basis_changing: is_provider_tx_basis_changing("transfer", "transfer", false),
            requires_security_key: provider_tx_requires_security_key("transfer", "transfer", false),
        }]);
        assert!(!cash_transfer_only.has_unattributed_event);
        assert!(cash_transfer_only.events.is_empty());
        let unattributed_buy = pair_provider_trade_events(&[RawProviderTx {
            id: "buy1",
            security_key: None,
            date: d("2026-11-10"),
            cancel_target: None,
            basis_changing: is_provider_tx_basis_changing("buy", "buy", true),
            requires_security_key: provider_tx_requires_security_key("buy", "buy", true),
        }]);
        assert!(unattributed_buy.has_unattributed_event);
    }

    #[test]
    fn suggested_opening_unit_cost_handles_mid_statement_start_and_intervening_trades() {
        let ev = |date: &str, cancelled_on: Option<&str>| super::super::ProviderTradeEvent {
            date: d(date),
            cancelled_on: cancelled_on.map(d),
        };
        let untouched = vec![stmt("2026-10-01", "2026-10-31", "7", Some("280"), vec![])];
        assert_eq!(
            suggested_opening_unit_cost(
                &untouched,
                "USD",
                "AMD",
                d("2026-10-14"),
                Decimal::from(7),
                &[],
                &[],
                &[],
                None,
            ),
            Some(Decimal::from(40))
        );
        // Incomplete activity on a covering statement blocks mid-interval suggestion.
        let mut incomplete_stmt = untouched.clone();
        incomplete_stmt[0].interval.activity_complete = false;
        assert_eq!(
            suggested_opening_unit_cost(
                &incomplete_stmt,
                "USD",
                "AMD",
                d("2026-10-14"),
                Decimal::from(7),
                &[],
                &[],
                &[],
                None,
            ),
            None
        );
        // Provider trade or straddling cancellation before statement close blocks mid-interval fallback.
        for blocking in [
            vec![ev("2026-10-20", None)],
            vec![ev("2026-10-20", Some("2026-11-02"))],
        ] {
            assert_eq!(
                suggested_opening_unit_cost(
                    &untouched,
                    "USD",
                    "AMD",
                    d("2026-10-14"),
                    Decimal::from(7),
                    &[],
                    &[],
                    &blocking,
                    None,
                ),
                None
            );
        }
        // Covering statement takes precedence over an older pre-gap statement even when
        // there is an unreviewed month in between, and if the covering statement is
        // blocked by a trade it does NOT fall back to the stale pre-gap statement.
        let aug_and_oct = vec![
            stmt("2026-08-01", "2026-08-31", "7", Some("280"), vec![]),
            stmt("2026-10-01", "2026-10-31", "7", Some("560"), vec![]),
        ];
        assert_eq!(
            suggested_opening_unit_cost(
                &aug_and_oct,
                "USD",
                "AMD",
                d("2026-10-14"),
                Decimal::from(7),
                &[],
                &[],
                &[],
                None,
            ),
            Some(Decimal::from(80))
        );
        assert_eq!(
            suggested_opening_unit_cost(
                &aug_and_oct,
                "USD",
                "AMD",
                d("2026-10-14"),
                Decimal::from(7),
                &[],
                &[],
                &[ev("2026-10-20", None)],
                None,
            ),
            None
        );
        // Split between opening_day and statement close (or between latest_before and
        // opening_day) blocks suggestion, as does missing/insufficient trade coverage
        // or an intervening observed snapshot with conflicting quantity/basis.
        let oct_split = vec![wealthfolio_portfolio_engine::SplitEvent {
            asset: wealthfolio_portfolio_engine::model::AssetId::new("AMD"),
            split_date: d("2026-10-20"),
            ratio: Decimal::from(2),
        }];
        assert_eq!(
            suggested_opening_unit_cost(
                &untouched,
                "USD",
                "AMD",
                d("2026-10-14"),
                Decimal::from(7),
                &[],
                &oct_split,
                &[],
                None,
            ),
            None
        );
        let nov_split = vec![wealthfolio_portfolio_engine::SplitEvent {
            asset: wealthfolio_portfolio_engine::model::AssetId::new("AMD"),
            split_date: d("2026-11-05"),
            ratio: Decimal::from(2),
        }];
        assert_eq!(
            suggested_opening_unit_cost(
                &untouched,
                "USD",
                "AMD",
                d("2026-11-10"),
                Decimal::from(7),
                &[],
                &nov_split,
                &[],
                Some(d("2026-11-01")),
            ),
            None
        );
        let intervening_obs = vec![snap(
            "2026-11-05",
            SnapshotSource::BrokerImported,
            5,
            Some(BasisStatus::Unknown),
        )];
        assert_eq!(
            suggested_opening_unit_cost(
                &untouched,
                "USD",
                "AMD",
                d("2026-11-10"),
                Decimal::from(7),
                &intervening_obs,
                &[],
                &[],
                Some(d("2026-11-01")),
            ),
            None
        );
        assert_eq!(
            suggested_opening_unit_cost(
                &untouched,
                "USD",
                "AMD",
                d("2026-11-10"),
                Decimal::from(7),
                &[],
                &[],
                &[],
                None,
            ),
            None
        );
        assert_eq!(
            suggested_opening_unit_cost(
                &untouched,
                "USD",
                "AMD",
                d("2026-11-10"),
                Decimal::from(7),
                &[],
                &[],
                &[],
                Some(d("2026-11-01")),
            ),
            Some(Decimal::from(40))
        );
        assert_eq!(
            suggested_opening_unit_cost(
                &untouched,
                "USD",
                "AMD",
                d("2026-11-10"),
                Decimal::from(7),
                &[],
                &[],
                &[],
                Some(d("2026-11-02")),
            ),
            None
        );
        // Statement trade after opening_day blocks mid-interval fallback.
        let traded = vec![stmt(
            "2026-10-01",
            "2026-10-31",
            "7",
            Some("280"),
            vec![StatementEvent {
                source_id: "e1".into(),
                date: d("2026-10-20"),
                kind: StatementEventKind::Buy,
                asset_id: Some("AMD".into()),
                quantity: Some("2".into()),
                cash: "-100".into(),
                source_page: 2,
                source_line: 1,
            }],
        )];
        assert_eq!(
            suggested_opening_unit_cost(
                &traded,
                "USD",
                "AMD",
                d("2026-10-14"),
                Decimal::from(7),
                &[],
                &[],
                &[],
                None,
            ),
            None
        );
    }

    #[test]
    fn covering_statement_conflicting_basis_withholds_fallback_and_opening_hint() {
        let oct = stmt("2026-10-01", "2026-10-31", "7", Some("280"), vec![]);
        let nov_conflict = stmt("2026-11-01", "2026-11-30", "7", Some("210"), vec![]);
        let nov_match = stmt("2026-11-01", "2026-11-30", "7", Some("280"), vec![]);
        let nov_missing_basis = stmt("2026-11-01", "2026-11-30", "7", None, vec![]);
        let nov_post_buy = stmt(
            "2026-11-01",
            "2026-11-30",
            "10",
            Some("430"),
            vec![StatementEvent {
                source_id: "e-post".into(),
                date: d("2026-11-28"),
                kind: StatementEventKind::Buy,
                asset_id: Some("AMD".into()),
                quantity: Some("3".into()),
                cash: "-150".into(),
                source_page: 2,
                source_line: 1,
            }],
        );
        let nov_prior_trade = stmt(
            "2026-11-01",
            "2026-11-30",
            "7",
            Some("210"),
            vec![
                StatementEvent {
                    source_id: "e-sell".into(),
                    date: d("2026-11-10"),
                    kind: StatementEventKind::Sell,
                    asset_id: Some("AMD".into()),
                    quantity: Some("7".into()),
                    cash: "210".into(),
                    source_page: 2,
                    source_line: 1,
                },
                StatementEvent {
                    source_id: "e-buy".into(),
                    date: d("2026-11-10"),
                    kind: StatementEventKind::Buy,
                    asset_id: Some("AMD".into()),
                    quantity: Some("7".into()),
                    cash: "-210".into(),
                    source_page: 2,
                    source_line: 2,
                },
            ],
        );

        let no_provider = super::super::ProviderTradeHistory::default();
        let covered_provider = super::super::ProviderTradeHistory {
            coverage_from: Some(d("2026-11-01")),
            coverage_through: Some(d("2026-11-24")),
            events: BTreeMap::new(),
            verification_incomplete: false,
        };

        // Two consecutive activity-complete statements with matching quantity (7) and
        // zero security events but conflicting closing basis ($280 vs $210) must
        // withhold fallback both when the later statement supplies coverage and when
        // provider trade history covers (Oct 31, Nov 24].
        let conflicting_chain = vec![oct.clone(), nov_conflict.clone()];
        for history in [&no_provider, &covered_provider] {
            let mut s = snap(
                "2026-11-24",
                SnapshotSource::BrokerImported,
                7,
                Some(BasisStatus::Unknown),
            );
            assert!(!apply_statement_basis_to_snapshot(
                &mut s,
                &conflicting_chain,
                &[],
                &[],
                history,
            ));
            assert_eq!(s.positions["AMD"].basis_status(), BasisStatus::Unknown);
        }

        // When the covering statement confirms the same quantity and closing basis
        // ($280) with no post-snapshot events or conflicting observations, fallback applies.
        let matching_chain = vec![oct.clone(), nov_match.clone()];
        let mut confirmed = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(apply_statement_basis_to_snapshot(
            &mut confirmed,
            &matching_chain,
            &[],
            &[],
            &no_provider,
        ));
        assert_eq!(
            confirmed.positions["AMD"].total_cost_basis,
            Decimal::from(280)
        );
        assert_eq!(
            confirmed.positions["AMD"].basis_status(),
            BasisStatus::Complete
        );

        // A post-snapshot observed quantity mismatch blocks statement-supplied coverage.
        let post_mismatch = snap(
            "2026-11-26",
            SnapshotSource::BrokerImported,
            5,
            Some(BasisStatus::Unknown),
        );
        let mut blocked_by_post_obs = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(!apply_statement_basis_to_snapshot(
            &mut blocked_by_post_obs,
            &matching_chain,
            &[post_mismatch],
            &[],
            &no_provider,
        ));

        // Covering statement with omitted closing basis cannot supply coverage alone,
        // but does not conflict when provider trade history already covers the interval.
        let missing_basis_chain = vec![oct.clone(), nov_missing_basis];
        let mut without_provider = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(!apply_statement_basis_to_snapshot(
            &mut without_provider,
            &missing_basis_chain,
            &[],
            &[],
            &no_provider,
        ));
        let mut with_provider = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(apply_statement_basis_to_snapshot(
            &mut with_provider,
            &missing_basis_chain,
            &[],
            &[],
            &covered_provider,
        ));
        assert_eq!(
            with_provider.positions["AMD"].total_cost_basis,
            Decimal::from(280)
        );

        // A post-snapshot Buy on Nov 28 prevents the Nov statement from confirming
        // pre-Nov-24 basis on its own, while Plaid trade coverage through Nov 24 allows it.
        let post_buy_chain = vec![oct.clone(), nov_post_buy];
        let mut post_buy_no_provider = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(!apply_statement_basis_to_snapshot(
            &mut post_buy_no_provider,
            &post_buy_chain,
            &[],
            &[],
            &no_provider,
        ));
        let mut post_buy_with_provider = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(apply_statement_basis_to_snapshot(
            &mut post_buy_with_provider,
            &post_buy_chain,
            &[],
            &[],
            &covered_provider,
        ));
        assert_eq!(
            post_buy_with_provider.positions["AMD"].total_cost_basis,
            Decimal::from(280)
        );

        // Symmetrically, suggested_opening_unit_cost withholds a mid-statement hint
        // when the contiguous preceding statement reports conflicting basis with no
        // dated event explaining the change, while allowing matching basis, dated
        // pre-opening trades, or an opening_day on the closing date itself.
        assert_eq!(
            suggested_opening_unit_cost(
                &conflicting_chain,
                "USD",
                "AMD",
                d("2026-11-24"),
                Decimal::from(7),
                &[],
                &[],
                &[],
                None,
            ),
            None
        );
        assert_eq!(
            suggested_opening_unit_cost(
                &matching_chain,
                "USD",
                "AMD",
                d("2026-11-24"),
                Decimal::from(7),
                &[],
                &[],
                &[],
                None,
            ),
            Some(Decimal::from(40))
        );
        assert_eq!(
            suggested_opening_unit_cost(
                &[oct.clone(), nov_prior_trade.clone()],
                "USD",
                "AMD",
                d("2026-11-24"),
                Decimal::from(7),
                &[],
                &[],
                &[],
                None,
            ),
            Some(Decimal::from(30))
        );
        assert_eq!(
            suggested_opening_unit_cost(
                &conflicting_chain,
                "USD",
                "AMD",
                d("2026-11-30"),
                Decimal::from(7),
                &[],
                &[],
                &[],
                None,
            ),
            Some(Decimal::from(30))
        );

        // Multi-hop contiguous chains with an intermediate omitted basis still detect
        // undated basis changes in both directions, and statement_basis_history_window
        // spans through contiguous later statements.
        let sep = stmt("2026-09-01", "2026-09-30", "7", Some("280"), vec![]);
        let oct_none = stmt("2026-10-01", "2026-10-31", "7", None, vec![]);
        let dec_conflict = stmt("2026-12-01", "2026-12-31", "7", Some("210"), vec![]);
        let multi_forward = vec![
            oct.clone(),
            stmt("2026-11-01", "2026-11-30", "7", None, vec![]),
            dec_conflict,
        ];
        assert_eq!(
            statement_basis_history_window(&multi_forward, "USD", d("2026-11-24")),
            Some((d("2026-10-31"), d("2026-12-31")))
        );
        let mut multi_forward_snap = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(!apply_statement_basis_to_snapshot(
            &mut multi_forward_snap,
            &multi_forward,
            &[],
            &[],
            &covered_provider,
        ));
        assert_eq!(
            suggested_opening_unit_cost(
                &[sep, oct_none, nov_conflict.clone()],
                "USD",
                "AMD",
                d("2026-11-24"),
                Decimal::from(7),
                &[],
                &[],
                &[],
                None,
            ),
            None
        );

        // Conflicting observed snapshots on the other side of the target date block
        // fallback/hints when no dated event explains the change, while a dated trade
        // between an earlier observation and opening_day still allows the new basis.
        let mut post_conflict_obs = snap(
            "2026-11-26",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Complete),
        );
        post_conflict_obs
            .positions
            .get_mut("AMD")
            .unwrap()
            .total_cost_basis = Decimal::from(210);
        let mut with_provider_post_conflict = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(!apply_statement_basis_to_snapshot(
            &mut with_provider_post_conflict,
            &missing_basis_chain,
            &[post_conflict_obs],
            &[],
            &covered_provider,
        ));
        let mut pre_opening_conflict = snap(
            "2026-11-10",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Complete),
        );
        pre_opening_conflict
            .positions
            .get_mut("AMD")
            .unwrap()
            .total_cost_basis = Decimal::from(280);
        assert_eq!(
            suggested_opening_unit_cost(
                std::slice::from_ref(&nov_conflict),
                "USD",
                "AMD",
                d("2026-11-24"),
                Decimal::from(7),
                &[pre_opening_conflict],
                &[],
                &[],
                None,
            ),
            None
        );
        let mut pre_trade_obs = snap(
            "2026-11-05",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Complete),
        );
        pre_trade_obs
            .positions
            .get_mut("AMD")
            .unwrap()
            .total_cost_basis = Decimal::from(280);
        assert_eq!(
            suggested_opening_unit_cost(
                &[oct.clone(), nov_prior_trade],
                "USD",
                "AMD",
                d("2026-11-24"),
                Decimal::from(7),
                &[pre_trade_obs],
                &[],
                &[],
                None,
            ),
            Some(Decimal::from(30))
        );

        // Duplicate closing_positions rows for the same asset fail closed.
        let mut dup_oct = oct;
        dup_oct.interval.closing_positions.push(ClosingPosition {
            asset_id: "AMD".into(),
            quantity: "3".into(),
            market_value: "150".into(),
            cost_basis: None,
        });
        let mut dup_snap = snap(
            "2026-10-31",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(!apply_statement_basis_to_snapshot(
            &mut dup_snap,
            std::slice::from_ref(&dup_oct),
            &[],
            &[],
            &no_provider,
        ));
        assert_eq!(
            suggested_opening_unit_cost(
                &[dup_oct],
                "USD",
                "AMD",
                d("2026-10-31"),
                Decimal::from(7),
                &[],
                &[],
                &[],
                None,
            ),
            None
        );
    }
}
