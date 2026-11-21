//! Core orchestration service for the Family Tax Center.
//!
//! All history-sized SQLite reads run inside `portfolio::coordinator::blocking`
//! so async Axum/Tauri callers can await them directly without blocking Tokio
//! workers. No network calls or financial mutations ever occur here.

use async_trait::async_trait;
use chrono::{Datelike, Duration, NaiveDate, TimeZone, Utc};
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use crate::accounts::{
    account_supports_purpose, normalize_member_name, Account, AccountPurpose,
    AccountRepositoryTrait, AccountTaxSettings, TaxTreatment, TrackingMode,
};
use crate::activities::{
    Activity, ActivityCompiler, ActivityRepositoryTrait, DefaultActivityCompiler,
    ACTIVITY_SUBTYPE_CONVERSION_IN, ACTIVITY_SUBTYPE_CONVERSION_OUT,
    ACTIVITY_SUBTYPE_NOTIONAL_DISTRIBUTION, ACTIVITY_SUBTYPE_OPTION_EXPIRY,
    ACTIVITY_SUBTYPE_POSITION_CLOSE, ACTIVITY_SUBTYPE_RETURN_OF_CAPITAL, ACTIVITY_TYPE_BUY,
    ACTIVITY_TYPE_DIVIDEND, ACTIVITY_TYPE_INTEREST, ACTIVITY_TYPE_TAX, ACTIVITY_TYPE_TRANSFER_IN,
    ACTIVITY_TYPE_TRANSFER_OUT,
};
use crate::assets::{Asset, AssetKind, AssetRepositoryTrait, InstrumentType};
use crate::constants::DISPLAY_DECIMAL_PRECISION;
use crate::errors::{Error, Result, ValidationError};
use crate::fx::currency::normalize_amount;
use crate::fx::FxServiceTrait;
use crate::limits::{
    limit_deadline_date, ContributionLimit, ContributionLimitServiceTrait, MAX_CONTRIBUTION_YEAR,
    MIN_CONTRIBUTION_YEAR,
};
use crate::lots::{LotDisposal, LotRecord, LotRepositoryTrait};
use crate::portfolio::coordinator::{blocking, raw_activity};
use crate::portfolio::holdings::{
    is_expired_option_asset, Holding, HoldingType, HoldingsServiceTrait,
};
use crate::portfolio::projection::{ActivityIssueKind, MarkerScope, ProjectionStoreTrait};
use crate::portfolio::statements::StatementRepository;
use crate::quotes::QuoteStore;
use crate::utils::time_utils::{activity_date_in_tz, parse_user_timezone_or_default, user_today};
use wealthfolio_portfolio_engine::{group_splits, SplitRow};

use super::export::build_tax_packet_export;
use super::holding_period::{classify_holding_period, days_until_long_term};
use super::model::{
    AccountIncomeTaxRow, AccountSummaryRef, AssetLocationBreakdown, AssetLocationBucketTotals,
    AssetLocationCategoryRow, ContributionLimitsSummary, DisposalKind, IncomeBucketTotals,
    MemberContributionGroup, RealizedGainRow, RealizedGainsSummary, TaxContributionLimitRow,
    TaxExportFormat, TaxPacketExport, TaxReadinessAccount, TaxReportWarnings, TaxYearReport,
    TaxableIncomeSummary, TlhCandidate, WashSaleSeverity, TAX_ADVISORY_DISCLAIMER,
};
use super::wash_sale::{
    active_wash_sale_cooldown, detect_wash_sales, AssetSplitEvent, LossDisposalInput,
    ReplacementBuyInput,
};

type AssetLocationAccountGroups = Vec<(Option<TaxTreatment>, Vec<String>)>;

#[async_trait]
pub trait TaxCenterServiceTrait: Send + Sync {
    async fn get_tax_year_report(
        &self,
        tax_year: i32,
        member_filter: Option<String>,
    ) -> Result<TaxYearReport>;

    async fn export_tax_packet(
        &self,
        tax_year: i32,
        format: TaxExportFormat,
        member_filter: Option<String>,
    ) -> Result<TaxPacketExport>;
}

#[derive(Clone)]
pub struct TaxCenterService {
    account_repository: Arc<dyn AccountRepositoryTrait>,
    activity_repository: Arc<dyn ActivityRepositoryTrait>,
    asset_repository: Arc<dyn AssetRepositoryTrait>,
    lot_repository: Arc<dyn LotRepositoryTrait>,
    quote_store: Arc<dyn QuoteStore>,
    fx_service: Arc<dyn FxServiceTrait>,
    holdings_service: Arc<dyn HoldingsServiceTrait>,
    limits_service: Arc<dyn ContributionLimitServiceTrait>,
    statement_repository: Arc<dyn StatementRepository>,
    projection_store: Arc<dyn ProjectionStoreTrait>,
    base_currency: Arc<RwLock<String>>,
    timezone: Arc<RwLock<String>>,
}

impl TaxCenterService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        account_repository: Arc<dyn AccountRepositoryTrait>,
        activity_repository: Arc<dyn ActivityRepositoryTrait>,
        asset_repository: Arc<dyn AssetRepositoryTrait>,
        lot_repository: Arc<dyn LotRepositoryTrait>,
        quote_store: Arc<dyn QuoteStore>,
        fx_service: Arc<dyn FxServiceTrait>,
        holdings_service: Arc<dyn HoldingsServiceTrait>,
        limits_service: Arc<dyn ContributionLimitServiceTrait>,
        statement_repository: Arc<dyn StatementRepository>,
        projection_store: Arc<dyn ProjectionStoreTrait>,
        base_currency: Arc<RwLock<String>>,
        timezone: Arc<RwLock<String>>,
    ) -> Self {
        Self {
            account_repository,
            activity_repository,
            asset_repository,
            lot_repository,
            quote_store,
            fx_service,
            holdings_service,
            limits_service,
            statement_repository,
            projection_store,
            base_currency,
            timezone,
        }
    }

    fn validate_year(tax_year: i32) -> Result<()> {
        if !(MIN_CONTRIBUTION_YEAR..=MAX_CONTRIBUTION_YEAR).contains(&tax_year) {
            return Err(Error::Validation(ValidationError::InvalidInput(format!(
                "Tax year must be between {MIN_CONTRIBUTION_YEAR} and {MAX_CONTRIBUTION_YEAR}"
            ))));
        }
        Ok(())
    }

    fn read_base_currency(&self) -> String {
        self.base_currency
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn read_timezone(&self) -> chrono_tz::Tz {
        parse_user_timezone_or_default(&self.timezone.read().unwrap_or_else(|p| p.into_inner()))
    }

    fn normalize_account_scope(account_ids: Option<&str>) -> String {
        let mut ids: Vec<&str> = account_ids
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .collect();
        ids.sort_unstable();
        ids.join(",")
    }

    fn parse_account_ids_list(account_ids: Option<&str>) -> Vec<String> {
        account_ids
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .collect()
    }

    fn matches_member(candidate: Option<&str>, filter_lower: Option<&str>) -> bool {
        let Some(target) = filter_lower else {
            return true;
        };
        candidate.is_some_and(|name| name.trim().to_ascii_lowercase() == target)
    }

    fn parse_decimal(value: &str) -> Decimal {
        Decimal::from_str(value.trim()).unwrap_or(Decimal::ZERO)
    }

    fn severity_rank(severity: WashSaleSeverity) -> u8 {
        match severity {
            WashSaleSeverity::IraPermanentDisallowance => 3,
            WashSaleSeverity::CrossAccount => 2,
            WashSaleSeverity::SameAccount => 1,
        }
    }

    fn add_bucket_value(
        totals: &mut AssetLocationBucketTotals,
        treatment: Option<TaxTreatment>,
        amount: Decimal,
    ) {
        match treatment {
            Some(TaxTreatment::Taxable) => totals.taxable_base += amount,
            Some(TaxTreatment::TaxDeferred) => totals.tax_deferred_base += amount,
            Some(TaxTreatment::TaxFree) => totals.tax_free_base += amount,
            None => totals.unclassified_base += amount,
        }
        totals.total_base += amount;
    }

    fn fallback_holding_category(holding: &Holding) -> String {
        if holding.holding_type == HoldingType::Cash {
            return "Cash".to_string();
        }
        if let Some(ref instrument) = holding.instrument {
            if let Some(ref inst_type) = instrument.instrument_type {
                return match inst_type {
                    InstrumentType::Equity => "Equity".to_string(),
                    InstrumentType::Bond => "Fixed Income".to_string(),
                    InstrumentType::Crypto => "Cryptocurrency".to_string(),
                    InstrumentType::Option => "Options".to_string(),
                    InstrumentType::Metal => "Precious Metals".to_string(),
                    InstrumentType::Fx => "Cash & FX".to_string(),
                };
            }
        }
        match holding.asset_kind {
            Some(AssetKind::Investment) => "Equity".to_string(),
            Some(AssetKind::Property) => "Real Estate".to_string(),
            Some(AssetKind::PreciousMetal) => "Precious Metals".to_string(),
            Some(AssetKind::PrivateEquity) => "Private Equity".to_string(),
            Some(AssetKind::Fx) => "Cash & FX".to_string(),
            Some(_) => "Alternatives".to_string(),
            None => "Other".to_string(),
        }
    }

    fn accumulate_asset_location_holdings(
        breakdown: &mut AssetLocationBreakdown,
        category_map: &mut BTreeMap<String, AssetLocationBucketTotals>,
        treatment: Option<TaxTreatment>,
        holdings: &[Holding],
    ) {
        for holding in holdings {
            let value = holding.market_value.base;
            if value <= Decimal::ZERO {
                continue;
            }
            Self::add_bucket_value(&mut breakdown.totals, treatment, value);

            let asset_classes = holding
                .instrument
                .as_ref()
                .and_then(|inst| inst.classifications.as_ref())
                .map(|c| c.asset_classes.as_slice())
                .unwrap_or(&[]);

            if asset_classes.is_empty() {
                let category = Self::fallback_holding_category(holding);
                let entry = category_map.entry(category).or_default();
                Self::add_bucket_value(entry, treatment, value);
            } else {
                let total_weight: f64 = asset_classes.iter().map(|c| c.weight.max(0.0)).sum();
                if total_weight <= f64::EPSILON {
                    let category = Self::fallback_holding_category(holding);
                    let entry = category_map.entry(category).or_default();
                    Self::add_bucket_value(entry, treatment, value);
                } else {
                    for assigned in asset_classes {
                        if assigned.weight <= 0.0 {
                            continue;
                        }
                        let share = Decimal::from_f64(assigned.weight / total_weight)
                            .unwrap_or(Decimal::ZERO);
                        let weighted_value = value * share;
                        let name = assigned.top_level_category.name.clone();
                        let entry = category_map.entry(name).or_default();
                        Self::add_bucket_value(entry, treatment, weighted_value);
                    }
                }
            }
        }
    }

    fn compute_blocking_report_part(
        &self,
        tax_year: i32,
        member_filter: Option<String>,
        base_currency: String,
        tz: chrono_tz::Tz,
        today: NaiveDate,
    ) -> Result<(TaxYearReport, AssetLocationAccountGroups)> {
        let member_filter_lower = member_filter.as_deref().map(str::to_ascii_lowercase);

        let all_accounts = self.account_repository.list(None, None, None)?;
        let all_limits = self.limits_service.get_contribution_limits()?;

        let mut warnings = TaxReportWarnings::default();
        let mut tax_settings_by_account: HashMap<String, AccountTaxSettings> =
            HashMap::with_capacity(all_accounts.len());
        let mut members_dedup: BTreeMap<String, String> = BTreeMap::new();

        for account in &all_accounts {
            let settings = match account.tax_settings() {
                Ok(parsed) => parsed,
                Err(_) => {
                    warnings
                        .invalid_tax_meta_account_ids
                        .push(account.id.clone());
                    AccountTaxSettings::default()
                }
            };
            if let Some(ref owner) = settings.owner_name {
                members_dedup
                    .entry(owner.to_ascii_lowercase())
                    .or_insert_with(|| owner.clone());
            }
            tax_settings_by_account.insert(account.id.clone(), settings);
        }

        for limit in &all_limits {
            if let Some(ref member) = limit.member_name {
                let trimmed = member.trim();
                if !trimmed.is_empty() {
                    members_dedup
                        .entry(trimmed.to_ascii_lowercase())
                        .or_insert_with(|| trimmed.to_string());
                }
            }
        }

        let mut available_members: Vec<String> = members_dedup.into_values().collect();
        available_members.sort_by_key(|m| m.to_ascii_lowercase());

        // Accounts explicitly designated as US IRA/Roth IRA (for IRS Rev. Rul. 2008-5 alerts).
        let mut ira_account_ids: HashSet<String> = HashSet::new();
        for (account_id, settings) in &tax_settings_by_account {
            if settings.is_explicit_ira() {
                ira_account_ids.insert(account_id.clone());
            }
        }
        for limit in &all_limits {
            if matches!(limit.limit_category.as_deref(), Some("IRA" | "ROTH_IRA")) {
                for account_id in Self::parse_account_ids_list(limit.account_ids.as_deref()) {
                    let has_non_ira_category = tax_settings_by_account
                        .get(&account_id)
                        .and_then(|s| s.account_category.as_deref())
                        .is_some_and(|cat| !matches!(cat, "IRA" | "ROTH_IRA"));
                    if !has_non_ira_category {
                        ira_account_ids.insert(account_id);
                    }
                }
            }
        }

        let account_by_id: HashMap<&str, &Account> =
            all_accounts.iter().map(|a| (a.id.as_str(), a)).collect();
        let all_account_ids: Vec<String> = all_accounts.iter().map(|a| a.id.clone()).collect();

        // Scope accounts (including archived for historical tax-year disposals/income).
        let mut scoped_all_accounts: Vec<&Account> = Vec::new();
        let mut scoped_active_accounts: Vec<&Account> = Vec::new();
        for account in &all_accounts {
            let settings = tax_settings_by_account
                .get(&account.id)
                .cloned()
                .unwrap_or_default();
            if member_filter_lower.is_some()
                && settings.owner_name.is_none()
                && !account.is_archived
            {
                warnings.unassigned_account_count += 1;
            }
            if Self::matches_member(
                settings.owner_name.as_deref(),
                member_filter_lower.as_deref(),
            ) {
                scoped_all_accounts.push(account);
                if !account.is_archived {
                    scoped_active_accounts.push(account);
                }
            }
        }

        // Unclassified & HOLDINGS-mode taxable warnings (active investment/income accounts).
        for account in &scoped_active_accounts {
            let settings = tax_settings_by_account
                .get(&account.id)
                .cloned()
                .unwrap_or_default();
            if account_supports_purpose(&account.account_type, AccountPurpose::Holdings) {
                if settings.tax_treatment.is_none() {
                    warnings.unclassified_accounts.push(AccountSummaryRef {
                        account_id: account.id.clone(),
                        account_name: account.name.clone(),
                        owner_name: settings.owner_name.clone(),
                        tax_treatment: None,
                    });
                } else if settings.tax_treatment == Some(TaxTreatment::Taxable)
                    && account.tracking_mode == TrackingMode::Holdings
                {
                    warnings
                        .holdings_mode_taxable_accounts
                        .push(AccountSummaryRef {
                            account_id: account.id.clone(),
                            account_name: account.name.clone(),
                            owner_name: settings.owner_name.clone(),
                            tax_treatment: Some(TaxTreatment::Taxable),
                        });
                }
            }
        }

        let scoped_all_ids: Vec<String> =
            scoped_all_accounts.iter().map(|a| a.id.clone()).collect();
        let scoped_active_ids: Vec<String> = scoped_active_accounts
            .iter()
            .map(|a| a.id.clone())
            .collect();

        // Projection staleness & engine-rejected activity IDs.
        let pending_markers = self.projection_store.pending_markers()?;
        let mut stale_set: HashSet<String> = HashSet::new();
        let mut all_stale = false;
        for marker in &pending_markers {
            match &marker.scope {
                MarkerScope::All => {
                    all_stale = true;
                }
                MarkerScope::Account(id) => {
                    stale_set.insert(id.clone());
                }
                _ => {}
            }
        }
        if all_stale {
            for id in &scoped_active_ids {
                stale_set.insert(id.clone());
            }
        }
        let mut stale_scoped: Vec<String> = scoped_active_ids
            .iter()
            .filter(|id| stale_set.contains(*id))
            .cloned()
            .collect();
        stale_scoped.sort();
        warnings.stale_account_ids = stale_scoped;

        let rejected_ids: HashSet<String> = self
            .projection_store
            .activity_issues(&all_account_ids)?
            .into_iter()
            .filter(|issue| issue.kind == ActivityIssueKind::Rejected)
            .map(|issue| issue.activity_id)
            .collect();

        // Date windows.
        let year_start = NaiveDate::from_ymd_opt(tax_year, 1, 1)
            .expect("Jan 1 is valid for every supported tax year");
        let year_end = NaiveDate::from_ymd_opt(tax_year, 12, 31)
            .expect("Dec 31 is valid for every supported tax year");
        let day_before_year = year_start
            .pred_opt()
            .expect("day before tax year is within NaiveDate range");

        // Fetch lot disposals in [year_start, year_end] across ALL household accounts (archived included)
        // so household wash-sale allocation runs before member filtering.
        let disposals: Vec<LotDisposal> = self
            .lot_repository
            .get_lot_disposals_for_accounts_in_date_range_sync(
                &all_account_ids,
                day_before_year,
                year_end,
            )?;

        // Fetch parent lots and disposal activities (archived included) in batch.
        let parent_lot_ids: Vec<String> = disposals
            .iter()
            .map(|d| d.lot_id.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let parent_lots: HashMap<String, LotRecord> = self
            .lot_repository
            .get_lots_by_ids_sync(&parent_lot_ids)?
            .into_iter()
            .map(|lot| (lot.id.clone(), lot))
            .collect();

        let disposal_activity_ids: Vec<String> = disposals
            .iter()
            .map(|d| d.disposal_activity_id.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let disposal_activities: HashMap<String, Activity> = self
            .activity_repository
            .get_activities_by_ids_including_archived(&disposal_activity_ids)?
            .into_iter()
            .map(|act| (act.id.clone(), act))
            .collect();

        // Fetch open taxable TRANSACTION-mode lots for TLH.
        let taxable_tx_account_ids: Vec<String> = scoped_active_accounts
            .iter()
            .filter(|account| {
                tax_settings_by_account
                    .get(&account.id)
                    .and_then(|s| s.tax_treatment)
                    == Some(TaxTreatment::Taxable)
                    && account.tracking_mode == TrackingMode::Transactions
                    && account_supports_purpose(&account.account_type, AccountPurpose::Holdings)
            })
            .map(|a| a.id.clone())
            .collect();
        let open_taxable_lots: Vec<LotRecord> = self
            .lot_repository
            .get_open_lots_for_accounts_sync(&taxable_tx_account_ids)?;

        // Fetch activities across ALL household accounts spanning:
        // - Cross-year 61-day wash-sale window around tax_year (±32 days UTC buffer for timezones)
        // - Live 30-day TLH cooldown window around `today`
        let wash_start_utc = Utc.from_utc_datetime(
            &(year_start - Duration::days(32))
                .and_hms_opt(0, 0, 0)
                .expect("midnight is valid"),
        );
        let wash_end_utc = Utc.from_utc_datetime(
            &(year_end + Duration::days(32))
                .and_hms_opt(23, 59, 59)
                .expect("end of day is valid"),
        );
        let household_year_activities = self
            .activity_repository
            .get_activities_by_account_ids_in_date_range_including_archived(
                &all_account_ids,
                wash_start_utc,
                wash_end_utc,
            )?;

        let recent_buys_for_tlh: Vec<Activity> =
            if today >= year_start - Duration::days(1) && today <= year_end + Duration::days(31) {
                household_year_activities.clone()
            } else {
                let tlh_start = Utc.from_utc_datetime(
                    &(today - Duration::days(32))
                        .and_hms_opt(0, 0, 0)
                        .expect("midnight is valid"),
                );
                let tlh_end = Utc.from_utc_datetime(
                    &(today + Duration::days(2))
                        .and_hms_opt(23, 59, 59)
                        .expect("end of day is valid"),
                );
                self.activity_repository
                    .get_activities_by_account_ids_in_date_range_including_archived(
                        &all_account_ids,
                        tlh_start,
                        tlh_end,
                    )?
            };

        // Gather all asset IDs needed for disposals + open TLH lots.
        let mut needed_asset_ids: HashSet<String> = HashSet::new();
        for d in &disposals {
            needed_asset_ids.insert(d.asset_id.clone());
        }
        for l in &open_taxable_lots {
            needed_asset_ids.insert(l.asset_id.clone());
        }
        let asset_id_vec: Vec<String> = needed_asset_ids.into_iter().collect();
        let asset_by_id: HashMap<String, Asset> = if asset_id_vec.is_empty() {
            HashMap::new()
        } else {
            self.asset_repository
                .list_by_asset_ids(&asset_id_vec)?
                .into_iter()
                .map(|a| (a.id.clone(), a))
                .collect()
        };

        // ── 1. Realized Capital Gains ──────────────────────────────────────────
        let scoped_all_id_set: HashSet<&str> = scoped_all_ids.iter().map(String::as_str).collect();
        let mut realized_rows: Vec<RealizedGainRow> = Vec::new();
        let mut loss_inputs: Vec<LossDisposalInput> = Vec::new();

        for disposal in &disposals {
            let Ok(disposal_date) = NaiveDate::parse_from_str(&disposal.disposal_date, "%Y-%m-%d")
            else {
                continue;
            };
            if disposal_date.year() != tax_year {
                continue;
            }
            let Some(account) = account_by_id.get(disposal.account_id.as_str()) else {
                continue;
            };
            let disposal_activity = disposal_activities.get(&disposal.disposal_activity_id);
            if let Some(act) = disposal_activity {
                let eff_type = act.effective_type();
                let subtype = act.subtype.as_deref();
                if matches!(
                    eff_type,
                    ACTIVITY_TYPE_TRANSFER_OUT | ACTIVITY_TYPE_TRANSFER_IN
                ) || matches!(
                    subtype,
                    Some(ACTIVITY_SUBTYPE_CONVERSION_OUT | ACTIVITY_SUBTYPE_CONVERSION_IN)
                ) {
                    continue;
                }
            }

            let raw_qty = Self::parse_decimal(&disposal.quantity);
            let subtype = disposal_activity.and_then(|a| a.subtype.as_deref());
            let (kind, is_short_sale) = if subtype == Some(ACTIVITY_SUBTYPE_RETURN_OF_CAPITAL) {
                (DisposalKind::ReturnOfCapitalExcess, false)
            } else if subtype == Some(ACTIVITY_SUBTYPE_OPTION_EXPIRY) {
                (DisposalKind::OptionExpiry, raw_qty < Decimal::ZERO)
            } else if raw_qty < Decimal::ZERO {
                (DisposalKind::ShortCover, true)
            } else {
                (DisposalKind::Sale, false)
            };

            let parent_lot = parent_lots.get(&disposal.lot_id);
            let open_date = parent_lot
                .and_then(|lot| NaiveDate::parse_from_str(&lot.open_date, "%Y-%m-%d").ok());

            let local_proceeds = Self::parse_decimal(&disposal.proceeds);
            let local_basis = Self::parse_decimal(&disposal.cost_basis);
            let local_pnl = Self::parse_decimal(&disposal.realized_pnl);

            let (proceeds_base, cost_basis_base, realized_pnl_base) =
                if disposal.currency.eq_ignore_ascii_case(&base_currency) {
                    (Some(local_proceeds), Some(local_basis), Some(local_pnl))
                } else {
                    let stored_proceeds_base = Self::parse_decimal(&disposal.proceeds_base);
                    let stored_basis_base = Self::parse_decimal(&disposal.cost_basis_base);
                    let stored_pnl_base = Self::parse_decimal(&disposal.realized_pnl_base);
                    let stored_fx = Self::parse_decimal(&disposal.fx_rate_to_base);
                    let base_matches = disposal.base_currency.eq_ignore_ascii_case(&base_currency);

                    // The portfolio engine records `0` placeholders for base attribution when
                    // either disposal or acquisition FX is missing (`project.rs`).
                    let stored_disposal_fx_ok = base_matches
                        && stored_fx > Decimal::ZERO
                        && !(local_proceeds != Decimal::ZERO && stored_proceeds_base.is_zero());
                    let stored_pnl_consistent = (stored_proceeds_base - stored_basis_base)
                        .round_dp(6)
                        == stored_pnl_base.round_dp(6);
                    let parent_fx_missing = parent_lot.is_some_and(|lot| {
                        !lot.currency.eq_ignore_ascii_case(&base_currency)
                            && Self::parse_decimal(&lot.fx_rate_to_base) <= Decimal::ZERO
                            && Self::parse_decimal(&lot.original_cost_basis) != Decimal::ZERO
                            && Self::parse_decimal(&lot.original_cost_basis_base).is_zero()
                    });
                    let stored_basis_fx_ok = base_matches
                        && (local_basis.is_zero()
                            || (!stored_basis_base.is_zero()
                                && !parent_fx_missing
                                && (stored_pnl_consistent || !stored_disposal_fx_ok)));

                    let resolved_proceeds = if local_proceeds.is_zero() {
                        Some(Decimal::ZERO)
                    } else if stored_disposal_fx_ok {
                        Some(stored_proceeds_base)
                    } else {
                        self.fx_service
                            .convert_currency_for_date(
                                local_proceeds,
                                &disposal.currency,
                                &base_currency,
                                disposal_date,
                            )
                            .ok()
                    };

                    let resolved_basis = if local_basis.is_zero() {
                        Some(Decimal::ZERO)
                    } else if stored_basis_fx_ok {
                        Some(stored_basis_base)
                    } else if let Some(acq_date) = open_date {
                        self.fx_service
                            .convert_currency_for_date(
                                local_basis,
                                &disposal.currency,
                                &base_currency,
                                acq_date,
                            )
                            .ok()
                    } else {
                        None
                    };

                    let resolved_pnl = match (resolved_proceeds, resolved_basis) {
                        (Some(p), Some(b)) => Some(p - b),
                        _ => None,
                    };
                    (resolved_proceeds, resolved_basis, resolved_pnl)
                };

            let fx_missing = realized_pnl_base.is_none();
            let in_scope = scoped_all_id_set.contains(account.id.as_str());
            if fx_missing && in_scope {
                warnings
                    .unconverted_currencies
                    .insert(disposal.currency.clone());
                warnings.unconverted_disposal_count += 1;
            }

            let (term, holding_days) =
                classify_holding_period(open_date, disposal_date, is_short_sale);

            let settings = tax_settings_by_account
                .get(&account.id)
                .cloned()
                .unwrap_or_default();
            let asset = asset_by_id.get(&disposal.asset_id);
            let symbol = asset
                .and_then(|a| {
                    a.instrument_symbol
                        .clone()
                        .or_else(|| a.display_code.clone())
                })
                .unwrap_or_else(|| disposal.asset_id.clone());
            let asset_name = asset.and_then(|a| a.name.clone());

            let display_qty = raw_qty.abs();
            let rounded_proceeds = proceeds_base.map(|v| v.round_dp(DISPLAY_DECIMAL_PRECISION));
            let rounded_basis = cost_basis_base.map(|v| v.round_dp(DISPLAY_DECIMAL_PRECISION));
            let rounded_pnl = realized_pnl_base.map(|v| v.round_dp(DISPLAY_DECIMAL_PRECISION));

            // Feed ALL household taxable & unclassified long loss disposals (including missing-FX
            // local-currency losses) into wash-sale matching BEFORE member filtering.
            let is_loss = rounded_pnl.map_or(local_pnl < Decimal::ZERO, |pnl| pnl < Decimal::ZERO);
            if is_loss
                && !is_short_sale
                && matches!(kind, DisposalKind::Sale | DisposalKind::OptionExpiry)
                && matches!(settings.tax_treatment, Some(TaxTreatment::Taxable) | None)
            {
                loss_inputs.push(LossDisposalInput {
                    disposal_id: disposal.id.clone(),
                    lot_id: disposal.lot_id.clone(),
                    open_activity_id: parent_lot.and_then(|l| l.open_activity_id.clone()),
                    open_date,
                    cost_basis_method: disposal.cost_basis_method.clone(),
                    disposal_activity_id: disposal.disposal_activity_id.clone(),
                    account_id: account.id.clone(),
                    account_name: account.name.clone(),
                    owner_name: settings.owner_name.clone(),
                    asset_id: disposal.asset_id.clone(),
                    symbol: symbol.clone(),
                    disposal_date,
                    disposed_quantity: display_qty,
                    realized_loss_base: rounded_pnl.map(|pnl| pnl.abs()),
                });
            }

            if in_scope {
                realized_rows.push(RealizedGainRow {
                    disposal_id: disposal.id.clone(),
                    lot_id: disposal.lot_id.clone(),
                    disposal_activity_id: disposal.disposal_activity_id.clone(),
                    account_id: account.id.clone(),
                    account_name: account.name.clone(),
                    owner_name: settings.owner_name,
                    tax_treatment: settings.tax_treatment,
                    asset_id: disposal.asset_id.clone(),
                    symbol,
                    asset_name,
                    open_date,
                    disposal_date,
                    holding_days,
                    term,
                    kind,
                    is_short_sale,
                    quantity: display_qty,
                    proceeds_base: rounded_proceeds,
                    cost_basis_base: rounded_basis,
                    realized_pnl_base: rounded_pnl,
                    fx_missing,
                    currency: disposal.currency.clone(),
                    cost_basis_method: disposal.cost_basis_method.clone(),
                    wash_sale_risk: None,
                    estimated_disallowed_loss_base: Some(Decimal::ZERO),
                });
            }
        }

        // ── 2. Household Wash-Sale Scanner ─────────────────────────────────────
        // Expand composite reinvestment activities (DRIP, DIVIDEND_IN_KIND, STAKING_REWARD)
        // using DefaultActivityCompiler so reinvested acquisitions enter wash-sale and TLH checks.
        let compiler = DefaultActivityCompiler::new();
        let build_replacement_buys = |acts: &[Activity]| -> Vec<ReplacementBuyInput> {
            let mut buys = Vec::new();
            for act in acts {
                if !act.is_posted() || rejected_ids.contains(&act.id) {
                    continue;
                }
                let Ok(compiled) = compiler.compile(act) else {
                    continue;
                };
                for leg in compiled {
                    if leg.effective_type() != ACTIVITY_TYPE_BUY
                        || leg.subtype.as_deref() == Some(ACTIVITY_SUBTYPE_POSITION_CLOSE)
                    {
                        continue;
                    }
                    let Some(asset_id) = leg.asset_id.clone() else {
                        continue;
                    };
                    let Some(qty) = leg.quantity else {
                        continue;
                    };
                    if qty <= Decimal::ZERO {
                        continue;
                    }
                    let Some(acc) = account_by_id.get(leg.account_id.as_str()) else {
                        continue;
                    };
                    let settings = tax_settings_by_account
                        .get(&acc.id)
                        .cloned()
                        .unwrap_or_default();
                    buys.push(ReplacementBuyInput {
                        // Preserve parent stored activity ID so `lot.open_activity_id` matches.
                        activity_id: act.id.clone(),
                        account_id: acc.id.clone(),
                        account_name: acc.name.clone(),
                        owner_name: settings.owner_name,
                        tax_treatment: settings.tax_treatment,
                        is_ira: ira_account_ids.contains(&acc.id),
                        asset_id,
                        buy_date: activity_date_in_tz(leg.activity_date, tz),
                        quantity: qty,
                    });
                }
            }
            buys
        };

        let replacement_buys = build_replacement_buys(&household_year_activities);
        // Reuse the portfolio engine's split clustering so duplicate split reports across
        // multiple brokerage accounts are applied only once per asset/event.
        let split_rows = household_year_activities
            .iter()
            .filter(|act| act.is_posted() && !rejected_ids.contains(&act.id))
            .filter_map(|act| SplitRow::from_raw(&raw_activity(act), &tz));
        let split_events: Vec<AssetSplitEvent> = group_splits(split_rows)
            .into_iter()
            .map(|event| AssetSplitEvent {
                asset_id: event.asset.as_str().to_string(),
                split_date: event.split_date,
                ratio: event.ratio,
            })
            .collect();

        let wash_sales: Vec<_> = detect_wash_sales(&loss_inputs, &replacement_buys, &split_events)
            .into_iter()
            .filter(|alert| scoped_all_id_set.contains(alert.disposal_account_id.as_str()))
            .collect();

        // Attach wash-sale risk & disallowed loss onto corresponding RealizedGainRow entries.
        let mut wash_by_disposal: HashMap<&str, (WashSaleSeverity, Option<Decimal>)> =
            HashMap::new();
        for alert in &wash_sales {
            wash_by_disposal
                .entry(alert.disposal_id.as_str())
                .and_modify(|(sev, total)| {
                    if Self::severity_rank(alert.severity) > Self::severity_rank(*sev) {
                        *sev = alert.severity;
                    }
                    *total = match (*total, alert.estimated_disallowed_loss_base) {
                        (Some(acc), Some(add)) => Some(acc + add),
                        _ => None,
                    };
                })
                .or_insert((alert.severity, alert.estimated_disallowed_loss_base));
        }

        let mut realized_summary = RealizedGainsSummary::default();
        for row in &mut realized_rows {
            if let Some((sev, disallowed)) = wash_by_disposal.get(row.disposal_id.as_str()) {
                row.wash_sale_risk = Some(*sev);
                row.estimated_disallowed_loss_base = match (*disallowed, row.realized_pnl_base) {
                    (Some(amt), Some(pnl)) => {
                        Some(amt.min(pnl.abs()).round_dp(DISPLAY_DECIMAL_PRECISION))
                    }
                    _ => None,
                };
            }

            // Exclude disposals with missing base-currency FX from numeric totals so unknown
            // gains/losses never masquerade as break-even `$0.00` sales.
            let Some(pnl) = row.realized_pnl_base else {
                continue;
            };

            match row.tax_treatment {
                Some(TaxTreatment::Taxable) => {
                    realized_summary.taxable_disposal_count += 1;
                    match row.term {
                        super::model::HoldingTerm::ShortTerm => {
                            if pnl >= Decimal::ZERO {
                                realized_summary.short_term_gains_base += pnl;
                            } else {
                                realized_summary.short_term_losses_base += pnl;
                            }
                            realized_summary.net_short_term_base += pnl;
                        }
                        super::model::HoldingTerm::LongTerm => {
                            if pnl >= Decimal::ZERO {
                                realized_summary.long_term_gains_base += pnl;
                            } else {
                                realized_summary.long_term_losses_base += pnl;
                            }
                            realized_summary.net_long_term_base += pnl;
                        }
                        super::model::HoldingTerm::Unknown => {
                            realized_summary.unknown_term_net_base += pnl;
                        }
                    }
                    realized_summary.total_net_realized_base += pnl;
                }
                None => {
                    realized_summary.unclassified_disposal_count += 1;
                    realized_summary.unclassified_realized_pnl_base += pnl;
                }
                Some(TaxTreatment::TaxDeferred | TaxTreatment::TaxFree) => {
                    realized_summary.tax_advantaged_disposal_count += 1;
                    realized_summary.tax_advantaged_realized_pnl_base += pnl;
                }
            }
        }

        realized_rows.sort_by(|a, b| {
            b.disposal_date
                .cmp(&a.disposal_date)
                .then_with(|| a.symbol.cmp(&b.symbol))
                .then_with(|| a.disposal_id.cmp(&b.disposal_id))
        });
        realized_summary.disposals = realized_rows;

        // ── 3. Taxable Investment Income & Withholding ─────────────────────────
        let mut income_by_account: HashMap<String, IncomeBucketTotals> = HashMap::new();
        let mut needs_review_by_account: HashMap<String, usize> = HashMap::new();

        for act in &household_year_activities {
            if !scoped_all_id_set.contains(act.account_id.as_str()) {
                continue;
            }
            let local_date = activity_date_in_tz(act.activity_date, tz);
            if local_date.year() != tax_year {
                continue;
            }
            if act.needs_review {
                *needs_review_by_account
                    .entry(act.account_id.clone())
                    .or_insert(0) += 1;
            }
            if !act.is_posted() || rejected_ids.contains(&act.id) {
                continue;
            }
            let Some(account) = account_by_id.get(act.account_id.as_str()) else {
                continue;
            };
            if !account_supports_purpose(&account.account_type, AccountPurpose::Income) {
                continue;
            }

            let eff_type = act.effective_type();
            let subtype = act.subtype.as_deref();
            let amount_abs = act.amount.unwrap_or(Decimal::ZERO).abs();
            let fee_abs = act.fee.unwrap_or(Decimal::ZERO).abs();
            let tax_abs = act.tax.unwrap_or(Decimal::ZERO).abs();

            let convert = |val: Decimal, warnings: &mut TaxReportWarnings| -> Option<Decimal> {
                if val.is_zero() {
                    return Some(Decimal::ZERO);
                }
                match self.fx_service.convert_currency_for_date(
                    val,
                    &act.currency,
                    &base_currency,
                    local_date,
                ) {
                    Ok(converted) => Some(converted),
                    Err(_) => {
                        warnings.unconverted_currencies.insert(act.currency.clone());
                        None
                    }
                }
            };

            if eff_type == ACTIVITY_TYPE_TAX {
                let tax_paid = if !amount_abs.is_zero() {
                    amount_abs
                } else {
                    tax_abs
                };
                if let Some(conv) = convert(tax_paid, &mut warnings) {
                    let entry = income_by_account.entry(act.account_id.clone()).or_default();
                    entry.standalone_tax_paid_base += conv;
                }
                continue;
            }

            let is_dividend = eff_type == ACTIVITY_TYPE_DIVIDEND;
            let is_interest = eff_type == ACTIVITY_TYPE_INTEREST;
            let is_other_taxable = eff_type == "OTHER_INCOME"
                || subtype == Some(ACTIVITY_SUBTYPE_NOTIONAL_DISTRIBUTION);

            if !is_dividend && !is_interest && !is_other_taxable {
                continue;
            }

            if is_dividend && subtype == Some(ACTIVITY_SUBTYPE_RETURN_OF_CAPITAL) {
                if let Some(conv) = convert(amount_abs, &mut warnings) {
                    let entry = income_by_account.entry(act.account_id.clone()).or_default();
                    entry.return_of_capital_base += conv;
                }
                continue;
            }

            // Wealthfolio stores final (net) cash in `activities.amount`; gross = amount + fee + tax.
            let gross_local = amount_abs + fee_abs + tax_abs;
            let (Some(gross_base), Some(tax_base), Some(fee_base), Some(net_base)) = (
                convert(gross_local, &mut warnings),
                convert(tax_abs, &mut warnings),
                convert(fee_abs, &mut warnings),
                convert(amount_abs, &mut warnings),
            ) else {
                continue;
            };

            let entry = income_by_account.entry(act.account_id.clone()).or_default();
            if is_dividend {
                entry.gross_dividends_base += gross_base;
            } else if is_interest {
                entry.gross_interest_base += gross_base;
            } else {
                entry.gross_other_income_base += gross_base;
            }
            entry.income_withholding_tax_base += tax_base;
            entry.income_fees_base += fee_base;
            entry.net_investment_income_base += net_base;
        }

        let mut income_summary = TaxableIncomeSummary::default();
        let add_income_totals = |dst: &mut IncomeBucketTotals, src: &IncomeBucketTotals| {
            dst.gross_dividends_base += src.gross_dividends_base;
            dst.gross_interest_base += src.gross_interest_base;
            dst.gross_other_income_base += src.gross_other_income_base;
            dst.return_of_capital_base += src.return_of_capital_base;
            dst.income_withholding_tax_base += src.income_withholding_tax_base;
            dst.income_fees_base += src.income_fees_base;
            dst.net_investment_income_base += src.net_investment_income_base;
            dst.standalone_tax_paid_base += src.standalone_tax_paid_base;
        };
        let round_income = |t: &mut IncomeBucketTotals| {
            t.gross_dividends_base = t.gross_dividends_base.round_dp(DISPLAY_DECIMAL_PRECISION);
            t.gross_interest_base = t.gross_interest_base.round_dp(DISPLAY_DECIMAL_PRECISION);
            t.gross_other_income_base = t
                .gross_other_income_base
                .round_dp(DISPLAY_DECIMAL_PRECISION);
            t.return_of_capital_base = t.return_of_capital_base.round_dp(DISPLAY_DECIMAL_PRECISION);
            t.income_withholding_tax_base = t
                .income_withholding_tax_base
                .round_dp(DISPLAY_DECIMAL_PRECISION);
            t.income_fees_base = t.income_fees_base.round_dp(DISPLAY_DECIMAL_PRECISION);
            t.net_investment_income_base = t
                .net_investment_income_base
                .round_dp(DISPLAY_DECIMAL_PRECISION);
            t.standalone_tax_paid_base = t
                .standalone_tax_paid_base
                .round_dp(DISPLAY_DECIMAL_PRECISION);
        };

        for (account_id, mut totals) in income_by_account {
            let Some(account) = account_by_id.get(account_id.as_str()) else {
                continue;
            };
            let settings = tax_settings_by_account
                .get(&account_id)
                .cloned()
                .unwrap_or_default();
            round_income(&mut totals);
            match settings.tax_treatment {
                Some(TaxTreatment::Taxable) => {
                    add_income_totals(&mut income_summary.taxable, &totals)
                }
                Some(TaxTreatment::TaxDeferred) => {
                    add_income_totals(&mut income_summary.tax_deferred, &totals)
                }
                Some(TaxTreatment::TaxFree) => {
                    add_income_totals(&mut income_summary.tax_free, &totals)
                }
                None => add_income_totals(&mut income_summary.unclassified, &totals),
            }
            income_summary.by_account.push(AccountIncomeTaxRow {
                account_id,
                account_name: account.name.clone(),
                owner_name: settings.owner_name,
                tax_treatment: settings.tax_treatment,
                totals,
            });
        }
        round_income(&mut income_summary.taxable);
        round_income(&mut income_summary.tax_deferred);
        round_income(&mut income_summary.tax_free);
        round_income(&mut income_summary.unclassified);
        income_summary
            .by_account
            .sort_by(|a, b| a.account_name.cmp(&b.account_name));

        // ── 4. Tax-Loss Harvesting (TLH) Opportunities ─────────────────────────
        let open_asset_ids: Vec<String> = open_taxable_lots
            .iter()
            .map(|l| l.asset_id.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let latest_quotes = if open_asset_ids.is_empty() {
            HashMap::new()
        } else {
            self.quote_store.get_latest_quotes(&open_asset_ids)?
        };
        let tlh_recent_buys = build_replacement_buys(&recent_buys_for_tlh);

        let mut tlh_candidates: Vec<TlhCandidate> = Vec::new();
        for lot in &open_taxable_lots {
            if lot.is_closed {
                continue;
            }
            let remaining_qty = Self::parse_decimal(&lot.remaining_quantity);
            if remaining_qty <= Decimal::ZERO {
                continue;
            }
            let Ok(open_date) = NaiveDate::parse_from_str(&lot.open_date, "%Y-%m-%d") else {
                continue;
            };
            let Some(account) = account_by_id.get(lot.account_id.as_str()) else {
                continue;
            };
            let Some(asset) = asset_by_id.get(&lot.asset_id) else {
                warnings.unpriced_tlh_lot_count += 1;
                continue;
            };

            let split_ratio_raw = Self::parse_decimal(&lot.split_ratio);
            let split_ratio = if split_ratio_raw > Decimal::ZERO {
                split_ratio_raw
            } else {
                Decimal::ONE
            };
            let effective_qty = remaining_qty * split_ratio;
            let multiplier = asset.contract_multiplier();

            let cost_basis_base = if lot.base_currency == base_currency {
                Self::parse_decimal(&lot.remaining_cost_basis_base)
            } else {
                let local_basis = Self::parse_decimal(&lot.remaining_cost_basis);
                match self.fx_service.convert_currency_for_date(
                    local_basis,
                    &lot.currency,
                    &base_currency,
                    open_date,
                ) {
                    Ok(converted) => converted,
                    Err(_) => {
                        warnings.unpriced_tlh_lot_count += 1;
                        warnings.unconverted_currencies.insert(lot.currency.clone());
                        continue;
                    }
                }
            };

            let market_value_base = if is_expired_option_asset(asset, today) {
                Decimal::ZERO
            } else {
                let Some(quote) = latest_quotes.get(&lot.asset_id) else {
                    warnings.unpriced_tlh_lot_count += 1;
                    continue;
                };
                let (norm_price, norm_ccy) = normalize_amount(quote.close, &quote.currency);
                let fx_rate = if norm_ccy == base_currency {
                    Decimal::ONE
                } else {
                    match self
                        .fx_service
                        .get_latest_exchange_rate(norm_ccy, &base_currency)
                    {
                        Ok(rate) => rate,
                        Err(_) => {
                            warnings.unpriced_tlh_lot_count += 1;
                            warnings.unconverted_currencies.insert(norm_ccy.to_string());
                            continue;
                        }
                    }
                };
                norm_price * effective_qty * multiplier * fx_rate
            };

            let unrealized_pnl =
                (market_value_base - cost_basis_base).round_dp(DISPLAY_DECIMAL_PRECISION);
            if unrealized_pnl >= Decimal::ZERO {
                continue;
            }

            let (current_term, holding_days_opt) =
                classify_holding_period(Some(open_date), today, false);
            let cooldown = active_wash_sale_cooldown(&lot.asset_id, today, &tlh_recent_buys);
            let settings = tax_settings_by_account
                .get(&account.id)
                .cloned()
                .unwrap_or_default();
            let symbol = asset
                .instrument_symbol
                .clone()
                .or_else(|| asset.display_code.clone())
                .unwrap_or_else(|| lot.asset_id.clone());

            tlh_candidates.push(TlhCandidate {
                lot_id: lot.id.clone(),
                account_id: account.id.clone(),
                account_name: account.name.clone(),
                owner_name: settings.owner_name,
                asset_id: lot.asset_id.clone(),
                symbol,
                asset_name: asset.name.clone(),
                open_date,
                holding_days: holding_days_opt.unwrap_or(0),
                current_term,
                days_until_long_term: days_until_long_term(open_date, today),
                effective_quantity: effective_qty,
                market_value_base: market_value_base.round_dp(DISPLAY_DECIMAL_PRECISION),
                cost_basis_base: cost_basis_base.round_dp(DISPLAY_DECIMAL_PRECISION),
                unrealized_loss_base: unrealized_pnl.abs(),
                wash_sale_cooldown_until: cooldown.as_ref().map(|(d, _)| *d),
                cooldown_trigger_account_name: cooldown.map(|(_, acc)| acc),
            });
        }
        tlh_candidates.sort_by(|a, b| {
            b.unrealized_loss_base
                .cmp(&a.unrealized_loss_base)
                .then_with(|| a.symbol.cmp(&b.symbol))
                .then_with(|| a.lot_id.cmp(&b.lot_id))
        });

        // ── 5. Per-Member Contribution Limits ──────────────────────────────────
        let mut limits_summary = ContributionLimitsSummary::default();
        let mut limits_by_member: BTreeMap<Option<String>, Vec<TaxContributionLimitRow>> =
            BTreeMap::new();

        // Group all limits by recurring composite key to detect unmaterialized recurring limits.
        type RecGroupKey = (String, String, String);
        let mut recurring_groups: HashMap<RecGroupKey, Vec<&ContributionLimit>> = HashMap::new();
        for limit in &all_limits {
            if member_filter_lower.is_some()
                && limit
                    .member_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .is_none()
                && limit.contribution_year == tax_year
            {
                warnings.unassigned_limit_count += 1;
            }
            if !Self::matches_member(limit.member_name.as_deref(), member_filter_lower.as_deref()) {
                continue;
            }
            let key = (
                limit.group_name.clone(),
                Self::normalize_account_scope(limit.account_ids.as_deref()),
                limit
                    .member_name
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase(),
            );
            recurring_groups.entry(key).or_default().push(limit);
        }

        let push_limit_row = |limit: &ContributionLimit,
                              contributed: Decimal,
                              is_projected: bool,
                              deadline_date: NaiveDate,
                              limits_summary: &mut ContributionLimitsSummary,
                              limits_by_member: &mut BTreeMap<
            Option<String>,
            Vec<TaxContributionLimitRow>,
        >| {
            let limit_amount = Decimal::from_f64(limit.limit_amount)
                .unwrap_or(Decimal::ZERO)
                .round_dp(DISPLAY_DECIMAL_PRECISION);
            let contributed_rounded = contributed.round_dp(DISPLAY_DECIMAL_PRECISION);
            let remaining = (limit_amount - contributed_rounded).max(Decimal::ZERO);
            let over = (contributed_rounded - limit_amount).max(Decimal::ZERO);
            let progress_pct = if limit_amount > Decimal::ZERO {
                ((contributed_rounded / limit_amount) * Decimal::from(100))
                    .round_dp(DISPLAY_DECIMAL_PRECISION)
            } else {
                Decimal::ZERO
            };
            let days_remaining = (deadline_date - today).num_days();
            let is_past_deadline = days_remaining < 0;
            let use_it_or_lose_it_warning = limit.limit_category.as_deref() == Some("FSA")
                && remaining > Decimal::ZERO
                && (0..=90).contains(&days_remaining);

            limits_summary.total_limit_base += limit_amount;
            limits_summary.total_contributed_base += contributed_rounded;
            limits_summary.total_remaining_base += remaining;
            if over > Decimal::ZERO {
                limits_summary.over_limit_count += 1;
            }
            if is_projected {
                limits_summary.unmaterialized_recurring_count += 1;
            }

            let member_key = limit
                .member_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);

            limits_by_member
                .entry(member_key.clone())
                .or_default()
                .push(TaxContributionLimitRow {
                    id: if is_projected {
                        format!("projected:{}:{tax_year}", limit.id)
                    } else {
                        limit.id.clone()
                    },
                    group_name: limit.group_name.clone(),
                    member_name: member_key,
                    limit_category: limit.limit_category.clone(),
                    contribution_year: tax_year,
                    limit_amount,
                    contributed_base: contributed_rounded,
                    remaining_amount: remaining,
                    over_contribution_amount: over,
                    progress_pct,
                    deadline_date,
                    days_remaining,
                    is_past_deadline,
                    is_recurring: limit.is_recurring,
                    is_projected,
                    use_it_or_lose_it_warning,
                    account_ids: Self::parse_account_ids_list(limit.account_ids.as_deref()),
                });
        };

        for group in recurring_groups.values() {
            let mut materialized = false;
            for limit in group {
                if limit.contribution_year == tax_year {
                    materialized = true;
                    let calc = self
                        .limits_service
                        .calculate_deposits_for_contribution_limit(&limit.id, &base_currency)?;
                    let deadline = limit_deadline_date(limit, tz);
                    push_limit_row(
                        limit,
                        calc.total,
                        false,
                        deadline,
                        &mut limits_summary,
                        &mut limits_by_member,
                    );
                }
            }
            if !materialized && tax_year >= today.year() {
                if let Some(template) = group
                    .iter()
                    .filter(|l| l.contribution_year < tax_year)
                    .max_by(|a, b| {
                        a.contribution_year
                            .cmp(&b.contribution_year)
                            .then_with(|| a.id.cmp(&b.id))
                    })
                    .filter(|l| l.is_recurring)
                {
                    let base_deadline = limit_deadline_date(template, tz);
                    let year_delta = tax_year - template.contribution_year;
                    let target_year = base_deadline.year() + year_delta;
                    let projected_deadline = NaiveDate::from_ymd_opt(
                        target_year,
                        base_deadline.month(),
                        base_deadline.day(),
                    )
                    .or_else(|| NaiveDate::from_ymd_opt(target_year, 2, 28))
                    .unwrap_or(year_end);
                    push_limit_row(
                        template,
                        Decimal::ZERO,
                        true,
                        projected_deadline,
                        &mut limits_summary,
                        &mut limits_by_member,
                    );
                }
            }
        }

        for (member_name, mut rows) in limits_by_member {
            rows.sort_by(|a, b| {
                a.group_name
                    .cmp(&b.group_name)
                    .then_with(|| a.id.cmp(&b.id))
            });
            let total_limit_base = rows.iter().map(|r| r.limit_amount).sum();
            let total_contributed_base = rows.iter().map(|r| r.contributed_base).sum();
            let total_remaining_base = rows.iter().map(|r| r.remaining_amount).sum();
            limits_summary.by_member.push(MemberContributionGroup {
                member_name,
                total_limit_base,
                total_contributed_base,
                total_remaining_base,
                limits: rows,
            });
        }
        // Named members alphabetically first, then Household/Unassigned (`None`) last.
        limits_summary
            .by_member
            .sort_by(|a, b| match (&a.member_name, &b.member_name) {
                (Some(x), Some(y)) => x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase()),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            });

        // ── 6. Statement & Data Readiness Checklist ────────────────────────────
        let active_securities_ids: Vec<String> = scoped_active_accounts
            .iter()
            .filter(|a| a.account_type == crate::accounts::account_types::SECURITIES)
            .map(|a| a.id.clone())
            .collect();
        let statement_counts = self
            .statement_repository
            .count_by_year(&active_securities_ids, tax_year)?;

        let mut readiness: Vec<TaxReadinessAccount> = scoped_active_accounts
            .iter()
            .filter(|a| account_supports_purpose(&a.account_type, AccountPurpose::Holdings))
            .map(|account| {
                let settings = tax_settings_by_account
                    .get(&account.id)
                    .cloned()
                    .unwrap_or_default();
                TaxReadinessAccount {
                    account_id: account.id.clone(),
                    account_name: account.name.clone(),
                    account_type: account.account_type.clone(),
                    owner_name: settings.owner_name,
                    tax_treatment: settings.tax_treatment,
                    tracking_mode: account.tracking_mode,
                    statements_in_year: statement_counts.get(&account.id).copied().unwrap_or(0),
                    needs_review_count: needs_review_by_account
                        .get(&account.id)
                        .copied()
                        .unwrap_or(0),
                    is_stale: stale_set.contains(&account.id),
                }
            })
            .collect();
        readiness.sort_by(|a, b| a.account_name.cmp(&b.account_name));

        // Group active holdings-eligible accounts by TaxTreatment for Asset Location async step.
        let mut bucket_accounts: HashMap<Option<TaxTreatment>, Vec<String>> = HashMap::new();
        for account in &scoped_active_accounts {
            if !account_supports_purpose(&account.account_type, AccountPurpose::Holdings) {
                continue;
            }
            let treatment = tax_settings_by_account
                .get(&account.id)
                .and_then(|s| s.tax_treatment);
            bucket_accounts
                .entry(treatment)
                .or_default()
                .push(account.id.clone());
        }
        let asset_location_groups: Vec<(Option<TaxTreatment>, Vec<String>)> =
            bucket_accounts.into_iter().collect();

        let report = TaxYearReport {
            tax_year,
            as_of_date: today,
            base_currency,
            member_filter,
            available_members,
            disclaimer: TAX_ADVISORY_DISCLAIMER.to_string(),
            realized_gains: realized_summary,
            wash_sales,
            income: income_summary,
            tlh_candidates,
            asset_location: AssetLocationBreakdown::default(),
            contribution_limits: limits_summary,
            readiness,
            warnings,
        };

        Ok((report, asset_location_groups))
    }
}

#[async_trait]
impl TaxCenterServiceTrait for TaxCenterService {
    async fn get_tax_year_report(
        &self,
        tax_year: i32,
        member_filter: Option<String>,
    ) -> Result<TaxYearReport> {
        Self::validate_year(tax_year)?;
        let normalized_member = normalize_member_name(member_filter.as_deref())?;
        let base_currency = self.read_base_currency();
        let tz = self.read_timezone();
        let today = user_today(tz);

        let this = self.clone();
        let base_for_blocking = base_currency.clone();
        let (mut report, asset_location_groups) = blocking(move || {
            this.compute_blocking_report_part(
                tax_year,
                normalized_member,
                base_for_blocking,
                tz,
                today,
            )
        })
        .await?;

        // Populate Asset Location using authoritative HoldingsService + `asset_classes` taxonomy.
        let mut breakdown = AssetLocationBreakdown::default();
        let mut category_map: BTreeMap<String, AssetLocationBucketTotals> = BTreeMap::new();
        for (treatment, account_ids) in asset_location_groups {
            if account_ids.is_empty() {
                continue;
            }
            let holdings = self
                .holdings_service
                .get_holdings_for_accounts(&account_ids, &base_currency, "")
                .await?;
            Self::accumulate_asset_location_holdings(
                &mut breakdown,
                &mut category_map,
                treatment,
                &holdings,
            );
        }

        let round_bucket = |b: &mut AssetLocationBucketTotals| {
            b.taxable_base = b.taxable_base.round_dp(DISPLAY_DECIMAL_PRECISION);
            b.tax_deferred_base = b.tax_deferred_base.round_dp(DISPLAY_DECIMAL_PRECISION);
            b.tax_free_base = b.tax_free_base.round_dp(DISPLAY_DECIMAL_PRECISION);
            b.unclassified_base = b.unclassified_base.round_dp(DISPLAY_DECIMAL_PRECISION);
            b.total_base = b.total_base.round_dp(DISPLAY_DECIMAL_PRECISION);
        };
        round_bucket(&mut breakdown.totals);
        let mut by_category: Vec<AssetLocationCategoryRow> = category_map
            .into_iter()
            .map(|(category_name, mut totals)| {
                round_bucket(&mut totals);
                AssetLocationCategoryRow {
                    category_name,
                    totals,
                }
            })
            .collect();
        by_category.sort_by(|a, b| {
            b.totals
                .total_base
                .cmp(&a.totals.total_base)
                .then_with(|| a.category_name.cmp(&b.category_name))
        });
        breakdown.by_category = by_category;
        report.asset_location = breakdown;

        Ok(report)
    }

    async fn export_tax_packet(
        &self,
        tax_year: i32,
        format: TaxExportFormat,
        member_filter: Option<String>,
    ) -> Result<TaxPacketExport> {
        let report = self.get_tax_year_report(tax_year, member_filter).await?;
        build_tax_packet_export(&report, format)
    }
}
