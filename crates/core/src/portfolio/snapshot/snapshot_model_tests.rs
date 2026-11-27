//! Tests for snapshot domain models including SnapshotSource.

#[cfg(test)]
mod tests {
    use crate::portfolio::snapshot::SnapshotSource;

    // ==================== SnapshotSource Serialization Tests ====================

    #[test]
    fn test_snapshot_source_serialization() {
        assert_eq!(
            serde_json::to_string(&SnapshotSource::Calculated).unwrap(),
            "\"CALCULATED\""
        );
        assert_eq!(
            serde_json::to_string(&SnapshotSource::ManualEntry).unwrap(),
            "\"MANUAL_ENTRY\""
        );
        assert_eq!(
            serde_json::to_string(&SnapshotSource::BrokerImported).unwrap(),
            "\"BROKER_IMPORTED\""
        );
        assert_eq!(
            serde_json::to_string(&SnapshotSource::CsvImport).unwrap(),
            "\"CSV_IMPORT\""
        );
    }

    #[test]
    fn test_snapshot_source_deserialization() {
        assert_eq!(
            serde_json::from_str::<SnapshotSource>("\"CALCULATED\"").unwrap(),
            SnapshotSource::Calculated
        );
        assert_eq!(
            serde_json::from_str::<SnapshotSource>("\"MANUAL_ENTRY\"").unwrap(),
            SnapshotSource::ManualEntry
        );
        assert_eq!(
            serde_json::from_str::<SnapshotSource>("\"BROKER_IMPORTED\"").unwrap(),
            SnapshotSource::BrokerImported
        );
        assert_eq!(
            serde_json::from_str::<SnapshotSource>("\"CSV_IMPORT\"").unwrap(),
            SnapshotSource::CsvImport
        );
    }

    #[test]
    fn test_snapshot_source_default() {
        let source = SnapshotSource::default();
        assert_eq!(source, SnapshotSource::Calculated);
    }

    // ==================== AccountStateSnapshot Source Field Tests ====================

    #[test]
    fn test_account_state_snapshot_default_source() {
        use crate::portfolio::snapshot::AccountStateSnapshot;

        let snapshot = AccountStateSnapshot::default();
        assert_eq!(snapshot.source, SnapshotSource::Calculated);
    }

    #[test]
    fn test_account_state_snapshot_serialization_with_source() {
        use crate::portfolio::snapshot::AccountStateSnapshot;
        use chrono::{NaiveDate, Utc};

        let snapshot = AccountStateSnapshot {
            id: "test-id".to_string(),
            account_id: "account-1".to_string(),
            snapshot_date: NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
            currency: "USD".to_string(),
            base_currency: None,
            source: SnapshotSource::ManualEntry,
            calculated_at: Utc::now().naive_utc(),
            ..Default::default()
        };

        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("\"source\":\"MANUAL_ENTRY\""));
    }

    #[test]
    fn test_account_state_snapshot_deserialization_with_source() {
        use crate::portfolio::snapshot::AccountStateSnapshot;

        let json = r#"{
            "id": "test-id",
            "accountId": "account-1",
            "snapshotDate": "2024-01-15",
            "currency": "USD",
            "positions": {},
            "cashBalances": {},
            "costBasis": "0",
            "netContribution": "0",
            "netContributionBase": "0",
            "cashTotalAccountCurrency": "0",
            "cashTotalBaseCurrency": "0",
            "calculatedAt": "2024-01-15T10:00:00",
            "source": "BROKER_IMPORTED"
        }"#;

        let snapshot: AccountStateSnapshot = serde_json::from_str(json).unwrap();
        assert_eq!(snapshot.source, SnapshotSource::BrokerImported);
    }

    #[test]
    fn test_account_state_snapshot_deserialization_missing_source() {
        use crate::portfolio::snapshot::AccountStateSnapshot;

        // Test that missing source field defaults to Calculated
        let json = r#"{
            "id": "test-id",
            "accountId": "account-1",
            "snapshotDate": "2024-01-15",
            "currency": "USD",
            "positions": {},
            "cashBalances": {},
            "costBasis": "0",
            "netContribution": "0",
            "netContributionBase": "0",
            "cashTotalAccountCurrency": "0",
            "cashTotalBaseCurrency": "0",
            "calculatedAt": "2024-01-15T10:00:00"
        }"#;

        let snapshot: AccountStateSnapshot = serde_json::from_str(json).unwrap();
        assert_eq!(snapshot.source, SnapshotSource::Calculated);
    }

    #[test]
    fn test_snapshot_stable_id_is_deterministic_uuid() {
        use crate::portfolio::snapshot::AccountStateSnapshot;
        use chrono::NaiveDate;

        let day = NaiveDate::from_ymd_opt(2026, 2, 18).unwrap();
        let id1 = AccountStateSnapshot::stable_id("acc-123", day);
        let id2 = AccountStateSnapshot::stable_id("acc-123", day);
        let id3 = AccountStateSnapshot::stable_id("acc-123", day.succ_opt().unwrap());

        assert_eq!(id1, id2, "stable id must be deterministic");
        assert_ne!(id1, id3, "snapshot date should affect stable id");
        assert!(
            uuid::Uuid::parse_str(&id1).is_ok(),
            "stable id must be valid UUID"
        );
    }

    // ==================== is_content_equal Tests ====================

    #[test]
    fn test_is_content_equal_identical_empty_snapshots() {
        use crate::portfolio::snapshot::AccountStateSnapshot;

        let snapshot1 = AccountStateSnapshot::default();
        let snapshot2 = AccountStateSnapshot::default();

        assert!(snapshot1.is_content_equal(&snapshot2));
    }

    #[test]
    fn test_is_content_equal_different_metadata_same_content() {
        use crate::portfolio::snapshot::AccountStateSnapshot;
        use chrono::{NaiveDate, Utc};

        let snapshot1 = AccountStateSnapshot {
            id: "snapshot-1".to_string(),
            account_id: "account-1".to_string(),
            snapshot_date: NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
            base_currency: None,
            source: SnapshotSource::BrokerImported,
            calculated_at: Utc::now().naive_utc(),
            ..Default::default()
        };

        let snapshot2 = AccountStateSnapshot {
            id: "snapshot-2".to_string(), // Different ID
            account_id: "account-1".to_string(),
            snapshot_date: NaiveDate::from_ymd_opt(2024, 2, 20).unwrap(), // Different date
            base_currency: None,
            source: SnapshotSource::ManualEntry, // Different source
            calculated_at: Utc::now().naive_utc(),
            ..Default::default()
        };

        // Should be equal because positions and cash_balances are the same (empty)
        assert!(snapshot1.is_content_equal(&snapshot2));
    }

    #[test]
    fn test_is_content_equal_different_cash_balances() {
        use crate::portfolio::snapshot::AccountStateSnapshot;
        use rust_decimal::Decimal;
        use std::collections::HashMap;

        let mut snapshot1 = AccountStateSnapshot::default();
        let mut cash1 = HashMap::new();
        cash1.insert("USD".to_string(), Decimal::from(1000));
        snapshot1.cash_balances = cash1;

        let mut snapshot2 = AccountStateSnapshot::default();
        let mut cash2 = HashMap::new();
        cash2.insert("USD".to_string(), Decimal::from(2000)); // Different amount
        snapshot2.cash_balances = cash2;

        assert!(!snapshot1.is_content_equal(&snapshot2));
    }

    #[test]
    fn test_is_content_equal_different_positions() {
        use crate::portfolio::snapshot::{AccountStateSnapshot, Position};
        use chrono::Utc;
        use rust_decimal::Decimal;
        use std::collections::HashMap;

        let now = Utc::now();

        let mut snapshot1 = AccountStateSnapshot::default();
        let mut positions1 = HashMap::new();
        positions1.insert(
            "AAPL".to_string(),
            Position {
                cost_basis_status: None,
                id: "pos-1".to_string(),
                account_id: "account-1".to_string(),
                asset_id: "AAPL".to_string(),
                quantity: Decimal::from(100),
                average_cost: Decimal::from(150),
                total_cost_basis: Decimal::from(15000),
                currency: "USD".to_string(),
                inception_date: now,
                lots: Default::default(),
                created_at: now,
                last_updated: now,
                is_alternative: false,
                contract_multiplier: Decimal::ONE,
                cost_basis_account: None,
                cost_basis_base: None,
            },
        );
        snapshot1.positions = positions1;

        let mut snapshot2 = AccountStateSnapshot::default();
        let mut positions2 = HashMap::new();
        positions2.insert(
            "AAPL".to_string(),
            Position {
                cost_basis_status: None,
                id: "pos-2".to_string(), // Different ID (should be ignored)
                account_id: "account-1".to_string(),
                asset_id: "AAPL".to_string(),
                quantity: Decimal::from(200), // Different quantity
                average_cost: Decimal::from(150),
                total_cost_basis: Decimal::from(30000), // Different cost basis
                currency: "USD".to_string(),
                inception_date: now,
                lots: Default::default(),
                created_at: now,
                last_updated: now,
                is_alternative: false,
                contract_multiplier: Decimal::ONE,
                cost_basis_account: None,
                cost_basis_base: None,
            },
        );
        snapshot2.positions = positions2;

        assert!(!snapshot1.is_content_equal(&snapshot2));
    }

    #[test]
    fn test_is_content_equal_same_positions_different_metadata() {
        use crate::portfolio::snapshot::{AccountStateSnapshot, Position};
        use chrono::Utc;
        use rust_decimal::Decimal;
        use std::collections::HashMap;

        let now = Utc::now();

        let mut snapshot1 = AccountStateSnapshot::default();
        let mut positions1 = HashMap::new();
        positions1.insert(
            "AAPL".to_string(),
            Position {
                cost_basis_status: None,
                id: "pos-1".to_string(),
                account_id: "account-1".to_string(),
                asset_id: "AAPL".to_string(),
                quantity: Decimal::from(100),
                average_cost: Decimal::from(150),
                total_cost_basis: Decimal::from(15000),
                currency: "USD".to_string(),
                inception_date: now,
                lots: Default::default(),
                created_at: now,
                last_updated: now,
                is_alternative: false,
                contract_multiplier: Decimal::ONE,
                cost_basis_account: None,
                cost_basis_base: None,
            },
        );
        snapshot1.positions = positions1;

        let mut snapshot2 = AccountStateSnapshot::default();
        let mut positions2 = HashMap::new();
        positions2.insert(
            "AAPL".to_string(),
            Position {
                cost_basis_status: None,
                id: "pos-2".to_string(), // Different ID (should be ignored)
                account_id: "account-1".to_string(),
                asset_id: "AAPL".to_string(),
                quantity: Decimal::from(100),           // Same quantity
                average_cost: Decimal::from(150),       // Same avg cost
                total_cost_basis: Decimal::from(15000), // Same cost basis
                currency: "USD".to_string(),
                inception_date: Utc::now(), // Different timestamp (should be ignored)
                lots: Default::default(),
                created_at: Utc::now(), // Different timestamp (should be ignored)
                last_updated: Utc::now(), // Different timestamp (should be ignored)
                is_alternative: false,
                contract_multiplier: Decimal::ONE,
                cost_basis_account: None,
                cost_basis_base: None,
            },
        );
        snapshot2.positions = positions2;

        // Should be equal because core financial fields match
        assert!(snapshot1.is_content_equal(&snapshot2));
    }

    #[test]
    fn test_is_content_equal_detects_contract_multiplier_change() {
        use crate::portfolio::snapshot::{AccountStateSnapshot, Position};
        use chrono::Utc;
        use rust_decimal::Decimal;
        use std::collections::HashMap;

        let now = Utc::now();
        let position = Position {
            cost_basis_status: None,
            id: "pos-1".to_string(),
            account_id: "account-1".to_string(),
            asset_id: "contract-1".to_string(),
            quantity: Decimal::from(2),
            average_cost: Decimal::from(500),
            total_cost_basis: Decimal::from(1000),
            currency: "USD".to_string(),
            inception_date: now,
            lots: Default::default(),
            created_at: now,
            last_updated: now,
            is_alternative: false,
            contract_multiplier: Decimal::ONE,
            cost_basis_account: None,
            cost_basis_base: None,
        };

        let mut first = AccountStateSnapshot::default();
        first
            .positions
            .insert(position.asset_id.clone(), position.clone());

        let mut second_position = position;
        second_position.contract_multiplier = Decimal::from(50);
        let second = AccountStateSnapshot {
            positions: HashMap::from([(second_position.asset_id.clone(), second_position)]),
            ..Default::default()
        };

        assert!(!first.is_content_equal(&second));
    }

    #[test]
    fn test_is_content_equal_different_position_count() {
        use crate::portfolio::snapshot::{AccountStateSnapshot, Position};
        use chrono::Utc;
        use rust_decimal::Decimal;
        use std::collections::HashMap;

        let now = Utc::now();

        let mut snapshot1 = AccountStateSnapshot::default();
        let mut positions1 = HashMap::new();
        positions1.insert(
            "AAPL".to_string(),
            Position {
                cost_basis_status: None,
                id: "pos-1".to_string(),
                account_id: "account-1".to_string(),
                asset_id: "AAPL".to_string(),
                quantity: Decimal::from(100),
                average_cost: Decimal::from(150),
                total_cost_basis: Decimal::from(15000),
                currency: "USD".to_string(),
                inception_date: now,
                lots: Default::default(),
                created_at: now,
                last_updated: now,
                is_alternative: false,
                contract_multiplier: Decimal::ONE,
                cost_basis_account: None,
                cost_basis_base: None,
            },
        );
        snapshot1.positions = positions1;

        // snapshot2 has no positions
        let snapshot2 = AccountStateSnapshot::default();

        assert!(!snapshot1.is_content_equal(&snapshot2));
    }
    #[test]
    fn plaid_basis_knowledge_changes_are_material_even_when_numeric_basis_is_zero() {
        use crate::portfolio::economic_events::BasisStatus;
        use crate::portfolio::snapshot::{AccountStateSnapshot, Position};
        use chrono::Utc;
        use rust_decimal::Decimal;
        let mut p = Position::new("a".into(), "stock".into(), "USD".into(), Utc::now());
        p.quantity = Decimal::ONE;
        p.cost_basis_status = Some(BasisStatus::Unknown);
        let first = AccountStateSnapshot {
            positions: std::collections::HashMap::from([("stock".into(), p)]),
            ..Default::default()
        };
        let mut second = first.clone();
        second.positions.get_mut("stock").unwrap().cost_basis_status = Some(BasisStatus::Complete);
        assert!(!first.is_content_equal(&second));
        assert!(first.is_content_equal(&first.clone()));
    }

    #[test]
    fn default_unit_price_fills_unknown_observed_basis_and_preserves_explicit_basis() {
        use crate::assets::{Asset, AssetKind, QuoteMode};
        use crate::portfolio::economic_events::BasisStatus;
        use crate::portfolio::snapshot::{
            apply_default_unit_price_basis_to_snapshot, AccountStateSnapshot, Position,
            SnapshotSource,
        };
        use chrono::Utc;
        use rust_decimal::Decimal;
        use rust_decimal_macros::dec;
        use serde_json::json;
        use std::collections::HashMap;

        let now = Utc::now();
        let make_asset = |id: &str, ccy: &str, price: Option<Decimal>| Asset {
            id: id.into(),
            kind: AssetKind::Investment,
            quote_mode: QuoteMode::Manual,
            quote_ccy: ccy.into(),
            metadata: price.map(|p| json!({ "defaultUnitPrice": p.to_string() })),
            ..Default::default()
        };

        let assets = HashMap::from([
            (
                "deposit".into(),
                make_asset("deposit", "USD", Some(dec!(1))),
            ),
            (
                "explicit".into(),
                make_asset("explicit", "USD", Some(dec!(1))),
            ),
            (
                "fx_mismatch".into(),
                make_asset("fx_mismatch", "CAD", Some(dec!(1))),
            ),
            ("pence".into(), make_asset("pence", "GBp", Some(dec!(100)))),
        ]);

        let mut p_unknown = Position::new("a".into(), "deposit".into(), "USD".into(), now);
        p_unknown.quantity = dec!(250.50);
        p_unknown.cost_basis_status = Some(BasisStatus::Unknown);

        let mut p_explicit = Position::new("a".into(), "explicit".into(), "USD".into(), now);
        p_explicit.quantity = dec!(10);
        p_explicit.average_cost = dec!(1.25);
        p_explicit.total_cost_basis = dec!(12.50);
        p_explicit.cost_basis_status = Some(BasisStatus::Complete);

        let mut p_mismatch = Position::new("a".into(), "fx_mismatch".into(), "USD".into(), now);
        p_mismatch.quantity = dec!(50);
        p_mismatch.cost_basis_status = Some(BasisStatus::Unknown);

        let mut p_gbp = Position::new("a".into(), "pence".into(), "GBP".into(), now);
        p_gbp.quantity = dec!(40);
        p_gbp.cost_basis_status = Some(BasisStatus::Unknown);

        let mut p_eur_with_acct_basis =
            Position::new("a".into(), "eur_eq".into(), "EUR".into(), now);
        p_eur_with_acct_basis.quantity = dec!(5);
        p_eur_with_acct_basis.average_cost = dec!(20);
        p_eur_with_acct_basis.total_cost_basis = dec!(100);
        p_eur_with_acct_basis.cost_basis_account = Some(dec!(110));
        p_eur_with_acct_basis.cost_basis_status = Some(BasisStatus::Complete);

        let mut snap = AccountStateSnapshot {
            currency: "USD".into(),
            base_currency: Some("USD".into()),
            source: SnapshotSource::BrokerImported,
            positions: HashMap::from([
                ("deposit".into(), p_unknown),
                ("explicit".into(), p_explicit),
                ("fx_mismatch".into(), p_mismatch),
                ("pence".into(), p_gbp),
                ("eur_eq".into(), p_eur_with_acct_basis),
            ]),
            ..Default::default()
        };

        assert!(apply_default_unit_price_basis_to_snapshot(
            &mut snap, &assets
        ));

        // Unknown USD sweep position filled at $1 par.
        let deposit = &snap.positions["deposit"];
        assert_eq!(deposit.basis_status(), BasisStatus::Complete);
        assert_eq!(deposit.average_cost, dec!(1));
        assert_eq!(deposit.total_cost_basis, dec!(250.50));
        assert_eq!(deposit.cost_basis_account, Some(dec!(250.50)));
        assert_eq!(deposit.cost_basis_base, Some(dec!(250.50)));

        // Explicit cost basis is never overwritten.
        let explicit = &snap.positions["explicit"];
        assert_eq!(explicit.average_cost, dec!(1.25));
        assert_eq!(explicit.total_cost_basis, dec!(12.50));

        // Cross-currency default price fails closed.
        assert_eq!(
            snap.positions["fx_mismatch"].basis_status(),
            BasisStatus::Unknown
        );

        // Minor-unit GBp 100 normalizes cleanly to GBP 1.00.
        let pence = &snap.positions["pence"];
        assert_eq!(pence.basis_status(), BasisStatus::Complete);
        assert_eq!(pence.average_cost, dec!(1));
        assert_eq!(pence.total_cost_basis, dec!(40));

        // Account-currency snapshot cost_basis sums USD complete positions plus any
        // cross-currency position's explicit cost_basis_account (250.50 + 12.50 + 110).
        assert_eq!(snap.cost_basis, dec!(373.00));

        // Calculated snapshots are untouched.
        let mut calc = AccountStateSnapshot {
            currency: "USD".into(),
            source: SnapshotSource::Calculated,
            positions: HashMap::from([(
                "deposit".into(),
                Position {
                    quantity: dec!(10),
                    cost_basis_status: Some(BasisStatus::Unknown),
                    ..Position::new("a".into(), "deposit".into(), "USD".into(), now)
                },
            )]),
            ..Default::default()
        };
        assert!(!apply_default_unit_price_basis_to_snapshot(
            &mut calc, &assets
        ));
        assert_eq!(
            calc.positions["deposit"].basis_status(),
            BasisStatus::Unknown
        );

        // Empty quote or position currencies fail closed.
        assert_eq!(
            crate::portfolio::snapshot::default_unit_price_in_currency(&assets["deposit"], ""),
            None
        );
        let empty_ccy_asset = make_asset("empty", "", Some(dec!(1)));
        assert_eq!(
            crate::portfolio::snapshot::default_unit_price_in_currency(&empty_ccy_asset, ""),
            None
        );
    }
}
