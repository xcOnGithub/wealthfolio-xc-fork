//! Value reconstructed statement states through the existing portfolio engine.
//! This isolated observed-state run cannot mutate the live ledger or settings.
use super::reconstruction::{
    amount, invalid, reconstruct_statement, ReconstructionError, StatementInterval,
};
use crate::{
    assets::{Asset, AssetKind, InstrumentType},
    portfolio::coordinator::{raw_asset, raw_quote},
    quotes::Quote,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use wealthfolio_portfolio_engine::{self as engine, model::*};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StatementPriceQuality {
    Complete,
    Stale,
    PartialUnpriced,
    Unavailable,
    ClosingPriceMismatch,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PricedStatementDay {
    pub date: NaiveDate,
    pub cash: String,
    /// Never expose a partial investment sum as a complete account value.
    pub value: Option<String>,
    pub quality: StatementPriceQuality,
    pub stale_assets: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PricedStatementInterval {
    pub from: NaiveDate,
    pub through: NaiveDate,
    pub currency: String,
    pub closing_prices_match: Option<bool>,
    /// Historical basis and contribution-adjusted returns are not established.
    pub returns_eligible: bool,
    pub days: Vec<PricedStatementDay>,
}

/// The caller supplies core-owned asset metadata and cached close-price rows.
/// No provider is enabled or contacted. Initial statement pricing supports USD
/// equities/ETFs only; unsupported metadata must not be guessed from a ticker.
pub fn price_statement(
    input: &StatementInterval,
    assets: &[Asset],
    quotes: &[Quote],
) -> Result<PricedStatementInterval, ReconstructionError> {
    if quotes.len() > 100_000 {
        return Err(invalid("size"));
    }
    let reconstructed = reconstruct_statement(input)?;
    let account = "statement-history";
    let universe: BTreeSet<_> = input
        .closing_positions
        .iter()
        .map(|p| p.asset_id.clone())
        .chain(input.events.iter().filter_map(|e| e.asset_id.clone()))
        .collect();
    let usd = Currency::parse("USD").ok_or_else(|| invalid("currency"))?;
    let policy = Policy::new(usd, chrono_tz::Tz::UTC, input.through);
    let mut metadata = BTreeMap::new();
    for asset in assets.iter().filter(|a| universe.contains(&a.id)) {
        if asset.kind != AssetKind::Investment
            || asset.instrument_type != Some(InstrumentType::Equity)
            || asset.contract_multiplier() != Decimal::ONE
            || policy.normalize_currency(&asset.quote_ccy).0 != "USD"
        {
            return Err(invalid("unsupportedAsset"));
        }
        if metadata
            .insert(asset.id.clone(), raw_asset(asset))
            .is_some()
        {
            return Err(invalid("security"));
        }
    }
    if metadata.len() != universe.len() {
        return Err(invalid("security"));
    }
    let observed_snapshots = reconstructed
        .days
        .iter()
        .map(|day| -> Result<_, ReconstructionError> {
            let cash = amount(&day.cash)?;
            let positions = day
                .positions
                .iter()
                .map(|(id, q)| -> Result<_, ReconstructionError> {
                    Ok(RawObservedPosition {
                        asset_id: id.clone(),
                        currency: String::new(),
                        quantity: amount(q)?,
                        cost_basis_status: Some(BasisStatus::Unknown),
                        average_cost: Decimal::ZERO,
                        total_cost_basis: Decimal::ZERO,
                        cost_basis_account: None,
                        cost_basis_base: None,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(RawObservedSnapshot {
                account_id: account.into(),
                date: day.date,
                positions,
                cash: vec![("USD".into(), cash)],
                cost_basis: Decimal::ZERO,
                net_contribution: Decimal::ZERO,
                net_contribution_base: Decimal::ZERO,
                cash_total_account_currency: cash,
                cash_total_base_currency: cash,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let raw = RawFacts {
        policy,
        accounts: vec![RawAccount {
            id: account.into(),
            currency: "USD".into(),
            account_type: "SECURITIES".into(),
            tracking_mode: "HOLDINGS".into(),
            is_archived: false,
            cost_basis_method: None,
        }],
        assets: metadata.into_values().collect(),
        activities: vec![],
        quotes: quotes
            .iter()
            .filter(|q| universe.contains(&q.asset_id) && q.timestamp.date_naive() <= input.through)
            .map(raw_quote)
            .collect(),
        fx_rates: vec![],
        observed_snapshots,
    };
    let kernel = engine::Engine::new(raw).map_err(|_| invalid("pricing"))?;
    let range = DateRange {
        start: input.from,
        end: input.through,
    };
    let surfaces = engine::resolve_surfaces(kernel.facts(), range);
    let projection = kernel
        .project_from(None, range)
        .map_err(|_| invalid("pricing"))?;
    let resolved = engine::Resolved {
        facts: kernel.facts(),
        ledger: kernel.ledger(),
        surfaces: &surfaces,
        range,
    };
    let series = engine::value(&engine::ValueInputs {
        resolved,
        bundle: &projection,
        lots: None,
    })
    .remove(&AccountId::from(account))
    .ok_or_else(|| invalid("pricing"))?;
    // Reconciliation, not a second valuation path: check each reported closing
    // market value against the same resolved price surface the engine used.
    // A large unit/price mismatch must not be hidden by offsetting assets.
    let checks: Vec<Option<bool>> = input
        .closing_positions
        .iter()
        .filter(|p| amount(&p.quantity).is_ok_and(|q| q > Decimal::ZERO))
        .map(|p| {
            let quote = surfaces
                .quotes
                .latest_on_or_before(&AssetId::from(p.asset_id.clone()), input.through)?;
            let (currency, factor) = kernel
                .facts()
                .policy()
                .normalize_currency(quote.currency.as_str());
            if currency != "USD" {
                return None;
            }
            let quantity = amount(&p.quantity).ok()?;
            let market = amount(&p.market_value).ok()?;
            quantity
                .checked_mul(quote.close)
                .and_then(|v| v.checked_mul(factor))
                .map(|v| (v - market).abs() <= Decimal::new(1, 2))
        })
        .collect();
    let closing_prices_match = if checks.contains(&Some(false)) {
        Some(false)
    } else if checks.contains(&None) {
        None
    } else {
        Some(true)
    };
    let mut days = Vec::new();
    for day in series.days {
        let state = reconstructed
            .days
            .iter()
            .find(|s| s.date == day.date)
            .ok_or_else(|| invalid("pricing"))?;
        let stale_assets: Vec<_> = state
            .positions
            .keys()
            .filter(|id| {
                surfaces
                    .quotes
                    .latest_on_or_before(&AssetId::from((*id).clone()), day.date)
                    .is_some_and(|q| (day.date - q.day).num_days() >= 7)
            })
            .cloned()
            .collect();
        let quality = match day.value_status {
            ValueStatus::Unavailable => StatementPriceQuality::Unavailable,
            ValueStatus::PartialUnpriced => StatementPriceQuality::PartialUnpriced,
            ValueStatus::Complete => match closing_prices_match {
                Some(false) => StatementPriceQuality::ClosingPriceMismatch,
                None => StatementPriceQuality::Unavailable,
                Some(true) if !stale_assets.is_empty() => StatementPriceQuality::Stale,
                Some(true) => StatementPriceQuality::Complete,
            },
        };
        let value = matches!(
            quality,
            StatementPriceQuality::Complete | StatementPriceQuality::Stale
        )
        .then(|| day.total_value.normalize().to_string());
        days.push(PricedStatementDay {
            date: day.date,
            cash: day.cash_balance.normalize().to_string(),
            value,
            quality,
            stale_assets,
        });
    }
    if days.len() != reconstructed.days.len() {
        return Err(invalid("pricing"));
    }
    Ok(PricedStatementInterval {
        from: input.from,
        through: input.through,
        currency: "USD".into(),
        closing_prices_match,
        returns_eligible: false,
        days,
    })
}

#[cfg(test)]
mod tests;
