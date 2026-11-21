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
    errors::{Result, ValidationError},
    fx::FxServiceTrait,
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
        let Some(repository) = &self.live_snapshots else {
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
        let live = repository.get_snapshots_by_account(account, Some(day), Some(day))?;
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
        if writes.is_empty() || writes.len() > 5 {
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
            if previous.is_some_and(|s| {
                s.interval == write.interval && s.closing_asset_ids == write.closing_asset_ids
            }) && snapshot.is_some_and(|s| s.snapshot == write.snapshot)
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
        self.repository.save_daily(&account, writes).await
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
        let mut ranges = BTreeSet::new();
        for w in writes {
            for id in asset_ids(&w.interval) {
                ranges.insert((id, w.interval.from, w.interval.through));
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
            values.insert(
                day.date,
                StatementValue {
                    date: day.date,
                    statement_date: interval.through,
                    value: convert(&raw_value)?,
                    cash_value: Some(convert(&day.cash)?),
                    currency: currency.into(),
                    source: "statementDaily".into(),
                    quality: Some(day.quality),
                },
            );
        }
    }
    Ok(values.into_values().collect())
}
