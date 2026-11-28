//! Portfolio snapshot domain models.

use chrono::{NaiveDate, NaiveDateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::HashMap;
use uuid::Uuid;

use super::Position;

/// Source of a snapshot - how it was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SnapshotSource {
    /// Calculated from transaction history
    #[default]
    Calculated,
    /// Manually entered by user
    ManualEntry,
    /// Imported from broker connection
    BrokerImported,
    /// Imported from CSV file
    CsvImport,
}

/// Lightweight snapshot metadata used by listing and remediation flows.
///
/// `snapshot_date` and `source` intentionally remain raw strings so a corrupt
/// stored row can still be inspected and deleted without deserializing it as a
/// valid portfolio snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotMetadata {
    pub id: String,
    pub account_id: String,
    pub snapshot_date: String,
    pub source: String,
    pub position_count: usize,
    pub cash_currency_count: usize,
    pub cash_total_account_currency: String,
}

impl From<&AccountStateSnapshot> for SnapshotMetadata {
    fn from(snapshot: &AccountStateSnapshot) -> Self {
        Self {
            id: snapshot.id.clone(),
            account_id: snapshot.account_id.clone(),
            snapshot_date: snapshot.snapshot_date.format("%Y-%m-%d").to_string(),
            source: snapshot.source.as_str().to_string(),
            position_count: snapshot.positions.len(),
            cash_currency_count: snapshot.cash_balances.len(),
            cash_total_account_currency: snapshot.cash_total_account_currency.to_string(),
        }
    }
}

impl SnapshotSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            SnapshotSource::Calculated => "CALCULATED",
            SnapshotSource::ManualEntry => "MANUAL_ENTRY",
            SnapshotSource::BrokerImported => "BROKER_IMPORTED",
            SnapshotSource::CsvImport => "CSV_IMPORT",
        }
    }
}

/// Represents a warning that occurred during holdings calculation.
/// These are non-fatal issues where calculation continued but with potential data quality concerns.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HoldingsCalculationWarning {
    pub activity_id: String,
    pub account_id: String,
    pub date: NaiveDate,
    pub message: String,
}

impl std::fmt::Display for HoldingsCalculationWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Activity {} (account: {}, date: {}): {}",
            self.activity_id, self.account_id, self.date, self.message
        )
    }
}

/// Result of holdings calculation containing both the snapshot and any warnings.
/// The snapshot is always returned (even on partial failures), but warnings indicate
/// which activities could not be processed correctly.
#[derive(Debug, Clone)]
pub struct HoldingsCalculationResult {
    pub snapshot: AccountStateSnapshot,
    pub warnings: Vec<HoldingsCalculationWarning>,
}

impl HoldingsCalculationResult {
    pub fn new(snapshot: AccountStateSnapshot) -> Self {
        Self {
            snapshot,
            warnings: Vec::new(),
        }
    }

    pub fn with_warnings(
        snapshot: AccountStateSnapshot,
        warnings: Vec<HoldingsCalculationWarning>,
    ) -> Self {
        Self { snapshot, warnings }
    }

    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }
}

/// Represents the comprehensive state of an account at the close of a specific day.
/// This becomes the primary data structure stored and retrieved by the ValuationRepository.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountStateSnapshot {
    pub id: String, // Stable UUID derived from (account_id, snapshot_date)
    pub account_id: String,
    pub snapshot_date: NaiveDate,
    pub currency: String, // Account's reporting currency
    /// Currency used for the precomputed base amounts. Absent on legacy or observed snapshots.
    #[serde(default)]
    pub base_currency: Option<String>,

    // --- Core State ---
    // Use the detailed Position struct for accuracy, including lots.
    #[serde(default)]
    pub positions: HashMap<String, Position>, // asset_id -> Position (holds quantity, lots, cost basis info)

    #[serde(default)]
    pub cash_balances: HashMap<String, Decimal>, // currency -> amount

    // --- Calculated Aggregates (Account Currency) ---
    #[serde(default)]
    pub cost_basis: Decimal, // Historical book cost of all positions in account currency
    #[serde(default)]
    pub net_contribution: Decimal, // Cumulative net deposits in account currency
    #[serde(default)]
    pub net_contribution_base: Decimal, // portfolio base currency

    /// Cached total of all cash balances converted to account currency.
    /// Computed once at end of daily holdings calculation.
    #[serde(default)]
    pub cash_total_account_currency: Decimal,

    /// Cached total of all cash balances converted to base currency.
    /// Computed once at end of daily holdings calculation.
    #[serde(default)]
    pub cash_total_base_currency: Decimal,

    pub calculated_at: NaiveDateTime, // When this snapshot was generated

    /// Source of this snapshot (how it was created)
    #[serde(default)]
    pub source: SnapshotSource,
}

impl Default for AccountStateSnapshot {
    fn default() -> Self {
        AccountStateSnapshot {
            id: String::new(),
            account_id: String::new(),
            snapshot_date: NaiveDate::from_ymd_opt(1970, 1, 1).expect("1970-01-01 is a valid date"),
            currency: String::new(),
            positions: HashMap::new(),
            cash_balances: HashMap::new(),
            cost_basis: Decimal::ZERO,
            net_contribution: Decimal::ZERO,
            net_contribution_base: Decimal::ZERO,
            cash_total_account_currency: Decimal::ZERO,
            cash_total_base_currency: Decimal::ZERO,
            base_currency: None,
            calculated_at: Utc::now().naive_utc(),
            source: SnapshotSource::default(),
        }
    }
}

impl AccountStateSnapshot {
    /// Returns a stable UUID for a snapshot identity (account_id + snapshot_date).
    pub fn stable_id(account_id: &str, snapshot_date: NaiveDate) -> String {
        Self::id_in_namespace("wealthfolio:snapshot", account_id, snapshot_date)
    }

    /// Derived rows must never replace a user's observation for the same date.
    pub fn calculated_id(account_id: &str, snapshot_date: NaiveDate) -> String {
        Self::id_in_namespace("wealthfolio:calculated-snapshot", account_id, snapshot_date)
    }

    fn id_in_namespace(namespace: &str, account_id: &str, snapshot_date: NaiveDate) -> String {
        // Derive a deterministic UUID from account/date so upserts target the same row.
        let name = format!(
            "{}:{}:{}",
            namespace,
            account_id,
            snapshot_date.format("%Y-%m-%d")
        );
        let digest = sha2::Sha256::digest(name.as_bytes());
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        // RFC4122 variant + version 5 bit layout.
        bytes[6] = (bytes[6] & 0x0f) | 0x50;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Uuid::from_bytes(bytes).to_string()
    }

    /// Compares the core content of two snapshots (positions and cash_balances).
    /// Returns true if the holdings are effectively the same, ignoring metadata
    /// like id, snapshot_date, calculated_at, source, etc.
    ///
    /// For positions, compares: asset_id, quantity, average_cost, total_cost_basis,
    /// currency, cost_basis_status, and contract_multiplier.
    /// Ignores: lots, timestamps, inception_date.
    pub fn is_content_equal(&self, other: &Self) -> bool {
        // Compare cash balances
        if self.cash_balances != other.cash_balances {
            return false;
        }

        // Compare positions count
        if self.positions.len() != other.positions.len() {
            return false;
        }

        // Compare each position by key fields
        for (asset_id, pos) in &self.positions {
            match other.positions.get(asset_id) {
                None => return false,
                Some(other_pos) => {
                    if !Self::positions_equal(pos, other_pos) {
                        return false;
                    }
                }
            }
        }

        true
    }

    /// Compares two positions by their essential financial fields.
    /// Ignores lots, timestamps, and other metadata.
    fn positions_equal(a: &Position, b: &Position) -> bool {
        a.asset_id == b.asset_id
            && a.quantity == b.quantity
            && a.average_cost == b.average_cost
            && a.total_cost_basis == b.total_cost_basis
            && a.cost_basis_status == b.cost_basis_status
            && a.currency == b.currency
            && a.contract_multiplier == b.contract_multiplier
    }
}

/// Returns the unit cost in `position_currency` implied by `asset.default_unit_price()`
/// when `asset.quote_ccy` and `position_currency` share the same major currency
/// (normalizing minor units like `GBp` -> `GBP`).
///
/// Returns `None` when the asset has no `default_unit_price` or when currencies
/// differ and would require an FX rate.
pub fn default_unit_price_in_currency(
    asset: &crate::assets::Asset,
    position_currency: &str,
) -> Option<Decimal> {
    let default_price = asset.default_unit_price()?;
    if position_currency.trim().is_empty() {
        return None;
    }
    let (quote_major_price, quote_major_ccy) =
        crate::fx::currency::normalize_amount(default_price, asset.quote_ccy.trim());
    let (pos_unit, pos_major_ccy) =
        crate::fx::currency::normalize_amount(Decimal::ONE, position_currency.trim());
    if quote_major_ccy.is_empty()
        || pos_major_ccy.is_empty()
        || quote_major_ccy != pos_major_ccy
        || pos_unit.is_zero()
    {
        return None;
    }
    quote_major_price.checked_div(pos_unit)
}

/// Computes `(unit_cost, total_basis)` in `position_currency` from
/// `asset.default_unit_price()`, using `asset.contract_multiplier()` (matching
/// historical engine valuation) and enforcing the portfolio engine's
/// `MAX_MAGNITUDE` bounds so synthesized basis can never push an observed
/// snapshot out of range.
pub fn default_unit_price_basis_for_position(
    asset: &crate::assets::Asset,
    position_currency: &str,
    quantity: Decimal,
) -> Option<(Decimal, Decimal)> {
    default_unit_price_basis_for_position_with_split_factor(
        asset,
        position_currency,
        quantity,
        Decimal::ONE,
    )
}

/// Same as [`default_unit_price_basis_for_position`], scaling the current-unit
/// `default_unit_price` by `split_factor` (product of recorded split ratios
/// strictly after the snapshot date up to `as_of`) so pre-split snapshots carry
/// basis in pre-split units. Carrying those splits forward then divides
/// `average_cost` back to the current-unit default while preserving total basis.
pub fn default_unit_price_basis_for_position_with_split_factor(
    asset: &crate::assets::Asset,
    position_currency: &str,
    quantity: Decimal,
    split_factor: Decimal,
) -> Option<(Decimal, Decimal)> {
    let max_magnitude = wealthfolio_portfolio_engine::model::MAX_MAGNITUDE;
    if quantity <= Decimal::ZERO || quantity > max_magnitude {
        return None;
    }
    if split_factor <= Decimal::ZERO || split_factor > max_magnitude {
        return None;
    }
    let raw_default = asset.default_unit_price()?;
    if raw_default > max_magnitude {
        return None;
    }
    let current_unit_cost = default_unit_price_in_currency(asset, position_currency)?;
    if current_unit_cost > max_magnitude {
        return None;
    }
    let unit_cost = current_unit_cost.checked_mul(split_factor)?;
    if unit_cost <= Decimal::ZERO || unit_cost > max_magnitude {
        return None;
    }
    let multiplier = asset.contract_multiplier();
    if multiplier <= Decimal::ZERO || multiplier > max_magnitude {
        return None;
    }
    let total_basis = quantity
        .checked_mul(unit_cost)
        .and_then(|v| v.checked_mul(multiplier))?;
    if total_basis > max_magnitude {
        return None;
    }
    Some((unit_cost, total_basis))
}

/// Fills missing (`BasisStatus::Unknown`) cost basis on observed holdings
/// snapshots using each asset's optional `defaultUnitPrice` fallback (e.g., `$1`
/// par for cash-equivalent funds) without split adjustment.
pub fn apply_default_unit_price_basis_to_snapshot(
    snapshot: &mut AccountStateSnapshot,
    assets_by_id: &HashMap<String, crate::assets::Asset>,
) -> bool {
    let as_of = snapshot.snapshot_date;
    apply_default_unit_price_basis_to_snapshot_with_splits(snapshot, assets_by_id, &[], as_of)
}

/// Fills missing (`BasisStatus::Unknown`) cost basis on observed holdings
/// snapshots using each asset's optional `defaultUnitPrice` fallback in
/// split-adjusted units as of `snapshot.snapshot_date` relative to `as_of`.
/// Explicit cost basis (provider, user-edited, or statement-reconciled) is never
/// overwritten.
pub fn apply_default_unit_price_basis_to_snapshot_with_splits(
    snapshot: &mut AccountStateSnapshot,
    assets_by_id: &HashMap<String, crate::assets::Asset>,
    splits: &[wealthfolio_portfolio_engine::SplitEvent],
    as_of: NaiveDate,
) -> bool {
    use crate::portfolio::economic_events::BasisStatus;

    if snapshot.source == SnapshotSource::Calculated {
        return false;
    }
    let max_magnitude = wealthfolio_portfolio_engine::model::MAX_MAGNITUDE;
    let snap_currency = snapshot.currency.clone();
    let snap_base_currency = snapshot.base_currency.clone();
    let snapshot_date = snapshot.snapshot_date;

    let mut running_account_basis = Some(Decimal::ZERO);
    for p in snapshot.positions.values() {
        if p.basis_status() != BasisStatus::Complete {
            continue;
        }
        let pos_basis_account = p.cost_basis_account.or_else(|| {
            (p.currency.is_empty() || p.currency == snap_currency).then_some(p.total_cost_basis)
        });
        if let Some(basis) = pos_basis_account {
            running_account_basis = running_account_basis.and_then(|acc| acc.checked_add(basis));
        }
    }

    let mut asset_keys: Vec<String> = snapshot.positions.keys().cloned().collect();
    asset_keys.sort();

    let mut updated = false;
    for key in asset_keys {
        let Some(pos) = snapshot.positions.get_mut(&key) else {
            continue;
        };
        if pos.is_alternative
            || pos.quantity <= Decimal::ZERO
            || pos.basis_status() != BasisStatus::Unknown
        {
            continue;
        }
        let Some(asset) = assets_by_id.get(&pos.asset_id) else {
            continue;
        };
        let pos_currency = if pos.currency.is_empty() {
            snap_currency.as_str()
        } else {
            pos.currency.as_str()
        };
        let Some(split_factor) = wealthfolio_portfolio_engine::split_quantity_factor(
            splits,
            &wealthfolio_portfolio_engine::model::AssetId::new(&pos.asset_id),
            snapshot_date,
            as_of,
        ) else {
            continue;
        };
        let Some((unit_cost, total_basis)) =
            default_unit_price_basis_for_position_with_split_factor(
                asset,
                pos_currency,
                pos.quantity,
                split_factor,
            )
        else {
            continue;
        };
        let same_account_currency = pos_currency == snap_currency;
        if same_account_currency {
            let Some(next_account_basis) =
                running_account_basis.and_then(|acc| acc.checked_add(total_basis))
            else {
                continue;
            };
            if next_account_basis.abs() > max_magnitude {
                continue;
            }
            running_account_basis = Some(next_account_basis);
        }
        pos.average_cost = unit_cost;
        pos.total_cost_basis = total_basis;
        if same_account_currency {
            pos.cost_basis_account = Some(total_basis);
        } else {
            pos.cost_basis_account = None;
        }
        if snap_base_currency.as_deref() == Some(pos_currency) {
            pos.cost_basis_base = Some(total_basis);
        } else {
            pos.cost_basis_base = None;
        }
        pos.cost_basis_status = Some(BasisStatus::Complete);
        updated = true;
    }
    if updated {
        if let Some(account_basis) = running_account_basis {
            snapshot.cost_basis = account_basis;
        }
    }
    updated
}
