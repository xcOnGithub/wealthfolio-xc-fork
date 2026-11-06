//! Deterministic conversion. Unknown cash-flow meaning blocks the complete batch.
use super::{state::*, BankTransaction};
use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use wealthfolio_core::activities::{
    ActivityStatus, ActivityUpsert, ACTIVITY_SUBTYPE_OPENING_BALANCE,
};

pub const MAX_RECORDS: usize = 10_000;
pub const MAX_STAGE_BYTES: usize = 8 * 1024 * 1024;

pub fn stable_id(mapping: &str, record: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(mapping.as_bytes());
    hash.update([0]);
    hash.update(record.as_bytes());
    format!("plaid-{:x}", hash.finalize())
}

pub fn digest(stage: &BankStage, decisions: &BTreeMap<String, Decision>) -> Result<String> {
    let mut copy = stage.clone();
    copy.digest.clear();
    copy.approved = false;
    copy.opening_balance = None;
    let bytes = serde_json::to_vec(&(copy, decisions)).map_err(|_| SyncError::Storage)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub fn currency(account: &super::Account) -> Result<String> {
    let currency = account
        .balances
        .iso_currency_code
        .as_ref()
        .ok_or(SyncError::Review)?;
    if currency.len() != 3
        || !currency.bytes().all(|c| c.is_ascii_uppercase())
        || account.balances.unofficial_currency_code.is_some()
    {
        return Err(SyncError::Review);
    }
    Ok(currency.clone())
}

/// Raw provider category is evidence for common purchases/income, never proof of
/// an internal transfer. Transfers/refunds with ambiguous category require review.
pub fn decision(tx: &BankTransaction, mapping: &Mapping) -> Result<Decision> {
    if let Some(decision) = mapping.decisions.get(&tx.transaction_id) {
        return Ok(decision.clone());
    }
    let category = tx
        .personal_finance_category
        .as_ref()
        .and_then(|v| v.get("primary"))
        .and_then(serde_json::Value::as_str);
    let kind = match category {
        Some("INCOME") if tx.amount <= Decimal::ZERO => "income",
        Some(
            "FOOD_AND_DRINK"
            | "GENERAL_MERCHANDISE"
            | "TRANSPORTATION"
            | "TRAVEL"
            | "RENT_AND_UTILITIES"
            | "MEDICAL"
            | "PERSONAL_CARE"
            | "ENTERTAINMENT"
            | "HOME_IMPROVEMENT"
            | "GENERAL_SERVICES"
            | "GOVERNMENT_AND_NON_PROFIT"
            | "BANK_FEES",
        ) => {
            if tx.amount >= Decimal::ZERO {
                "expense"
            } else {
                "refund"
            }
        }
        _ => return Err(SyncError::Review),
    };
    Ok(Decision {
        kind: kind.into(),
        group: None,
    })
}

pub fn suggested_opening(mapping: &Mapping, stage: &BankStage) -> Result<Decimal> {
    let balance = stage
        .balance
        .parse::<Decimal>()
        .map_err(|_| SyncError::Review)?;
    let mut total = Decimal::ZERO;
    for tx in &stage.records {
        validate_transaction(tx, mapping)?;
        if !tx.pending {
            total = total.checked_add(tx.amount).ok_or(SyncError::Review)?;
        }
    }
    balance.checked_add(total).ok_or(SyncError::Review)
}

pub fn validate_transaction(tx: &BankTransaction, m: &Mapping) -> Result<()> {
    if tx.account_id != m.external_id
        || tx.transaction_id.is_empty()
        || tx.transaction_id.len() > 512
        || tx.iso_currency_code.as_deref() != Some(&m.currency)
        || tx.unofficial_currency_code.is_some()
        || tx.date > Utc::now().date_naive()
        || tx.name.len() > 4096
    {
        return Err(SyncError::Review);
    }
    Ok(())
}

fn activity(
    m: &Mapping,
    record: &str,
    date: NaiveDate,
    amount: Decimal,
    kind: &str,
) -> ActivityUpsert {
    ActivityUpsert {
        id: stable_id(&m.id, record),
        account_id: m.account_id.clone(),
        asset_id: None,
        activity_type: kind.into(),
        subtype: None,
        activity_date: format!("{date}T12:00:00Z"),
        quantity: None,
        unit_price: None,
        currency: m.currency.clone(),
        fee: None,
        tax: None,
        amount: Some(amount.abs()),
        status: Some(ActivityStatus::Posted),
        notes: None,
        fx_rate: None,
        metadata: None,
        needs_review: Some(false),
        source_system: Some("PLAID".into()),
        source_record_id: Some(record.into()),
        source_group_id: None,
        idempotency_key: Some(stable_id(&m.id, record)),
        import_run_id: None,
    }
}

/// Imported rows corrected outside the owner's cutover are ledger removals,
/// while their new pre-cutover source records remain part of provider history.
pub fn cutover_removals(mapping: &Mapping, stage: &BankStage) -> Result<Vec<String>> {
    let Some(cutover) = mapping.cutover_date else {
        return Ok(Vec::new());
    };
    let mut removed = Vec::new();
    for tx in stage
        .records
        .iter()
        .filter(|tx| !tx.pending && tx.date < cutover)
    {
        if let Some(old) = mapping.source_history.get(&tx.transaction_id) {
            let old: BankTransaction =
                serde_json::from_value(old.clone()).map_err(|_| SyncError::Storage)?;
            if !old.pending && old.date >= cutover {
                removed.push(tx.transaction_id.clone());
            }
        }
    }
    Ok(removed)
}

pub fn bank_commit(mut mapping: Mapping) -> Result<BankCommit> {
    let stage = mapping.staged.as_ref().ok_or(SyncError::NotReady)?;
    if stage.checkpoint_in != mapping.cursor
        || (Utc::now() - stage.fetched_at).num_minutes() > 30
        || !stage.approved
    {
        return Err(SyncError::Review);
    }
    let mut activities = Vec::new();
    let mut seen = HashSet::new();
    let corrections = cutover_removals(&mapping, stage)?;
    let correction_ids: HashSet<_> = corrections.iter().map(String::as_str).collect();
    let provider_removals: HashSet<_> = stage.removed.iter().map(String::as_str).collect();
    let mut removed = stage.removed.clone();
    removed.extend(corrections.iter().cloned());
    let mut history: BTreeMap<String, BankTransaction> =
        serde_json::from_value(mapping.source_history.clone()).map_err(|_| SyncError::Storage)?;
    let earliest_existing = history
        .values()
        .filter(|t| mapping.cutover_date.is_none_or(|d| t.date >= d))
        .map(|t| t.date)
        .min();
    for tx in &stage.records {
        validate_transaction(tx, &mapping)?;
        if !seen.insert(tx.transaction_id.clone()) {
            return Err(SyncError::Review);
        }
        if tx.pending {
            continue;
        }
        if mapping.cutover_date.is_some_and(|d| tx.date < d) {
            history.insert(tx.transaction_id.clone(), tx.clone());
            continue;
        }
        let selected = decision(tx, &mapping)?;
        let incoming = tx.amount < Decimal::ZERO;
        let kind = match selected.kind.as_str() {
            "income" if tx.amount <= Decimal::ZERO => {
                if mapping.credit_card {
                    "CREDIT"
                } else {
                    "DEPOSIT"
                }
            }
            "refund" if tx.amount <= Decimal::ZERO => "CREDIT",
            "expense" if tx.amount >= Decimal::ZERO => "WITHDRAWAL",
            "external" => {
                if incoming {
                    "DEPOSIT"
                } else {
                    "WITHDRAWAL"
                }
            }
            "internal"
                if selected
                    .group
                    .as_ref()
                    .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()) =>
            {
                if incoming {
                    "TRANSFER_IN"
                } else {
                    "TRANSFER_OUT"
                }
            }
            _ => return Err(SyncError::Review),
        };
        let mut row = activity(&mapping, &tx.transaction_id, tx.date, tx.amount, kind);
        if selected.kind == "refund" {
            row.subtype = Some("REFUND".into());
        }
        row.notes = Some(tx.name.clone());
        row.source_group_id = selected.group;
        activities.push(row);
        if let Some(pending) = &tx.pending_transaction_id {
            removed.push(pending.clone());
        }
        history.insert(tx.transaction_id.clone(), tx.clone());
    }
    if removed.iter().any(|id| {
        activities
            .iter()
            .any(|a| a.source_record_id.as_ref() == Some(id))
    }) {
        return Err(SyncError::Review);
    }
    for id in &removed {
        if !correction_ids.contains(id.as_str()) || provider_removals.contains(id.as_str()) {
            history.remove(id);
        }
    }
    if mapping.cursor.is_none() && mapping.cutover_date.is_none() {
        let opening = stage
            .opening_balance
            .as_ref()
            .ok_or(SyncError::Review)?
            .parse::<Decimal>()
            .map_err(|_| SyncError::Review)?;
        if opening != Decimal::ZERO {
            let first = stage
                .records
                .iter()
                .filter(|t| !t.pending)
                .map(|t| t.date)
                .min()
                .unwrap_or(Utc::now().date_naive());
            let mut row = activity(
                &mapping,
                "opening-balance",
                first.pred_opt().ok_or(SyncError::Review)?,
                opening,
                if opening > Decimal::ZERO {
                    "DEPOSIT"
                } else {
                    "WITHDRAWAL"
                },
            );
            row.subtype = Some(ACTIVITY_SUBTYPE_OPENING_BALANCE.into());
            activities.push(row);
        }
    }
    let earliest = activities
        .iter()
        .filter_map(|a| {
            chrono::DateTime::parse_from_rfc3339(&a.activity_date)
                .ok()
                .map(|d| d.with_timezone(&Utc))
        })
        .chain(
            earliest_existing
                .and_then(|d| d.and_hms_opt(12, 0, 0))
                .map(|d| d.and_utc()),
        )
        .min();
    let expected_cursor = mapping.cursor.clone();
    mapping.cursor = Some(stage.next_cursor.clone());
    mapping.source_history = serde_json::to_value(history).map_err(|_| SyncError::Storage)?;
    mapping.staged = None;
    mapping.last_error = None;
    mapping.last_success = Some(Utc::now());
    mapping.status = "idle".into();
    Ok(BankCommit {
        mapping,
        expected_cursor,
        activities,
        earliest,
        removed_ids: removed,
    })
}

/// Quotes are daily calendar observations. Preserve raw provider/fetch timestamps
/// in source history; normalize the quote key to the account's snapshot calendar.
pub fn holdings_quote_timestamp<Tz: chrono::TimeZone>(
    holding: &super::Holding,
    fetched_at: chrono::DateTime<Utc>,
    timezone: &Tz,
) -> Result<chrono::DateTime<Utc>> {
    if holding
        .institution_price_datetime
        .is_some_and(|time| time > fetched_at)
    {
        return Err(SyncError::Review);
    }
    let today = fetched_at.with_timezone(timezone).date_naive();
    let day = holding
        .institution_price_as_of
        .or_else(|| {
            holding
                .institution_price_datetime
                .map(|time| time.with_timezone(timezone).date_naive())
        })
        .unwrap_or(today);
    if day > today {
        return Err(SyncError::Review);
    }
    Ok(day
        .and_hms_opt(12, 0, 0)
        .ok_or(SyncError::Review)?
        .and_utc())
}
