//! Deterministic, split-aware, 1-to-1 household wash-sale scanner (advisory only).
//!
//! Scans replacement `BUY` acquisitions across ALL household accounts (including
//! spouses' taxable accounts and IRA/Roth accounts) within the 61-day window
//! `[disposal_date - 30 days, disposal_date + 30 days]`.

use chrono::{Duration, NaiveDate};
use rust_decimal::Decimal;
use std::collections::HashMap;

use crate::accounts::TaxTreatment;

use super::model::{WashSaleAlert, WashSaleConfidence, WashSaleSeverity};

/// A taxable/unclassified loss disposal eligible for advisory wash-sale screening.
#[derive(Debug, Clone)]
pub struct LossDisposalInput {
    pub disposal_id: String,
    pub lot_id: String,
    pub open_activity_id: Option<String>,
    pub open_date: Option<NaiveDate>,
    pub cost_basis_method: String,
    pub disposal_activity_id: String,
    pub account_id: String,
    pub account_name: String,
    pub owner_name: Option<String>,
    pub asset_id: String,
    pub symbol: String,
    pub disposal_date: NaiveDate,
    pub disposed_quantity: Decimal,
    /// Positive magnitude of the realized loss in base currency (`abs(realized_pnl_base)`).
    pub realized_loss_base: Decimal,
}

/// A candidate replacement purchase across any household account.
#[derive(Debug, Clone)]
pub struct ReplacementBuyInput {
    pub activity_id: String,
    pub account_id: String,
    pub account_name: String,
    pub owner_name: Option<String>,
    pub tax_treatment: Option<TaxTreatment>,
    pub asset_id: String,
    pub buy_date: NaiveDate,
    pub quantity: Decimal,
}

/// A stock split event affecting unit parity between two dates.
#[derive(Debug, Clone)]
pub struct AssetSplitEvent {
    pub asset_id: String,
    pub split_date: NaiveDate,
    pub ratio: Decimal,
}

/// Computes the multiplier that converts share units on `from_date` into
/// share units on `to_date` across any `SPLIT` events between them.
pub fn split_factor_between(
    splits: &[AssetSplitEvent],
    asset_id: &str,
    from_date: NaiveDate,
    to_date: NaiveDate,
) -> Decimal {
    if from_date == to_date {
        return Decimal::ONE;
    }
    let mut factor = Decimal::ONE;
    if from_date < to_date {
        for split in splits {
            if split.asset_id == asset_id
                && split.split_date > from_date
                && split.split_date <= to_date
                && split.ratio > Decimal::ZERO
            {
                factor *= split.ratio;
            }
        }
    } else {
        for split in splits {
            if split.asset_id == asset_id
                && split.split_date > to_date
                && split.split_date <= from_date
                && split.ratio > Decimal::ZERO
            {
                factor /= split.ratio;
            }
        }
    }
    factor
}

/// Runs deterministic chronological 1-to-1 wash-sale matching across all loss disposals.
///
/// Invariants:
/// 1. Each replacement buy's shares can wash at most `buy.quantity` shares across all loss slices
///    (prevents multi-lot single-sell double counting).
/// 2. A lot's own `open_activity_id` cannot wash that loss slice, and the disposed shares are
///    deducted from that opening activity's replacement pool so buying & selling the same lot
///    within 30 days never triggers a false positive.
/// 3. Split ratios between `replacement_date` and `disposal_date` normalize quantities before matching.
/// 4. Transferred/pooled lots lacking `open_activity_id` (or WAC lots) produce `NeedsReview`
///    confidence when matched to an earlier/same-day buy rather than silently false-flagging.
pub fn detect_wash_sales(
    loss_disposals: &[LossDisposalInput],
    replacement_buys: &[ReplacementBuyInput],
    splits: &[AssetSplitEvent],
) -> Vec<WashSaleAlert> {
    if loss_disposals.is_empty() || replacement_buys.is_empty() {
        return Vec::new();
    }

    let mut ordered_losses: Vec<&LossDisposalInput> = loss_disposals
        .iter()
        .filter(|d| d.disposed_quantity > Decimal::ZERO && d.realized_loss_base > Decimal::ZERO)
        .collect();
    ordered_losses.sort_by(|a, b| {
        a.disposal_date
            .cmp(&b.disposal_date)
            .then_with(|| a.disposal_activity_id.cmp(&b.disposal_activity_id))
            .then_with(|| a.lot_id.cmp(&b.lot_id))
            .then_with(|| a.disposal_id.cmp(&b.disposal_id))
    });

    let mut ordered_buys: Vec<&ReplacementBuyInput> = replacement_buys
        .iter()
        .filter(|b| b.quantity > Decimal::ZERO)
        .collect();
    ordered_buys.sort_by(|a, b| {
        a.buy_date
            .cmp(&b.buy_date)
            .then_with(|| a.activity_id.cmp(&b.activity_id))
    });

    // Remaining unallocated replacement shares per buy activity, stored in that buy's native date units.
    let mut remaining_buy_qty: HashMap<&str, Decimal> = ordered_buys
        .iter()
        .map(|b| (b.activity_id.as_str(), b.quantity))
        .collect();

    // Deduct each loss lot's own disposed quantity from its own opening activity so a simple
    // "buy 100 shares, sell 100 shares at a loss 10 days later" cannot self-wash or wash sibling slices.
    for loss in &ordered_losses {
        if let Some(open_id) = loss.open_activity_id.as_deref() {
            if let Some(buy) = ordered_buys.iter().find(|b| b.activity_id == open_id) {
                let to_buy_units =
                    split_factor_between(splits, &loss.asset_id, loss.disposal_date, buy.buy_date);
                let disposed_in_buy_units = loss.disposed_quantity * to_buy_units;
                if let Some(rem) = remaining_buy_qty.get_mut(open_id) {
                    *rem = (*rem - disposed_in_buy_units).max(Decimal::ZERO);
                }
            }
        }
    }

    let mut alerts = Vec::new();
    let window = Duration::days(30);

    for loss in ordered_losses {
        let window_start = loss.disposal_date - window;
        let window_end = loss.disposal_date + window;
        let mut remaining_loss_qty = loss.disposed_quantity;

        for buy in &ordered_buys {
            if remaining_loss_qty <= Decimal::ZERO {
                break;
            }
            if buy.asset_id != loss.asset_id {
                continue;
            }
            if buy.buy_date < window_start || buy.buy_date > window_end {
                continue;
            }
            if loss.open_activity_id.as_deref() == Some(buy.activity_id.as_str()) {
                continue;
            }

            let avail_in_buy_units = remaining_buy_qty
                .get(buy.activity_id.as_str())
                .copied()
                .unwrap_or(Decimal::ZERO);
            if avail_in_buy_units <= Decimal::ZERO {
                continue;
            }

            let to_disposal_units =
                split_factor_between(splits, &loss.asset_id, buy.buy_date, loss.disposal_date);
            if to_disposal_units <= Decimal::ZERO {
                continue;
            }
            let avail_in_disposal_units = avail_in_buy_units * to_disposal_units;
            if avail_in_disposal_units <= Decimal::ZERO {
                continue;
            }

            let matched_disposal_units = remaining_loss_qty.min(avail_in_disposal_units);
            if matched_disposal_units <= Decimal::ZERO {
                continue;
            }

            let consumed_buy_units = matched_disposal_units / to_disposal_units;
            if let Some(rem) = remaining_buy_qty.get_mut(buy.activity_id.as_str()) {
                *rem = (*rem - consumed_buy_units).max(Decimal::ZERO);
            }
            remaining_loss_qty = (remaining_loss_qty - matched_disposal_units).max(Decimal::ZERO);

            let ratio = (matched_disposal_units / loss.disposed_quantity).min(Decimal::ONE);
            let estimated_disallowed = (loss.realized_loss_base * ratio).round_dp(2);

            let severity = match buy.tax_treatment {
                Some(TaxTreatment::TaxDeferred) | Some(TaxTreatment::TaxFree) => {
                    WashSaleSeverity::IraPermanentDisallowance
                }
                _ if buy.account_id != loss.account_id => WashSaleSeverity::CrossAccount,
                _ => WashSaleSeverity::SameAccount,
            };

            let is_ambiguous_opening = (loss.open_activity_id.is_none()
                || loss.cost_basis_method.eq_ignore_ascii_case("WAC"))
                && buy.buy_date <= loss.disposal_date;
            let (confidence, note) = if is_ambiguous_opening {
                (
                    WashSaleConfidence::NeedsReview,
                    Some(
                        "Transferred or weighted-average lot lacks a direct opening-activity link; confirm this acquisition is not the disposed lot itself."
                            .to_string(),
                    ),
                )
            } else if severity == WashSaleSeverity::IraPermanentDisallowance {
                (
                    WashSaleConfidence::High,
                    Some(
                        "Replacement purchase occurred in a tax-advantaged account (IRA/Roth/401k); under IRS Rev. Rul. 2008-5 the loss may be permanently disallowed."
                            .to_string(),
                    ),
                )
            } else {
                (WashSaleConfidence::High, None)
            };

            alerts.push(WashSaleAlert {
                disposal_id: loss.disposal_id.clone(),
                disposal_activity_id: loss.disposal_activity_id.clone(),
                disposal_account_id: loss.account_id.clone(),
                disposal_account_name: loss.account_name.clone(),
                disposal_owner_name: loss.owner_name.clone(),
                disposal_date: loss.disposal_date,
                asset_id: loss.asset_id.clone(),
                symbol: loss.symbol.clone(),
                disposed_quantity: loss.disposed_quantity,
                realized_loss_base: loss.realized_loss_base,
                replacement_activity_id: buy.activity_id.clone(),
                replacement_account_id: buy.account_id.clone(),
                replacement_account_name: buy.account_name.clone(),
                replacement_owner_name: buy.owner_name.clone(),
                replacement_tax_treatment: buy.tax_treatment,
                replacement_date: buy.buy_date,
                matched_quantity: matched_disposal_units,
                estimated_disallowed_loss_base: estimated_disallowed,
                severity,
                confidence,
                note,
            });
        }
    }

    alerts
}

/// Checks if any household account bought `asset_id` in `[as_of_date - 30 days, as_of_date]`.
/// Returns `(cooldown_until, triggering_account_name)` where `cooldown_until = latest_buy + 31 days`.
pub fn active_wash_sale_cooldown(
    asset_id: &str,
    as_of_date: NaiveDate,
    recent_buys: &[ReplacementBuyInput],
) -> Option<(NaiveDate, String)> {
    let cutoff = as_of_date - Duration::days(30);
    recent_buys
        .iter()
        .filter(|b| b.asset_id == asset_id && b.buy_date >= cutoff && b.buy_date <= as_of_date)
        .max_by(|a, b| {
            a.buy_date
                .cmp(&b.buy_date)
                .then_with(|| a.activity_id.cmp(&b.activity_id))
        })
        .map(|latest| {
            (
                latest.buy_date + Duration::days(31),
                latest.account_name.clone(),
            )
        })
}
