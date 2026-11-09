//! Validate a proposed broker ledger using the same kernel as portfolio reads.
//! The caller runs this on the database writer's blocking thread, before commit.
use super::Activity;
use crate::{
    accounts::Account,
    assets::Asset,
    errors::{Error, Result},
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use std::collections::BTreeMap;
use wealthfolio_portfolio_engine::{model::*, Engine};

#[allow(clippy::too_many_arguments)]
pub fn validate_brokerage_ledger(
    account: &Account,
    activities: &[Activity],
    assets: &[Asset],
    expected_quantities: &BTreeMap<String, Decimal>,
    expected_basis: &BTreeMap<String, Decimal>,
    expected_cash: Decimal,
    timezone: &str,
    as_of: NaiveDate,
) -> Result<()> {
    let invalid = || {
        Error::ConstraintViolation(
            "Brokerage ledger does not reconcile; review the source data and edited activities"
                .into(),
        )
    };
    let currency = Currency::parse(&account.currency).ok_or_else(invalid)?;
    let raw = RawFacts {
        policy: Policy::new(
            currency.clone(),
            crate::utils::time_utils::parse_user_timezone_or_default(timezone),
            as_of,
        ),
        accounts: vec![RawAccount {
            id: account.id.clone(),
            currency: account.currency.clone(),
            account_type: account.account_type.clone(),
            tracking_mode: "TRANSACTIONS".into(),
            is_archived: false,
            cost_basis_method: Some(
                account
                    .accounting_settings()?
                    .cost_basis_method
                    .as_str()
                    .into(),
            ),
        }],
        assets: assets
            .iter()
            .map(|a| RawAsset {
                id: a.id.clone(),
                quote_currency: a.quote_ccy.clone(),
                kind: a.kind.as_db_str().into(),
                instrument_type: a.instrument_type.as_ref().map(|t| t.as_db_str().into()),
                contract_multiplier: Some(a.contract_multiplier()),
                alternative: a.kind.is_alternative(),
            })
            .collect(),
        activities: activities
            .iter()
            .map(crate::portfolio::coordinator::raw_activity)
            .collect(),
        quotes: vec![],
        fx_rates: vec![],
        observed_snapshots: vec![],
    };
    let engine = Engine::new(raw).map_err(|_| invalid())?;
    let projection = engine.project().map_err(|_| invalid())?;
    if !projection.rejected_activities().is_empty() {
        return Err(invalid());
    }
    let Some(state) = projection
        .final_state
        .accounts
        .get(&AccountId::new(&account.id))
    else {
        return if expected_cash.is_zero()
            && expected_quantities.values().all(Decimal::is_zero)
            && expected_basis.values().all(Decimal::is_zero)
        {
            Ok(())
        } else {
            Err(invalid())
        };
    };
    if state
        .cash
        .get(&currency)
        .copied()
        .unwrap_or_default()
        .checked_sub(expected_cash)
        .is_none_or(|d| d.abs() > Decimal::new(1, 2))
        || state
            .cash
            .iter()
            .any(|(c, v)| c != &currency && !v.is_zero())
    {
        return Err(invalid());
    }
    for (id, qty) in expected_quantities {
        let position = state.positions.get(&AssetId::new(id));
        if position
            .map(|p| p.quantity)
            .unwrap_or_default()
            .checked_sub(*qty)
            .is_none_or(|d| d.abs() > Decimal::new(1, 8))
        {
            return Err(invalid());
        }
        if let Some(basis) = expected_basis.get(id) {
            if position
                .map(|p| p.total_cost_basis)
                .unwrap_or_default()
                .checked_sub(*basis)
                .is_none_or(|d| d.abs() > Decimal::new(1, 2))
            {
                return Err(invalid());
            }
        }
    }
    if state.positions.iter().any(|(id, p)| {
        !expected_quantities.contains_key(id.as_str()) && p.quantity.abs() > Decimal::new(1, 8)
    }) {
        return Err(invalid());
    }
    Ok(())
}
