//! Statement-only, settlement-date quantity and cash reconstruction. No I/O,
//! ledger writes, prices, clock, Plaid records or inferred acquisition costs.
use chrono::{Datelike, Days, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_STATEMENT_DAYS: i64 = 31;
pub const MAX_STATEMENT_EVENTS: usize = 10_000;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClosingPosition {
    pub asset_id: String,
    pub quantity: String,
    pub market_value: String,
    pub cost_basis: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StatementEventKind {
    Buy,
    Sell,
    TransferIn,
    TransferOut,
    Cash,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatementEvent {
    pub source_id: String,
    pub date: NaiveDate,
    pub kind: StatementEventKind,
    pub asset_id: Option<String>,
    /// Unsigned share amount. Kind determines direction. Cash events have none.
    pub quantity: Option<String>,
    /// Final posted cash, already including fees. In-kind transfers use zero.
    pub cash: String,
    pub source_page: u32,
    pub source_line: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatementDatePolicy {
    Settlement,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatementInterval {
    pub date_policy: StatementDatePolicy,
    pub from: NaiveDate,
    pub through: NaiveDate,
    pub currency: String,
    pub opening_cash: String,
    pub closing_cash: String,
    pub closing_value: String,
    pub closing_positions_complete: bool,
    pub activity_complete: bool,
    pub closing_positions: Vec<ClosingPosition>,
    pub events: Vec<StatementEvent>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementDay {
    pub date: NaiveDate,
    pub cash: String,
    /// Zero positions are omitted. Includes securities fully sold later.
    pub positions: BTreeMap<String, String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconstructedInterval {
    pub from: NaiveDate,
    pub through: NaiveDate,
    pub currency: String,
    pub opening_cash: String,
    pub opening_positions: BTreeMap<String, String>,
    pub days: Vec<StatementDay>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[error("Statement history requires review: {code}")]
#[serde(rename_all = "camelCase")]
pub struct ReconstructionError {
    pub code: &'static str,
    pub record: Option<usize>,
}
type Result<T> = std::result::Result<T, ReconstructionError>;
fn invalid(code: &'static str) -> ReconstructionError {
    ReconstructionError { code, record: None }
}
fn amount(raw: &str) -> Result<Decimal> {
    if raw.is_empty()
        || raw.len() > 64
        || !raw
            .bytes()
            .all(|c| c.is_ascii_digit() || c == b'.' || c == b'-')
    {
        return Err(invalid("number"));
    }
    let n: Decimal = raw.parse().map_err(|_| invalid("number"))?;
    if n.abs() > Decimal::from(1_000_000_000_000_000_i64) || n.scale() > 18 {
        return Err(invalid("number"));
    }
    Ok(n)
}
fn identifier(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= 128 && !id.chars().any(char::is_control)
}
fn add(a: Decimal, b: Decimal) -> Result<Decimal> {
    a.checked_add(b).ok_or_else(|| invalid("overflow"))
}
fn subtract(a: Decimal, b: Decimal) -> Result<Decimal> {
    a.checked_sub(b).ok_or_else(|| invalid("overflow"))
}
fn strings(positions: &BTreeMap<String, Decimal>) -> BTreeMap<String, String> {
    positions
        .iter()
        .filter(|(_, q)| !q.is_zero())
        .map(|(id, q)| (id.clone(), q.normalize().to_string()))
        .collect()
}
fn event_delta(event: &StatementEvent) -> Result<(Option<(&str, Decimal)>, Decimal)> {
    use StatementEventKind::*;
    if !identifier(&event.source_id)
        || event.source_page == 0
        || event.source_page > 100
        || event.source_line == 0
        || event.source_line > 50_000
    {
        return Err(invalid("source"));
    }
    let cash = amount(&event.cash)?;
    if event.kind == Cash {
        if event.asset_id.is_some() || event.quantity.is_some() {
            return Err(invalid("cashQuantity"));
        }
        return Ok((None, cash));
    }
    let asset = event
        .asset_id
        .as_deref()
        .filter(|s| identifier(s))
        .ok_or_else(|| invalid("security"))?;
    let q = event
        .quantity
        .as_deref()
        .ok_or_else(|| invalid("quantity"))
        .and_then(amount)?;
    if q <= Decimal::ZERO {
        return Err(invalid("quantity"));
    }
    if (event.kind == Buy && cash > Decimal::ZERO)
        || (event.kind == Sell && cash < Decimal::ZERO)
        || (matches!(event.kind, TransferIn | TransferOut) && !cash.is_zero())
    {
        return Err(invalid("cashSign"));
    }
    Ok((
        Some((
            asset,
            if matches!(event.kind, Sell | TransferOut) {
                -q
            } else {
                q
            },
        )),
        cash,
    ))
}

/// A complete closing position list is needed to interpret an absent asset as
/// zero. Reverse quantity deltas establish opening positions, then replay daily
/// net changes. Same-day ordering cannot be inferred from a monthly statement;
/// only end-of-day states are claimed. Cost basis is never an input to value.
pub fn reconstruct_statement(input: &StatementInterval) -> Result<ReconstructedInterval> {
    let length = (input.through - input.from).num_days();
    if input.currency != "USD"
        || !(0..MAX_STATEMENT_DAYS).contains(&length)
        || input.from.year_ce().1 < 1900
        || input.through.year_ce().1 > 9999
    {
        return Err(invalid("period"));
    }
    if !input.closing_positions_complete || !input.activity_complete {
        return Err(invalid("incomplete"));
    }
    if input.events.len() > MAX_STATEMENT_EVENTS || input.closing_positions.len() > 1000 {
        return Err(invalid("size"));
    }
    let opening_cash = amount(&input.opening_cash)?;
    let closing_cash = amount(&input.closing_cash)?;
    let closing_value = amount(&input.closing_value)?;
    let mut positions = BTreeMap::new();
    let mut market = closing_cash;
    for position in &input.closing_positions {
        let q = amount(&position.quantity)?;
        let value = amount(&position.market_value)?;
        if !identifier(&position.asset_id)
            || q < Decimal::ZERO
            || value < Decimal::ZERO
            || (q.is_zero() && !value.is_zero())
            || positions.insert(position.asset_id.clone(), q).is_some()
        {
            return Err(invalid("closingPosition"));
        }
        if let Some(basis) = &position.cost_basis {
            if amount(basis)? < Decimal::ZERO {
                return Err(invalid("basis"));
            }
        }
        market = add(market, value)?;
    }
    if (market - closing_value).abs() > Decimal::new(1, 2) {
        return Err(invalid("closingValue"));
    }
    let closing_positions = positions.clone();
    let mut seen = BTreeSet::new();
    let mut cash_delta = Decimal::ZERO;
    let mut deltas: BTreeMap<NaiveDate, (BTreeMap<String, Decimal>, Decimal)> = BTreeMap::new();
    for (i, event) in input.events.iter().enumerate() {
        let apply = || -> Result<_> {
            if event.date < input.from || event.date > input.through {
                return Err(invalid("eventDate"));
            }
            event_delta(event)
        };
        let (delta, cash) = apply().map_err(|mut error| {
            error.record = Some(i);
            error
        })?;
        if !seen.insert(&event.source_id) {
            return Err(ReconstructionError {
                code: "duplicate",
                record: Some(i),
            });
        }
        cash_delta = add(cash_delta, cash)?;
        let daily = deltas.entry(event.date).or_default();
        daily.1 = add(daily.1, cash)?;
        if let Some((asset, quantity)) = delta {
            let q = positions.entry(asset.to_owned()).or_default();
            *q = subtract(*q, quantity)?;
            let change = daily.0.entry(asset.to_owned()).or_default();
            *change = add(*change, quantity)?;
        }
    }
    if (add(opening_cash, cash_delta)? - closing_cash).abs() > Decimal::new(1, 2) {
        return Err(invalid("cashMismatch"));
    }
    if positions.len() > 1000 {
        return Err(invalid("size"));
    }
    if positions.values().any(|q| *q < Decimal::ZERO) {
        return Err(invalid("openingShort"));
    }
    let opening_positions = strings(&positions);
    let mut cash = opening_cash;
    let mut days = Vec::new();
    for offset in 0..=length {
        let date = input
            .from
            .checked_add_days(Days::new(offset as u64))
            .ok_or_else(|| invalid("period"))?;
        if let Some((changes, movement)) = deltas.get(&date) {
            cash = add(cash, *movement)?;
            for (asset, quantity) in changes {
                let q = positions.entry(asset.clone()).or_default();
                *q = add(*q, *quantity)?;
            }
        }
        if positions.values().any(|q| *q < Decimal::ZERO) {
            return Err(invalid("negativePosition"));
        }
        days.push(StatementDay {
            date,
            cash: cash.normalize().to_string(),
            positions: strings(&positions),
        });
    }
    if strings(&positions) != strings(&closing_positions) {
        return Err(invalid("quantityMismatch"));
    }
    Ok(ReconstructedInterval {
        from: input.from,
        through: input.through,
        currency: input.currency.clone(),
        opening_cash: opening_cash.normalize().to_string(),
        opening_positions,
        days,
    })
}

/// Check only truly contiguous intervals. Missing months remain explicit gaps;
/// equality at distant endpoints does not establish coverage between them.
pub fn validate_statement_chain(
    inputs: &[StatementInterval],
) -> Result<Vec<ReconstructedInterval>> {
    if inputs.len() > 1200 {
        return Err(invalid("size"));
    }
    let mut sorted: Vec<_> = inputs.iter().collect();
    sorted.sort_by_key(|s| s.from);
    let mut out: Vec<ReconstructedInterval> = Vec::new();
    for input in sorted {
        let interval = reconstruct_statement(input)?;
        if let Some(previous) = out.last() {
            if interval.from <= previous.through {
                return Err(invalid("overlap"));
            }
            if previous.through.succ_opt() == Some(interval.from) {
                let last = previous.days.last().ok_or_else(|| invalid("incomplete"))?;
                if amount(&last.cash)? != amount(&interval.opening_cash)?
                    || last.positions != interval.opening_positions
                {
                    return Err(invalid("continuity"));
                }
            }
        }
        out.push(interval);
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
