use chrono::NaiveDate;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::accounts::{
    merge_account_meta, validate_account_meta_tax, TaxTreatment, MEMBER_NAME_MAX_CHARS,
};

use super::{
    active_wash_sale_cooldown, build_tax_packet_export, classify_holding_period, csv_decimal_cell,
    csv_text_cell, days_until_long_term, detect_wash_sales, one_year_anniversary,
    AssetLocationBreakdown, AssetSplitEvent, ContributionLimitsSummary, DisposalKind, HoldingTerm,
    LossDisposalInput, RealizedGainRow, RealizedGainsSummary, ReplacementBuyInput, TaxExportFormat,
    TaxReportWarnings, TaxYearReport, TaxableIncomeSummary, WashSaleConfidence, WashSaleSeverity,
    TAX_ADVISORY_DISCLAIMER,
};

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).expect("valid test date")
}

#[test]
fn holding_period_calendar_anniversary_and_leap_day() {
    // Exact 1-year anniversary is still short-term; +1 day is long-term.
    assert_eq!(one_year_anniversary(d(2025, 3, 15)), d(2026, 3, 15));
    assert_eq!(
        classify_holding_period(Some(d(2025, 3, 15)), d(2026, 3, 15), false),
        (HoldingTerm::ShortTerm, Some(365))
    );
    assert_eq!(
        classify_holding_period(Some(d(2025, 3, 15)), d(2026, 3, 16), false),
        (HoldingTerm::LongTerm, Some(366))
    );

    // Leap day Feb 29, 2024 -> anniversary Feb 28, 2025 -> Mar 1, 2025 is long-term.
    assert_eq!(one_year_anniversary(d(2024, 2, 29)), d(2025, 2, 28));
    assert_eq!(
        classify_holding_period(Some(d(2024, 2, 29)), d(2025, 2, 28), false).0,
        HoldingTerm::ShortTerm
    );
    assert_eq!(
        classify_holding_period(Some(d(2024, 2, 29)), d(2025, 3, 1), false).0,
        HoldingTerm::LongTerm
    );

    // Short sales default to ShortTerm even when held > 1 year (IRC §1233).
    assert_eq!(
        classify_holding_period(Some(d(2024, 1, 1)), d(2026, 6, 1), true).0,
        HoldingTerm::ShortTerm
    );

    // Missing open date -> Unknown.
    assert_eq!(
        classify_holding_period(None, d(2026, 6, 1), false),
        (HoldingTerm::Unknown, None)
    );

    // Days until long-term.
    assert_eq!(
        days_until_long_term(d(2025, 3, 15), d(2026, 3, 15)),
        Some(1)
    );
    assert_eq!(days_until_long_term(d(2025, 3, 15), d(2026, 3, 16)), None);
}

#[test]
fn wash_sale_does_not_false_flag_simple_buy_and_sell_of_same_lot() {
    let losses = vec![LossDisposalInput {
        disposal_id: "disp-1".into(),
        lot_id: "lot-1".into(),
        open_activity_id: Some("buy-1".into()),
        open_date: Some(d(2026, 5, 1)),
        cost_basis_method: "FIFO".into(),
        disposal_activity_id: "sell-1".into(),
        account_id: "acc-alex".into(),
        account_name: "Alex Taxable".into(),
        owner_name: Some("Alex".into()),
        asset_id: "aapl".into(),
        symbol: "AAPL".into(),
        disposal_date: d(2026, 5, 10),
        disposed_quantity: dec!(100),
        realized_loss_base: Some(dec!(500)),
    }];
    let buys = vec![ReplacementBuyInput {
        activity_id: "buy-1".into(),
        account_id: "acc-alex".into(),
        account_name: "Alex Taxable".into(),
        owner_name: Some("Alex".into()),
        tax_treatment: Some(TaxTreatment::Taxable),
        is_ira: false,
        asset_id: "aapl".into(),
        buy_date: d(2026, 5, 1),
        quantity: dec!(100),
    }];

    let alerts = detect_wash_sales(&losses, &buys, &[]);
    assert!(alerts.is_empty());
}

#[test]
fn wash_sale_allocates_replacement_shares_one_to_one_without_double_counting() {
    // Single 100-share sell consuming Lot A (60) + Lot B (40), followed by one 50-share buy.
    let losses = vec![
        LossDisposalInput {
            disposal_id: "disp-a".into(),
            lot_id: "lot-a".into(),
            open_activity_id: Some("old-buy-a".into()),
            open_date: Some(d(2025, 1, 10)),
            cost_basis_method: "FIFO".into(),
            disposal_activity_id: "sell-1".into(),
            account_id: "acc-alex".into(),
            account_name: "Alex Taxable".into(),
            owner_name: Some("Alex".into()),
            asset_id: "vti".into(),
            symbol: "VTI".into(),
            disposal_date: d(2026, 6, 1),
            disposed_quantity: dec!(60),
            realized_loss_base: Some(dec!(600)),
        },
        LossDisposalInput {
            disposal_id: "disp-b".into(),
            lot_id: "lot-b".into(),
            open_activity_id: Some("old-buy-b".into()),
            open_date: Some(d(2025, 2, 10)),
            cost_basis_method: "FIFO".into(),
            disposal_activity_id: "sell-1".into(),
            account_id: "acc-alex".into(),
            account_name: "Alex Taxable".into(),
            owner_name: Some("Alex".into()),
            asset_id: "vti".into(),
            symbol: "VTI".into(),
            disposal_date: d(2026, 6, 1),
            disposed_quantity: dec!(40),
            realized_loss_base: Some(dec!(400)),
        },
    ];
    let buys = vec![ReplacementBuyInput {
        activity_id: "rep-buy".into(),
        account_id: "acc-spouse".into(),
        account_name: "Jordan Taxable".into(),
        owner_name: Some("Jordan".into()),
        tax_treatment: Some(TaxTreatment::Taxable),
        is_ira: false,
        asset_id: "vti".into(),
        buy_date: d(2026, 6, 15),
        quantity: dec!(50),
    }];

    let alerts = detect_wash_sales(&losses, &buys, &[]);
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].disposal_id, "disp-a");
    assert_eq!(alerts[0].matched_quantity, dec!(50));
    assert_eq!(alerts[0].estimated_disallowed_loss_base, Some(dec!(500.00)));
    assert_eq!(alerts[0].severity, WashSaleSeverity::CrossAccount);
    assert_eq!(alerts[0].confidence, WashSaleConfidence::High);
}

#[test]
fn wash_sale_requires_explicit_ira_designation_and_respects_30_day_boundary() {
    let losses = vec![LossDisposalInput {
        disposal_id: "disp-dec".into(),
        lot_id: "lot-1".into(),
        open_activity_id: Some("old-buy".into()),
        open_date: Some(d(2025, 3, 1)),
        cost_basis_method: "FIFO".into(),
        disposal_activity_id: "sell-dec".into(),
        account_id: "acc-taxable".into(),
        account_name: "Family Brokerage".into(),
        owner_name: Some("Alex".into()),
        asset_id: "spy".into(),
        symbol: "SPY".into(),
        disposal_date: d(2026, 12, 20),
        disposed_quantity: dec!(10),
        realized_loss_base: Some(dec!(1000)),
    }];

    // Day +31 is outside window -> no alert.
    let outside = vec![ReplacementBuyInput {
        activity_id: "buy-late".into(),
        account_id: "acc-ira".into(),
        account_name: "Jordan Roth IRA".into(),
        owner_name: Some("Jordan".into()),
        tax_treatment: Some(TaxTreatment::TaxFree),
        is_ira: true,
        asset_id: "spy".into(),
        buy_date: d(2027, 1, 20),
        quantity: dec!(10),
    }];
    assert!(detect_wash_sales(&losses, &outside, &[]).is_empty());

    // Tax-free TFSA without explicit IRA designation -> general CrossAccount + NeedsReview (NOT IRA permanent).
    let inside_tfsa = vec![ReplacementBuyInput {
        activity_id: "buy-tfsa".into(),
        account_id: "acc-tfsa".into(),
        account_name: "Jordan TFSA".into(),
        owner_name: Some("Jordan".into()),
        tax_treatment: Some(TaxTreatment::TaxFree),
        is_ira: false,
        asset_id: "spy".into(),
        buy_date: d(2027, 1, 10),
        quantity: dec!(10),
    }];
    let tfsa_alerts = detect_wash_sales(&losses, &inside_tfsa, &[]);
    assert_eq!(tfsa_alerts.len(), 1);
    assert_eq!(tfsa_alerts[0].severity, WashSaleSeverity::CrossAccount);
    assert_eq!(tfsa_alerts[0].confidence, WashSaleConfidence::NeedsReview);

    // Jan 10 (day +21 across year boundary) in explicitly designated Roth IRA -> permanent disallowance!
    let inside_ira = vec![ReplacementBuyInput {
        activity_id: "buy-jan".into(),
        account_id: "acc-ira".into(),
        account_name: "Jordan Roth IRA".into(),
        owner_name: Some("Jordan".into()),
        tax_treatment: Some(TaxTreatment::TaxFree),
        is_ira: true,
        asset_id: "spy".into(),
        buy_date: d(2027, 1, 10),
        quantity: dec!(10),
    }];
    let alerts = detect_wash_sales(&losses, &inside_ira, &[]);
    assert_eq!(alerts.len(), 1);
    assert_eq!(
        alerts[0].severity,
        WashSaleSeverity::IraPermanentDisallowance
    );
    assert_eq!(alerts[0].confidence, WashSaleConfidence::High);
    assert_eq!(
        alerts[0].estimated_disallowed_loss_base,
        Some(dec!(1000.00))
    );
}

#[test]
fn wash_sale_normalizes_split_and_flags_transferred_lot_as_needs_review() {
    // Sold 10 pre-split shares on Jun 1; 2:1 split on Jun 5; bought 10 post-split shares (= 5 pre-split) on Jun 10.
    let losses = vec![LossDisposalInput {
        disposal_id: "disp-split".into(),
        lot_id: "lot-split".into(),
        open_activity_id: None, // Transferred sub-lot
        open_date: Some(d(2026, 5, 20)),
        cost_basis_method: "FIFO".into(),
        disposal_activity_id: "sell-split".into(),
        account_id: "acc-1".into(),
        account_name: "Taxable".into(),
        owner_name: None,
        asset_id: "nvda".into(),
        symbol: "NVDA".into(),
        disposal_date: d(2026, 6, 1),
        disposed_quantity: dec!(10),
        realized_loss_base: Some(dec!(200)),
    }];
    let buys = vec![
        ReplacementBuyInput {
            activity_id: "buy-before".into(),
            account_id: "acc-1".into(),
            account_name: "Taxable".into(),
            owner_name: None,
            tax_treatment: Some(TaxTreatment::Taxable),
            is_ira: false,
            asset_id: "nvda".into(),
            buy_date: d(2026, 5, 20),
            quantity: dec!(4),
        },
        ReplacementBuyInput {
            activity_id: "buy-post-split".into(),
            account_id: "acc-1".into(),
            account_name: "Taxable".into(),
            owner_name: None,
            tax_treatment: Some(TaxTreatment::Taxable),
            is_ira: false,
            asset_id: "nvda".into(),
            buy_date: d(2026, 6, 10),
            quantity: dec!(10), // equals 5 pre-split shares
        },
    ];
    let splits = vec![AssetSplitEvent {
        asset_id: "nvda".into(),
        split_date: d(2026, 6, 5),
        ratio: dec!(2),
    }];

    let alerts = detect_wash_sales(&losses, &buys, &splits);
    assert_eq!(alerts.len(), 2);
    // Earlier buy on transferred lot -> NeedsReview, 4 shares.
    assert_eq!(alerts[0].confidence, WashSaleConfidence::NeedsReview);
    assert_eq!(alerts[0].matched_quantity, dec!(4));
    // Post-split buy -> High confidence, 10 post-split / 2 = 5 disposal-date shares.
    assert_eq!(alerts[1].confidence, WashSaleConfidence::High);
    assert_eq!(alerts[1].matched_quantity, dec!(5));
    assert_eq!(alerts[1].estimated_disallowed_loss_base, Some(dec!(100.00)));
}

#[test]
fn tlh_cooldown_detects_recent_household_buy() {
    let buys = vec![ReplacementBuyInput {
        activity_id: "b1".into(),
        account_id: "acc-ira".into(),
        account_name: "Spouse IRA".into(),
        owner_name: Some("Jordan".into()),
        tax_treatment: Some(TaxTreatment::TaxDeferred),
        is_ira: true,
        asset_id: "vxus".into(),
        buy_date: d(2026, 11, 5),
        quantity: dec!(20),
    }];

    assert_eq!(
        active_wash_sale_cooldown("vxus", d(2026, 11, 20), &buys),
        Some((d(2026, 12, 6), "Spouse IRA".into()))
    );
    assert_eq!(
        active_wash_sale_cooldown("vxus", d(2026, 12, 6), &buys),
        None
    );
}

#[test]
fn csv_export_uses_rfc4180_and_neutralizes_formula_injection_while_preserving_negative_decimals() {
    assert_eq!(csv_text_cell("=CMD()"), "'=CMD()");
    assert_eq!(csv_text_cell("  +SUM(A1)"), "'  +SUM(A1)");
    assert_eq!(
        csv_text_cell("He said \"hi\", now"),
        "\"He said \"\"hi\"\", now\""
    );
    assert_eq!(csv_decimal_cell(dec!(-123.450)), "-123.45");

    let report = TaxYearReport {
        tax_year: 2026,
        as_of_date: d(2026, 11, 20),
        base_currency: "USD".into(),
        member_filter: Some("Alex".into()),
        available_members: vec!["Alex".into()],
        disclaimer: TAX_ADVISORY_DISCLAIMER.into(),
        realized_gains: RealizedGainsSummary {
            disposals: vec![
                RealizedGainRow {
                    disposal_id: "d1".into(),
                    lot_id: "l1".into(),
                    disposal_activity_id: "a1".into(),
                    account_id: "acc1".into(),
                    account_name: "=Malicious, \"Account\"".into(),
                    owner_name: Some("Alex".into()),
                    tax_treatment: Some(TaxTreatment::Taxable),
                    asset_id: "aapl".into(),
                    symbol: "AAPL".into(),
                    asset_name: Some("Apple, Inc.".into()),
                    open_date: Some(d(2024, 1, 1)),
                    disposal_date: d(2026, 5, 1),
                    holding_days: Some(851),
                    term: HoldingTerm::LongTerm,
                    kind: DisposalKind::Sale,
                    is_short_sale: false,
                    quantity: dec!(10),
                    proceeds_base: Some(dec!(1500)),
                    cost_basis_base: Some(dec!(1700)),
                    realized_pnl_base: Some(dec!(-200)),
                    fx_missing: false,
                    currency: "USD".into(),
                    cost_basis_method: "FIFO".into(),
                    wash_sale_risk: None,
                    estimated_disallowed_loss_base: Some(Decimal::ZERO),
                },
                RealizedGainRow {
                    disposal_id: "d2".into(),
                    lot_id: "l2".into(),
                    disposal_activity_id: "a2".into(),
                    account_id: "acc2".into(),
                    account_name: "Euro Brokerage".into(),
                    owner_name: Some("Alex".into()),
                    tax_treatment: Some(TaxTreatment::Taxable),
                    asset_id: "sap".into(),
                    symbol: "SAP".into(),
                    asset_name: Some("SAP SE".into()),
                    open_date: Some(d(2025, 1, 1)),
                    disposal_date: d(2026, 6, 1),
                    holding_days: Some(516),
                    term: HoldingTerm::LongTerm,
                    kind: DisposalKind::Sale,
                    is_short_sale: false,
                    quantity: dec!(5),
                    proceeds_base: None,
                    cost_basis_base: None,
                    realized_pnl_base: None,
                    fx_missing: true,
                    currency: "EUR".into(),
                    cost_basis_method: "FIFO".into(),
                    wash_sale_risk: Some(WashSaleSeverity::SameAccount),
                    estimated_disallowed_loss_base: None,
                },
            ],
            ..Default::default()
        },
        wash_sales: Vec::new(),
        income: TaxableIncomeSummary::default(),
        tlh_candidates: Vec::new(),
        asset_location: AssetLocationBreakdown::default(),
        contribution_limits: ContributionLimitsSummary::default(),
        readiness: Vec::new(),
        warnings: TaxReportWarnings::default(),
    };

    let exported =
        build_tax_packet_export(&report, TaxExportFormat::RealizedGainsCsv).expect("csv export");
    assert_eq!(
        exported.filename,
        "wealthfolio-tax-2026-alex-realized-gains.csv"
    );
    assert!(exported
        .content
        .contains("\"'=Malicious, \"\"Account\"\"\""));
    assert!(exported.content.contains(",-200,"));
    // Missing FX row emits empty cells (not 0) for proceeds/basis/pnl/disallowed loss.
    assert!(
        exported.content.contains(",5,,,,USD,FIFO,SAME_ACCOUNT,\n")
            || exported.content.ends_with(",5,,,,USD,FIFO,SAME_ACCOUNT,")
    );
}

#[test]
fn account_meta_merge_preserves_siblings_and_validates_tax_fields() {
    let existing =
        r#"{"accounting":{"costBasisMethod":"LIFO"},"allocation":{"cashCategoryId":"cash-1"}}"#;
    let incoming = r#"{"tax":{"taxTreatment":"TAX_FREE","ownerName":"  Alex  ","accountCategory":"roth_ira"}}"#;

    assert!(validate_account_meta_tax(Some(incoming)).is_ok());
    let merged = merge_account_meta(Some(existing), Some(incoming)).expect("merged");
    let parsed: serde_json::Value = serde_json::from_str(&merged).expect("json");
    assert_eq!(parsed["accounting"]["costBasisMethod"], "LIFO");
    assert_eq!(parsed["allocation"]["cashCategoryId"], "cash-1");
    assert_eq!(parsed["tax"]["taxTreatment"], "TAX_FREE");
    assert_eq!(parsed["tax"]["ownerName"], "Alex");
    assert_eq!(parsed["tax"]["accountCategory"], "ROTH_IRA");

    // Explicit null clears only the targeted key while preserving siblings.
    let cleared = merge_account_meta(Some(&merged), Some(r#"{"tax":null}"#)).expect("cleared");
    let parsed_cleared: serde_json::Value = serde_json::from_str(&cleared).expect("json");
    assert!(parsed_cleared.get("tax").is_none());
    assert_eq!(parsed_cleared["accounting"]["costBasisMethod"], "LIFO");

    // Invalid taxTreatment, accountCategory, or oversized ownerName is rejected.
    assert!(validate_account_meta_tax(Some(r#"{"tax":{"taxTreatment":"BOGUS"}}"#)).is_err());
    assert!(validate_account_meta_tax(Some(r#"{"tax":{"accountCategory":"BOGUS"}}"#)).is_err());
    let long_name = "a".repeat(MEMBER_NAME_MAX_CHARS + 1);
    let oversized = format!(r#"{{"tax":{{"ownerName":"{long_name}"}}}}"#);
    assert!(validate_account_meta_tax(Some(&oversized)).is_err());
}
