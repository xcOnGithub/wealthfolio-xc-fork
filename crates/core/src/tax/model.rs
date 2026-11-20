//! Domain models and DTOs for the Family Tax Center.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::accounts::{TaxTreatment, TrackingMode};

pub const TAX_ADVISORY_DISCLAIMER: &str = "Informational estimates only — not tax advice. Realized gains follow each account's configured cost-basis method; wash-sale flags are advisory, do not modify persisted lot basis, and only detect matching asset IDs (not different-ticker substantially identical securities). Verify all figures against official broker tax forms (1099-B, 1099-DIV, 1099-INT) and consult a qualified tax professional.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HoldingTerm {
    ShortTerm,
    LongTerm,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DisposalKind {
    Sale,
    ShortCover,
    OptionExpiry,
    ReturnOfCapitalExcess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WashSaleSeverity {
    IraPermanentDisallowance,
    CrossAccount,
    SameAccount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WashSaleConfidence {
    High,
    NeedsReview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaxExportFormat {
    RealizedGainsCsv,
    IncomeCsv,
    ContributionLimitsCsv,
    FullJson,
}

impl TaxExportFormat {
    pub fn from_code(value: &str) -> crate::Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "realized_gains_csv" => Ok(Self::RealizedGainsCsv),
            "income_csv" => Ok(Self::IncomeCsv),
            "contribution_limits_csv" => Ok(Self::ContributionLimitsCsv),
            "full_json" => Ok(Self::FullJson),
            other => Err(crate::Error::Validation(
                crate::errors::ValidationError::InvalidInput(format!(
                    "Unknown tax export format '{other}'"
                )),
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaxPacketExport {
    pub filename: String,
    pub content_type: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummaryRef {
    pub account_id: String,
    pub account_name: String,
    pub owner_name: Option<String>,
    pub tax_treatment: Option<TaxTreatment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RealizedGainRow {
    pub disposal_id: String,
    pub lot_id: String,
    pub disposal_activity_id: String,
    pub account_id: String,
    pub account_name: String,
    pub owner_name: Option<String>,
    pub tax_treatment: Option<TaxTreatment>,
    pub asset_id: String,
    pub symbol: String,
    pub asset_name: Option<String>,
    pub open_date: Option<NaiveDate>,
    pub disposal_date: NaiveDate,
    pub holding_days: Option<i64>,
    pub term: HoldingTerm,
    pub kind: DisposalKind,
    pub is_short_sale: bool,
    pub quantity: Decimal,
    pub proceeds_base: Decimal,
    pub cost_basis_base: Decimal,
    pub realized_pnl_base: Decimal,
    pub currency: String,
    pub cost_basis_method: String,
    pub wash_sale_risk: Option<WashSaleSeverity>,
    pub estimated_disallowed_loss_base: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RealizedGainsSummary {
    pub short_term_gains_base: Decimal,
    pub short_term_losses_base: Decimal,
    pub net_short_term_base: Decimal,
    pub long_term_gains_base: Decimal,
    pub long_term_losses_base: Decimal,
    pub net_long_term_base: Decimal,
    pub unknown_term_net_base: Decimal,
    pub total_net_realized_base: Decimal,
    pub taxable_disposal_count: usize,
    pub unclassified_realized_pnl_base: Decimal,
    pub unclassified_disposal_count: usize,
    pub tax_advantaged_realized_pnl_base: Decimal,
    pub tax_advantaged_disposal_count: usize,
    pub disposals: Vec<RealizedGainRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WashSaleAlert {
    pub disposal_id: String,
    pub disposal_activity_id: String,
    pub disposal_account_id: String,
    pub disposal_account_name: String,
    pub disposal_owner_name: Option<String>,
    pub disposal_date: NaiveDate,
    pub asset_id: String,
    pub symbol: String,
    pub disposed_quantity: Decimal,
    pub realized_loss_base: Decimal,
    pub replacement_activity_id: String,
    pub replacement_account_id: String,
    pub replacement_account_name: String,
    pub replacement_owner_name: Option<String>,
    pub replacement_tax_treatment: Option<TaxTreatment>,
    pub replacement_date: NaiveDate,
    pub matched_quantity: Decimal,
    pub estimated_disallowed_loss_base: Decimal,
    pub severity: WashSaleSeverity,
    pub confidence: WashSaleConfidence,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IncomeBucketTotals {
    pub gross_dividends_base: Decimal,
    pub gross_interest_base: Decimal,
    pub gross_other_income_base: Decimal,
    pub return_of_capital_base: Decimal,
    pub income_withholding_tax_base: Decimal,
    pub income_fees_base: Decimal,
    pub net_investment_income_base: Decimal,
    pub standalone_tax_paid_base: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountIncomeTaxRow {
    pub account_id: String,
    pub account_name: String,
    pub owner_name: Option<String>,
    pub tax_treatment: Option<TaxTreatment>,
    pub totals: IncomeBucketTotals,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaxableIncomeSummary {
    pub taxable: IncomeBucketTotals,
    pub tax_deferred: IncomeBucketTotals,
    pub tax_free: IncomeBucketTotals,
    pub unclassified: IncomeBucketTotals,
    pub by_account: Vec<AccountIncomeTaxRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TlhCandidate {
    pub lot_id: String,
    pub account_id: String,
    pub account_name: String,
    pub owner_name: Option<String>,
    pub asset_id: String,
    pub symbol: String,
    pub asset_name: Option<String>,
    pub open_date: NaiveDate,
    pub holding_days: i64,
    pub current_term: HoldingTerm,
    pub days_until_long_term: Option<i64>,
    pub effective_quantity: Decimal,
    pub market_value_base: Decimal,
    pub cost_basis_base: Decimal,
    pub unrealized_loss_base: Decimal,
    pub wash_sale_cooldown_until: Option<NaiveDate>,
    pub cooldown_trigger_account_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AssetLocationBucketTotals {
    pub taxable_base: Decimal,
    pub tax_deferred_base: Decimal,
    pub tax_free_base: Decimal,
    pub unclassified_base: Decimal,
    pub total_base: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetLocationCategoryRow {
    pub category_name: String,
    pub totals: AssetLocationBucketTotals,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AssetLocationBreakdown {
    pub totals: AssetLocationBucketTotals,
    pub by_category: Vec<AssetLocationCategoryRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaxContributionLimitRow {
    pub id: String,
    pub group_name: String,
    pub member_name: Option<String>,
    pub limit_category: Option<String>,
    pub contribution_year: i32,
    pub limit_amount: Decimal,
    pub contributed_base: Decimal,
    pub remaining_amount: Decimal,
    pub over_contribution_amount: Decimal,
    pub progress_pct: Decimal,
    pub deadline_date: NaiveDate,
    pub days_remaining: i64,
    pub is_past_deadline: bool,
    pub is_recurring: bool,
    pub is_projected: bool,
    pub use_it_or_lose_it_warning: bool,
    pub account_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberContributionGroup {
    pub member_name: Option<String>,
    pub total_limit_base: Decimal,
    pub total_contributed_base: Decimal,
    pub total_remaining_base: Decimal,
    pub limits: Vec<TaxContributionLimitRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ContributionLimitsSummary {
    pub total_limit_base: Decimal,
    pub total_contributed_base: Decimal,
    pub total_remaining_base: Decimal,
    pub over_limit_count: usize,
    pub unmaterialized_recurring_count: usize,
    pub by_member: Vec<MemberContributionGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaxReadinessAccount {
    pub account_id: String,
    pub account_name: String,
    pub account_type: String,
    pub owner_name: Option<String>,
    pub tax_treatment: Option<TaxTreatment>,
    pub tracking_mode: TrackingMode,
    pub statements_in_year: usize,
    pub needs_review_count: usize,
    pub is_stale: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaxReportWarnings {
    pub unclassified_accounts: Vec<AccountSummaryRef>,
    pub holdings_mode_taxable_accounts: Vec<AccountSummaryRef>,
    pub stale_account_ids: Vec<String>,
    pub invalid_tax_meta_account_ids: Vec<String>,
    pub unconverted_currencies: BTreeSet<String>,
    pub unpriced_tlh_lot_count: usize,
    pub unassigned_account_count: usize,
    pub unassigned_limit_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaxYearReport {
    pub tax_year: i32,
    pub as_of_date: NaiveDate,
    pub base_currency: String,
    pub member_filter: Option<String>,
    pub available_members: Vec<String>,
    pub disclaimer: String,
    pub realized_gains: RealizedGainsSummary,
    pub wash_sales: Vec<WashSaleAlert>,
    pub income: TaxableIncomeSummary,
    pub tlh_candidates: Vec<TlhCandidate>,
    pub asset_location: AssetLocationBreakdown,
    pub contribution_limits: ContributionLimitsSummary,
    pub readiness: Vec<TaxReadinessAccount>,
    pub warnings: TaxReportWarnings,
}
