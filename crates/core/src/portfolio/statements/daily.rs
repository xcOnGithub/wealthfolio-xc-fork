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
        self.invalidate_holdings_projection(&account).await
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

fn apply_statement_basis_with_history(
    snapshot: &mut crate::portfolio::snapshot::AccountStateSnapshot,
    statements: &[StoredDailyStatement],
    observed_history: &[ObservedHistoryEntry],
    splits: &[wealthfolio_portfolio_engine::SplitEvent],
) -> bool {
    use crate::portfolio::{economic_events::BasisStatus, snapshot::SnapshotSource};
    use rust_decimal::Decimal;

    if snapshot.source != SnapshotSource::BrokerImported
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
    let mut closing = BTreeMap::new();
    for p in &latest.interval.closing_positions {
        let Some(raw_basis) = p.cost_basis.as_deref() else {
            continue;
        };
        let (Ok(qty), Ok(basis)) = (amount(&p.quantity), amount(raw_basis)) else {
            continue;
        };
        if qty > Decimal::ZERO && basis >= Decimal::ZERO {
            closing.insert(p.asset_id.as_str(), (qty, basis));
        }
    }
    if closing.is_empty() {
        return false;
    }
    let stmt_date = latest.interval.through;
    let mut updated = false;
    for pos in snapshot.positions.values_mut() {
        if pos.basis_status() != BasisStatus::Unknown
            || pos.quantity <= Decimal::ZERO
            || (!pos.currency.is_empty() && pos.currency != snapshot.currency)
        {
            continue;
        }
        let Some(&(stmt_qty, stmt_basis)) = closing.get(pos.asset_id.as_str()) else {
            continue;
        };
        if stmt_qty != pos.quantity
            || has_statement_security_event(
                statements,
                &pos.asset_id,
                stmt_date,
                snapshot.snapshot_date,
            )
            || wealthfolio_portfolio_engine::split_quantity_factor(
                splits,
                &wealthfolio_portfolio_engine::model::AssetId::new(&pos.asset_id),
                stmt_date,
                snapshot.snapshot_date,
            ) != Some(Decimal::ONE)
            || observed_history.iter().any(|(date, positions)| {
                stmt_date < *date
                    && *date < snapshot.snapshot_date
                    && positions.get(&pos.asset_id).is_none_or(|(qty, basis)| {
                        *qty != pos.quantity || basis.is_some_and(|known| known != stmt_basis)
                    })
            })
        {
            continue;
        }
        let Some(avg_cost) = stmt_basis.checked_div(pos.quantity) else {
            continue;
        };
        pos.total_cost_basis = stmt_basis;
        pos.average_cost = avg_cost;
        pos.cost_basis_account = Some(stmt_basis);
        if snapshot.base_currency.as_deref() == Some(snapshot.currency.as_str()) {
            pos.cost_basis_base = Some(stmt_basis);
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
/// share quantity and split state stayed unchanged since statement close.
pub fn apply_statement_basis_to_snapshot(
    snapshot: &mut crate::portfolio::snapshot::AccountStateSnapshot,
    statements: &[StoredDailyStatement],
    observed_history: &[crate::portfolio::snapshot::AccountStateSnapshot],
    splits: &[wealthfolio_portfolio_engine::SplitEvent],
) -> bool {
    let history = collect_observed_history(observed_history);
    apply_statement_basis_with_history(snapshot, statements, &history, splits)
}

/// Populate missing (`BasisStatus::Unknown`) cost basis across an account's
/// observed snapshots using `snapshots` itself for intervening-quantity checks.
pub fn apply_statement_basis_to_snapshots(
    snapshots: &mut [crate::portfolio::snapshot::AccountStateSnapshot],
    statements: &[StoredDailyStatement],
    splits: &[wealthfolio_portfolio_engine::SplitEvent],
) {
    if statements.is_empty() {
        return;
    }
    let history = collect_observed_history(snapshots);
    for snapshot in snapshots {
        apply_statement_basis_with_history(snapshot, statements, &history, splits);
    }
}

/// Suggests an opening per-share unit cost at `opening_day` (`start_date - 1`)
/// from reviewed daily statements when share quantity matches and no security
/// events occurred between `opening_day` and the statement's closing date.
pub fn suggested_opening_unit_cost(
    statements: &[StoredDailyStatement],
    currency: &str,
    asset_id: &str,
    opening_day: NaiveDate,
    opening_quantity: rust_decimal::Decimal,
    has_provider_trade_through: impl Fn(NaiveDate) -> bool,
) -> Option<rust_decimal::Decimal> {
    use rust_decimal::Decimal;
    if opening_quantity <= Decimal::ZERO {
        return None;
    }
    let unit_cost = |stmt: &StoredDailyStatement| -> Option<Decimal> {
        if stmt.interval.currency != currency {
            return None;
        }
        let cp = stmt
            .interval
            .closing_positions
            .iter()
            .find(|p| p.asset_id == asset_id)?;
        let qty = amount(&cp.quantity).ok()?;
        if qty != opening_quantity {
            return None;
        }
        let basis = amount(cp.cost_basis.as_deref()?).ok()?;
        if basis < Decimal::ZERO {
            return None;
        }
        basis.checked_div(opening_quantity).map(|v| v.normalize())
    };
    if let Some(latest_before) = statements
        .iter()
        .filter(|s| s.interval.through <= opening_day && s.interval.currency == currency)
        .max_by_key(|s| s.interval.through)
    {
        if !has_statement_security_event(
            statements,
            asset_id,
            latest_before.interval.through,
            opening_day,
        ) {
            if let Some(cost) = unit_cost(latest_before) {
                return Some(cost);
            }
            if latest_before.interval.through == opening_day {
                // Fall through only if a subsequent month's statement covers a later date
                // without trades; handled below when opening_day is strictly inside an interval.
                return None;
            }
        }
    }
    let containing = statements.iter().find(|s| {
        s.interval.currency == currency
            && s.interval.from <= opening_day
            && opening_day < s.interval.through
    })?;
    if has_statement_security_event(
        statements,
        asset_id,
        opening_day,
        containing.interval.through,
    ) || has_provider_trade_through(containing.interval.through)
    {
        return None;
    }
    unit_cost(containing)
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
            &[]
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
        ));
        assert_eq!(
            later_matching.positions["AMD"].total_cost_basis,
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
        ));
        let mut unchanged = snap(
            "2026-11-24",
            SnapshotSource::BrokerImported,
            7,
            Some(BasisStatus::Unknown),
        );
        assert!(apply_statement_basis_to_snapshot(
            &mut unchanged,
            &statements,
            &[],
            &[],
        ));
        assert_eq!(
            unchanged.positions["AMD"].total_cost_basis,
            Decimal::from(280)
        );
        assert_eq!(unchanged.positions["AMD"].average_cost, Decimal::from(40));
        assert_eq!(
            unchanged.positions["AMD"].basis_status(),
            BasisStatus::Complete
        );
    }

    #[test]
    fn suggested_opening_unit_cost_handles_mid_statement_start_and_intervening_trades() {
        let untouched = vec![stmt("2026-10-01", "2026-10-31", "7", Some("280"), vec![])];
        assert_eq!(
            suggested_opening_unit_cost(
                &untouched,
                "USD",
                "AMD",
                d("2026-10-14"),
                Decimal::from(7),
                |_| false
            ),
            Some(Decimal::from(40))
        );
        // Provider trade before statement close blocks mid-interval fallback.
        assert_eq!(
            suggested_opening_unit_cost(
                &untouched,
                "USD",
                "AMD",
                d("2026-10-14"),
                Decimal::from(7),
                |_| true
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
                |_| false
            ),
            None
        );
    }
}
