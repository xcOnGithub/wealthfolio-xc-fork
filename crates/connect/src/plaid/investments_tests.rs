use super::{investments::*, state::*};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn tx(
    id: &str,
    kind: &str,
    subtype: &str,
    amount: &str,
    quantity: &str,
    price: &str,
    fee: &str,
) -> Value {
    json!({"investment_transaction_id":id,"account_id":"external","security_id":"s","date":(Utc::now().date_naive()-chrono::Days::new(2)).to_string(),"name":"Synthetic activity","type":kind,"subtype":subtype,"amount":amount,"quantity":quantity,"price":price,"fees":fee,"iso_currency_code":"USD"})
}
fn stage(records: Value) -> HoldingsStage {
    HoldingsStage {response:serde_json::from_value(json!({"accounts":[{"account_id":"external","name":"Synthetic","type":"investment","balances":{"current":"110","iso_currency_code":"USD"}}],"item":{"item_id":"item","institution_id":"ins_1","available_products":["investments"],"billed_products":["investments"],"products":["investments"]},"securities":[{"security_id":"s","ticker_symbol":"TEST","name":"Synthetic","type":"equity","iso_currency_code":"USD"}],"holdings":[{"account_id":"external","security_id":"s","quantity":"10","institution_price":"10","institution_value":"100","iso_currency_code":"USD"}]})).unwrap(),history:serde_json::from_value(records).unwrap(),fetched_at:Utc::now(),digest:"review".into()}
}
fn mapping() -> Mapping {
    let mut m = super::ingest_tests::mapping();
    m.external_id = "external".into();
    m.investment_ledger = None;
    m
}

#[test]
fn plaid_investment_trades_reconcile_fee_inclusive_cash_and_require_verified_opening_cost() {
    let m = mapping();
    let s = stage(json!([
        tx("buy", "buy", "buy", "21", "2", "10", "1"),
        tx("sell", "sell", "sell", "-9", "-1", "10", "1")
    ]));
    let p = plan(&m, &s).unwrap();
    assert!(p.issues.is_empty());
    assert_eq!(p.opening_quantities["s"].to_string(), "9");
    assert_eq!(p.opening_cash.to_string(), "22");
    let mut l = InvestmentLedger {
        transfer_unit_costs: std::collections::BTreeMap::new(),
        start_date: p.start_date,
        opening_cash: p.opening_cash.to_string(),
        opening_positions: BTreeMap::new(),
    };
    let assets = BTreeMap::from([("s".into(), "asset".into())]);
    assert!(activities(&m, &p, &l, &assets, "America/Los_Angeles").is_err());
    l.opening_positions.insert(
        "s".into(),
        OpeningPosition {
            quantity: "9".into(),
            unit_cost: "5".into(),
        },
    );
    let rows = activities(&m, &p, &l, &assets, "America/Los_Angeles").unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(
        rows.iter()
            .find(|r| r.activity_type == "BUY")
            .unwrap()
            .amount
            .unwrap()
            .to_string(),
        "21"
    );
    assert_eq!(
        rows.iter()
            .find(|r| r.activity_type == "SELL")
            .unwrap()
            .amount
            .unwrap()
            .to_string(),
        "9"
    );
    assert!(rows
        .iter()
        .all(|r| r.source_system.as_deref() == Some("PLAID")));
    l.opening_positions.get_mut("s").unwrap().unit_cost = "-1".into();
    assert!(activities(&m, &p, &l, &assets, "UTC").is_err());
}

#[test]
fn plaid_investment_cash_events_have_explicit_meaning_and_currency() {
    for (kind, subtype, amount, expected) in [
        ("cash", "dividend", "-10", "DIVIDEND"),
        ("cash", "qualified dividend", "-10", "DIVIDEND"),
        ("cash", "non-qualified dividend", "-10", "DIVIDEND"),
        ("cash", "interest", "-10", "INTEREST"),
        ("cash", "deposit", "-10", "DEPOSIT"),
        ("cash", "contribution", "-10", "DEPOSIT"),
        ("transfer", "contribution", "-10", "DEPOSIT"),
        ("cash", "withdrawal", "10", "WITHDRAWAL"),
        ("transfer", "distribution", "10", "WITHDRAWAL"),
        ("fee", "account fee", "10", "FEE"),
        ("fee", "management fee", "10", "FEE"),
        ("fee", "miscellaneous fee", "10", "FEE"),
        ("fee", "legal fee", "10", "FEE"),
        ("fee", "trust fee", "10", "FEE"),
        ("cash", "tax", "10", "TAX"),
        ("cash", "tax withheld", "10", "TAX"),
        ("cash", "non-resident tax", "10", "TAX"),
    ] {
        let mut row = tx("one", kind, subtype, amount, "0", "0", "0");
        if !expected.eq("DIVIDEND") {
            row["security_id"] = Value::Null;
        }
        let p = plan(&mapping(), &stage(json!([row]))).unwrap();
        assert!(p.issues.is_empty(), "{kind}/{subtype}");
        assert_eq!(p.drafts[0].kind, expected);
        assert_eq!(
            p.drafts[0].cash.to_string(),
            (-amount.parse::<rust_decimal::Decimal>().unwrap()).to_string()
        );
    }
}

#[test]
fn plaid_investment_unproven_economics_are_blocked_instead_of_guessed() {
    for case in [
        "currency",
        "unofficial",
        "negative-fee",
        "buy-sign",
        "sell-sign",
        "zero-quantity",
        "zero-price",
        "cash-mismatch",
        "no-security",
        "unknown-security",
        "reinvestment",
        "corporate-action",
        "short-sale",
        "unknown-event",
        "income-sign",
    ] {
        let mut row = tx("one", "buy", "buy", "10", "1", "10", "0");
        match case {
            "currency" => row["iso_currency_code"] = json!("EUR"),
            "unofficial" => row["unofficial_currency_code"] = json!("BTC"),
            "negative-fee" => row["fees"] = json!("-1"),
            "buy-sign" => row["amount"] = json!("-10"),
            "sell-sign" => {
                row["type"] = json!("sell");
                row["subtype"] = json!("sell");
            }
            "zero-quantity" => row["quantity"] = json!("0"),
            "zero-price" => row["price"] = json!("0"),
            "cash-mismatch" => row["amount"] = json!("999"),
            "no-security" => row["security_id"] = Value::Null,
            "unknown-security" => row["security_id"] = json!("missing"),
            "reinvestment" => {
                row["type"] = json!("cash");
                row["subtype"] = json!("dividend reinvestment");
            }
            "corporate-action" => {
                row["type"] = json!("transfer");
                row["subtype"] = json!("split");
            }
            "short-sale" => {
                row["type"] = json!("sell");
                row["subtype"] = json!("sell short");
            }
            "income-sign" => {
                row["type"] = json!("cash");
                row["subtype"] = json!("dividend");
            }
            _ => row["type"] = json!("future-provider-type"),
        }
        let p = plan(&mapping(), &stage(json!([row]))).unwrap();
        assert!(!p.issues.is_empty(), "{case}");
    }
}

#[test]
fn plaid_investment_cancellations_are_not_counted_as_new_trades() {
    let original = tx("old", "buy", "buy", "10", "1", "10", "0");
    let mut cancel = tx("cancel", "buy", "buy", "10", "1", "10", "0");
    cancel["cancel_transaction_id"] = json!("old");
    let p = plan(&mapping(), &stage(json!([original, cancel.clone()]))).unwrap();
    assert!(p.issues.is_empty());
    assert!(p.drafts.is_empty());
    cancel["cancel_transaction_id"] = json!("missing");
    assert!(!plan(&mapping(), &stage(json!([cancel])))
        .unwrap()
        .issues
        .is_empty());
}

#[test]
fn plaid_investment_baseline_cannot_silently_change_to_hide_missing_history() {
    let mut m = mapping();
    let s = stage(json!([tx("buy", "buy", "buy", "10", "1", "10", "0")]));
    let p = plan(&m, &s).unwrap();
    m.investment_ledger = Some(InvestmentLedger {
        transfer_unit_costs: std::collections::BTreeMap::new(),
        start_date: p.start_date,
        opening_cash: p.opening_cash.to_string(),
        opening_positions: BTreeMap::from([(
            "s".into(),
            OpeningPosition {
                quantity: "9".into(),
                unit_cost: "10".into(),
            },
        )]),
    });
    assert!(plan(&m, &s).unwrap().issues.is_empty());
    assert!(!plan(&m, &stage(json!([]))).unwrap().issues.is_empty());
}

#[test]
fn plaid_investment_duplicate_ids_wrong_owner_and_unknown_holdings_fail_closed() {
    let row = tx("buy", "buy", "buy", "10", "1", "10", "0");
    assert!(plan(&mapping(), &stage(json!([row.clone(), row.clone()]))).is_err());
    let mut foreign = row.clone();
    foreign["account_id"] = json!("foreign");
    assert!(plan(&mapping(), &stage(json!([foreign]))).is_err());
    let mut empty = row;
    empty["investment_transaction_id"] = json!("");
    assert!(plan(&mapping(), &stage(json!([empty]))).is_err());
    let mut s = stage(json!([]));
    s.response.holdings[0].iso_currency_code = Some("EUR".into());
    assert!(plan(&mapping(), &s).is_err());
    let mut s = stage(json!([]));
    s.response.securities[0].security_type = Some("derivative".into());
    assert!(!plan(&mapping(), &s).unwrap().issues.is_empty());
}

#[test]
fn plaid_investment_provider_edge_cases_are_explicitly_rejected() {
    let m = mapping();
    let base = stage(json!([]));
    for case in 0..11 {
        let mut s = base.clone();
        match case {
            0 => s.response.securities.push(s.response.securities[0].clone()),
            1 => s.response.holdings[0].account_id = "foreign".into(),
            2 => s.response.holdings[0].unofficial_currency_code = Some("USD?".into()),
            3 => s.response.holdings[0].security_id = "unknown".into(),
            4 => s.response.holdings.push(s.response.holdings[0].clone()),
            5 => s.response.accounts.clear(),
            6 => s.response.accounts[0].balances.current = None,
            7 => s.response.securities[0].ticker_symbol = None,
            8 => s.response.securities[0].iso_currency_code = Some("EUR".into()),
            9 => s.response.securities[0].unofficial_currency_code = Some("OTHER".into()),
            _ => s.response.holdings[0].quantity = Decimal::NEGATIVE_ONE,
        }
        let result = plan(&m, &s);
        assert!(
            result.is_err() || !result.unwrap().issues.is_empty(),
            "case {case}"
        );
    }
    let mut cash = base.clone();
    cash.response.securities[0].security_type = Some("cash".into());
    let p = plan(&m, &cash).unwrap();
    assert!(p.current_quantities.is_empty());
    assert_eq!(p.current_cash, Decimal::from(110));
    let mut future = stage(json!([tx("future", "buy", "buy", "10", "1", "10", "0")]));
    future.history[0].date = Utc::now().date_naive() + chrono::Days::new(1);
    assert!(plan(&m, &future).is_err());
    future.history[0].date = base.fetched_at.date_naive();
    future.history[0].cancel_transaction_id = Some("future".into());
    assert!(!plan(&m, &future).unwrap().issues.is_empty());
    future.history[0].account_id = "foreign".into();
    assert!(plan(&m, &future).is_err());
}

#[test]
fn plaid_investment_cash_guards_opening_shorts_and_missing_fixed_balances_block() {
    let m = mapping();
    for (kind, subtype, amount, fee) in [
        ("cash", "deposit", "10", "0"),
        ("cash", "withdrawal", "-10", "0"),
        ("fee", "account fee", "-10", "0"),
        ("fee", "account fee", "10", "1"),
        ("cash", "tax", "-10", "0"),
        ("cash", "tax", "10", "1"),
    ] {
        let mut row = tx("guard", kind, subtype, amount, "0", "0", fee);
        row["security_id"] = Value::Null;
        assert!(!plan(&m, &stage(json!([row]))).unwrap().issues.is_empty());
    }
    let short = stage(json!([tx("large", "buy", "buy", "110", "11", "10", "0")]));
    assert!(plan(&m, &short)
        .unwrap()
        .issues
        .iter()
        .any(|i| i.reason == "openingShort"));
    let mut early = tx("sell", "sell", "sell", "-10", "-1", "10", "0");
    early["date"] = json!((Utc::now().date_naive() - chrono::Days::new(3)).to_string());
    let s = stage(json!([
        early,
        tx("buy", "buy", "buy", "110", "11", "10", "0")
    ]));
    assert!(plan(&m, &s)
        .unwrap()
        .issues
        .iter()
        .any(|i| i.reason == "negativePosition"));
    let s = stage(json!([]));
    let mut fixed = m.clone();
    fixed.investment_ledger = Some(InvestmentLedger {
        transfer_unit_costs: std::collections::BTreeMap::new(),
        start_date: Utc::now().date_naive(),
        opening_cash: "10".into(),
        opening_positions: BTreeMap::new(),
    });
    let p = plan(&fixed, &s).unwrap();
    assert!(p.issues.iter().any(|i| i.reason == "reconciliation"));
    assert!(activities(
        &fixed,
        &p,
        fixed.investment_ledger.as_ref().unwrap(),
        &BTreeMap::new(),
        "UTC"
    )
    .is_err());
    fixed
        .investment_ledger
        .as_mut()
        .unwrap()
        .opening_positions
        .insert(
            "s".into(),
            OpeningPosition {
                quantity: "10".into(),
                unit_cost: "1".into(),
            },
        );
    fixed.investment_ledger.as_mut().unwrap().opening_cash = "11".into();
    assert!(plan(&fixed, &s)
        .unwrap()
        .issues
        .iter()
        .any(|i| i.reason == "reconciliation"));
    let old = stage(json!([tx("old", "buy", "buy", "10", "1", "10", "0")]));
    assert!(plan(&fixed, &old).is_err());
}

#[test]
fn plaid_investment_opening_cash_zero_negative_and_mismatched_basis_are_not_guessed() {
    let m = mapping();
    let mut s = stage(json!([]));
    let assets = BTreeMap::from([("s".into(), "asset".into())]);
    for (cash, expected) in [
        ("0", None),
        ("-10", Some("WITHDRAWAL")),
        ("10", Some("DEPOSIT")),
    ] {
        s.response.accounts[0].balances.current =
            Some(Decimal::from(100) + cash.parse::<Decimal>().unwrap());
        let p = plan(&m, &s).unwrap();
        let mut l = InvestmentLedger {
            transfer_unit_costs: std::collections::BTreeMap::new(),
            start_date: p.start_date,
            opening_cash: cash.into(),
            opening_positions: BTreeMap::from([(
                "s".into(),
                OpeningPosition {
                    quantity: "10".into(),
                    unit_cost: "2".into(),
                },
            )]),
        };
        let rows = activities(&m, &p, &l, &assets, "UTC").unwrap();
        assert_eq!(
            rows.iter()
                .find(|r| r.asset_id.is_none())
                .map(|r| r.activity_type.as_str()),
            expected
        );
        l.opening_positions.get_mut("s").unwrap().quantity = "11".into();
        assert!(activities(&m, &p, &l, &assets, "UTC").is_err());
    }
}

#[test]
fn plaid_investment_cash_security_references_are_cash_only_when_explicitly_identified() {
    for subtype in [
        "deposit",
        "withdrawal",
        "dividend",
        "qualified dividend",
        "non-qualified dividend",
    ] {
        for currency in [Some("USD"), Some("EUR"), None] {
            let amount = if subtype == "withdrawal" { "10" } else { "-10" };
            let mut s = stage(json!([tx("cash", "cash", subtype, amount, "0", "0", "0")]));
            s.response.securities[0].security_type = Some("cash".into());
            s.response.securities[0].iso_currency_code = currency.map(str::to_string);
            let p = plan(&mapping(), &s).unwrap();
            if currency == Some("EUR") {
                assert!(!p.issues.is_empty());
            } else {
                assert!(p.issues.is_empty());
                assert!(p.drafts[0].security.is_none());
                if subtype.contains("dividend") {
                    assert_eq!(p.drafts[0].kind, "DIVIDEND");
                    assert_eq!(p.drafts[0].cash, Decimal::from(10));
                }
            }
        }
    }
}

#[test]
fn plaid_investment_zero_quantity_requires_zero_noncash_value() {
    for value in ["100", "-100", "0"] {
        let mut s = stage(json!([]));
        s.response.holdings[0].quantity = Decimal::ZERO;
        s.response.holdings[0].institution_value = value.parse().unwrap();
        let result = plan(&mapping(), &s);
        if value == "0" {
            let p = result.unwrap();
            assert!(p.issues.is_empty());
            assert_eq!(p.current_cash, Decimal::from(110));
        } else {
            assert!(matches!(result, Err(SyncError::Review)), "{value}");
        }
    }
}

#[test]
fn plaid_investment_cash_dividend_does_not_normalize_unknown_or_incompatible_securities() {
    for case in [
        "missing",
        "unknown-type",
        "derivative",
        "currency",
        "unofficial",
        "reinvestment",
    ] {
        let mut s = stage(json!([tx(
            "dividend", "cash", "dividend", "-2", "0", "0", "0"
        )]));
        s.response.securities[0].security_type = Some("cash".into());
        match case {
            "missing" => s.history[0].security_id = Some("missing".into()),
            "unknown-type" => s.response.securities[0].security_type = None,
            "derivative" => s.response.securities[0].security_type = Some("derivative".into()),
            "currency" => s.response.securities[0].iso_currency_code = Some("EUR".into()),
            "unofficial" => {
                s.response.securities[0].unofficial_currency_code = Some("OTHER".into())
            }
            _ => s.history[0].subtype = "dividend reinvestment".into(),
        }
        assert!(
            plan(&mapping(), &s)
                .unwrap()
                .issues
                .iter()
                .any(|issue| issue.record_id == "dividend"
                    && issue.reason == "unsupportedTransaction"),
            "{case}"
        );
    }
}

#[test]
fn plaid_investment_trade_quantity_sign_must_match_provider_semantics() {
    for (kind, qty, amount) in [("buy", "-1", "10"), ("sell", "1", "-10")] {
        let s = stage(json!([tx("sign", kind, kind, amount, qty, "10", "0")]));
        assert!(!plan(&mapping(), &s).unwrap().issues.is_empty());
    }
}

#[test]
fn plaid_investment_security_transfers_reconstruct_quantities_and_require_incoming_book_cost() {
    let m = mapping();
    let s = stage(json!([
        tx("in", "transfer", "transfer", "-5000", "5", "1000", "0"),
        tx("out", "transfer", "transfer", "5000", "-5", "1000", "0")
    ]));
    let p = plan(&m, &s).unwrap();
    assert!(p.issues.is_empty());
    assert_eq!(p.opening_quantities["s"], Decimal::from(10));
    assert_eq!(p.opening_cash, Decimal::from(10));
    let mut l = InvestmentLedger {
        start_date: p.start_date,
        opening_cash: p.opening_cash.to_string(),
        opening_positions: BTreeMap::from([(
            "s".into(),
            OpeningPosition {
                quantity: "10".into(),
                unit_cost: "4".into(),
            },
        )]),
        transfer_unit_costs: BTreeMap::new(),
    };
    let assets = BTreeMap::from([("s".into(), "asset".into())]);
    assert!(activities(&m, &p, &l, &assets, "UTC").is_err());
    l.transfer_unit_costs
        .insert("investment:in".into(), "7.5".into());
    let rows = activities(&m, &p, &l, &assets, "UTC").unwrap();
    let incoming = rows
        .iter()
        .find(|r| r.source_record_id.as_deref() == Some("investment:in"))
        .unwrap();
    assert_eq!(incoming.activity_type, "TRANSFER_IN");
    assert_eq!(incoming.unit_price, Some(Decimal::new(75, 1)));
    assert_eq!(incoming.amount, None);
    let outgoing = rows
        .iter()
        .find(|r| r.activity_type == "TRANSFER_OUT")
        .unwrap();
    assert_eq!(outgoing.quantity, Some(Decimal::from(5)));
    assert_eq!(outgoing.unit_price, None);
    assert_eq!(outgoing.amount, None);
    l.transfer_unit_costs
        .insert("investment:in".into(), "-1".into());
    assert!(activities(&m, &p, &l, &assets, "UTC").is_err());
}
#[test]
fn plaid_investment_rounding_uses_net_cash_for_effective_execution_price() {
    let mut s = stage(json!([tx(
        "rounded", "buy", "buy", "99996", "1000", "100.00", "0"
    )]));
    s.response.holdings[0].quantity = Decimal::from(1010);
    let p = plan(&mapping(), &s).unwrap();
    assert!(p.issues.is_empty());
    assert_eq!(p.opening_quantities["s"], Decimal::from(10));
    assert_eq!(p.drafts[0].price, Decimal::new(99996, 3));
    s.history[0].amount = Decimal::from(99990);
    let blocked = plan(&mapping(), &s).unwrap();
    assert!(blocked.issues.iter().any(|i| i.reason == "tradeValues"));
    assert_eq!(blocked.opening_quantities["s"], Decimal::from(10));
}
#[test]
fn plaid_investment_ordinary_etf_exercise_disposal_is_a_sale_not_a_missing_share_change() {
    let mut s = stage(json!([tx(
        "disposed", "sell", "exercise", "-10", "-1", "10", "0"
    )]));
    s.response.securities[0].security_type = Some("etf".into());
    let p = plan(&mapping(), &s).unwrap();
    assert!(p.issues.is_empty());
    assert_eq!(p.opening_quantities["s"], Decimal::from(11));
    assert_eq!(p.drafts[0].kind, "SELL");
    s.response.securities[0].security_type = Some("derivative".into());
    assert!(!plan(&mapping(), &s).unwrap().issues.is_empty());
}
#[test]
fn plaid_investment_legacy_ledger_has_no_implicitly_reviewed_transfer_costs() {
    let ledger: InvestmentLedger = serde_json::from_value(
        json!({"startDate":"2025-01-01","openingCash":"0","openingPositions":{}}),
    )
    .unwrap();
    assert!(ledger.transfer_unit_costs.is_empty());
}

#[test]
fn plaid_investment_reviewed_transfer_cost_cannot_survive_source_economic_changes() {
    let original = tx("in", "transfer", "transfer", "-5000", "5", "1000", "0");
    let initial = stage(json!([original.clone()]));
    let preview = plan(&mapping(), &initial).unwrap();
    let mut reviewed = mapping();
    reviewed.investment_ledger = Some(InvestmentLedger {
        start_date: preview.start_date,
        opening_cash: preview.opening_cash.to_string(),
        opening_positions: BTreeMap::from([(
            "s".into(),
            OpeningPosition {
                quantity: "5".into(),
                unit_cost: "4".into(),
            },
        )]),
        transfer_unit_costs: BTreeMap::from([("investment:in".into(), "7.5".into())]),
    });
    reviewed.source_history = json!({"transactions": [original.clone()]});
    let assets = BTreeMap::from([("s".into(), "asset".into())]);
    assert!(plan(&reviewed, &initial).unwrap().issues.is_empty());

    let next_date = initial.history[0].date.succ_opt().unwrap().to_string();
    for (field, value) in [
        ("security_id", json!("replacement-security")),
        ("quantity", json!("6")),
        ("date", json!(next_date)),
        ("type", json!("buy")),
        ("subtype", json!("deposit")),
        ("fees", json!("1")),
        ("iso_currency_code", json!("EUR")),
        ("unofficial_currency_code", json!("USDC")),
    ] {
        let mut corrected = original.clone();
        corrected[field] = value;
        let p = plan(&reviewed, &stage(json!([corrected]))).unwrap();
        assert!(
            p.issues
                .iter()
                .any(|issue| issue.record_id == "investment:in"
                    && issue.reason == "reviewedTransferChanged"),
            "{field} correction must invalidate reviewed cost"
        );
        assert!(
            activities(
                &reviewed,
                &p,
                reviewed.investment_ledger.as_ref().unwrap(),
                &assets,
                "UTC"
            )
            .is_err(),
            "{field}"
        );
    }

    // Stored evidence from another account cannot authorize this source's cost.
    let mut wrong_owner = reviewed.clone();
    wrong_owner.source_history["transactions"][0]["account_id"] = json!("other-account");
    let p = plan(&wrong_owner, &initial).unwrap();
    assert!(p
        .issues
        .iter()
        .any(|issue| issue.reason == "reviewedTransferChanged"));
    assert!(activities(
        &wrong_owner,
        &p,
        wrong_owner.investment_ledger.as_ref().unwrap(),
        &assets,
        "UTC"
    )
    .is_err());

    // Provider market-value corrections do not change verified historical cost.
    let mut market_only = original;
    market_only["amount"] = json!("-6000");
    market_only["price"] = json!("1200");
    let p = plan(&reviewed, &stage(json!([market_only]))).unwrap();
    assert!(p.issues.is_empty());
    let rows = activities(
        &reviewed,
        &p,
        reviewed.investment_ledger.as_ref().unwrap(),
        &assets,
        "UTC",
    )
    .unwrap();
    let incoming = rows
        .iter()
        .find(|row| row.source_record_id.as_deref() == Some("investment:in"))
        .unwrap();
    assert_eq!(incoming.unit_price, Some(Decimal::new(75, 1)));
    assert_eq!(incoming.amount, None);
}

#[test]
fn plaid_investment_invalid_transfer_shapes_and_nonpositive_execution_are_blocked() {
    let mut missing_security = tx("missing", "transfer", "transfer", "-10", "1", "10", "0");
    missing_security["security_id"] = Value::Null;
    for record in [
        missing_security,
        tx("zero", "transfer", "transfer", "-10", "0", "10", "0"),
        tx("wrong-sale-sign", "sell", "sell", "10", "-1", "10", "0"),
        tx("no-execution", "buy", "buy", "1", "1", "0.001", "1"),
    ] {
        let p = plan(&mapping(), &stage(json!([record]))).unwrap();
        assert!(!p.issues.is_empty());
        let ledger = InvestmentLedger {
            start_date: p.start_date,
            opening_cash: p.opening_cash.to_string(),
            opening_positions: BTreeMap::new(),
            transfer_unit_costs: BTreeMap::new(),
        };
        assert!(activities(&mapping(), &p, &ledger, &BTreeMap::new(), "UTC").is_err());
    }
}
