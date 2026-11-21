//! End-to-end SQLite regression tests for `TaxCenterService` covering PR #28 review findings.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use chrono::{Datelike, Duration, Utc};
use diesel::sql_query;
use diesel::RunQueryDsl;
use rust_decimal::Decimal;
use wealthfolio_core::fx::{FxService, FxServiceTrait};
use wealthfolio_core::limits::ContributionLimitService;
use wealthfolio_core::lots::AssetLotView;
use wealthfolio_core::portfolio::holdings::{Holding, HoldingsServiceTrait};
use wealthfolio_core::tax::{
    TaxCenterService, TaxCenterServiceTrait, TaxExportFormat, WashSaleConfidence, WashSaleSeverity,
};
use wealthfolio_core::Result;

use crate::accounts::AccountRepository;
use crate::activities::ActivityRepository;
use crate::assets::AssetRepository;
use crate::db::{DbAccess, DbPool, WriteHandle};
use crate::fx::FxRepository;
use crate::limits::ContributionLimitRepository;
use crate::lots::LotsRepository;
use crate::market_data::MarketDataRepository;
use crate::portfolio::projection::ProjectionStore;
use crate::portfolio::statements::StatementStore;

struct EmptyHoldingsService;

#[async_trait]
impl HoldingsServiceTrait for EmptyHoldingsService {
    async fn get_holdings(&self, _account_id: &str, _base_currency: &str) -> Result<Vec<Holding>> {
        Ok(Vec::new())
    }

    async fn get_asset_lot_view(
        &self,
        _asset_id: &str,
        _include_snapshot_positions: bool,
    ) -> Result<Vec<AssetLotView>> {
        Ok(Vec::new())
    }

    async fn get_holdings_for_accounts(
        &self,
        _account_ids: &[String],
        _base_currency: &str,
        _aggregated_account_id: &str,
    ) -> Result<Vec<Holding>> {
        Ok(Vec::new())
    }

    async fn get_holding(
        &self,
        _account_id: &str,
        _asset_id: &str,
        _base_currency: &str,
    ) -> Result<Option<Holding>> {
        Ok(None)
    }

    async fn holdings_from_snapshot(
        &self,
        _snapshot: &wealthfolio_core::portfolio::snapshot::AccountStateSnapshot,
        _base_currency: &str,
    ) -> Result<Vec<Holding>> {
        Ok(Vec::new())
    }
}

struct TaxRig {
    _dir: tempfile::TempDir,
    pool: Arc<DbPool>,
    _writer: WriteHandle,
    service: TaxCenterService,
}

impl TaxRig {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = DbAccess::plaintext(dir.path().join("tax.db").to_string_lossy());
        db.prepare().expect("prepare");
        db.run_migrations().expect("migrations");
        let pool = db.create_pool().expect("pool");
        let writer = crate::db::write_actor::spawn_writer(pool.as_ref().clone()).expect("writer");

        let account_repo = Arc::new(AccountRepository::new(pool.clone(), writer.clone()));
        let activity_repo = Arc::new(ActivityRepository::new(pool.clone(), writer.clone()));
        let asset_repo = Arc::new(AssetRepository::new(pool.clone(), writer.clone()));
        let lots_repo = Arc::new(LotsRepository::new(pool.clone(), writer.clone()));
        let market_data_repo = Arc::new(MarketDataRepository::new(pool.clone(), writer.clone()));
        let fx_repo = Arc::new(FxRepository::new(pool.clone(), writer.clone()));
        let fx_service = Arc::new(FxService::new(fx_repo));
        fx_service.initialize().expect("fx init");
        let limits_repo = Arc::new(ContributionLimitRepository::new(
            pool.clone(),
            writer.clone(),
        ));
        let timezone = Arc::new(RwLock::new("UTC".to_string()));
        let base_currency = Arc::new(RwLock::new("USD".to_string()));
        let limits_service = Arc::new(ContributionLimitService::new_with_timezone(
            fx_service.clone(),
            limits_repo,
            activity_repo.clone(),
            timezone.clone(),
        ));
        let statement_store = Arc::new(StatementStore::new(pool.clone(), writer.clone()));
        let projection_store = Arc::new(ProjectionStore::new(pool.clone(), writer.clone()));
        let holdings_service: Arc<dyn HoldingsServiceTrait> = Arc::new(EmptyHoldingsService);

        let service = TaxCenterService::new(
            account_repo,
            activity_repo,
            asset_repo,
            lots_repo,
            market_data_repo,
            fx_service,
            holdings_service,
            limits_service,
            statement_store,
            projection_store,
            base_currency,
            timezone,
        );

        Self {
            _dir: dir,
            pool,
            _writer: writer,
            service,
        }
    }

    fn exec(&self, sql: &str) {
        let mut conn = self.pool.get().expect("conn");
        sql_query(sql).execute(&mut conn).expect("exec sql");
    }

    fn seed_asset(&self, id: &str, symbol: &str, ccy: &str) {
        self.exec(&format!(
            "INSERT INTO assets(id, kind, name, display_code, is_active, quote_mode, quote_ccy, instrument_type, instrument_symbol, created_at, updated_at) \
             VALUES ('{id}', 'INVESTMENT', '{symbol} Corp', '{symbol}', 1, 'MARKET', '{ccy}', 'EQUITY', '{symbol}', '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')"
        ));
    }

    fn seed_account(
        &self,
        id: &str,
        name: &str,
        ccy: &str,
        is_archived: bool,
        meta_json: Option<&str>,
    ) {
        let archived = if is_archived { 1 } else { 0 };
        let meta = match meta_json {
            Some(m) => format!("'{}'", m.replace('\'', "''")),
            None => "NULL".to_string(),
        };
        self.exec(&format!(
            "INSERT INTO accounts(id, name, account_type, currency, is_default, is_active, tracking_mode, is_archived, meta) \
             VALUES ('{id}', '{name}', 'SECURITIES', '{ccy}', 0, 1, 'TRANSACTIONS', {archived}, {meta})"
        ));
    }

    fn seed_open_buy(&self, id: &str, account_id: &str, asset_id: &str, date: &str, ccy: &str) {
        self.exec(&format!(
            "INSERT INTO activities(id, account_id, asset_id, activity_type, status, activity_date, quantity, unit_price, amount, fee, tax, currency, created_at, updated_at) \
             VALUES ('{id}', '{account_id}', '{asset_id}', 'BUY', 'POSTED', '{date}T12:00:00Z', '10', '100', '1000', '0', '0', '{ccy}', '{date}T12:00:00Z', '{date}T12:00:00Z')"
        ));
    }
}

#[tokio::test]
async fn missing_fx_disposal_preserves_unknown_pnl_warns_excludes_totals_and_still_screens_wash_sale(
) {
    let rig = TaxRig::new();
    rig.seed_asset("sap", "SAP", "EUR");
    rig.seed_account(
        "acc-eu",
        "Euro Brokerage",
        "EUR",
        false,
        Some(r#"{"tax":{"taxTreatment":"TAXABLE","ownerName":"Alex"}}"#),
    );
    rig.seed_open_buy("buy-old", "acc-eu", "sap", "2025-01-10", "EUR");

    // Original lot bought in EUR with missing FX rate (0 placeholder in base columns).
    rig.exec(
        "INSERT INTO lots(id, account_id, asset_id, open_date, open_activity_id, original_quantity, cost_per_unit, \
         original_cost_basis, remaining_cost_basis, original_cost_basis_base, remaining_cost_basis_base, \
         fee_allocated, fee_allocated_base, tax_allocated, tax_allocated_base, currency, base_currency, \
         fx_rate_to_base, cost_basis_method, remaining_quantity, split_ratio, is_closed, created_at, updated_at) \
         VALUES ('lot-eu', 'acc-eu', 'sap', '2025-01-10', 'buy-old', '10', '100', '1000', '0', '0', '0', \
         '0', '0', '0', '0', 'EUR', 'USD', '0', 'FIFO', '0', '1', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
    );
    // Disposal SELL activity + engine-written zero-placeholder lot_disposal (local loss = -200 EUR).
    rig.exec(
        "INSERT INTO activities(id, account_id, asset_id, activity_type, status, activity_date, quantity, unit_price, amount, fee, tax, currency, created_at, updated_at) \
         VALUES ('sell-eu', 'acc-eu', 'sap', 'SELL', 'POSTED', '2026-05-10T12:00:00Z', '10', '80', '800', '0', '0', 'EUR', '2026-05-10T12:00:00Z', '2026-05-10T12:00:00Z')",
    );
    rig.exec(
        "INSERT INTO lot_disposals(id, lot_id, account_id, asset_id, disposal_activity_id, disposal_date, quantity, \
         proceeds, cost_basis, realized_pnl, proceeds_base, cost_basis_base, realized_pnl_base, currency, base_currency, \
         fx_rate_to_base, cost_basis_method, created_at) \
         VALUES ('disp-eu', 'lot-eu', 'acc-eu', 'sap', 'sell-eu', '2026-05-10', '10', \
         '800', '1000', '-200', '0', '0', '0', 'EUR', 'USD', '0', 'FIFO', '2026-05-10T12:00:00Z')",
    );
    // Replacement purchase 5 days later -> must still trigger wash-sale alert even though FX is missing!
    rig.exec(
        "INSERT INTO activities(id, account_id, asset_id, activity_type, status, activity_date, quantity, unit_price, amount, fee, tax, currency, created_at, updated_at) \
         VALUES ('rep-eu', 'acc-eu', 'sap', 'BUY', 'POSTED', '2026-05-15T12:00:00Z', '10', '82', '820', '0', '0', 'EUR', '2026-05-15T12:00:00Z', '2026-05-15T12:00:00Z')",
    );

    let report = rig
        .service
        .get_tax_year_report(2026, None)
        .await
        .expect("report");

    assert!(report.warnings.unconverted_currencies.contains("EUR"));
    assert_eq!(report.warnings.unconverted_disposal_count, 1);
    assert_eq!(report.realized_gains.taxable_disposal_count, 0);
    assert_eq!(report.realized_gains.total_net_realized_base, Decimal::ZERO);
    assert_eq!(report.realized_gains.disposals.len(), 1);
    let row = &report.realized_gains.disposals[0];
    assert!(row.fx_missing);
    assert_eq!(row.proceeds_base, None);
    assert_eq!(row.cost_basis_base, None);
    assert_eq!(row.realized_pnl_base, None);
    assert_eq!(row.wash_sale_risk, Some(WashSaleSeverity::SameAccount));
    assert_eq!(row.estimated_disallowed_loss_base, None);

    assert_eq!(report.wash_sales.len(), 1);
    assert_eq!(report.wash_sales[0].realized_loss_base, None);
    assert_eq!(report.wash_sales[0].estimated_disallowed_loss_base, None);

    let csv = rig
        .service
        .export_tax_packet(2026, TaxExportFormat::RealizedGainsCsv, None)
        .await
        .expect("csv");
    // Missing FX columns must be blank (not 0).
    assert!(
        csv.content.contains(",10,,,,USD,FIFO,SAME_ACCOUNT,"),
        "unexpected csv: {}",
        csv.content
    );
}

#[tokio::test]
async fn household_wash_sale_allocates_before_member_filter() {
    let rig = TaxRig::new();
    rig.seed_asset("vti", "VTI", "USD");
    rig.seed_account(
        "acc-alex",
        "Alex Taxable",
        "USD",
        false,
        Some(r#"{"tax":{"taxTreatment":"TAXABLE","ownerName":"Alex"}}"#),
    );
    rig.seed_account(
        "acc-jordan",
        "Jordan Taxable",
        "USD",
        false,
        Some(r#"{"tax":{"taxTreatment":"TAXABLE","ownerName":"Jordan"}}"#),
    );

    // Alex sells 10 shares at a loss on May 1; Jordan sells 10 shares at a loss on May 2.
    for (lot_id, acc_id, sell_id, disp_id, day) in [
        (
            "lot-alex",
            "acc-alex",
            "sell-alex",
            "disp-alex",
            "2026-05-01",
        ),
        (
            "lot-jordan",
            "acc-jordan",
            "sell-jordan",
            "disp-jordan",
            "2026-05-02",
        ),
    ] {
        rig.seed_open_buy(&format!("old-{lot_id}"), acc_id, "vti", "2025-01-01", "USD");
        rig.exec(&format!(
            "INSERT INTO lots(id, account_id, asset_id, open_date, open_activity_id, original_quantity, cost_per_unit, \
             original_cost_basis, remaining_cost_basis, original_cost_basis_base, remaining_cost_basis_base, \
             fee_allocated, fee_allocated_base, tax_allocated, tax_allocated_base, currency, base_currency, \
             fx_rate_to_base, cost_basis_method, remaining_quantity, split_ratio, is_closed, created_at, updated_at) \
             VALUES ('{lot_id}', '{acc_id}', 'vti', '2025-01-01', 'old-{lot_id}', '10', '100', '1000', '0', '1000', '0', \
             '0', '0', '0', '0', 'USD', 'USD', '1', 'FIFO', '0', '1', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
        ));
        rig.exec(&format!(
            "INSERT INTO activities(id, account_id, asset_id, activity_type, status, activity_date, quantity, unit_price, amount, fee, tax, currency, created_at, updated_at) \
             VALUES ('{sell_id}', '{acc_id}', 'vti', 'SELL', 'POSTED', '{day}T12:00:00Z', '10', '90', '900', '0', '0', 'USD', '{day}T12:00:00Z', '{day}T12:00:00Z')"
        ));
        rig.exec(&format!(
            "INSERT INTO lot_disposals(id, lot_id, account_id, asset_id, disposal_activity_id, disposal_date, quantity, \
             proceeds, cost_basis, realized_pnl, proceeds_base, cost_basis_base, realized_pnl_base, currency, base_currency, \
             fx_rate_to_base, cost_basis_method, created_at) \
             VALUES ('{disp_id}', '{lot_id}', '{acc_id}', 'vti', '{sell_id}', '{day}', '10', \
             '900', '1000', '-100', '900', '1000', '-100', 'USD', 'USD', '1', 'FIFO', '{day}T12:00:00Z')"
        ));
    }

    // One single 10-share replacement buy on May 10.
    rig.exec(
        "INSERT INTO activities(id, account_id, asset_id, activity_type, status, activity_date, quantity, unit_price, amount, fee, tax, currency, created_at, updated_at) \
         VALUES ('rep-10', 'acc-jordan', 'vti', 'BUY', 'POSTED', '2026-05-10T12:00:00Z', '10', '91', '910', '0', '0', 'USD', '2026-05-10T12:00:00Z', '2026-05-10T12:00:00Z')",
    );

    let family = rig
        .service
        .get_tax_year_report(2026, None)
        .await
        .expect("family report");
    assert_eq!(family.wash_sales.len(), 1);
    assert_eq!(family.wash_sales[0].disposal_id, "disp-alex");

    let jordan = rig
        .service
        .get_tax_year_report(2026, Some("Jordan".into()))
        .await
        .expect("jordan report");
    assert!(
        jordan.wash_sales.is_empty(),
        "Jordan's filtered report must not re-allocate shares already consumed by Alex's earlier loss"
    );
    assert_eq!(jordan.realized_gains.disposals.len(), 1);
    assert_eq!(jordan.realized_gains.disposals[0].wash_sale_risk, None);
}

#[tokio::test]
async fn duplicate_splits_across_accounts_are_counted_once() {
    let rig = TaxRig::new();
    rig.seed_asset("nvda", "NVDA", "USD");
    rig.seed_account(
        "acc-1",
        "Broker A",
        "USD",
        false,
        Some(r#"{"tax":{"taxTreatment":"TAXABLE","ownerName":"Alex"}}"#),
    );
    rig.seed_account(
        "acc-2",
        "Broker B",
        "USD",
        false,
        Some(r#"{"tax":{"taxTreatment":"TAXABLE","ownerName":"Jordan"}}"#),
    );

    // 10 pre-split shares sold at a $200 loss on Jun 1.
    rig.seed_open_buy("old-buy", "acc-1", "nvda", "2025-01-01", "USD");
    rig.exec(
        "INSERT INTO lots(id, account_id, asset_id, open_date, open_activity_id, original_quantity, cost_per_unit, \
         original_cost_basis, remaining_cost_basis, original_cost_basis_base, remaining_cost_basis_base, \
         fee_allocated, fee_allocated_base, tax_allocated, tax_allocated_base, currency, base_currency, \
         fx_rate_to_base, cost_basis_method, remaining_quantity, split_ratio, is_closed, created_at, updated_at) \
         VALUES ('lot-1', 'acc-1', 'nvda', '2025-01-01', 'old-buy', '10', '100', '1000', '0', '1000', '0', \
         '0', '0', '0', '0', 'USD', 'USD', '1', 'FIFO', '0', '1', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
    );
    rig.exec(
        "INSERT INTO activities(id, account_id, asset_id, activity_type, status, activity_date, quantity, unit_price, amount, fee, tax, currency, created_at, updated_at) \
         VALUES ('sell-1', 'acc-1', 'nvda', 'SELL', 'POSTED', '2026-06-01T12:00:00Z', '10', '80', '800', '0', '0', 'USD', '2026-06-01T12:00:00Z', '2026-06-01T12:00:00Z')",
    );
    rig.exec(
        "INSERT INTO lot_disposals(id, lot_id, account_id, asset_id, disposal_activity_id, disposal_date, quantity, \
         proceeds, cost_basis, realized_pnl, proceeds_base, cost_basis_base, realized_pnl_base, currency, base_currency, \
         fx_rate_to_base, cost_basis_method, created_at) \
         VALUES ('disp-1', 'lot-1', 'acc-1', 'nvda', 'sell-1', '2026-06-01', '10', \
         '800', '1000', '-200', '800', '1000', '-200', 'USD', 'USD', '1', 'FIFO', '2026-06-01T12:00:00Z')",
    );
    // Two brokerage accounts both record the same 2-for-1 split on Jun 5.
    rig.exec(
        "INSERT INTO activities(id, account_id, asset_id, activity_type, status, activity_date, quantity, amount, currency, created_at, updated_at) \
         VALUES ('split-a', 'acc-1', 'nvda', 'SPLIT', 'POSTED', '2026-06-05T12:00:00Z', '2', '2', 'USD', '2026-06-05T12:00:00Z', '2026-06-05T12:00:00Z'), \
                ('split-b', 'acc-2', 'nvda', 'SPLIT', 'POSTED', '2026-06-05T12:00:00Z', '2', '2', 'USD', '2026-06-05T12:00:00Z', '2026-06-05T12:00:00Z')",
    );
    // 20 post-split replacement shares (= 10 pre-split shares) bought on Jun 10.
    rig.exec(
        "INSERT INTO activities(id, account_id, asset_id, activity_type, status, activity_date, quantity, unit_price, amount, fee, tax, currency, created_at, updated_at) \
         VALUES ('rep-20', 'acc-2', 'nvda', 'BUY', 'POSTED', '2026-06-10T12:00:00Z', '20', '42', '840', '0', '0', 'USD', '2026-06-10T12:00:00Z', '2026-06-10T12:00:00Z')",
    );

    let report = rig
        .service
        .get_tax_year_report(2026, None)
        .await
        .expect("report");
    assert_eq!(report.wash_sales.len(), 1);
    assert_eq!(report.wash_sales[0].matched_quantity, Decimal::from(10));
    assert_eq!(
        report.wash_sales[0].estimated_disallowed_loss_base,
        Some(Decimal::new(20000, 2))
    );
}

#[tokio::test]
async fn drip_dividend_triggers_wash_sale_and_tlh_cooldown() {
    let rig = TaxRig::new();
    rig.seed_asset("vti", "VTI", "USD");
    rig.seed_account(
        "acc-1",
        "Alex Taxable",
        "USD",
        false,
        Some(r#"{"tax":{"taxTreatment":"TAXABLE","ownerName":"Alex"}}"#),
    );

    let today = Utc::now().date_naive();
    let tax_year = today.year();
    let sell_date = today - Duration::days(10);
    let drip_date = today - Duration::days(5);
    let open_date = today - Duration::days(200);

    // Closed loss lot sold 10 days ago.
    let open_date_str = open_date.to_string();
    rig.seed_open_buy("old-buy", "acc-1", "vti", &open_date_str, "USD");
    rig.seed_open_buy("old-buy-2", "acc-1", "vti", &open_date_str, "USD");
    rig.exec(&format!(
        "INSERT INTO lots(id, account_id, asset_id, open_date, open_activity_id, original_quantity, cost_per_unit, \
         original_cost_basis, remaining_cost_basis, original_cost_basis_base, remaining_cost_basis_base, \
         fee_allocated, fee_allocated_base, tax_allocated, tax_allocated_base, currency, base_currency, \
         fx_rate_to_base, cost_basis_method, remaining_quantity, split_ratio, is_closed, created_at, updated_at) \
         VALUES ('lot-closed', 'acc-1', 'vti', '{open_date}', 'old-buy', '10', '100', '1000', '0', '1000', '0', \
         '0', '0', '0', '0', 'USD', 'USD', '1', 'FIFO', '0', '1', 1, '{open_date}T00:00:00Z', '{open_date}T00:00:00Z')"
    ));
    rig.exec(&format!(
        "INSERT INTO activities(id, account_id, asset_id, activity_type, status, activity_date, quantity, unit_price, amount, fee, tax, currency, created_at, updated_at) \
         VALUES ('sell-1', 'acc-1', 'vti', 'SELL', 'POSTED', '{sell_date}T12:00:00Z', '10', '80', '800', '0', '0', 'USD', '{sell_date}T12:00:00Z', '{sell_date}T12:00:00Z')"
    ));
    rig.exec(&format!(
        "INSERT INTO lot_disposals(id, lot_id, account_id, asset_id, disposal_activity_id, disposal_date, quantity, \
         proceeds, cost_basis, realized_pnl, proceeds_base, cost_basis_base, realized_pnl_base, currency, base_currency, \
         fx_rate_to_base, cost_basis_method, created_at) \
         VALUES ('disp-1', 'lot-closed', 'acc-1', 'vti', 'sell-1', '{sell_date}', '10', \
         '800', '1000', '-200', '800', '1000', '-200', 'USD', 'USD', '1', 'FIFO', '{sell_date}T12:00:00Z')"
    ));

    // Open underwater lot for TLH + latest quote below cost basis.
    rig.exec(&format!(
        "INSERT INTO lots(id, account_id, asset_id, open_date, open_activity_id, original_quantity, cost_per_unit, \
         original_cost_basis, remaining_cost_basis, original_cost_basis_base, remaining_cost_basis_base, \
         fee_allocated, fee_allocated_base, tax_allocated, tax_allocated_base, currency, base_currency, \
         fx_rate_to_base, cost_basis_method, remaining_quantity, split_ratio, is_closed, created_at, updated_at) \
         VALUES ('lot-open', 'acc-1', 'vti', '{open_date}', 'old-buy-2', '10', '100', '1000', '1000', '1000', '1000', \
         '0', '0', '0', '0', 'USD', 'USD', '1', 'FIFO', '10', '1', 0, '{open_date}T00:00:00Z', '{open_date}T00:00:00Z')"
    ));
    rig.exec(&format!(
        "INSERT INTO quotes(id, asset_id, day, source, close, currency, created_at, timestamp) \
         VALUES ('q-vti', 'vti', '{today}', 'MANUAL', '80', 'USD', '{today}T12:00:00Z', '{today}T12:00:00Z')"
    ));

    // DRIP stored as DIVIDEND + subtype = DRIP 5 days ago (must trigger BOTH wash sale AND TLH cooldown!).
    rig.exec(&format!(
        "INSERT INTO activities(id, account_id, asset_id, activity_type, subtype, status, activity_date, quantity, unit_price, amount, fee, tax, currency, created_at, updated_at) \
         VALUES ('drip-1', 'acc-1', 'vti', 'DIVIDEND', 'DRIP', 'POSTED', '{drip_date}T12:00:00Z', '2', '80', '160', '0', '0', 'USD', '{drip_date}T12:00:00Z', '{drip_date}T12:00:00Z')"
    ));

    let report = rig
        .service
        .get_tax_year_report(tax_year, None)
        .await
        .expect("report");

    assert_eq!(
        report.wash_sales.len(),
        1,
        "DRIP acquisition must trigger wash-sale alert"
    );
    assert_eq!(report.wash_sales[0].replacement_activity_id, "drip-1");
    assert_eq!(report.wash_sales[0].matched_quantity, Decimal::from(2));

    assert_eq!(report.tlh_candidates.len(), 1);
    assert_eq!(
        report.tlh_candidates[0].wash_sale_cooldown_until,
        Some(drip_date + Duration::days(31)),
        "DRIP acquisition must trigger 30-day TLH cooldown"
    );
}

#[tokio::test]
async fn archived_account_transfer_disposal_is_excluded_from_realized_gains() {
    let rig = TaxRig::new();
    rig.seed_asset("aapl", "AAPL", "USD");
    rig.seed_account(
        "acc-old",
        "Closed Brokerage",
        "USD",
        true,
        Some(r#"{"tax":{"taxTreatment":"TAXABLE","ownerName":"Alex"}}"#),
    );
    rig.seed_open_buy("buy-old", "acc-old", "aapl", "2024-01-01", "USD");

    rig.exec(
        "INSERT INTO lots(id, account_id, asset_id, open_date, open_activity_id, original_quantity, cost_per_unit, \
         original_cost_basis, remaining_cost_basis, original_cost_basis_base, remaining_cost_basis_base, \
         fee_allocated, fee_allocated_base, tax_allocated, tax_allocated_base, currency, base_currency, \
         fx_rate_to_base, cost_basis_method, remaining_quantity, split_ratio, is_closed, created_at, updated_at) \
         VALUES ('lot-arch', 'acc-old', 'aapl', '2024-01-01', 'buy-old', '10', '100', '1000', '0', '1000', '0', \
         '0', '0', '0', '0', 'USD', 'USD', '1', 'FIFO', '0', '1', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
    );
    // TRANSFER_OUT on an archived account — must still be loaded and excluded from realized gains!
    rig.exec(
        "INSERT INTO activities(id, account_id, asset_id, activity_type, status, activity_date, quantity, unit_price, amount, fee, tax, currency, created_at, updated_at) \
         VALUES ('xfer-arch', 'acc-old', 'aapl', 'TRANSFER_OUT', 'POSTED', '2026-04-01T12:00:00Z', '10', '100', '1000', '0', '0', 'USD', '2026-04-01T12:00:00Z', '2026-04-01T12:00:00Z')",
    );
    rig.exec(
        "INSERT INTO lot_disposals(id, lot_id, account_id, asset_id, disposal_activity_id, disposal_date, quantity, \
         proceeds, cost_basis, realized_pnl, proceeds_base, cost_basis_base, realized_pnl_base, currency, base_currency, \
         fx_rate_to_base, cost_basis_method, created_at) \
         VALUES ('disp-arch', 'lot-arch', 'acc-old', 'aapl', 'xfer-arch', '2026-04-01', '10', \
         '1000', '1000', '0', '1000', '1000', '0', 'USD', 'USD', '1', 'FIFO', '2026-04-01T12:00:00Z')",
    );

    let report = rig
        .service
        .get_tax_year_report(2026, None)
        .await
        .expect("report");
    assert!(
        report.realized_gains.disposals.is_empty(),
        "archived account TRANSFER_OUT disposal must be excluded from realized gains"
    );
}

#[tokio::test]
async fn recurring_limit_projection_honors_latest_non_recurring_year() {
    let rig = TaxRig::new();
    let current_year = Utc::now().date_naive().year();
    let year_prev2 = current_year - 2;
    let year_prev1 = current_year - 1;

    // Older year is recurring, newer prior year explicitly disabled recurrence -> must NOT project current_year!
    rig.exec(&format!(
        "INSERT INTO contribution_limits(id, group_name, contribution_year, limit_amount, account_ids, is_recurring, member_name, limit_category, created_at, updated_at) \
         VALUES ('lim-old', '401(k)', {year_prev2}, 23000, 'acc-1', 1, 'Alex', '401K', '2024-01-01 00:00:00', '2024-01-01 00:00:00'), \
                ('lim-new', '401(k)', {year_prev1}, 23500, 'acc-1', 0, 'Alex', '401K', '2025-01-01 00:00:00', '2025-01-01 00:00:00')"
    ));

    let report = rig
        .service
        .get_tax_year_report(current_year, None)
        .await
        .expect("report");
    assert_eq!(report.contribution_limits.unmaterialized_recurring_count, 0);
    assert!(
        report.contribution_limits.by_member.is_empty(),
        "disabled recurrence in latest prior year must suppress future projection"
    );
}

#[tokio::test]
async fn tfsa_tax_free_uses_general_alert_while_explicit_ira_uses_ira_alert() {
    let rig = TaxRig::new();
    rig.seed_asset("spy", "SPY", "USD");
    rig.seed_asset("qqq", "QQQ", "USD");
    rig.seed_account(
        "acc-taxable",
        "Taxable",
        "USD",
        false,
        Some(r#"{"tax":{"taxTreatment":"TAXABLE","ownerName":"Alex"}}"#),
    );
    // Tax-free TFSA (NOT an IRA!)
    rig.seed_account(
        "acc-tfsa",
        "TFSA",
        "USD",
        false,
        Some(r#"{"tax":{"taxTreatment":"TAX_FREE","ownerName":"Alex","accountCategory":"TFSA"}}"#),
    );
    // Explicitly designated Roth IRA
    rig.seed_account(
        "acc-roth",
        "Roth IRA",
        "USD",
        false,
        Some(
            r#"{"tax":{"taxTreatment":"TAX_FREE","ownerName":"Jordan","accountCategory":"ROTH_IRA"}}"#,
        ),
    );

    for (asset, lot_id, sell_id, disp_id) in [
        ("spy", "lot-spy", "sell-spy", "disp-spy"),
        ("qqq", "lot-qqq", "sell-qqq", "disp-qqq"),
    ] {
        rig.seed_open_buy(
            &format!("old-{asset}"),
            "acc-taxable",
            asset,
            "2025-01-01",
            "USD",
        );
        rig.exec(&format!(
            "INSERT INTO lots(id, account_id, asset_id, open_date, open_activity_id, original_quantity, cost_per_unit, \
             original_cost_basis, remaining_cost_basis, original_cost_basis_base, remaining_cost_basis_base, \
             fee_allocated, fee_allocated_base, tax_allocated, tax_allocated_base, currency, base_currency, \
             fx_rate_to_base, cost_basis_method, remaining_quantity, split_ratio, is_closed, created_at, updated_at) \
             VALUES ('{lot_id}', 'acc-taxable', '{asset}', '2025-01-01', 'old-{asset}', '10', '100', '1000', '0', '1000', '0', \
             '0', '0', '0', '0', 'USD', 'USD', '1', 'FIFO', '0', '1', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
        ));
        rig.exec(&format!(
            "INSERT INTO activities(id, account_id, asset_id, activity_type, status, activity_date, quantity, unit_price, amount, fee, tax, currency, created_at, updated_at) \
             VALUES ('{sell_id}', 'acc-taxable', '{asset}', 'SELL', 'POSTED', '2026-05-01T12:00:00Z', '10', '90', '900', '0', '0', 'USD', '2026-05-01T12:00:00Z', '2026-05-01T12:00:00Z')"
        ));
        rig.exec(&format!(
            "INSERT INTO lot_disposals(id, lot_id, account_id, asset_id, disposal_activity_id, disposal_date, quantity, \
             proceeds, cost_basis, realized_pnl, proceeds_base, cost_basis_base, realized_pnl_base, currency, base_currency, \
             fx_rate_to_base, cost_basis_method, created_at) \
             VALUES ('{disp_id}', '{lot_id}', 'acc-taxable', '{asset}', '{sell_id}', '2026-05-01', '10', \
             '900', '1000', '-100', '900', '1000', '-100', 'USD', 'USD', '1', 'FIFO', '2026-05-01T12:00:00Z')"
        ));
    }

    // SPY repurchased in TFSA; QQQ repurchased in explicitly designated Roth IRA.
    rig.exec(
        "INSERT INTO activities(id, account_id, asset_id, activity_type, status, activity_date, quantity, unit_price, amount, fee, tax, currency, created_at, updated_at) \
         VALUES ('rep-spy-tfsa', 'acc-tfsa', 'spy', 'BUY', 'POSTED', '2026-05-05T12:00:00Z', '10', '91', '910', '0', '0', 'USD', '2026-05-05T12:00:00Z', '2026-05-05T12:00:00Z'), \
                ('rep-qqq-roth', 'acc-roth', 'qqq', 'BUY', 'POSTED', '2026-05-05T12:00:00Z', '10', '91', '910', '0', '0', 'USD', '2026-05-05T12:00:00Z', '2026-05-05T12:00:00Z')",
    );

    let report = rig
        .service
        .get_tax_year_report(2026, None)
        .await
        .expect("report");

    let spy_alert = report
        .wash_sales
        .iter()
        .find(|a| a.symbol == "SPY")
        .expect("SPY alert");
    assert_eq!(spy_alert.severity, WashSaleSeverity::CrossAccount);
    assert_eq!(spy_alert.confidence, WashSaleConfidence::NeedsReview);

    let qqq_alert = report
        .wash_sales
        .iter()
        .find(|a| a.symbol == "QQQ")
        .expect("QQQ alert");
    assert_eq!(
        qqq_alert.severity,
        WashSaleSeverity::IraPermanentDisallowance
    );
    assert_eq!(qqq_alert.confidence, WashSaleConfidence::High);
}
