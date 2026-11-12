//! Reviewed historical observations. Never replayed as holdings or cash flows.
use super::{
    coordinator::blocking,
    valuation::{ValuationRepositoryTrait, ValuationStatus},
};
use crate::{
    accounts::AccountRepositoryTrait,
    errors::{Result, ValidationError},
    fx::FxServiceTrait,
    utils::time_utils::{parse_user_timezone_or_default, user_today},
};
use async_trait::async_trait;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    sync::{Arc, RwLock},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatementPosition {
    pub label: String,
    pub quantity: String,
    pub market_value: Option<String>,
    pub cost_basis: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatementSnapshot {
    pub date: NaiveDate,
    pub currency: String,
    pub total_value: String,
    pub cash_value: Option<String>,
    pub positions: Vec<StatementPosition>,
    pub positions_complete: bool,
    pub format: String,
    pub source_page: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatementWrite {
    pub snapshot: StatementSnapshot,
    pub expected_revision: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredStatement {
    pub snapshot: StatementSnapshot,
    pub revision: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementValue {
    pub cash_value: Option<String>,
    pub date: NaiveDate,
    pub statement_date: NaiveDate,
    pub value: String,
    pub currency: String,
}
#[async_trait]
pub trait StatementRepository: Send + Sync {
    fn list(&self, account_id: &str) -> Result<Vec<StoredStatement>>;
    async fn save(&self, account_id: &str, writes: Vec<StatementWrite>) -> Result<()>;
    async fn delete(&self, account_id: &str, date: NaiveDate, revision: &str) -> Result<()>;
}
pub fn invalid() -> crate::errors::Error {
    ValidationError::InvalidInput(
        "Invalid or changed statement data; review the statements again".into(),
    )
    .into()
}
pub fn amount(value: &str) -> Result<Decimal> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == '-')
    {
        return Err(invalid());
    }
    let n: Decimal = value.parse().map_err(|_| invalid())?;
    if n.abs() > Decimal::from(1_000_000_000_000_000_i64) || n.scale() > 18 {
        return Err(invalid());
    }
    Ok(n)
}
impl StatementSnapshot {
    pub fn validate(&self, currency: &str, today: NaiveDate) -> Result<()> {
        if self.currency != currency
            || self.currency != "USD"
            || self.date > today
            || self.date < NaiveDate::from_ymd_opt(1900, 1, 1).ok_or_else(invalid)?
            || self.format != "merrill-cma-monthly-v1"
            || self.source_page == 0
            || self.source_page > 100
            || self.positions.len() > 1000
        {
            return Err(invalid());
        }
        let total = amount(&self.total_value)?;
        // CMA net values may be negative where the account carries a debit.
        if let Some(cash) = &self.cash_value {
            amount(cash)?;
        }
        let mut market = Decimal::ZERO;
        for p in &self.positions {
            if p.label.trim().is_empty()
                || p.label.len() > 256
                || amount(&p.quantity)? < Decimal::ZERO
            {
                return Err(invalid());
            }
            if let Some(v) = &p.cost_basis {
                if amount(v)? < Decimal::ZERO {
                    return Err(invalid());
                }
            }
            if let Some(v) = &p.market_value {
                market = market.checked_add(amount(v)?).ok_or_else(invalid)?;
            } else if self.positions_complete {
                return Err(invalid());
            }
        }
        if self.positions_complete {
            let cash = self
                .cash_value
                .as_deref()
                .ok_or_else(invalid)
                .and_then(amount)?;
            if (market.checked_add(cash).ok_or_else(invalid)? - total).abs() > Decimal::new(1, 2) {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
#[derive(Clone)]
pub struct StatementService {
    repository: Arc<dyn StatementRepository>,
    accounts: Arc<dyn AccountRepositoryTrait>,
    valuations: Arc<dyn ValuationRepositoryTrait>,
    timezone: Arc<RwLock<String>>,
}
impl StatementService {
    pub fn new(
        repository: Arc<dyn StatementRepository>,
        accounts: Arc<dyn AccountRepositoryTrait>,
        valuations: Arc<dyn ValuationRepositoryTrait>,
        timezone: Arc<RwLock<String>>,
    ) -> Self {
        Self {
            repository,
            accounts,
            valuations,
            timezone,
        }
    }
    pub async fn list(&self, account_id: String) -> Result<Vec<StoredStatement>> {
        let this = self.clone();
        blocking(move || {
            this.accounts.get_by_id(&account_id)?;
            this.repository.list(&account_id)
        })
        .await
    }
    pub async fn save(
        &self,
        account_id: String,
        writes: Vec<StatementWrite>,
        confirmed: bool,
    ) -> Result<()> {
        if !confirmed || writes.is_empty() || writes.len() > 5 {
            return Err(invalid());
        }
        let account = self.accounts.get_by_id(&account_id)?;
        if account.account_type != crate::accounts::account_types::SECURITIES {
            return Err(invalid());
        }
        let today = user_today(parse_user_timezone_or_default(
            &self.timezone.read().unwrap_or_else(|p| p.into_inner()),
        ));
        let mut dates = BTreeSet::new();
        for w in &writes {
            w.snapshot.validate(&account.currency, today)?;
            if !dates.insert(w.snapshot.date) {
                return Err(invalid());
            }
        }
        self.repository.save(&account_id, writes).await
    }
    pub async fn delete(
        &self,
        account_id: String,
        date: NaiveDate,
        revision: String,
    ) -> Result<()> {
        self.accounts.get_by_id(&account_id)?;
        self.repository.delete(&account_id, date, &revision).await
    }
    pub async fn history(
        &self,
        account_id: String,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<Vec<StatementValue>> {
        let this = self.clone();
        blocking(move || {
            let account = this.accounts.get_by_id(&account_id)?;
            let stored = this.repository.list(&account_id)?;
            let valuations = this
                .valuations
                .get_historical_valuations(&account_id, None, None)?;
            let boundary = valuations
                .iter()
                .filter(|v| v.value_status == ValuationStatus::Complete)
                .map(|v| v.valuation_date)
                .min();
            statement_values(&stored, start, end, boundary, &account.currency, None)
        })
        .await
    }
}
/// Only the prefix before calculated coverage is eligible. Never fill later gaps.
pub fn statement_values(
    statements: &[StoredStatement],
    start: NaiveDate,
    end: NaiveDate,
    boundary: Option<NaiveDate>,
    currency: &str,
    fx: Option<&dyn FxServiceTrait>,
) -> Result<Vec<StatementValue>> {
    if end < start || (end - start).num_days() > 73050 {
        return Err(invalid());
    }
    let mut ordered: Vec<_> = statements
        .iter()
        .filter(|s| s.snapshot.date <= end)
        .collect();
    ordered.sort_by_key(|s| s.snapshot.date);
    let mut result = Vec::new();
    let mut cursor = 0;
    let mut current: Option<&StoredStatement> = None;
    let mut date = start;
    while date <= end && boundary.is_none_or(|b| date < b) {
        while cursor < ordered.len() && ordered[cursor].snapshot.date <= date {
            current = Some(ordered[cursor]);
            cursor += 1;
        }
        if let Some(s) = current {
            let value = amount(&s.snapshot.total_value)?;
            let value = if currency == s.snapshot.currency {
                value
            } else {
                fx.ok_or_else(invalid)?.convert_currency_for_date(
                    value,
                    &s.snapshot.currency,
                    currency,
                    s.snapshot.date,
                )?
            };
            let cash_value = s
                .snapshot
                .cash_value
                .as_deref()
                .map(|cash| {
                    let cash = amount(cash)?;
                    let converted = if currency == s.snapshot.currency {
                        cash
                    } else {
                        fx.ok_or_else(invalid)?.convert_currency_for_date(
                            cash,
                            &s.snapshot.currency,
                            currency,
                            s.snapshot.date,
                        )?
                    };
                    Ok::<_, crate::errors::Error>(converted.to_string())
                })
                .transpose()?;
            result.push(StatementValue {
                cash_value,
                date,
                statement_date: s.snapshot.date,
                value: value.to_string(),
                currency: currency.into(),
            });
        }
        date = date.succ_opt().ok_or_else(invalid)?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn date(value: &str) -> NaiveDate {
        value.parse().unwrap()
    }
    fn statement(day: &str, total: &str) -> StoredStatement {
        StoredStatement {
            revision: "synthetic".into(),
            snapshot: StatementSnapshot {
                date: date(day),
                currency: "USD".into(),
                total_value: total.into(),
                cash_value: None,
                positions: Vec::new(),
                positions_complete: false,
                format: "merrill-cma-monthly-v1".into(),
                source_page: 1,
            },
        }
    }
    #[test]
    fn statement_history_monthly_prefix_carries_forward_without_backfill_or_overlap() {
        let rows = vec![
            statement("2025-02-28", "103"),
            statement("2025-01-31", "100"),
            statement("2025-03-31", "150"),
        ];
        let result = statement_values(
            &rows,
            date("2025-01-01"),
            date("2025-04-01"),
            Some(date("2025-03-10")),
            "USD",
            None,
        )
        .unwrap();
        assert_eq!(result.first().unwrap().date, date("2025-01-31"));
        assert_eq!(result.last().unwrap().date, date("2025-03-09"));
        for (day, value, source) in [
            ("2025-01-31", "100", "2025-01-31"),
            ("2025-02-15", "100", "2025-01-31"),
            ("2025-02-28", "103", "2025-02-28"),
            ("2025-03-09", "103", "2025-02-28"),
        ] {
            let row = result.iter().find(|v| v.date == date(day)).unwrap();
            assert_eq!(row.value, value);
            assert_eq!(row.statement_date, date(source));
        }
        assert!(statement_values(
            &rows,
            date("2025-03-11"),
            date("2025-04-01"),
            Some(date("2025-03-10")),
            "USD",
            None
        )
        .unwrap()
        .is_empty());
    }
    #[test]
    fn statement_history_seed_before_requested_range_and_missing_fx_are_explicit() {
        let rows = vec![statement("2025-01-31", "100")];
        let values = statement_values(
            &rows,
            date("2025-02-10"),
            date("2025-02-12"),
            None,
            "USD",
            None,
        )
        .unwrap();
        assert_eq!(values.len(), 3);
        assert!(values.iter().all(|v| v.value == "100"));
        assert!(statement_values(
            &rows,
            date("2025-02-10"),
            date("2025-02-12"),
            None,
            "EUR",
            None
        )
        .is_err());
        assert!(statement_values(
            &rows,
            date("2025-02-12"),
            date("2025-02-10"),
            None,
            "USD",
            None
        )
        .is_err());
    }
    #[test]
    fn statement_history_missing_basis_does_not_prevent_account_value_import() {
        let mut s = statement("2025-01-31", "100").snapshot;
        s.cash_value = Some("10".into());
        s.positions_complete = true;
        s.positions.push(StatementPosition {
            label: "Synthetic equity".into(),
            quantity: "2".into(),
            market_value: Some("90".into()),
            cost_basis: None,
        });
        s.validate("USD", date("2025-02-01")).unwrap();
        s.positions[0].cost_basis = Some("0".into());
        s.validate("USD", date("2025-02-01")).unwrap();
        s.total_value = "101".into();
        assert!(s.validate("USD", date("2025-02-01")).is_err());
        s.positions_complete = false;
        s.validate("USD", date("2025-02-01")).unwrap();
        assert!(s.validate("EUR", date("2025-02-01")).is_err());
        assert!(s.validate("USD", date("2025-01-01")).is_err());
        s.positions[0].cost_basis = Some("-1".into());
        assert!(s.validate("USD", date("2025-02-01")).is_err());
    }
}
