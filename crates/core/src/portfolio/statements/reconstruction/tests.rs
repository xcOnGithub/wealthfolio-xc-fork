use super::*;
fn date(raw: &str) -> NaiveDate {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d").expect("synthetic date")
}
fn event(
    id: &str,
    day: &str,
    kind: StatementEventKind,
    asset: Option<&str>,
    quantity: Option<&str>,
    cash: &str,
) -> StatementEvent {
    StatementEvent {
        source_id: id.into(),
        date: date(day),
        kind,
        asset_id: asset.map(str::to_owned),
        quantity: quantity.map(str::to_owned),
        cash: cash.into(),
        source_page: 2,
        source_line: 10,
    }
}
fn fixture() -> StatementInterval {
    StatementInterval {
        date_policy: StatementDatePolicy::Settlement,
        from: date("2025-05-01"),
        through: date("2025-05-05"),
        currency: "USD".into(),
        opening_cash: "100".into(),
        closing_cash: "105".into(),
        closing_value: "145".into(),
        closing_positions_complete: true,
        activity_complete: true,
        closing_positions: vec![ClosingPosition {
            asset_id: "demo".into(),
            quantity: "2".into(),
            market_value: "40".into(),
            cost_basis: None,
        }],
        events: vec![
            event(
                "buy",
                "2025-05-02",
                StatementEventKind::Buy,
                Some("demo"),
                Some("1"),
                "-11",
            ),
            event(
                "income",
                "2025-05-03",
                StatementEventKind::Cash,
                None,
                None,
                "1",
            ),
            event(
                "sell",
                "2025-05-04",
                StatementEventKind::Sell,
                Some("demo"),
                Some("1"),
                "15",
            ),
        ],
    }
}
#[test]
fn reconstructs_each_day_from_statements_without_basis_or_plaid() {
    let input = fixture();
    let output = reconstruct_statement(&input).expect("complete statement");
    assert_eq!(output.opening_positions["demo"], "2");
    assert_eq!(
        output
            .days
            .iter()
            .map(|d| d.cash.as_str())
            .collect::<Vec<_>>(),
        vec!["100", "89", "90", "105", "105"]
    );
    assert_eq!(
        output
            .days
            .iter()
            .map(|d| d.positions["demo"].as_str())
            .collect::<Vec<_>>(),
        vec!["2", "3", "3", "2", "2"]
    );
    assert_eq!(output.days.first().expect("first").date, input.from);
    assert_eq!(output.days.last().expect("last").date, input.through);
}
#[test]
fn includes_fully_disposed_securities_and_preserves_fractions() {
    let mut input = fixture();
    input.events.push(event(
        "old-buy",
        "2025-05-01",
        StatementEventKind::Buy,
        Some("sold-out"),
        Some("2"),
        "-8",
    ));
    input.events.push(event(
        "old-sell",
        "2025-05-03",
        StatementEventKind::Sell,
        Some("sold-out"),
        Some("2"),
        "12",
    ));
    input.events.push(event(
        "fraction",
        "2025-05-04",
        StatementEventKind::TransferIn,
        Some("tiny"),
        Some("0.0001"),
        "0",
    ));
    input.closing_positions.push(ClosingPosition {
        asset_id: "tiny".into(),
        quantity: "0.0001".into(),
        market_value: "0.02".into(),
        cost_basis: None,
    });
    input.closing_cash = "109".into();
    input.closing_value = "149.02".into();
    let output = reconstruct_statement(&input).expect("reconciled with sold-out asset");
    assert_eq!(output.days[0].positions["sold-out"], "2");
    assert!(!output.days[2].positions.contains_key("sold-out"));
    assert_eq!(output.days[3].positions["tiny"], "0.0001");
}
#[test]
fn groups_end_of_day_changes_without_inventing_intraday_order() {
    let mut input = fixture();
    input.events.push(event(
        "sell-fill",
        "2025-05-02",
        StatementEventKind::Sell,
        Some("demo"),
        Some("5"),
        "50",
    ));
    input.events.push(event(
        "buy-fill",
        "2025-05-02",
        StatementEventKind::Buy,
        Some("demo"),
        Some("5"),
        "-50",
    ));
    let expected = reconstruct_statement(&input).expect("daily net quantities");
    input.events.reverse();
    assert_eq!(
        reconstruct_statement(&input).expect("same daily net quantities"),
        expected
    );
}
#[test]
fn keeps_legitimate_identical_fills_but_rejects_duplicate_source_occurrences() {
    let mut input = fixture();
    input.events.push(event(
        "buy-2",
        "2025-05-02",
        StatementEventKind::Buy,
        Some("demo"),
        Some("1"),
        "-11",
    ));
    input.closing_cash = "94".into();
    input.closing_value = "134".into();
    assert!(reconstruct_statement(&input).is_ok());
    input.events.last_mut().expect("second buy").source_id = "buy".into();
    assert_eq!(
        reconstruct_statement(&input)
            .expect_err("duplicate occurrence")
            .code,
        "duplicate"
    );
}
#[test]
fn refuses_incomplete_or_inconsistent_sources() {
    for case in [
        "cash",
        "market",
        "missing",
        "negative",
        "period",
        "currency",
        "source",
        "quantity",
        "sign",
        "transfer-value",
        "date",
        "unknown-basis",
        "scale",
    ] {
        let mut input = fixture();
        match case {
            "cash" => input.closing_cash = "104".into(),
            "market" => input.closing_value = "999".into(),
            "missing" => input.activity_complete = false,
            "negative" => {
                input.events[0].quantity = Some("100".into());
            }
            "period" => input.through = date("2025-07-01"),
            "currency" => input.currency = "EUR".into(),
            "source" => input.events[0].source_page = 0,
            "quantity" => input.events[0].quantity = Some("0".into()),
            "sign" => input.events[0].cash = "11".into(),
            "transfer-value" => input.events[0].kind = StatementEventKind::TransferIn,
            "date" => input.events[0].date = date("2025-04-30"),
            "unknown-basis" => input.closing_positions[0].cost_basis = Some("N/A".into()),
            "scale" => input.events[0].quantity = Some("0.0000000000000000001".into()),
            _ => unreachable!("listed synthetic cases"),
        }
        assert!(reconstruct_statement(&input).is_err(), "{case}");
    }
}
#[test]
fn rejects_an_unexplained_negative_daily_position() {
    let mut input = fixture();
    input.events = vec![
        event(
            "early-sale",
            "2025-05-01",
            StatementEventKind::Sell,
            Some("demo"),
            Some("3"),
            "10",
        ),
        event(
            "late-buy",
            "2025-05-05",
            StatementEventKind::Buy,
            Some("demo"),
            Some("3"),
            "-5",
        ),
    ];
    assert_eq!(
        reconstruct_statement(&input)
            .expect_err("short before purchase")
            .code,
        "negativePosition"
    );
}
#[test]
fn checks_contiguous_boundaries_without_filling_missing_days() {
    let first = fixture();
    let mut next = fixture();
    next.from = date("2025-05-06");
    next.through = date("2025-05-07");
    next.events.clear();
    next.opening_cash = "105".into();
    let chain = validate_statement_chain(&[next.clone(), first.clone()])
        .expect("contiguous and reconciled");
    assert_eq!(chain[0].from, first.from);
    next.from = date("2025-06-01");
    next.through = date("2025-06-02");
    assert_eq!(
        validate_statement_chain(&[first.clone(), next.clone()])
            .expect("gap retained")
            .iter()
            .map(|i| i.days.len())
            .sum::<usize>(),
        7
    );
    next.from = date("2025-05-06");
    next.through = date("2025-05-07");
    next.opening_cash = "104".into();
    next.closing_cash = "104".into();
    next.closing_value = "144".into();
    assert_eq!(
        validate_statement_chain(&[first.clone(), next.clone()])
            .expect_err("cash handoff")
            .code,
        "continuity"
    );
    next.from = date("2025-05-05");
    assert_eq!(
        validate_statement_chain(&[first, next])
            .expect_err("overlap")
            .code,
        "overlap"
    );
}
#[test]
fn unknown_kinds_and_unreviewed_fields_cannot_bypass_validation() {
    let input = fixture();
    let mut value = serde_json::to_value(input).expect("serialize fixture");
    value["events"][0]["kind"] = "SPLIT".into();
    assert!(serde_json::from_value::<StatementInterval>(value.clone()).is_err());
    value["events"][0]["kind"] = "BUY".into();
    value["rawPdf"] = "not stored".into();
    assert!(serde_json::from_value::<StatementInterval>(value).is_err());
}

proptest::proptest! {
    #[test]
    fn daily_states_recover_independently_generated_openings(opening in 0_i64..1000, bought in 1_i64..100, sold in 1_i64..100) {
        proptest::prop_assume!(sold <= opening + bought);
        let mut input = fixture();
        input.through = date("2025-05-02"); input.opening_cash = "10000".into();
        let closing = opening + bought - sold;
        let cash = 10000 - bought * 10 + sold * 12;
        input.closing_cash = cash.to_string(); input.closing_value = (cash + closing * 20).to_string();
        input.closing_positions[0].quantity = closing.to_string(); input.closing_positions[0].market_value = (closing * 20).to_string();
        input.events = vec![event("b", "2025-05-01", StatementEventKind::Buy, Some("demo"), Some(&bought.to_string()), &(-bought*10).to_string()), event("s", "2025-05-02", StatementEventKind::Sell, Some("demo"), Some(&sold.to_string()), &(sold*12).to_string())];
        let result = reconstruct_statement(&input).expect("generated complete source");
        proptest::prop_assert_eq!(result.opening_positions.get("demo").cloned().unwrap_or_else(|| "0".into()), opening.to_string());
        proptest::prop_assert_eq!(&result.days[0].positions["demo"], &(opening+bought).to_string());
        proptest::prop_assert_eq!(&result.days[1].cash, &cash.to_string());
    }
}
