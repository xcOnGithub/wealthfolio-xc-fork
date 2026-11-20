use super::*;
use crate::portfolio::statements::reconstruction::{
    ClosingPosition, StatementDatePolicy, StatementEvent, StatementEventKind,
};
fn date(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").expect("synthetic date")
}
fn decimal(s: &str) -> Decimal {
    s.parse().expect("synthetic amount")
}
fn asset(id: &str) -> Asset {
    Asset {
        id: id.into(),
        kind: AssetKind::Investment,
        quote_ccy: "USD".into(),
        instrument_type: Some(InstrumentType::Equity),
        ..Asset::default()
    }
}
fn quote(id: &str, day: &str, close: &str) -> Quote {
    Quote {
        asset_id: id.into(),
        timestamp: date(day)
            .and_hms_opt(16, 0, 0)
            .expect("synthetic timestamp")
            .and_utc(),
        close: decimal(close),
        adjclose: decimal("999"),
        currency: "USD".into(),
        data_source: "MANUAL".into(),
        ..Quote::default()
    }
}
fn input() -> StatementInterval {
    StatementInterval {
        date_policy: StatementDatePolicy::Settlement,
        from: date("2025-05-02"),
        through: date("2025-05-05"),
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
            source_id: "buy".into(),
            date: date("2025-05-02"),
            kind: StatementEventKind::Buy,
            asset_id: Some("demo".into()),
            quantity: Some("1".into()),
            cash: "-10".into(),
            source_page: 2,
            source_line: 10,
        }],
    }
}
#[test]
fn prices_every_historical_day_using_shared_engine_close_not_adjclose() {
    let p = price_statement(
        &input(),
        &[asset("demo")],
        &[
            quote("demo", "2025-05-02", "10"),
            quote("demo", "2025-05-05", "12"),
        ],
    )
    .expect("quoted statement");
    assert_eq!(
        p.days
            .iter()
            .map(|d| d.value.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("110"), Some("110"), Some("110"), Some("114")]
    );
    assert_eq!(p.days[0].date, date("2025-05-02"));
    assert_eq!(p.days[3].date, date("2025-05-05"));
    assert!(p
        .days
        .iter()
        .all(|d| d.quality == StatementPriceQuality::Complete));
    assert!(!p.returns_eligible);
    assert_eq!(p.closing_prices_match, Some(true));
    let json = serde_json::to_string(&p).expect("serialize values");
    assert!(!json.contains("costBasis"));
    assert!(!json.contains("netContribution"));
}
#[test]
fn never_uses_a_future_quote_or_presents_an_unpriced_sum_as_complete() {
    let p = price_statement(
        &input(),
        &[asset("demo")],
        &[quote("demo", "2025-05-05", "12")],
    )
    .expect("partial pricing");
    assert!(p.days[..3]
        .iter()
        .all(|d| d.value.is_none() && d.quality == StatementPriceQuality::PartialUnpriced));
    assert_eq!(p.days[3].value.as_deref(), Some("114"));
    let absent = price_statement(&input(), &[asset("demo")], &[]).expect("unpriced report");
    assert!(absent.days.iter().all(|d| d.value.is_none()));
    assert_eq!(absent.closing_prices_match, None);
}
#[test]
fn marks_long_price_carries_explicitly() {
    let mut source = input();
    source.closing_value = "110".into();
    source.closing_positions[0].market_value = "20".into();
    let p = price_statement(
        &source,
        &[asset("demo")],
        &[quote("demo", "2025-04-01", "10")],
    )
    .expect("stale pricing");
    assert!(p
        .days
        .iter()
        .all(|d| d.quality == StatementPriceQuality::Stale
            && d.stale_assets == vec!["demo".to_string()]));
}
#[test]
fn rejects_offsetting_per_security_price_discrepancies() {
    let mut source = input();
    source.closing_positions.push(ClosingPosition {
        asset_id: "other".into(),
        quantity: "1".into(),
        market_value: "10".into(),
        cost_basis: None,
    });
    source.closing_value = "124".into();
    let p = price_statement(
        &source,
        &[asset("demo"), asset("other")],
        &[
            quote("demo", "2025-05-02", "11"),
            quote("other", "2025-05-02", "12"),
        ],
    )
    .expect("price reconciliation result");
    assert!(p
        .days
        .iter()
        .all(|d| d.value.is_none() && d.quality == StatementPriceQuality::ClosingPriceMismatch));
}
#[test]
fn validates_core_metadata_instead_of_guessing_from_a_ticker() {
    for kind in ["bond", "foreign", "alternative", "multiplier", "unknown"] {
        let mut a = asset("demo");
        match kind {
            "bond" => a.instrument_type = Some(InstrumentType::Bond),
            "foreign" => a.quote_ccy = "EUR".into(),
            "alternative" => a.kind = AssetKind::Property,
            "multiplier" => a.metadata = Some(serde_json::json!({"contractMultiplier":"10"})),
            "unknown" => a.instrument_type = None,
            _ => unreachable!("listed cases"),
        };
        assert_eq!(
            price_statement(&input(), &[a], &[])
                .expect_err("unsupported asset")
                .code,
            "unsupportedAsset"
        );
    }
    assert_eq!(
        price_statement(&input(), &[], &[])
            .expect_err("missing security")
            .code,
        "security"
    );
}
#[test]
fn cash_only_history_needs_no_market_price_or_basis() {
    let mut source = input();
    source.closing_positions.clear();
    source.events.clear();
    source.closing_cash = "100".into();
    source.closing_value = "100".into();
    let p = price_statement(&source, &[], &[]).expect("cash only");
    assert!(
        p.days
            .iter()
            .all(|d| d.value.as_deref() == Some("100")
                && d.quality == StatementPriceQuality::Complete)
    );
}
#[test]
fn includes_assets_sold_before_month_end_in_the_price_universe() {
    let mut source = input();
    source.events.push(StatementEvent {
        source_id: "sold-out".into(),
        date: date("2025-05-05"),
        kind: StatementEventKind::Sell,
        asset_id: Some("old".into()),
        quantity: Some("1".into()),
        cash: "5".into(),
        source_page: 2,
        source_line: 11,
    });
    source.closing_cash = "95".into();
    source.closing_value = "119".into();
    let p = price_statement(
        &source,
        &[asset("demo"), asset("old")],
        &[
            quote("demo", "2025-05-02", "10"),
            quote("demo", "2025-05-05", "12"),
            quote("old", "2025-05-02", "4"),
        ],
    )
    .expect("sold-out security priced");
    assert_eq!(p.days[0].value.as_deref(), Some("114"));
    assert_eq!(p.days[3].value.as_deref(), Some("119"));
}

#[test]
fn retains_unknown_basis_fractional_holdings_in_market_value() {
    let mut source = input();
    source.events.clear();
    source.closing_cash = "100".into();
    source.closing_value = "100.02".into();
    source.closing_positions[0].quantity = "0.0001".into();
    source.closing_positions[0].market_value = "0.02".into();
    let result = price_statement(
        &source,
        &[asset("demo")],
        &[quote("demo", "2025-05-02", "200")],
    )
    .expect("fractional position priced");
    assert!(result
        .days
        .iter()
        .all(|day| day.value.as_deref() == Some("100.02")));
    assert!(!result.returns_eligible);
}
