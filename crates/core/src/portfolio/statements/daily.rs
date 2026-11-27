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

fn recorded_in_currency(
    amount: Option<rust_decimal::Decimal>,
    currency: &str,
    target: &str,
) -> Option<rust_decimal::Decimal> {
    use crate::fx::currency::{normalize_amount, normalize_currency_code};
    let ccy = currency.trim();
    let tgt = target.trim();
    if ccy.is_empty()
        || tgt.is_empty()
        || !normalize_currency_code(ccy).eq_ignore_ascii_case(normalize_currency_code(tgt))
    {
        return None;
    }
    let denom = normalize_amount(rust_decimal::Decimal::ONE, tgt).0;
    normalize_amount(amount?, ccy).0.checked_div(denom)
}

#[derive(Clone)]
struct ObservedPosition {
    quantity: rust_decimal::Decimal,
    is_complete: bool,
    position_currency: String,
    snapshot_currency: String,
    snapshot_base_currency: Option<String>,
    total_cost_basis: rust_decimal::Decimal,
    cost_basis_account: Option<rust_decimal::Decimal>,
    cost_basis_base: Option<rust_decimal::Decimal>,
}

impl ObservedPosition {
    fn conflicts_with(
        &self,
        expected_qty: rust_decimal::Decimal,
        expected_basis: rust_decimal::Decimal,
        target_currency: &str,
    ) -> bool {
        if self.quantity != expected_qty {
            return true;
        }
        if !self.is_complete {
            return false;
        }
        let pos_ccy = if self.position_currency.trim().is_empty() {
            self.snapshot_currency.as_str()
        } else {
            self.position_currency.as_str()
        };
        let direct = recorded_in_currency(Some(self.total_cost_basis), pos_ccy, target_currency);
        let acct = recorded_in_currency(
            self.cost_basis_account,
            &self.snapshot_currency,
            target_currency,
        );
        let base = self.snapshot_base_currency.as_deref().and_then(|base_ccy| {
            recorded_in_currency(self.cost_basis_base, base_ccy, target_currency)
        });
        let mut agreed: Option<rust_decimal::Decimal> = None;
        for candidate in [direct, acct, base].into_iter().flatten() {
            if let Some(prev) = agreed {
                if prev != candidate {
                    return true;
                }
            } else {
                agreed = Some(candidate);
            }
        }
        agreed != Some(expected_basis)
    }
}

type ObservedHistoryEntry = (NaiveDate, BTreeMap<String, ObservedPosition>);

fn collect_observed_history(
    snapshots: &[crate::portfolio::snapshot::AccountStateSnapshot],
) -> Vec<ObservedHistoryEntry> {
    use crate::portfolio::{economic_events::BasisStatus, snapshot::SnapshotSource};
    let mut history: Vec<ObservedHistoryEntry> = snapshots
        .iter()
        .filter(|s| s.source != SnapshotSource::Calculated)
        .map(|s| {
            let positions = s
                .positions
                .values()
                .filter(|p| !p.quantity.is_zero())
                .map(|p| {
                    (
                        p.asset_id.clone(),
                        ObservedPosition {
                            quantity: p.quantity,
                            is_complete: p.basis_status() == BasisStatus::Complete,
                            position_currency: p.currency.clone(),
                            snapshot_currency: s.currency.clone(),
                            snapshot_base_currency: s.base_currency.clone(),
                            total_cost_basis: p.total_cost_basis,
                            cost_basis_account: p.cost_basis_account,
                            cost_basis_base: p.cost_basis_base,
                        },
                    )
                })
                .collect();
            (s.snapshot_date, positions)
        })
        .collect();
    history.sort_by_key(|(date, _)| *date);
    history
}

fn observed_slice(
    observed_history: &[ObservedHistoryEntry],
    from_inclusive: NaiveDate,
    to_inclusive: NaiveDate,
) -> &[ObservedHistoryEntry] {
    if from_inclusive > to_inclusive {
        return &[];
    }
    let start = observed_history.partition_point(|(date, _)| *date < from_inclusive);
    let end = start + observed_history[start..].partition_point(|(date, _)| *date <= to_inclusive);
    &observed_history[start..end]
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

/// Merges a prior verified trade coverage window `[prior_start, prior_end]` with
/// a newly fetched `[fetch_start, fetch_end]` window when the two windows overlap
/// or abut (`fetch_start <= prior_end + 1` and `prior_start <= fetch_end + 1`).
/// Disjoint or inverted prior windows reset to `(fetch_start, fetch_end)`.
pub fn merge_trade_coverage_window(
    prior_start: Option<NaiveDate>,
    prior_end: Option<NaiveDate>,
    fetch_start: NaiveDate,
    fetch_end: NaiveDate,
) -> (NaiveDate, NaiveDate) {
    match (prior_start, prior_end) {
        (Some(start), Some(end))
            if start <= end
                && fetch_start <= fetch_end
                && end.succ_opt().is_some_and(|next| fetch_start <= next)
                && fetch_end.succ_opt().is_some_and(|next| start <= next) =>
        {
            (start.min(fetch_start), end.max(fetch_end))
        }
        _ => (fetch_start, fetch_end),
    }
}

/// Updates the continuous provider trade history start date when a new fetch
/// covers `[fetch_start, fetch_end]`.
pub fn merge_trade_coverage_start(
    prior_start: Option<NaiveDate>,
    prior_end: Option<NaiveDate>,
    fetch_start: NaiveDate,
) -> NaiveDate {
    match (prior_start, prior_end) {
        (Some(start), Some(end))
            if start <= end && end.succ_opt().is_some_and(|next| fetch_start <= next) =>
        {
            start.min(fetch_start)
        }
        _ => fetch_start,
    }
}

/// Bounds retained provider trade records to `max_records` and any additional
/// caller size invariant `fits` by dropping complete oldest calendar days so no
/// remaining day has partial trade evidence, while preserving cancellation-linked
/// pairs across the cutoff whenever any member falls in the retained coverage
/// window. Returns the adjusted inclusive `(coverage_start, coverage_end)`
/// window (or `None` if `coverage_end` itself must be discarded).
#[allow(clippy::too_many_arguments)]
pub fn prune_retained_trades_until<T>(
    items: &mut Vec<T>,
    coverage_start: NaiveDate,
    coverage_end: NaiveDate,
    max_records: usize,
    id_of: impl Fn(&T) -> &str,
    cancel_target_of: impl Fn(&T) -> Option<&str>,
    date_of: impl Fn(&T) -> NaiveDate,
    mut fits: impl FnMut(&[T], Option<(NaiveDate, NaiveDate)>) -> bool,
) -> Option<(NaiveDate, NaiveDate)> {
    if coverage_start > coverage_end {
        items.clear();
        return None;
    }
    items.retain(|item| date_of(item) <= coverage_end);
    items.sort_by_key(&date_of);

    let n = items.len();
    let mut by_id: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::with_capacity(n);
    for (idx, item) in items.iter().enumerate() {
        let id = id_of(item);
        if !id.is_empty() {
            by_id.entry(id.to_owned()).or_default().push(idx);
        }
    }
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (idx, item) in items.iter().enumerate() {
        let id = id_of(item);
        if let Some(target) = cancel_target_of(item).filter(|t| !t.is_empty() && *t != id) {
            if let Some(peers) = by_id.get(target) {
                for &peer in peers {
                    if peer != idx {
                        adj[idx].push(peer);
                        adj[peer].push(idx);
                    }
                }
            }
        }
    }
    let mut eff_dates: Vec<NaiveDate> = items.iter().map(&date_of).collect();
    let mut visited = vec![false; n];
    let mut stack = Vec::new();
    let mut component = Vec::new();
    for start_idx in 0..n {
        if visited[start_idx] || adj[start_idx].is_empty() {
            continue;
        }
        visited[start_idx] = true;
        stack.clear();
        component.clear();
        stack.push(start_idx);
        let mut max_date = eff_dates[start_idx];
        while let Some(curr) = stack.pop() {
            component.push(curr);
            if eff_dates[curr] > max_date {
                max_date = eff_dates[curr];
            }
            for &next in &adj[curr] {
                if !visited[next] {
                    visited[next] = true;
                    stack.push(next);
                }
            }
        }
        for &member in &component {
            eff_dates[member] = max_date;
        }
    }

    let mut tagged: Vec<(NaiveDate, T)> = eff_dates
        .into_iter()
        .zip(items.drain(..))
        .filter(|(eff, _)| *eff >= coverage_start)
        .collect();
    tagged.sort_by_key(|(eff, item)| (*eff, date_of(item)));
    let mut tagged_effs: Vec<NaiveDate> = Vec::with_capacity(tagged.len());
    for (eff, item) in tagged {
        tagged_effs.push(eff);
        items.push(item);
    }

    let mut start = coverage_start;
    let mut offset = 0usize;
    while items.len() - offset > max_records {
        let oldest = tagged_effs[offset];
        let step = tagged_effs[offset..].partition_point(|&eff| eff <= oldest);
        offset += step;
        match oldest.succ_opt() {
            Some(next) if next <= coverage_end => start = start.max(next),
            _ => {
                items.clear();
                return None;
            }
        }
    }
    if start > coverage_end {
        items.clear();
        return None;
    }
    if offset > 0 {
        items.drain(..offset);
        tagged_effs.drain(..offset);
    }
    if items.is_empty() || fits(items, Some((start, coverage_end))) {
        items.sort_by_key(&date_of);
        return Some((start, coverage_end));
    }
    // Collect whole-day cut checkpoints `(cut_index, next_coverage_start)`.
    let mut day_cuts: Vec<(usize, Option<NaiveDate>)> = Vec::new();
    let mut idx = 0;
    while idx < items.len() {
        let day = tagged_effs[idx];
        let next_idx = idx + tagged_effs[idx..].partition_point(|&eff| eff <= day);
        let next_start = day
            .succ_opt()
            .filter(|next| *next <= coverage_end)
            .map(|next| start.max(next));
        day_cuts.push((next_idx, next_start));
        idx = next_idx;
    }
    let chosen = day_cuts.partition_point(|&(cut, next_start)| match next_start {
        Some(ns) => !fits(&items[cut..], Some((ns, coverage_end))),
        None => false,
    });
    if let Some(&(cut, Some(ns))) = day_cuts.get(chosen) {
        items.drain(..cut);
        items.sort_by_key(&date_of);
        Some((ns, coverage_end))
    } else {
        items.clear();
        None
    }
}

/// Bounds retained provider trade records to `max_records` by dropping complete
/// oldest calendar days so no remaining day has partial trade evidence.
pub fn prune_retained_trades_by_day<T>(
    items: &mut Vec<T>,
    coverage_start: NaiveDate,
    coverage_end: NaiveDate,
    max_records: usize,
    date_of: impl Fn(&T) -> NaiveDate,
) -> Option<(NaiveDate, NaiveDate)> {
    prune_retained_trades_until(
        items,
        coverage_start,
        coverage_end,
        max_records,
        |_| "",
        |_| None,
        date_of,
        |_, _| true,
    )
}

/// Fresh provider transaction record supplied to [`reconcile_source_history_and_stage_trades`].
pub struct StageProviderTx<'a> {
    pub id: &'a str,
    pub account_id: &'a str,
    pub security_id: Option<&'a str>,
    pub cancel_target: Option<&'a str>,
    pub date: NaiveDate,
    pub transaction_type: &'a str,
    pub subtype: &'a str,
    pub quantity_nonzero: bool,
}

/// Reconciled security trade group for investment preview opening-cost checks.
pub struct ReconciledSecurityTrades {
    pub security_id: String,
    pub ticker_symbol: Option<String>,
    pub explicit_asset_ids: Vec<String>,
    pub events: Vec<super::ProviderTradeEvent>,
}

/// Reconciled trade coverage and per-security paired events across saved
/// `source_history` and a fresh provider fetch.
pub struct ReconciledPreviewTrades {
    pub source_coverage: (Option<NaiveDate>, Option<NaiveDate>),
    pub coverage: (NaiveDate, NaiveDate),
    pub by_security: Vec<ReconciledSecurityTrades>,
}

/// Validates and merges saved `source_history` trade records with fresh `stage_txs`
/// by transaction ID (fresh fetch wins; empty IDs are preserved without overwriting)
/// before pairing cancellations. Returns `None` (fail closed) on any malformed
/// `source_history` field or unattributed security event.
pub fn reconcile_source_history_and_stage_trades(
    external_account_id: &str,
    account_currency: &str,
    source_history: &serde_json::Value,
    stage_txs: &[StageProviderTx<'_>],
    plan_security_symbols: &BTreeMap<String, Option<String>>,
    fetch_start: NaiveDate,
    fetch_end: NaiveDate,
) -> Option<ReconciledPreviewTrades> {
    #[derive(serde::Deserialize)]
    struct SavedTxView {
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
    struct SavedSecView {
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
        Some(serde_json::Value::String(s)) => chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.with_timezone(&chrono::Utc)),
        Some(_) => return None,
    };
    if matches!(
        source_history.get("asOf"),
        Some(serde_json::Value::String(_))
    ) && as_of.is_none()
    {
        return None;
    }
    let history_start = parse_opt_date("historyStart").ok()?;
    let history_end = parse_opt_date("historyEnd").ok()?;
    let coverage_start = parse_opt_date("coverageStart").ok()?;
    let has_coverage_meta = source_history.get("asOf").is_some()
        || source_history.get("historyStart").is_some()
        || source_history.get("historyEnd").is_some()
        || source_history.get("coverageStart").is_some();
    let raw_from = history_start
        .or(coverage_start)
        .or_else(|| as_of.and_then(|ts| ts.date_naive().checked_sub_days(chrono::Days::new(730))));
    let raw_through = history_end.or_else(|| as_of.map(|ts| ts.date_naive()));
    let source_coverage = match (raw_from, raw_through) {
        (Some(s), Some(e)) if s > e => (None, None),
        pair => pair,
    };

    let saved_txs: Vec<SavedTxView> = match source_history.get("transactions") {
        None => {
            if has_coverage_meta {
                return None;
            }
            Vec::new()
        }
        Some(serde_json::Value::Array(arr)) => {
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                out.push(serde_json::from_value::<SavedTxView>(item.clone()).ok()?);
            }
            out
        }
        Some(_) => return None,
    };
    let mut saved_symbols: BTreeMap<String, String> = BTreeMap::new();
    match source_history.get("securities") {
        None | Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::Array(arr)) => {
            for item in arr {
                let sec = serde_json::from_value::<SavedSecView>(item.clone()).ok()?;
                if sec
                    .iso_currency_code
                    .as_deref()
                    .is_none_or(|ccy| ccy.is_empty() || ccy.eq_ignore_ascii_case(account_currency))
                {
                    if let Some(sym) = sec
                        .ticker_symbol
                        .map(|t| t.trim().to_owned())
                        .filter(|t| !t.is_empty())
                    {
                        saved_symbols.insert(sec.security_id, sym);
                    }
                }
            }
        }
        Some(_) => return None,
    }
    let mut explicit_assets: BTreeMap<String, Vec<String>> = BTreeMap::new();
    match source_history.get("assetIdsBySecurity") {
        None | Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::Object(map)) => {
            for (sec_id, val) in map {
                let aid = val.as_str()?;
                if !aid.is_empty() {
                    explicit_assets
                        .entry(sec_id.clone())
                        .or_default()
                        .push(aid.to_owned());
                }
            }
        }
        Some(_) => return None,
    }
    match source_history.get("basisBySecurity") {
        None | Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::Object(map)) => {
            for (sec_id, entry) in map {
                if let Some(aid) = entry
                    .get("assetId")
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    let list = explicit_assets.entry(sec_id.clone()).or_default();
                    if !list.iter().any(|existing| existing == aid) {
                        list.push(aid.to_owned());
                    }
                }
            }
        }
        Some(_) => return None,
    }

    let parse_qty_nonzero = |v: &serde_json::Value| -> Option<bool> {
        match v {
            serde_json::Value::Null => Some(false),
            serde_json::Value::String(s) => s
                .parse::<rust_decimal::Decimal>()
                .ok()
                .map(|d| !d.is_zero()),
            serde_json::Value::Number(n) => n
                .to_string()
                .parse::<rust_decimal::Decimal>()
                .ok()
                .map(|d| !d.is_zero()),
            _ => None,
        }
    };
    let mut by_id: BTreeMap<&str, RawProviderTx<'_>> = BTreeMap::new();
    let mut unnamed: Vec<RawProviderTx<'_>> = Vec::new();
    for tx in &saved_txs {
        if !tx.account_id.is_empty() && tx.account_id != external_account_id {
            continue;
        }
        let qty_nonzero = parse_qty_nonzero(&tx.quantity)?;
        let raw = RawProviderTx {
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
        };
        if raw.id.is_empty() {
            unnamed.push(raw);
        } else {
            by_id.insert(raw.id, raw);
        }
    }
    for tx in stage_txs {
        if !tx.account_id.is_empty() && tx.account_id != external_account_id {
            continue;
        }
        let raw = RawProviderTx {
            id: tx.id,
            security_key: tx.security_id,
            date: tx.date,
            cancel_target: tx.cancel_target,
            basis_changing: is_provider_tx_basis_changing(
                tx.transaction_type,
                tx.subtype,
                tx.quantity_nonzero,
            ),
            requires_security_key: provider_tx_requires_security_key(
                tx.transaction_type,
                tx.subtype,
                tx.quantity_nonzero,
            ),
        };
        if raw.id.is_empty() {
            unnamed.push(raw);
        } else {
            by_id.insert(raw.id, raw);
        }
    }
    let mut raw_txs: Vec<RawProviderTx<'_>> = by_id.into_values().collect();
    raw_txs.extend(unnamed);
    let paired = pair_provider_trade_events(&raw_txs);
    if paired.has_unattributed_event {
        return None;
    }
    let coverage =
        merge_trade_coverage_window(source_coverage.0, source_coverage.1, fetch_start, fetch_end);
    let by_security = paired
        .events
        .into_iter()
        .map(|(sec_id, events)| {
            let ticker_symbol = plan_security_symbols
                .get(sec_id)
                .and_then(|s| s.clone())
                .filter(|s| !s.is_empty())
                .or_else(|| saved_symbols.get(sec_id).cloned());
            let explicit_asset_ids = explicit_assets.remove(sec_id).unwrap_or_default();
            ReconciledSecurityTrades {
                security_id: sec_id.to_owned(),
                ticker_symbol,
                explicit_asset_ids,
                events,
            }
        })
        .collect();
    Some(ReconciledPreviewTrades {
        source_coverage,
        coverage,
        by_security,
    })
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

fn covered_post_horizon(
    statements: &[StoredDailyStatement],
    currency: &str,
    snapshot_date: NaiveDate,
    provider_trades: &super::ProviderTradeHistory,
) -> NaiveDate {
    let mut horizon = snapshot_date;
    loop {
        let prev_horizon = horizon;
        if let (Some(cov_from), Some(cov_through)) = (
            provider_trades.coverage_from,
            provider_trades.coverage_through,
        ) {
            if !provider_trades.verification_incomplete
                && horizon.succ_opt().is_some_and(|next| cov_from <= next)
                && cov_through > horizon
            {
                horizon = cov_through;
            }
        }
        for s in statements {
            if s.interval.currency == currency
                && s.interval.activity_complete
                && horizon
                    .succ_opt()
                    .is_some_and(|next| s.interval.from <= next)
                && s.interval.through > horizon
            {
                horizon = s.interval.through;
            }
        }
        if horizon == prev_horizon {
            break;
        }
    }
    horizon
}

fn covered_pre_horizon(
    statements: &[StoredDailyStatement],
    currency: &str,
    target_date: NaiveDate,
    trade_coverage: Option<(NaiveDate, NaiveDate)>,
) -> NaiveDate {
    let mut horizon = target_date;
    loop {
        let prev_horizon = horizon;
        if let Some((cov_from, cov_through)) = trade_coverage {
            if cov_from <= horizon && horizon <= cov_through {
                if let Some(pred) = cov_from.pred_opt() {
                    if pred < horizon {
                        horizon = pred;
                    }
                }
            }
        }
        for s in statements {
            if s.interval.currency == currency
                && s.interval.activity_complete
                && s.interval.from <= horizon
                && horizon <= s.interval.through
            {
                if let Some(pred) = s.interval.from.pred_opt() {
                    if pred < horizon {
                        horizon = pred;
                    }
                }
            }
        }
        if horizon == prev_horizon {
            break;
        }
    }
    horizon
}

/// Returns the inclusive `(from, through)` date range of observed snapshots
/// needed to verify statement basis fallback for `snapshot_date` in `currency`,
/// or `None` when no earlier statement applies (or `stmt_date == snapshot_date`).
pub fn statement_basis_history_window(
    statements: &[StoredDailyStatement],
    currency: &str,
    snapshot_date: NaiveDate,
    provider_trades: &super::ProviderTradeHistory,
) -> Option<(NaiveDate, NaiveDate)> {
    let stmt_date = statements
        .iter()
        .filter(|s| s.interval.through <= snapshot_date && s.interval.currency == currency)
        .map(|s| s.interval.through)
        .max()?;
    if stmt_date == snapshot_date {
        return None;
    }
    let upper = covered_post_horizon(statements, currency, snapshot_date, provider_trades);
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
    let post_horizon = (stmt_date < snapshot.snapshot_date).then(|| {
        covered_post_horizon(
            statements,
            &snapshot.currency,
            snapshot.snapshot_date,
            provider_trades,
        )
    });
    let post_stmts: Vec<&StoredDailyStatement> = match post_horizon {
        Some(horizon) if covered_by_provider && horizon > snapshot.snapshot_date => {
            let mut list: Vec<&StoredDailyStatement> = statements
                .iter()
                .filter(|s| {
                    s.interval.currency == snapshot.currency
                        && snapshot.snapshot_date < s.interval.through
                        && s.interval.through <= horizon
                })
                .collect();
            list.sort_by_key(|s| s.interval.through);
            list
        }
        _ => Vec::new(),
    };
    let snap_currency = snapshot.currency.clone();
    let snap_date = snapshot.snapshot_date;
    let snap_base_currency = snapshot.base_currency.clone();
    let mut updated = false;
    for pos in snapshot.positions.values_mut() {
        if pos.basis_status() != BasisStatus::Unknown
            || pos.quantity <= Decimal::ZERO
            || (!pos.currency.is_empty() && pos.currency != snap_currency)
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
                from_exclusive.succ_opt().is_some_and(|from_inc| {
                    observed_slice(observed_history, from_inc, to_inclusive)
                        .iter()
                        .any(|(_, positions)| {
                            positions.get(&pos.asset_id).is_none_or(|obs| {
                                obs.conflicts_with(pos.quantity, stmt_basis, &snap_currency)
                            })
                        })
                })
            };
        let pre_conflict = snap_date.pred_opt().is_some_and(|before_snap| {
            observed_slice(observed_history, stmt_date, before_snap)
                .iter()
                .any(|(_, positions)| {
                    positions.get(&pos.asset_id).is_none_or(|obs| {
                        obs.conflicts_with(pos.quantity, stmt_basis, &snap_currency)
                    })
                })
        });
        if stmt_qty != pos.quantity || has_event_in(stmt_date, snap_date) || pre_conflict {
            continue;
        }
        if let Some(horizon) = post_horizon {
            if !covered_by_provider {
                let confirmed_by_next = containing.is_some_and(|next_stmt| {
                    !has_event_in(snap_date, next_stmt.interval.through)
                        && !observed_mismatch_after(snap_date, next_stmt.interval.through)
                        && match lookup_closing_position(next_stmt, &pos.asset_id) {
                            ClosingLookup::Unique(cp) => {
                                amount(&cp.quantity).ok() == Some(pos.quantity)
                                    && cp
                                        .cost_basis
                                        .as_deref()
                                        .is_some_and(|raw| amount(raw).ok() == Some(stmt_basis))
                            }
                            ClosingLookup::Missing | ClosingLookup::Duplicate => false,
                        }
                });
                if !confirmed_by_next {
                    continue;
                }
            } else if horizon > snap_date {
                let mut check_obs_through = horizon;
                let mut conflicts_with_later = false;
                for stmt in &post_stmts {
                    if has_event_in(snap_date, stmt.interval.through) {
                        continue;
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
                                } else {
                                    check_obs_through = stmt.interval.through;
                                }
                                break;
                            }
                        }
                    }
                }
                let post_obs_conflict = snap_date.succ_opt().is_some_and(|after_snap| {
                    observed_slice(observed_history, after_snap, check_obs_through)
                        .iter()
                        .any(|(obs_date, positions)| {
                            !has_event_in(snap_date, *obs_date)
                                && positions.get(&pos.asset_id).is_none_or(|obs| {
                                    obs.conflicts_with(pos.quantity, stmt_basis, &snap_currency)
                                })
                        })
                });
                if conflicts_with_later || post_obs_conflict {
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
        if snap_base_currency.as_deref() == Some(snap_currency.as_str()) {
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
            if (p.currency.is_empty() || p.currency == snapshot.currency)
                && p.basis_status() == BasisStatus::Complete
            {
                if let Some(sum) = total_basis.checked_add(p.total_cost_basis) {
                    total_basis = sum;
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

/// Populate missing (`BasisStatus::Unknown`) cost basis across a slice of
/// snapshots using an explicit `observed_history` slice sorted once.
pub fn apply_statement_basis_to_snapshots_with_history(
    snapshots: &mut [crate::portfolio::snapshot::AccountStateSnapshot],
    statements: &[StoredDailyStatement],
    observed_history: &[crate::portfolio::snapshot::AccountStateSnapshot],
    splits: &[wealthfolio_portfolio_engine::SplitEvent],
    provider_trades: &super::ProviderTradeHistory,
) {
    if statements.is_empty() {
        return;
    }
    let history = collect_observed_history(observed_history);
    for snapshot in snapshots {
        apply_statement_basis_with_history(snapshot, statements, &history, splits, provider_trades);
    }
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
    trade_coverage: Option<(NaiveDate, NaiveDate)>,
) -> Option<rust_decimal::Decimal> {
    use rust_decimal::Decimal;
    if opening_quantity <= Decimal::ZERO {
        return None;
    }
    let asset = wealthfolio_portfolio_engine::model::AssetId::new(asset_id);
    let observed_history = collect_observed_history(observed_snapshots);
    let trade_bounds = match trade_coverage {
        Some((from, through)) => super::ProviderTradeHistory {
            coverage_from: Some(from),
            coverage_through: Some(through),
            events: BTreeMap::new(),
            verification_incomplete: false,
        },
        None => super::ProviderTradeHistory::default(),
    };
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
            !observed_slice(&observed_history, from_inclusive, to_inclusive)
                .iter()
                .any(|(_, positions)| {
                    positions.get(asset_id).is_none_or(|obs| {
                        obs.conflicts_with(opening_quantity, expected_basis, currency)
                    })
                })
        };
    let unchanged_between =
        |from_exclusive: NaiveDate, to_inclusive: NaiveDate, expected_basis: Decimal| -> bool {
            !has_event_in(from_exclusive, to_inclusive)
                && observed_consistent(from_exclusive, to_inclusive, expected_basis)
        };
    let has_post_conflict = |expected_basis: Decimal| -> bool {
        let horizon = covered_post_horizon(statements, currency, opening_day, &trade_bounds);
        if horizon <= opening_day {
            return false;
        }
        let mut post_stmts: Vec<&StoredDailyStatement> = statements
            .iter()
            .filter(|s| {
                s.interval.currency == currency
                    && opening_day < s.interval.through
                    && s.interval.through <= horizon
            })
            .collect();
        post_stmts.sort_by_key(|s| s.interval.through);
        let mut check_obs_through = horizon;
        for stmt in post_stmts {
            if has_event_in(opening_day, stmt.interval.through) {
                continue;
            }
            match lookup_closing_position(stmt, asset_id) {
                ClosingLookup::Duplicate => return true,
                ClosingLookup::Missing => {
                    if stmt.interval.closing_positions_complete {
                        return true;
                    }
                }
                ClosingLookup::Unique(cp) => {
                    if amount(&cp.quantity).ok() != Some(opening_quantity) {
                        return true;
                    }
                    if let Some(raw) = cp.cost_basis.as_deref() {
                        if amount(raw).ok() != Some(expected_basis) {
                            return true;
                        }
                        check_obs_through = stmt.interval.through;
                        break;
                    }
                }
            }
        }
        opening_day.succ_opt().is_some_and(|after_open| {
            observed_slice(&observed_history, after_open, check_obs_through)
                .iter()
                .any(|(obs_date, positions)| {
                    !has_event_in(opening_day, *obs_date)
                        && positions.get(asset_id).is_none_or(|obs| {
                            obs.conflicts_with(opening_quantity, expected_basis, currency)
                        })
                })
        })
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
        let pre_horizon = covered_pre_horizon(statements, currency, opening_day, trade_coverage);
        let mut pre_stmts: Vec<&StoredDailyStatement> = statements
            .iter()
            .filter(|s| {
                s.interval.currency == currency
                    && pre_horizon <= s.interval.through
                    && s.interval.through < containing.interval.from
            })
            .collect();
        pre_stmts.sort_by_key(|s| std::cmp::Reverse(s.interval.through));
        let mut check_obs_from = pre_horizon;
        for prev in pre_stmts {
            if has_event_in(prev.interval.through, opening_day) {
                continue;
            }
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
                        check_obs_from = prev.interval.through;
                        break;
                    }
                }
            }
        }
        if let Some(before_open) = opening_day.pred_opt() {
            for (obs_date, positions) in
                observed_slice(&observed_history, check_obs_from, before_open)
            {
                if !has_event_in(*obs_date, opening_day)
                    && positions.get(asset_id).is_none_or(|obs| {
                        obs.conflicts_with(opening_quantity, stmt_basis, currency)
                    })
                {
                    return None;
                }
            }
        }
        return Some(unit);
    }
    let latest_before = statements
        .iter()
        .filter(|s| s.interval.through < opening_day && s.interval.currency == currency)
        .max_by_key(|s| s.interval.through)?;
    if !trade_bounds.covers_window(latest_before.interval.through, opening_day) {
        return None;
    }
    let (stmt_basis, unit) = statement_basis_and_unit_cost(latest_before)?;
    if !unchanged_between(latest_before.interval.through, opening_day, stmt_basis)
        || has_post_conflict(stmt_basis)
    {
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
                Some((d("2026-11-01"), d("2026-11-30"))),
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
                Some((d("2026-11-01"), d("2026-11-30"))),
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
                Some((d("2026-11-01"), d("2026-11-30"))),
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
                Some((d("2026-11-02"), d("2026-11-30"))),
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
            statement_basis_history_window(
                &multi_forward,
                "USD",
                d("2026-11-24"),
                &covered_provider,
            ),
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
        // fallback/hints when no dated event explains the change — including when a
        // later trade occurs after the conflicting observation — while an observation
        // after a dated trade does not falsely block fallback.
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
            std::slice::from_ref(&post_conflict_obs),
            &[],
            &covered_provider,
        ));
        let mut with_later_buy_after_conflict = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(!apply_statement_basis_to_snapshot(
            &mut with_later_buy_after_conflict,
            &post_buy_chain,
            std::slice::from_ref(&post_conflict_obs),
            &[],
            &covered_provider,
        ));
        assert_eq!(
            with_later_buy_after_conflict.positions["AMD"].basis_status(),
            BasisStatus::Unknown
        );
        let provider_through_nov30 = super::super::ProviderTradeHistory {
            coverage_from: Some(d("2026-11-01")),
            coverage_through: Some(d("2026-11-30")),
            events: BTreeMap::from([(
                "AMD".into(),
                vec![super::super::ProviderTradeEvent {
                    date: d("2026-11-28"),
                    cancelled_on: None,
                }],
            )]),
            verification_incomplete: false,
        };
        assert_eq!(
            statement_basis_history_window(
                std::slice::from_ref(&oct),
                "USD",
                d("2026-11-24"),
                &provider_through_nov30,
            ),
            Some((d("2026-10-31"), d("2026-11-30")))
        );
        let mut provider_only_later_buy = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(!apply_statement_basis_to_snapshot(
            &mut provider_only_later_buy,
            std::slice::from_ref(&oct),
            &[post_conflict_obs],
            &[],
            &provider_through_nov30,
        ));
        let mut obs_after_later_buy = snap(
            "2026-11-29",
            SnapshotSource::BrokerImported,
            10,
            Some(BasisStatus::Complete),
        );
        obs_after_later_buy
            .positions
            .get_mut("AMD")
            .unwrap()
            .total_cost_basis = Decimal::from(430);
        let mut allowed_when_obs_follows_buy = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(apply_statement_basis_to_snapshot(
            &mut allowed_when_obs_follows_buy,
            &post_buy_chain,
            &[obs_after_later_buy],
            &[],
            &covered_provider,
        ));
        assert_eq!(
            allowed_when_obs_follows_buy.positions["AMD"].total_cost_basis,
            Decimal::from(280)
        );
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
                std::slice::from_ref(&dup_oct),
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

        // Cancelled trade pairs straddling an intermediate statement close do not
        // mask post-cancellation (or pre-cancellation) basis conflicts.
        let oct_base = stmt("2026-10-01", "2026-10-31", "7", Some("280"), vec![]);
        let nov_straddled = stmt("2026-11-01", "2026-11-30", "10", None, vec![]);
        let dec_conflict = stmt("2026-12-01", "2026-12-31", "7", Some("210"), vec![]);
        let straddling_provider = super::super::ProviderTradeHistory {
            coverage_from: Some(d("2026-11-01")),
            coverage_through: Some(d("2026-12-31")),
            events: BTreeMap::from([(
                "AMD".into(),
                vec![super::super::ProviderTradeEvent {
                    date: d("2026-11-28"),
                    cancelled_on: Some(d("2026-12-02")),
                }],
            )]),
            verification_incomplete: false,
        };
        let mut snap_straddled = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(!apply_statement_basis_to_snapshot(
            &mut snap_straddled,
            &[oct_base.clone(), nov_straddled, dec_conflict],
            &[],
            &[],
            &straddling_provider,
        ));
        let sep_base = stmt("2026-09-01", "2026-09-30", "7", Some("280"), vec![]);
        let oct_straddled = stmt("2026-10-01", "2026-10-31", "10", None, vec![]);
        assert_eq!(
            suggested_opening_unit_cost(
                &[sep_base.clone(), oct_straddled, nov_conflict.clone()],
                "USD",
                "AMD",
                d("2026-11-24"),
                Decimal::from(7),
                &[],
                &[],
                &[super::super::ProviderTradeEvent {
                    date: d("2026-10-28"),
                    cancelled_on: Some(d("2026-11-02")),
                }],
                None,
            ),
            None
        );

        // Provider trade coverage bridges statement gaps backward and checks
        // post-opening_day conflicts forward in suggested_opening_unit_cost.
        assert_eq!(
            suggested_opening_unit_cost(
                &[sep_base, nov_conflict],
                "USD",
                "AMD",
                d("2026-11-24"),
                Decimal::from(7),
                &[],
                &[],
                &[],
                Some((d("2026-09-01"), d("2026-11-30"))),
            ),
            None
        );
        let mut post_open_obs = snap(
            "2026-11-26",
            SnapshotSource::ManualEntry,
            7,
            Some(BasisStatus::Complete),
        );
        post_open_obs
            .positions
            .get_mut("AMD")
            .unwrap()
            .total_cost_basis = Decimal::from(210);
        assert_eq!(
            suggested_opening_unit_cost(
                &[oct_base],
                "USD",
                "AMD",
                d("2026-11-24"),
                Decimal::from(7),
                &[post_open_obs],
                &[],
                &[],
                Some((d("2026-11-01"), d("2026-11-30"))),
            ),
            None
        );
    }

    #[test]
    fn omitted_csv_basis_allows_fallback_and_hint_while_explicit_zero_blocks_both() {
        let oct_stmt = stmt("2026-10-01", "2026-10-31", "7", Some("280"), vec![]);
        let zero_stmt = stmt("2026-10-01", "2026-10-31", "7", Some("0"), vec![]);
        let trades = super::super::ProviderTradeHistory {
            coverage_from: Some(d("2026-11-01")),
            coverage_through: Some(d("2026-11-30")),
            events: BTreeMap::new(),
            verification_incomplete: false,
        };

        // CSV importer writes omitted average cost as cost_basis_status: None, total_cost_basis: 0.
        let mut csv_omitted_before = snap("2026-11-10", SnapshotSource::CsvImport, 7, None);
        csv_omitted_before
            .positions
            .get_mut("AMD")
            .unwrap()
            .total_cost_basis = Decimal::ZERO;
        let mut csv_omitted_after = snap("2026-11-26", SnapshotSource::CsvImport, 7, None);
        csv_omitted_after
            .positions
            .get_mut("AMD")
            .unwrap()
            .total_cost_basis = Decimal::ZERO;

        let mut target = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(apply_statement_basis_to_snapshot(
            &mut target,
            std::slice::from_ref(&oct_stmt),
            &[csv_omitted_before.clone(), csv_omitted_after.clone()],
            &[],
            &trades,
        ));
        assert_eq!(target.positions["AMD"].total_cost_basis, Decimal::from(280));
        assert_eq!(
            suggested_opening_unit_cost(
                std::slice::from_ref(&oct_stmt),
                "USD",
                "AMD",
                d("2026-11-24"),
                Decimal::from(7),
                &[csv_omitted_before, csv_omitted_after],
                &[],
                &[],
                Some((d("2026-11-01"), d("2026-11-30"))),
            ),
            Some(Decimal::from(40))
        );

        // An explicitly known zero (cost_basis_status: Some(Complete), total_cost_basis: 0)
        // conflicts with USD 280 and blocks both fallback and opening-cost hint, while
        // confirming a USD 0 statement.
        let mut explicit_zero = snap(
            "2026-11-10",
            SnapshotSource::ManualEntry,
            7,
            Some(BasisStatus::Complete),
        );
        explicit_zero
            .positions
            .get_mut("AMD")
            .unwrap()
            .total_cost_basis = Decimal::ZERO;
        let mut blocked = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(!apply_statement_basis_to_snapshot(
            &mut blocked,
            std::slice::from_ref(&oct_stmt),
            std::slice::from_ref(&explicit_zero),
            &[],
            &trades,
        ));
        assert_eq!(
            suggested_opening_unit_cost(
                std::slice::from_ref(&oct_stmt),
                "USD",
                "AMD",
                d("2026-11-24"),
                Decimal::from(7),
                std::slice::from_ref(&explicit_zero),
                &[],
                &[],
                Some((d("2026-11-01"), d("2026-11-30"))),
            ),
            None
        );

        let mut zero_target = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(apply_statement_basis_to_snapshot(
            &mut zero_target,
            std::slice::from_ref(&zero_stmt),
            std::slice::from_ref(&explicit_zero),
            &[],
            &trades,
        ));
        assert_eq!(
            zero_target.positions["AMD"].cost_basis_status,
            Some(BasisStatus::Complete)
        );
        assert_eq!(zero_target.positions["AMD"].total_cost_basis, Decimal::ZERO);
    }

    #[test]
    fn observed_basis_compares_verified_account_currency_amounts() {
        let oct_stmt = stmt("2026-10-01", "2026-10-31", "7", Some("280"), vec![]);
        let trades = super::super::ProviderTradeHistory {
            coverage_from: Some(d("2026-11-01")),
            coverage_through: Some(d("2026-11-30")),
            events: BTreeMap::new(),
            verification_incomplete: false,
        };

        // CAD 280 observation without verified USD conversion cannot confirm USD 280.
        let mut cad_unconverted = snap("2026-11-26", SnapshotSource::ManualEntry, 7, None);
        {
            let p = cad_unconverted.positions.get_mut("AMD").unwrap();
            p.currency = "CAD".into();
            p.total_cost_basis = Decimal::from(280);
            p.cost_basis_account = None;
        }
        let mut target = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(!apply_statement_basis_to_snapshot(
            &mut target,
            std::slice::from_ref(&oct_stmt),
            std::slice::from_ref(&cad_unconverted),
            &[],
            &trades,
        ));
        assert_eq!(
            suggested_opening_unit_cost(
                std::slice::from_ref(&oct_stmt),
                "USD",
                "AMD",
                d("2026-11-24"),
                Decimal::from(7),
                std::slice::from_ref(&cad_unconverted),
                &[],
                &[],
                Some((d("2026-11-01"), d("2026-11-30"))),
            ),
            None
        );

        // Supplying its USD 210 account-currency equivalent also blocks USD 280.
        let mut cad_210_usd = cad_unconverted.clone();
        cad_210_usd
            .positions
            .get_mut("AMD")
            .unwrap()
            .cost_basis_account = Some(Decimal::from(210));
        assert!(!apply_statement_basis_to_snapshot(
            &mut target,
            std::slice::from_ref(&oct_stmt),
            std::slice::from_ref(&cad_210_usd),
            &[],
            &trades,
        ));
        assert_eq!(
            suggested_opening_unit_cost(
                std::slice::from_ref(&oct_stmt),
                "USD",
                "AMD",
                d("2026-11-24"),
                Decimal::from(7),
                std::slice::from_ref(&cad_210_usd),
                &[],
                &[],
                Some((d("2026-11-01"), d("2026-11-30"))),
            ),
            None
        );

        // Verified USD 280 account-currency equivalent confirms USD 280.
        let mut cad_280_usd = cad_unconverted;
        {
            let p = cad_280_usd.positions.get_mut("AMD").unwrap();
            p.total_cost_basis = Decimal::from(380);
            p.cost_basis_account = Some(Decimal::from(280));
        }
        assert!(apply_statement_basis_to_snapshot(
            &mut target,
            std::slice::from_ref(&oct_stmt),
            std::slice::from_ref(&cad_280_usd),
            &[],
            &trades,
        ));
        assert_eq!(target.positions["AMD"].total_cost_basis, Decimal::from(280));
        assert_eq!(
            suggested_opening_unit_cost(
                std::slice::from_ref(&oct_stmt),
                "USD",
                "AMD",
                d("2026-11-24"),
                Decimal::from(7),
                std::slice::from_ref(&cad_280_usd),
                &[],
                &[],
                Some((d("2026-11-01"), d("2026-11-30"))),
            ),
            Some(Decimal::from(40))
        );
        // Contradictory USD cost_basis_account vs USD cost_basis_base fails closed.
        let mut contradictory_base = snap("2026-11-26", SnapshotSource::ManualEntry, 7, None);
        contradictory_base.base_currency = Some("USD".into());
        {
            let p = contradictory_base.positions.get_mut("AMD").unwrap();
            p.currency = "CAD".into();
            p.total_cost_basis = Decimal::from(380);
            p.cost_basis_account = Some(Decimal::from(280));
            p.cost_basis_base = Some(Decimal::from(210));
        }
        let mut contrad_target = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(!apply_statement_basis_to_snapshot(
            &mut contrad_target,
            std::slice::from_ref(&oct_stmt),
            std::slice::from_ref(&contradictory_base),
            &[],
            &trades,
        ));
    }

    #[test]
    fn prune_retained_trades_by_day_drops_complete_oldest_days_and_updates_coverage() {
        let mut items = vec![
            (d("2024-01-01"), "a"),
            (d("2024-01-01"), "b"),
            (d("2024-01-02"), "c"),
            (d("2024-01-03"), "d"),
        ];
        let coverage =
            prune_retained_trades_by_day(&mut items, d("2024-01-01"), d("2026-01-01"), 3, |x| x.0);
        assert_eq!(coverage, Some((d("2024-01-02"), d("2026-01-01"))));
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|(day, _)| *day >= d("2024-01-02")));

        // Predicate-driven pruning (used for MAX_STAGE_BYTES) also drops complete oldest days.
        let mut by_size = vec![
            (d("2024-01-01"), 60_usize),
            (d("2024-01-02"), 50_usize),
            (d("2024-01-03"), 40_usize),
        ];
        let cov_size = prune_retained_trades_until(
            &mut by_size,
            d("2024-01-01"),
            d("2026-01-01"),
            100,
            |_| "",
            |_| None,
            |x| x.0,
            |slice, _| slice.iter().map(|x| x.1).sum::<usize>() <= 95,
        );
        assert_eq!(cov_size, Some((d("2024-01-02"), d("2026-01-01"))));
        assert_eq!(by_size.len(), 2);

        // Forward and reverse cancellation pairs straddling the cutoff keep both legs
        // together while dropping unlinked older records and pre-window pairs.
        #[derive(Clone, Debug, PartialEq, Eq)]
        struct TestTx {
            id: &'static str,
            sec: Option<&'static str>,
            cancel: Option<&'static str>,
            date: NaiveDate,
        }
        let mut cancel_items = vec![
            // Entirely before coverage_start: dropped immediately.
            TestTx {
                id: "pre-buy",
                sec: Some("sec-a"),
                cancel: None,
                date: d("2023-12-30"),
            },
            TestTx {
                id: "pre-cancel",
                sec: None,
                cancel: Some("pre-buy"),
                date: d("2023-12-31"),
            },
            // Day 1 filler trades to force pruning of 2024-01-01.
            TestTx {
                id: "filler-1",
                sec: Some("sec-b"),
                cancel: None,
                date: d("2024-01-01"),
            },
            TestTx {
                id: "filler-2",
                sec: Some("sec-b"),
                cancel: None,
                date: d("2024-01-01"),
            },
            // Day 1 buy canceled on Day 2 (no security_id on cancellation).
            TestTx {
                id: "target-buy",
                sec: Some("sec-a"),
                cancel: None,
                date: d("2024-01-01"),
            },
            TestTx {
                id: "cancel-next-day",
                sec: None,
                cancel: Some("target-buy"),
                date: d("2024-01-02"),
            },
            // Reverse-dated cancel on Day 1 targeting a Day 3 buy.
            TestTx {
                id: "early-cancel",
                sec: None,
                cancel: Some("late-buy"),
                date: d("2024-01-01"),
            },
            TestTx {
                id: "late-buy",
                sec: Some("sec-a"),
                cancel: None,
                date: d("2024-01-03"),
            },
        ];
        let cov_cancel = prune_retained_trades_until(
            &mut cancel_items,
            d("2024-01-01"),
            d("2026-01-01"),
            4,
            |tx| tx.id,
            |tx| tx.cancel,
            |tx| tx.date,
            |_, _| true,
        );
        assert_eq!(cov_cancel, Some((d("2024-01-02"), d("2026-01-01"))));
        let kept_ids: Vec<&str> = cancel_items.iter().map(|tx| tx.id).collect();
        assert_eq!(
            kept_ids,
            vec!["target-buy", "early-cancel", "cancel-next-day", "late-buy"]
        );
        let raw: Vec<RawProviderTx<'_>> = cancel_items
            .iter()
            .map(|tx| RawProviderTx {
                id: tx.id,
                security_key: tx.sec,
                date: tx.date,
                cancel_target: tx.cancel,
                basis_changing: tx.cancel.is_none(),
                requires_security_key: tx.cancel.is_none(),
            })
            .collect();
        let paired = pair_provider_trade_events(&raw);
        assert!(!paired.has_unattributed_event);
        assert_eq!(
            paired.events.get("sec-a"),
            Some(&vec![
                super::super::ProviderTradeEvent {
                    date: d("2024-01-01"),
                    cancelled_on: Some(d("2024-01-02")),
                },
                super::super::ProviderTradeEvent {
                    date: d("2024-01-03"),
                    cancelled_on: Some(d("2024-01-01")),
                },
            ])
        );

        // If keeping the Day 2 cancellation + its Day 1 target still exceeds max_records,
        // pruning advances coverage past Day 2 and drops both legs together.
        let cov_cancel_tight = prune_retained_trades_until(
            &mut cancel_items,
            d("2024-01-02"),
            d("2026-01-01"),
            2,
            |tx| tx.id,
            |tx| tx.cancel,
            |tx| tx.date,
            |_, _| true,
        );
        assert_eq!(cov_cancel_tight, Some((d("2024-01-03"), d("2026-01-01"))));
        let tight_ids: Vec<&str> = cancel_items.iter().map(|tx| tx.id).collect();
        assert_eq!(tight_ids, vec!["early-cancel", "late-buy"]);

        // Forward gaps reset coverage to the fresh fetch window rather than fabricating continuity.
        assert_eq!(
            merge_trade_coverage_window(
                Some(d("2026-06-10")),
                Some(d("2026-06-20")),
                d("2024-05-01"),
                d("2026-05-01"),
            ),
            (d("2024-05-01"), d("2026-05-01"))
        );

        // Schema-minimal saved records, multiple empty-ID records, and corrupt source_history
        // behave deterministically and fail closed in reconcile_source_history_and_stage_trades.
        let minimal_source = serde_json::json!({
            "historyStart": "2024-01-01",
            "historyEnd": "2026-01-01",
            "transactions": [
                { "investment_transaction_id": "", "security_id": "sec-old", "date": "2024-06-01", "type": "buy", "quantity": "2" },
                { "investment_transaction_id": "t1", "security_id": "sec-old", "date": "2024-07-01", "type": "buy", "quantity": "3" }
            ],
            "securities": [
                { "security_id": "sec-old", "ticker_symbol": "AMD" }
            ],
            "assetIdsBySecurity": { "sec-old": "asset-amd" }
        });
        let stage = vec![
            StageProviderTx {
                id: "",
                account_id: "ext-1",
                security_id: None,
                cancel_target: None,
                date: d("2025-01-01"),
                transaction_type: "cash",
                subtype: "deposit",
                quantity_nonzero: false,
            },
            StageProviderTx {
                id: "c1",
                account_id: "ext-1",
                security_id: Some("sec-old"),
                cancel_target: Some("t1"),
                date: d("2024-07-02"),
                transaction_type: "cancel",
                subtype: "",
                quantity_nonzero: true,
            },
        ];
        let reconciled = reconcile_source_history_and_stage_trades(
            "ext-1",
            "USD",
            &minimal_source,
            &stage,
            &BTreeMap::new(),
            d("2024-06-01"),
            d("2026-06-01"),
        )
        .expect("minimal saved schema and empty-ID cash row must reconcile");
        assert_eq!(reconciled.coverage, (d("2024-01-01"), d("2026-06-01")));
        assert_eq!(reconciled.by_security.len(), 1);
        assert_eq!(reconciled.by_security[0].events.len(), 2); // empty-ID buy preserved + paired t1

        let corrupt_source = serde_json::json!({
            "historyStart": "2024-01-01",
            "transactions": "not-an-array"
        });
        assert!(reconcile_source_history_and_stage_trades(
            "ext-1",
            "USD",
            &corrupt_source,
            &[],
            &BTreeMap::new(),
            d("2024-06-01"),
            d("2026-06-01"),
        )
        .is_none());

        // When coverage_end itself exceeds max_records, coverage is withheld rather than partial.
        let mut same_day = vec![
            (d("2026-01-01"), "x"),
            (d("2026-01-01"), "y"),
            (d("2026-01-01"), "z"),
        ];
        assert_eq!(
            prune_retained_trades_by_day(&mut same_day, d("2024-01-01"), d("2026-01-01"), 2, |x| x
                .0,),
            None
        );
        assert!(same_day.is_empty());
    }
}
