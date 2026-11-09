//! Pure brokerage conversion and opening-state reconciliation. No provider I/O.
use super::{ingest, state::*, InvestmentTransaction, Security};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use wealthfolio_core::activities::{ActivityStatus, ActivityUpsert};

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpeningPosition {
    pub quantity: String,
    pub unit_cost: String,
}

/// Fixed reviewed baseline. Recomputing this during sync would conceal missing trades.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InvestmentLedger {
    pub start_date: NaiveDate,
    pub opening_cash: String,
    pub opening_positions: BTreeMap<String, OpeningPosition>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub record_id: String,
    pub reason: String,
}

pub struct Draft {
    pub source: String,
    pub date: NaiveDate,
    pub security: Option<String>,
    pub kind: &'static str,
    pub quantity: Decimal,
    pub price: Decimal,
    pub cash: Decimal,
    pub fee: Decimal,
}

pub struct Plan {
    pub start_date: NaiveDate,
    pub current_cash: Decimal,
    pub opening_cash: Decimal,
    pub opening_quantities: BTreeMap<String, Decimal>,
    pub current_quantities: BTreeMap<String, Decimal>,
    pub drafts: Vec<Draft>,
    pub issues: Vec<Issue>,
    pub securities: BTreeMap<String, Security>,
}

fn number(value: &str) -> Result<Decimal> {
    value.parse().map_err(|_| SyncError::Review)
}
fn add(left: Decimal, right: Decimal) -> Result<Decimal> {
    left.checked_add(right).ok_or(SyncError::Review)
}
fn ordinary(security: &Security, currency: &str) -> bool {
    matches!(
        security.security_type.as_deref(),
        Some("equity" | "etf" | "mutual fund")
    ) && security
        .ticker_symbol
        .as_ref()
        .is_some_and(|s| !s.is_empty())
        && security
            .iso_currency_code
            .as_deref()
            .is_none_or(|c| c == currency)
        && security.unofficial_currency_code.is_none()
}

fn convert(
    tx: &InvestmentTransaction,
    currency: &str,
    securities: &BTreeMap<String, Security>,
) -> Result<Vec<Draft>> {
    if tx.iso_currency_code.as_deref() != Some(currency)
        || tx.unofficial_currency_code.is_some()
        || tx.fees.is_some_and(|f| f < Decimal::ZERO)
    {
        return Err(SyncError::Review);
    }
    let cash_security = tx
        .security_id
        .as_ref()
        .and_then(|id| securities.get(id))
        .is_some_and(|s| {
            s.security_type.as_deref() == Some("cash")
                && s.iso_currency_code.as_deref().is_none_or(|c| c == currency)
                && s.unofficial_currency_code.is_none()
        });
    let fee = tx.fees.unwrap_or_default();
    let mut row = Draft {
        source: format!("investment:{}", tx.investment_transaction_id),
        date: tx.date,
        security: tx.security_id.clone(),
        kind: "",
        quantity: tx.quantity.abs(),
        price: tx.price,
        cash: -tx.amount,
        fee,
    };
    match (tx.transaction_type.as_str(), tx.subtype.as_str()) {
        ("buy", "buy" | "purchase" | "trade") | ("sell", "sell" | "trade") => {
            let buy = tx.transaction_type == "buy";
            if row.security.is_none()
                || row.quantity <= Decimal::ZERO
                || row.price <= Decimal::ZERO
                || (buy && tx.quantity <= Decimal::ZERO)
                || (!buy && tx.quantity >= Decimal::ZERO)
                || (buy && tx.amount <= Decimal::ZERO)
                || (!buy && tx.amount >= Decimal::ZERO)
            {
                return Err(SyncError::Review);
            }
            let gross = row
                .quantity
                .checked_mul(row.price)
                .ok_or(SyncError::Review)?;
            let expected = if buy {
                add(gross, fee)?
            } else {
                add(gross, -fee)?
            };
            if add(expected, -tx.amount.abs())?.abs() > Decimal::new(1, 2) {
                return Err(SyncError::Review);
            }
            row.kind = if buy { "BUY" } else { "SELL" };
        }
        ("cash", "dividend" | "qualified dividend" | "non-qualified dividend") => {
            row.kind = "DIVIDEND";
            // Cash sweeps (including money-market funds) pay cash income,
            // not a dividend on a separately tracked equity position.
            if cash_security {
                row.security = None;
            }
        }
        ("cash", "interest") => {
            row.kind = "INTEREST";
            row.security = None;
        }
        ("cash", "deposit" | "contribution") | ("transfer", "deposit" | "contribution")
            if (tx.security_id.is_none() || cash_security) && tx.amount < Decimal::ZERO =>
        {
            row.kind = "DEPOSIT";
            row.security = None;
        }
        ("cash", "withdrawal" | "distribution") | ("transfer", "withdrawal" | "distribution")
            if (tx.security_id.is_none() || cash_security) && tx.amount > Decimal::ZERO =>
        {
            row.kind = "WITHDRAWAL";
            row.security = None;
        }
        (
            "fee",
            "account fee" | "management fee" | "miscellaneous fee" | "legal fee" | "trust fee",
        ) if tx.amount > Decimal::ZERO && fee == Decimal::ZERO => {
            row.kind = "FEE";
            row.security = None;
        }
        ("cash", "tax" | "tax withheld" | "non-resident tax")
            if tx.amount > Decimal::ZERO && fee == Decimal::ZERO =>
        {
            row.kind = "TAX";
            row.security = None;
        }
        _ => return Err(SyncError::Review),
    }
    if matches!(row.kind, "DIVIDEND" | "INTEREST") && row.cash < Decimal::ZERO {
        return Err(SyncError::Review);
    }
    if !matches!(row.kind, "BUY" | "SELL") {
        row.quantity = Decimal::ZERO;
        row.price = Decimal::ZERO;
    }
    Ok(vec![row])
}

pub fn plan(m: &Mapping, stage: &HoldingsStage) -> Result<Plan> {
    let account = stage
        .response
        .accounts
        .iter()
        .find(|a| a.account_id == m.external_id)
        .ok_or(SyncError::Review)?;
    let securities: BTreeMap<_, _> = stage
        .response
        .securities
        .iter()
        .cloned()
        .map(|s| (s.security_id.clone(), s))
        .collect();
    if securities.len() != stage.response.securities.len() {
        return Err(SyncError::Review);
    }
    let mut issues = Vec::new();
    let mut quantities = BTreeMap::new();
    let mut noncash = Decimal::ZERO;
    for h in &stage.response.holdings {
        if h.account_id != m.external_id
            || h.iso_currency_code.as_deref() != Some(&m.currency)
            || h.unofficial_currency_code.is_some()
        {
            return Err(SyncError::Review);
        }
        let security = securities.get(&h.security_id).ok_or(SyncError::Review)?;
        if security.security_type.as_deref() == Some("cash") {
            continue;
        }
        if h.quantity.is_zero() && !h.institution_value.is_zero() {
            return Err(SyncError::Review);
        }
        if !ordinary(security, &m.currency) || h.quantity < Decimal::ZERO {
            issues.push(Issue {
                record_id: h.security_id.clone(),
                reason: "unsupportedInstrument".into(),
            });
        }
        if quantities
            .insert(h.security_id.clone(), h.quantity)
            .is_some()
        {
            return Err(SyncError::Review);
        }
        noncash = add(noncash, h.institution_value)?;
    }
    let cash = add(account.balances.current.ok_or(SyncError::Review)?, -noncash)?;
    let ids: BTreeSet<_> = stage
        .history
        .iter()
        .map(|t| t.investment_transaction_id.as_str())
        .collect();
    if ids.len() != stage.history.len() || ids.contains("") {
        return Err(SyncError::Review);
    }
    let mut cancelled = BTreeSet::new();
    for tx in &stage.history {
        if tx.account_id != m.external_id
            || tx.date > stage.fetched_at.date_naive()
            || m.investment_ledger
                .as_ref()
                .is_some_and(|l| tx.date < l.start_date)
        {
            return Err(SyncError::Review);
        }
        if let Some(target) = &tx.cancel_transaction_id {
            if target == &tx.investment_transaction_id || !ids.contains(target.as_str()) {
                issues.push(Issue {
                    record_id: tx.investment_transaction_id.clone(),
                    reason: "missingCancellation".into(),
                });
            }
            cancelled.insert(target.as_str());
            cancelled.insert(tx.investment_transaction_id.as_str());
        }
    }
    let mut drafts = Vec::new();
    for tx in &stage.history {
        if cancelled.contains(tx.investment_transaction_id.as_str()) {
            continue;
        }
        match convert(tx, &m.currency, &securities) {
            Ok(rows)
                if rows.iter().all(|row| {
                    row.security.as_ref().is_none_or(|id| {
                        securities.get(id).is_some_and(|s| ordinary(s, &m.currency))
                    })
                }) =>
            {
                drafts.extend(rows)
            }
            _ => issues.push(Issue {
                record_id: tx.investment_transaction_id.clone(),
                reason: "unsupportedTransaction".into(),
            }),
        }
    }
    drafts.sort_by(|a, b| {
        (a.date, if a.kind == "BUY" { 0 } else { 1 }, &a.source).cmp(&(
            b.date,
            if b.kind == "BUY" { 0 } else { 1 },
            &b.source,
        ))
    });
    let start = m
        .investment_ledger
        .as_ref()
        .map(|l| l.start_date)
        .unwrap_or_else(|| {
            stage
                .history
                .iter()
                .map(|t| t.date)
                .min()
                .unwrap_or(stage.fetched_at.date_naive())
        });
    let mut opening = quantities.clone();
    let mut net_cash = Decimal::ZERO;
    for row in &drafts {
        net_cash = add(net_cash, row.cash)?;
        if matches!(row.kind, "BUY" | "SELL") {
            let id = row.security.as_ref().ok_or(SyncError::Review)?;
            let delta = if row.kind == "BUY" {
                row.quantity
            } else {
                -row.quantity
            };
            let qty = opening.entry(id.clone()).or_default();
            *qty = add(*qty, -delta)?;
        }
    }
    let opening_cash = add(cash, -net_cash)?;
    if let Some(ledger) = &m.investment_ledger {
        opening = ledger
            .opening_positions
            .iter()
            .map(|(id, p)| Ok((id.clone(), number(&p.quantity)?)))
            .collect::<Result<_>>()?;
    }
    for (id, qty) in &opening {
        if *qty < Decimal::ZERO {
            issues.push(Issue {
                record_id: id.clone(),
                reason: "openingShort".into(),
            });
        }
    }
    let mut projected = opening.clone();
    let mut projected_cash = m
        .investment_ledger
        .as_ref()
        .map(|l| number(&l.opening_cash))
        .transpose()?
        .unwrap_or(opening_cash);
    for row in &drafts {
        projected_cash = add(projected_cash, row.cash)?;
        if matches!(row.kind, "BUY" | "SELL") {
            let id = row.security.as_ref().ok_or(SyncError::Review)?;
            let qty = projected.entry(id.clone()).or_default();
            *qty = add(
                *qty,
                if row.kind == "BUY" {
                    row.quantity
                } else {
                    -row.quantity
                },
            )?;
            if *qty < Decimal::ZERO {
                issues.push(Issue {
                    record_id: row.source.clone(),
                    reason: "negativePosition".into(),
                });
            }
        }
    }
    let all: BTreeSet<_> = projected.keys().chain(quantities.keys()).collect();
    let position_mismatch = all.iter().any(|id| {
        projected
            .get(*id)
            .copied()
            .unwrap_or_default()
            .checked_sub(quantities.get(*id).copied().unwrap_or_default())
            .is_none_or(|d| d.abs() > Decimal::new(1, 8))
    });
    if position_mismatch || add(projected_cash, -cash)?.abs() > Decimal::new(1, 2) {
        issues.push(Issue {
            record_id: m.id.clone(),
            reason: "reconciliation".into(),
        });
    }
    Ok(Plan {
        start_date: start,
        current_cash: cash,
        opening_cash: m
            .investment_ledger
            .as_ref()
            .map(|l| number(&l.opening_cash))
            .transpose()?
            .unwrap_or(opening_cash),
        opening_quantities: opening,
        current_quantities: quantities,
        drafts,
        issues,
        securities,
    })
}

pub fn activities(
    m: &Mapping,
    plan: &Plan,
    ledger: &InvestmentLedger,
    asset_ids: &BTreeMap<String, String>,
    timezone: &str,
) -> Result<Vec<ActivityUpsert>> {
    if !plan.issues.is_empty() {
        return Err(SyncError::Review);
    }
    let tz = wealthfolio_core::utils::time_utils::parse_user_timezone_or_default(timezone);
    use chrono::TimeZone;
    let make = |source: &str,
                date: NaiveDate,
                kind: &str,
                security: Option<&str>,
                qty: Decimal,
                price: Decimal,
                cash: Decimal,
                fee: Decimal|
     -> Result<ActivityUpsert> {
        Ok(ActivityUpsert {id:ingest::stable_id(&m.id,source),account_id:m.account_id.clone(),
            asset_id:security.map(|id|asset_ids.get(id).cloned().ok_or(SyncError::Review)).transpose()?,
            activity_type:kind.into(),subtype:None,activity_date:tz.from_local_datetime(&date.and_hms_opt(12,0,0).ok_or(SyncError::Review)?).single().ok_or(SyncError::Review)?.to_rfc3339(),
            quantity:security.map(|_|qty),unit_price:security.map(|_|price),currency:m.currency.clone(),fee:Some(fee),tax:None,
            amount:Some(cash.abs()),status:Some(ActivityStatus::Posted),notes:None,fx_rate:None,
            metadata:Some(serde_json::json!({"flow":{"is_external":true},"plaid":{"coverageStart":ledger.start_date,"completeLifetimeHistory":false}}).to_string()),
            needs_review:Some(false),source_system:Some("PLAID".into()),source_record_id:Some(source.into()),source_group_id:None,idempotency_key:Some(ingest::stable_id(&m.id,source)),import_run_id:None})
    };
    let mut rows = Vec::new();
    let opening_day = ledger.start_date.pred_opt().ok_or(SyncError::Review)?;
    let cash = number(&ledger.opening_cash)?;
    if cash != Decimal::ZERO {
        rows.push(make(
            "investment:opening:cash",
            opening_day,
            if cash > Decimal::ZERO {
                "DEPOSIT"
            } else {
                "WITHDRAWAL"
            },
            None,
            Decimal::ZERO,
            Decimal::ZERO,
            cash,
            Decimal::ZERO,
        )?);
    }
    for (id, qty) in &plan.opening_quantities {
        if *qty == Decimal::ZERO {
            continue;
        }
        let opening = ledger.opening_positions.get(id).ok_or(SyncError::Review)?;
        let price = number(&opening.unit_cost)?;
        if number(&opening.quantity)? != *qty || price < Decimal::ZERO {
            return Err(SyncError::Review);
        }
        let mut row = make(
            &format!("investment:opening:{id}"),
            opening_day,
            "TRANSFER_IN",
            Some(id),
            *qty,
            price,
            Decimal::ZERO,
            Decimal::ZERO,
        )?;
        row.subtype = Some("OPENING_BALANCE".into());
        rows.push(row);
    }
    for draft in &plan.drafts {
        rows.push(make(
            &draft.source,
            draft.date,
            draft.kind,
            draft.security.as_deref(),
            draft.quantity,
            draft.price,
            draft.cash,
            draft.fee,
        )?);
    }
    Ok(rows)
}
