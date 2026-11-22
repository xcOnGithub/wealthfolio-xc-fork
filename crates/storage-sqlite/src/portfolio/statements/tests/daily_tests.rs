use super::*;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use wealthfolio_core::{
    portfolio::{
        net_worth::{NetWorthService, NetWorthServiceTrait},
        snapshot::{AccountStateSnapshot, Position, SnapshotService},
        statements::{
            amount,
            daily::{DailyStatementWrite, LiveHandoff},
            reconstruction::{
                ClosingPosition, StatementDatePolicy, StatementEvent, StatementEventKind,
                StatementInterval,
            },
        },
    },
    quotes::{Quote, QuoteService, QuoteServiceTrait},
    secrets::SecretStore,
};
fn day(s: &str) -> NaiveDate {
    s.parse().unwrap()
}
fn daily() -> DailyStatementWrite {
    DailyStatementWrite {
        closing_asset_ids: vec![],
        activity_security_keys: None,
        expected_revision: None,
        expected_snapshot_revision: None,
        snapshot: StatementSnapshot {
            date: day("2025-05-05"),
            currency: "USD".into(),
            total_value: "114".into(),
            cash_value: Some("90".into()),
            positions: vec![wealthfolio_core::portfolio::statements::StatementPosition {
                label: "Synthetic equity".into(),
                quantity: "2".into(),
                market_value: Some("24".into()),
                cost_basis: None,
            }],
            positions_complete: true,
            format: "merrill-cma-monthly-v1".into(),
            source_page: 1,
        },
        interval: StatementInterval {
            date_policy: StatementDatePolicy::Settlement,
            from: day("2025-05-02"),
            through: day("2025-05-05"),
            currency: "USD".into(),
            opening_cash: "100".into(),
            closing_cash: "90".into(),
            closing_value: "114".into(),
            closing_positions_complete: true,
            activity_complete: true,
            closing_positions: vec![ClosingPosition {
                asset_id: "demo".into(),
                quantity: "2".into(),
                market_value: "24".into(),
                cost_basis: None,
            }],
            events: vec![StatementEvent {
                source_id: "row-1".into(),
                date: day("2025-05-02"),
                kind: StatementEventKind::Buy,
                asset_id: Some("demo".into()),
                quantity: Some("1".into()),
                cash: "-10".into(),
                source_page: 2,
                source_line: 10,
            }],
        },
    }
}
struct NoSecrets;
impl SecretStore for NoSecrets {
    fn set_secret(&self, _: &str, _: &str) -> Result<()> {
        Err(invalid())
    }
    fn get_secret(&self, key: &str) -> Result<Option<String>> {
        assert!(!key.starts_with("plaid:"));
        Ok(None)
    }
    fn delete_secret(&self, _: &str) -> Result<()> {
        Err(invalid())
    }
}
async fn services(
    r: &Rig,
) -> (
    StatementService,
    NetWorthService,
    Arc<dyn QuoteServiceTrait>,
) {
    let assets = Arc::new(crate::assets::AssetRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    sql_query("INSERT INTO assets(id,kind,name,display_code,is_active,quote_mode,quote_ccy,instrument_type,created_at,updated_at) VALUES ('demo','INVESTMENT','Synthetic equity','DEMO',1,'MANUAL','USD','EQUITY',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)").execute(&mut r.pool.get().unwrap()).unwrap();
    let market = Arc::new(crate::market_data::MarketDataRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let activities = Arc::new(crate::activities::ActivityRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let quotes: Arc<dyn QuoteServiceTrait> = Arc::new(
        QuoteService::new_with_custom_provider(
            market.clone(),
            Arc::new(crate::market_data::QuoteSyncStateRepository::new(
                r.pool.clone(),
                r.writer.clone(),
            )),
            market,
            assets.clone(),
            activities.clone(),
            Arc::new(NoSecrets),
            None,
        )
        .await
        .unwrap(),
    );
    let accounts = Arc::new(crate::accounts::AccountRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let valuations = Arc::new(crate::portfolio::valuation::ValuationRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let snapshots = Arc::new(crate::portfolio::snapshot::SnapshotRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let timezone = Arc::new(RwLock::new("UTC".into()));
    let snapshot_service = Arc::new(SnapshotService::new(
        timezone.clone(),
        accounts.clone(),
        snapshots,
        activities,
    ));
    let service = StatementService::new(
        r.store.clone(),
        accounts.clone(),
        valuations.clone(),
        timezone,
    )
    .with_pricing(assets.clone(), quotes.clone())
    .with_live_snapshots(snapshot_service.clone());
    let net = NetWorthService::new(
        Arc::new(RwLock::new("USD".into())),
        accounts,
        assets,
        snapshot_service,
        quotes.clone(),
        valuations,
        Arc::new(wealthfolio_core::fx::FxService::new(Arc::new(
            crate::fx::FxRepository::new(r.pool.clone(), r.writer.clone()),
        ))),
    )
    .with_statement_history(r.store.clone());
    (service, net, quotes)
}
async fn seed(quotes: &dyn QuoteServiceTrait) {
    quotes
        .bulk_upsert_quotes(
            [("2025-05-02", "10"), ("2025-05-05", "12")]
                .into_iter()
                .map(|(d, p)| Quote {
                    id: format!("{d}_demo_MANUAL"),
                    asset_id: "demo".into(),
                    timestamp: day(d).and_hms_opt(16, 0, 0).unwrap().and_utc(),
                    close: p.parse().unwrap(),
                    open: p.parse().unwrap(),
                    high: p.parse().unwrap(),
                    low: p.parse().unwrap(),
                    adjclose: "999".parse().unwrap(),
                    currency: "USD".into(),
                    data_source: "MANUAL".into(),
                    ..Default::default()
                })
                .collect(),
        )
        .await
        .unwrap();
}
#[tokio::test]
async fn daily_import_prices_persisted_facts_and_net_worth_without_a_plaid_connection() {
    let r = Rig::new();
    let (service, net, quotes) = services(&r).await;
    seed(quotes.as_ref()).await;
    assert!(service
        .save_daily("a".into(), vec![daily()], false)
        .await
        .is_err());
    let preview = service
        .preview_daily("a".into(), vec![daily()])
        .await
        .unwrap();
    assert_eq!(preview[0].live_handoff, LiveHandoff::Unverified);
    assert!(!preview[0].priced.returns_eligible);
    service
        .save_daily("a".into(), vec![daily()], true)
        .await
        .unwrap();
    let rows = service
        .history("a".into(), day("2025-05-01"), day("2025-05-05"))
        .await
        .unwrap();
    assert_eq!(
        rows.iter()
            .map(|v| amount(&v.value).unwrap())
            .collect::<Vec<_>>(),
        vec![
            Decimal::from(110),
            Decimal::from(110),
            Decimal::from(110),
            Decimal::from(114)
        ]
    );
    assert_eq!(rows[0].source, "statementDaily");
    assert_eq!(rows[3].source, "statementClosing");
    let history = net
        .get_net_worth_history(day("2025-05-02"), day("2025-05-05"))
        .await
        .unwrap();
    assert_eq!(history.len(), 4);
    for (point, row) in history.iter().zip(&rows) {
        assert_eq!(point.net_worth, amount(&row.value).unwrap());
        assert_eq!(point.breakdown.get("cash"), Some(&Decimal::from(90)));
        assert_eq!(point.statement_sources.len(), 1);
        assert_eq!(point.statement_sources[0].source, row.source);
    }
    let serialized = serde_json::to_string(&service.list_daily("a".into()).await.unwrap()).unwrap();
    assert!(!serialized.contains("counterparty"));
    #[derive(QueryableByName)]
    struct Count {
        #[diesel(sql_type=diesel::sql_types::BigInt)]
        n: i64,
    }
    assert_eq!(
        sql_query("SELECT COUNT(*) AS n FROM activities")
            .get_result::<Count>(&mut r.pool.get().unwrap())
            .unwrap()
            .n,
        0
    );
    // Reinitializing the service reads source facts from SQLite, with no ledger writes.
    let reopened = StatementService::new(
        r.store.clone(),
        Arc::new(crate::accounts::AccountRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        )),
        Arc::new(crate::portfolio::valuation::ValuationRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        )),
        Arc::new(RwLock::new("UTC".into())),
    );
    assert_eq!(
        reopened.list_daily("a".into()).await.unwrap()[0].interval,
        daily().interval
    );
}
#[tokio::test]
async fn daily_security_fingerprints_follow_confirmed_revisions() {
    let r = Rig::new();
    let (service, _, _) = services(&r).await;
    let mut write = daily();
    let fingerprint = "a".repeat(64);
    write.activity_security_keys = Some([("unknown-event".into(), fingerprint.clone())].into());
    assert!(service
        .save_daily("a".into(), vec![write.clone()], true)
        .await
        .is_err());
    assert!(r.store.list_daily("a").unwrap().is_empty());

    let source = write.interval.events[0].source_id.clone();
    write.activity_security_keys = Some([(source.clone(), fingerprint.clone())].into());
    assert!(service
        .save_daily("a".into(), vec![write.clone()], false)
        .await
        .is_err());
    service
        .save_daily("a".into(), vec![write.clone()], true)
        .await
        .unwrap();
    let first = r.store.list_daily("a").unwrap().remove(0);
    assert_eq!(
        first.activity_security_keys.get(&source),
        Some(&fingerprint)
    );
    assert_eq!(first.review_order, 1);
    assert!(r.store.list_daily("b").unwrap().is_empty());

    service
        .save_daily("a".into(), vec![write.clone()], true)
        .await
        .unwrap();
    assert_eq!(r.store.list_daily("a").unwrap()[0].review_order, 1);

    let revised_key = "b".repeat(64);
    write.activity_security_keys = Some([(source.clone(), revised_key.clone())].into());
    assert!(service
        .save_daily("a".into(), vec![write.clone()], true)
        .await
        .is_err());
    write.expected_revision = Some(first.revision);
    write.expected_snapshot_revision = Some(r.store.list("a").unwrap()[0].revision.clone());
    service
        .save_daily("a".into(), vec![write], true)
        .await
        .unwrap();
    let revised = r.store.list_daily("a").unwrap().remove(0);
    assert_eq!(
        revised.activity_security_keys.get(&source),
        Some(&revised_key)
    );
    assert_eq!(revised.review_order, 2);

    sql_query("UPDATE account_statement_reconstructions SET revision = 'older-app-write' WHERE account_id = 'a'")
        .execute(&mut r.pool.get().unwrap())
        .unwrap();
    let replaced = r.store.list_daily("a").unwrap().remove(0);
    assert!(replaced.activity_security_keys.is_empty());
    assert_eq!(replaced.review_order, 0);
    assert_eq!(replaced.interval, revised.interval);
}

#[tokio::test]
async fn daily_revision_conflicts_roll_back_batches_and_monthly_replacement_invalidates_facts() {
    let r = Rig::new();
    let (service, _, _) = services(&r).await;
    service
        .save_daily("a".into(), vec![daily()], true)
        .await
        .unwrap();
    let original = r.store.list_daily("a").unwrap();
    let monthly = r.store.list("a").unwrap();
    service
        .save_daily("a".into(), vec![daily()], true)
        .await
        .unwrap();
    assert_eq!(
        r.store.list_daily("a").unwrap()[0].revision,
        original[0].revision
    );
    let mut updated = daily();
    updated.interval.events[0].source_line = 11;
    assert!(service
        .save_daily("a".into(), vec![updated.clone()], true)
        .await
        .is_err());
    updated.expected_revision = Some(original[0].revision.clone());
    updated.expected_snapshot_revision = Some(monthly[0].revision.clone());
    service
        .save_daily("a".into(), vec![updated.clone()], true)
        .await
        .unwrap();
    assert_ne!(
        r.store.list_daily("a").unwrap()[0].revision,
        original[0].revision
    );
    assert!(service
        .save_daily("a".into(), vec![daily()], true)
        .await
        .is_err());
    assert!(service
        .save_daily("missing".into(), vec![daily()], true)
        .await
        .is_err());
    assert!(r.store.list_daily("b").unwrap().is_empty());
    let mut second = updated.clone();
    second.interval.from = day("2025-06-02");
    second.interval.through = day("2025-06-05");
    second.interval.events[0].date = day("2025-06-02");
    second.snapshot.date = day("2025-06-05");
    second.expected_revision = None;
    second.expected_snapshot_revision = None;
    assert!(r
        .store
        .save_daily("a", vec![second, daily()])
        .await
        .is_err());
    assert_eq!(r.store.list("a").unwrap().len(), 1);
    assert!(r
        .service
        .delete(
            "a".into(),
            monthly[0].snapshot.date,
            monthly[0].revision.clone()
        )
        .await
        .is_err());
    let monthly = r.store.list("a").unwrap();
    let mut replacement = monthly[0].snapshot.clone();
    replacement.positions[0].cost_basis = Some("10".into());
    r.service
        .save(
            "a".into(),
            vec![StatementWrite {
                snapshot: replacement,
                expected_revision: Some(monthly[0].revision.clone()),
            }],
            true,
        )
        .await
        .unwrap();
    assert!(r.store.list_daily("a").unwrap().is_empty());
    assert!(r.service.list("a".into()).await.unwrap()[0]
        .snapshot
        .positions[0]
        .cost_basis
        .is_some());
}
#[tokio::test]
async fn daily_facts_survive_backup_and_cascade_on_statement_or_account_deletion() {
    let r = Rig::new();
    let (service, _, _) = services(&r).await;
    service
        .save_daily("a".into(), vec![daily()], true)
        .await
        .unwrap();
    let backup = r._dir.path().join("backup.db");
    sql_query("VACUUM INTO ?")
        .bind::<Text, _>(backup.to_string_lossy().as_ref())
        .execute(&mut r.pool.get().unwrap())
        .unwrap();
    let db = crate::db::DbAccess::plaintext(backup.to_string_lossy());
    db.run_migrations().unwrap();
    let pool = db.create_pool().unwrap();
    let writer = crate::db::write_actor::spawn_writer(pool.as_ref().clone()).unwrap();
    let restored = StatementStore::new(pool, writer);
    assert_eq!(
        restored.list_daily("a").unwrap()[0].interval,
        daily().interval
    );
    let stored = r.store.list("a").unwrap();
    r.service
        .delete(
            "a".into(),
            stored[0].snapshot.date,
            stored[0].revision.clone(),
        )
        .await
        .unwrap();
    assert!(r.store.list_daily("a").unwrap().is_empty());
    assert_eq!(restored.list_daily("a").unwrap().len(), 1);
    sql_query("DELETE FROM accounts WHERE id='a'")
        .execute(&mut r.pool.get().unwrap())
        .unwrap();
    assert!(r.store.list_daily("a").unwrap().is_empty());
}
#[tokio::test]
async fn daily_missing_prices_do_not_publish_partial_values_or_change_unknown_basis() {
    let r = Rig::new();
    let (service, _, quotes) = services(&r).await;
    let preview = service
        .preview_daily("a".into(), vec![daily()])
        .await
        .unwrap();
    assert!(preview[0].priced.days.iter().all(|d| d.value.is_none()));
    service
        .save_daily("a".into(), vec![daily()], true)
        .await
        .unwrap();
    assert!(service
        .history("a".into(), day("2025-05-02"), day("2025-05-04"))
        .await
        .unwrap()
        .is_empty());
    // Explicit refresh of a MANUAL asset uses local quotes, never a provider.
    assert_eq!(
        service
            .refresh_daily_quotes("a".into(), vec![daily()])
            .await
            .unwrap(),
        0
    );
    seed(quotes.as_ref()).await;
    assert_eq!(
        service
            .history("a".into(), day("2025-05-02"), day("2025-05-04"))
            .await
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        r.store.list_daily("a").unwrap()[0]
            .interval
            .closing_positions[0]
            .cost_basis,
        None
    );
}
#[tokio::test]
async fn daily_scope_rejects_overlaps_incomplete_checkpoints_and_future_dates() {
    let r = Rig::new();
    let (service, _, _) = services(&r).await;
    for mutate in [0, 1, 2, 3] {
        let mut w = daily();
        match mutate {
            0 => w.interval.activity_complete = false,
            1 => w.snapshot.total_value = "115".into(),
            2 => w.interval.closing_positions[0].quantity = "3".into(),
            _ => {
                w.snapshot.date = day("9999-05-05");
                w.interval.from = day("9999-05-02");
                w.interval.through = w.snapshot.date;
                w.interval.events[0].date = w.interval.from;
            }
        }
        assert!(service.save_daily("a".into(), vec![w], true).await.is_err());
    }
    service
        .save_daily("a".into(), vec![daily()], true)
        .await
        .unwrap();
    let mut overlap = daily();
    overlap.interval.from = day("2025-05-03");
    overlap.interval.through = day("2025-05-06");
    overlap.interval.events[0].date = day("2025-05-03");
    overlap.snapshot.date = day("2025-05-06");
    assert!(service
        .save_daily("a".into(), vec![overlap], true)
        .await
        .is_err());
}

fn live_value(date: NaiveDate) -> wealthfolio_core::portfolio::valuation::DailyAccountValuation {
    use wealthfolio_core::portfolio::{
        economic_events::BasisStatus,
        valuation::{DailyAccountValuation, ExternalFlowSource, ValuationStatus},
    };
    DailyAccountValuation {
        id: format!("live-{date}"),
        account_id: "a".into(),
        valuation_date: date,
        account_currency: "USD".into(),
        base_currency: "USD".into(),
        fx_rate_to_base: Decimal::ONE,
        cash_balance: Decimal::from(90),
        investment_market_value: Decimal::from(24),
        total_value: Decimal::from(114),
        cash_balance_base: Decimal::from(90),
        investment_market_value_base: Decimal::from(24),
        total_value_base: Decimal::from(114),
        cost_basis: Decimal::ZERO,
        book_basis: Decimal::ZERO,
        net_contribution: Decimal::ZERO,
        cost_basis_base: Decimal::ZERO,
        book_basis_base: Decimal::ZERO,
        net_contribution_base: Decimal::ZERO,
        external_inflow_base: Decimal::ZERO,
        external_outflow_base: Decimal::ZERO,
        external_flow_source: ExternalFlowSource::NoFlow,
        performance_eligible_value_base: Decimal::ZERO,
        value_status: ValuationStatus::Complete,
        basis_status: BasisStatus::Unknown,
        calculated_at: chrono::Utc::now(),
    }
}
#[tokio::test]
async fn reviewed_activity_pages_preserve_fills_and_stop_at_live_coverage() {
    let r = Rig::new();
    let (service, _, _) = services(&r).await;
    let mut write = daily();
    for line in 1..=52 {
        write.interval.events.push(StatementEvent {
            source_id: format!("cash-{line}"),
            date: day("2025-05-03"),
            kind: StatementEventKind::Cash,
            asset_id: None,
            quantity: None,
            cash: if line % 2 == 0 { "1" } else { "-1" }.into(),
            source_page: 3,
            source_line: line,
        });
    }
    service
        .save_daily("a".into(), vec![write], true)
        .await
        .unwrap();
    let first = service.list_activity("a".into(), 0).await.unwrap();
    assert_eq!(first.rows.len(), 50);
    assert_eq!(first.next_offset, Some(50));
    assert_eq!(first.rows[0].event.source_id, "cash-1");
    assert_eq!(first.rows[2].event.source_id, "cash-3");
    assert_eq!(first.rows[49].event.source_id, "cash-50");
    let second = service.list_activity("a".into(), 50).await.unwrap();
    assert_eq!(second.rows.len(), 3);
    assert_eq!(second.next_offset, None);
    assert_eq!(second.rows[0].event.source_id, "cash-51");
    assert_eq!(second.rows[2].asset_label.as_deref(), Some("DEMO"));
    assert_eq!(second.rows[2].statement_date, day("2025-05-05"));

    let valuations =
        crate::portfolio::valuation::ValuationRepository::new(r.pool.clone(), r.writer.clone());
    let mut cutoff = live_value(day("2025-05-03"));
    cutoff.value_status = wealthfolio_core::portfolio::valuation::ValuationStatus::Unavailable;
    valuations
        .replace_valuations_for_account("a", None, &[cutoff])
        .await
        .unwrap();
    let page = service.list_activity("a".into(), 0).await.unwrap();
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0].event.date, day("2025-05-02"));
    assert!(service
        .list_activity("b".into(), 0)
        .await
        .unwrap()
        .rows
        .is_empty());

    let snapshot = r.store.list("a").unwrap().remove(0);
    service
        .delete("a".into(), snapshot.snapshot.date, snapshot.revision)
        .await
        .unwrap();
    assert!(service
        .list_activity("a".into(), 0)
        .await
        .unwrap()
        .rows
        .is_empty());
}

#[tokio::test]
async fn daily_source_never_enters_live_gaps_and_reports_handoff_mismatch_without_repairing_ledger()
{
    let r = Rig::new();
    let (service, net, quotes) = services(&r).await;
    seed(quotes.as_ref()).await;
    let valuations =
        crate::portfolio::valuation::ValuationRepository::new(r.pool.clone(), r.writer.clone());
    let live = live_value(day("2025-05-04"));
    valuations
        .replace_valuations_for_account("a", None, std::slice::from_ref(&live))
        .await
        .unwrap();
    let stored_live = valuations
        .get_historical_valuations("a", None, None)
        .unwrap();
    let snapshots =
        crate::portfolio::snapshot::SnapshotRepository::new(r.pool.clone(), r.writer.clone());
    let mut snapshot = AccountStateSnapshot {
        id: "live".into(),
        account_id: "a".into(),
        snapshot_date: day("2025-05-04"),
        currency: "USD".into(),
        cash_total_account_currency: Decimal::from(90),
        ..Default::default()
    };
    snapshot.positions.insert(
        "demo".into(),
        Position {
            asset_id: "demo".into(),
            quantity: Decimal::from(3),
            ..Default::default()
        },
    );
    snapshots
        .save_snapshots(std::slice::from_ref(&snapshot))
        .await
        .unwrap();
    assert_eq!(
        service
            .preview_daily("a".into(), vec![daily()])
            .await
            .unwrap()[0]
            .live_handoff,
        LiveHandoff::Mismatch
    );
    service
        .save_daily("a".into(), vec![daily()], true)
        .await
        .unwrap();
    let prefix = service
        .history("a".into(), day("2025-05-02"), day("2025-05-10"))
        .await
        .unwrap();
    assert_eq!(prefix.len(), 2);
    assert!(prefix.iter().all(|v| v.date < day("2025-05-04")));
    let history = net
        .get_net_worth_history(day("2025-05-02"), day("2025-05-06"))
        .await
        .unwrap();
    assert!(history
        .iter()
        .filter(|v| v.date >= day("2025-05-04"))
        .all(|v| v.statement_sources.is_empty()));
    assert_eq!(
        valuations
            .get_historical_valuations("a", None, None)
            .unwrap(),
        stored_live
    );
    snapshot.positions.get_mut("demo").unwrap().quantity = Decimal::from(2);
    snapshots.save_snapshots(&[snapshot]).await.unwrap();
    assert_eq!(
        service.list_daily("a".into()).await.unwrap()[0].live_handoff,
        LiveHandoff::Match
    );
    let mut unpriced = stored_live[0].clone();
    unpriced.id = "earliest-unpriced".into();
    unpriced.valuation_date = day("2025-05-03");
    unpriced.value_status = wealthfolio_core::portfolio::valuation::ValuationStatus::Unavailable;
    let mut live_rows = stored_live;
    live_rows.push(unpriced);
    valuations
        .replace_valuations_for_account("a", None, &live_rows)
        .await
        .unwrap();
    assert_eq!(
        service
            .history("a".into(), day("2025-05-02"), day("2025-05-10"))
            .await
            .unwrap()
            .len(),
        1
    );
}
#[tokio::test]
async fn daily_adjacent_months_require_cash_and_quantity_continuity_but_gaps_are_not_invented() {
    let r = Rig::new();
    let (service, _, _) = services(&r).await;
    service
        .save_daily("a".into(), vec![daily()], true)
        .await
        .unwrap();
    let mut next = daily();
    next.interval.from = day("2025-05-06");
    next.interval.through = day("2025-05-08");
    next.snapshot.date = day("2025-05-08");
    next.interval.events.clear();
    next.interval.opening_cash = "90".into();
    service
        .save_daily("a".into(), vec![next.clone()], true)
        .await
        .unwrap();
    let mut changed = next.clone();
    changed.interval.opening_cash = "91".into();
    changed.interval.closing_cash = "91".into();
    changed.interval.closing_value = "115".into();
    changed.snapshot.cash_value = Some("91".into());
    changed.snapshot.total_value = "115".into();
    assert!(service
        .preview_daily("a".into(), vec![changed])
        .await
        .is_err());
    let mut gap = next;
    gap.interval.from = day("2025-06-01");
    gap.interval.through = day("2025-06-03");
    gap.snapshot.date = day("2025-06-03");
    gap.interval.opening_cash = "91".into();
    gap.interval.closing_cash = "91".into();
    gap.interval.closing_value = "115".into();
    gap.snapshot.cash_value = Some("91".into());
    gap.snapshot.total_value = "115".into();
    service
        .save_daily("a".into(), vec![gap], true)
        .await
        .unwrap();
    assert_eq!(r.store.list_daily("a").unwrap().len(), 3);
    assert_eq!(
        r.store.list_daily("a").unwrap()[2].interval.from,
        day("2025-06-01")
    );
}

#[tokio::test]
async fn daily_fx_uses_each_valuation_day_and_never_looks_ahead_to_the_closing_rate() {
    use wealthfolio_core::{
        fx::{ExchangeRate, FxService},
        portfolio::statements::daily::{history_values, PricingSources},
    };
    let r = Rig::new();
    let (service, _, quotes) = services(&r).await;
    seed(quotes.as_ref()).await;
    service
        .save_daily("a".into(), vec![daily()], true)
        .await
        .unwrap();
    let repo = Arc::new(crate::fx::FxRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let fx = FxService::new(repo.clone());
    let pricing = PricingSources {
        assets: Arc::new(crate::assets::AssetRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        )),
        quotes,
    };
    assert!(history_values(
        r.store.as_ref(),
        &r.store.list("a").unwrap(),
        Some(&pricing),
        "a",
        day("2025-05-02"),
        day("2025-05-05"),
        None,
        "EUR",
        Some(&fx)
    )
    .is_err());
    let id = repo.create_fx_asset("USD", "EUR", "MANUAL").await.unwrap();
    for (date, rate) in [("2025-05-03", 2), ("2025-05-05", 3)] {
        repo.save_exchange_rate(ExchangeRate {
            id: id.clone(),
            from_currency: "USD".into(),
            to_currency: "EUR".into(),
            rate: Decimal::from(rate),
            source: "MANUAL".into(),
            timestamp: day(date).and_hms_opt(0, 0, 0).unwrap().and_utc(),
        })
        .await
        .unwrap();
    }
    wealthfolio_core::fx::FxServiceTrait::initialize(&fx).unwrap();
    let values = history_values(
        r.store.as_ref(),
        &r.store.list("a").unwrap(),
        Some(&pricing),
        "a",
        day("2025-05-02"),
        day("2025-05-05"),
        None,
        "EUR",
        Some(&fx),
    )
    .unwrap();
    assert_eq!(
        values
            .iter()
            .map(|v| (v.date, v.source.as_str(), amount(&v.value).unwrap()))
            .collect::<Vec<_>>(),
        vec![
            (day("2025-05-03"), "statementDaily", Decimal::from(220)),
            (day("2025-05-04"), "statementDaily", Decimal::from(220)),
            (day("2025-05-05"), "statementClosing", Decimal::from(342)),
        ]
    );
    assert_eq!(
        values
            .iter()
            .map(|v| amount(v.cash_value.as_deref().unwrap()).unwrap())
            .collect::<Vec<_>>(),
        vec![Decimal::from(180), Decimal::from(180), Decimal::from(270)]
    );
}

#[tokio::test]
async fn daily_groups_reviewed_security_rows_without_discarding_unknown_basis_fractions() {
    let r = Rig::new();
    let (service, _, quotes) = services(&r).await;
    seed(quotes.as_ref()).await;
    let mut w = daily();
    w.closing_asset_ids = vec!["demo".into(), "demo".into()];
    let mut fraction = w.snapshot.positions[0].clone();
    fraction.quantity = "0.0001".into();
    fraction.market_value = Some("0.0012".into());
    fraction.cost_basis = None;
    w.snapshot.positions[0].quantity = "1.9999".into();
    w.snapshot.positions[0].market_value = Some("23.9988".into());
    w.snapshot.positions[0].cost_basis = Some("10".into());
    w.snapshot.positions.push(fraction);
    let preview = service
        .preview_daily("a".into(), vec![w.clone()])
        .await
        .unwrap();
    assert_eq!(
        preview[0].priced.days.last().unwrap().value.as_deref(),
        Some("114")
    );
    service
        .save_daily("a".into(), vec![w.clone()], true)
        .await
        .unwrap();
    let stored = r.store.list_daily("a").unwrap();
    assert_eq!(stored[0].closing_asset_ids, w.closing_asset_ids);
    assert_eq!(stored[0].interval.closing_positions[0].quantity, "2");
    assert_eq!(stored[0].interval.closing_positions[0].cost_basis, None);
    service
        .save_daily("a".into(), vec![w.clone()], true)
        .await
        .unwrap();
    assert_eq!(
        r.store.list_daily("a").unwrap()[0].revision,
        stored[0].revision
    );
    w.closing_asset_ids[1] = "different".into();
    assert!(service.preview_daily("a".into(), vec![w]).await.is_err());
}
