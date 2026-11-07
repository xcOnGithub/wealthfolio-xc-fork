use super::{ingest::*, state::*, *};
use chrono::{Duration, Utc};
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn crypto_upgrade_preserves_persisted_transaction_ids() {
    assert_eq!(
        stable_id("mapping-1", "record-1"),
        "plaid-de24ed904ca869cedbe80847f8bd2927c66a099388bbac01ee8558de430cd146"
    );
}

fn tx(amount: &str, category: &str) -> BankTransaction {
    serde_json::from_value(json!({"transaction_id":"one","account_id":"external","pending":false,"date":"2026-11-01","amount":amount,"iso_currency_code":"USD","name":"Synthetic","personal_finance_category":{"primary":category}})).unwrap()
}
fn mapping() -> Mapping {
    Mapping {
        id: uuid::Uuid::new_v4().to_string(),
        connection_id: uuid::Uuid::new_v4().to_string(),
        external_id: "external".into(),
        account_id: uuid::Uuid::new_v4().to_string(),
        currency: "USD".into(),
        credit_card: false,
        cutover_date: None,
        cursor: None,
        last_success: None,
        status: "review".into(),
        last_error: None,
        decisions: BTreeMap::new(),
        staged: Some(BankStage {
            records: vec![tx("10", "FOOD_AND_DRINK")],
            removed: vec![],
            next_cursor: "next".into(),
            checkpoint_in: None,
            balance: "100".into(),
            fetched_at: Utc::now(),
            approved: true,
            opening_balance: Some("110".into()),
            digest: String::new(),
        }),
        source_history: json!({}),
        holdings_staged: None,
        opaque_security_ids: Default::default(),
    }
}
#[test]
fn cash_flow_signs_and_explicit_choices_preserve_decimal_and_card_semantics() {
    for card in [false, true] {
        for (amount, category, choice, expected, subtype) in [
            ("10.123456789", "FOOD_AND_DRINK", None, "WITHDRAWAL", None),
            ("-2.25", "FOOD_AND_DRINK", None, "CREDIT", Some("REFUND")),
            (
                "-25",
                "INCOME",
                None,
                if card { "CREDIT" } else { "DEPOSIT" },
                None,
            ),
            ("-15", "TRANSFER_IN", Some("external"), "DEPOSIT", None),
            ("15", "TRANSFER_OUT", Some("external"), "WITHDRAWAL", None),
            ("-15", "TRANSFER_IN", Some("internal"), "TRANSFER_IN", None),
            ("15", "TRANSFER_OUT", Some("internal"), "TRANSFER_OUT", None),
            ("0", "FOOD_AND_DRINK", None, "WITHDRAWAL", None),
            (
                "0",
                "INCOME",
                None,
                if card { "CREDIT" } else { "DEPOSIT" },
                None,
            ),
        ] {
            let mut m = mapping();
            m.credit_card = card;
            m.staged.as_mut().unwrap().records = vec![tx(amount, category)];
            if let Some(kind) = choice {
                m.decisions.insert(
                    "one".into(),
                    Decision {
                        kind: kind.into(),
                        group: if kind == "internal" {
                            Some(uuid::Uuid::new_v4().to_string())
                        } else {
                            None
                        },
                    },
                );
            }
            let batch = bank_commit(m).unwrap();
            let a = &batch.activities[0];
            assert_eq!(a.activity_type, expected);
            assert_eq!(a.subtype.as_deref(), subtype);
            assert_eq!(
                a.amount.unwrap(),
                amount.parse::<rust_decimal::Decimal>().unwrap().abs()
            );
            assert_eq!(batch.mapping.cursor.as_deref(), Some("next"));
            assert!(batch.mapping.staged.is_none());
        }
    }
}
#[test]
fn invalid_classification_or_source_identity_never_produces_partial_batch() {
    for kind in ["income", "expense", "refund", "internal", "invalid"] {
        let mut m = mapping();
        m.staged.as_mut().unwrap().records = vec![tx(
            if kind == "expense" { "-10" } else { "10" },
            "TRANSFER_OUT",
        )];
        m.decisions.insert(
            "one".into(),
            Decision {
                kind: kind.into(),
                group: None,
            },
        );
        assert!(matches!(bank_commit(m), Err(SyncError::Review)));
    }
    let mut m = mapping();
    m.staged.as_mut().unwrap().records = vec![tx("10", "TRANSFER_OUT")];
    assert!(bank_commit(m).is_err());
    for field in [
        "account",
        "id",
        "long-id",
        "currency",
        "unofficial",
        "future",
        "name",
    ] {
        let mut m = mapping();
        let t = &mut m.staged.as_mut().unwrap().records[0];
        match field {
            "account" => t.account_id = "foreign".into(),
            "id" => t.transaction_id.clear(),
            "long-id" => t.transaction_id = "a".repeat(513),
            "currency" => t.iso_currency_code = None,
            "unofficial" => t.unofficial_currency_code = Some("USD".into()),
            "future" => t.date = (Utc::now() + Duration::days(1)).date_naive(),
            _ => t.name = "x".repeat(4097),
        }
        assert!(matches!(bank_commit(m), Err(SyncError::Review)), "{field}");
    }
    let mut m = mapping();
    let t = m.staged.as_ref().unwrap().records[0].clone();
    m.staged.as_mut().unwrap().records.push(t);
    assert!(bank_commit(m).is_err());
}
#[test]
fn review_approval_checkpoint_age_and_opening_are_required() {
    for case in [
        "missing",
        "unapproved",
        "checkpoint",
        "expired",
        "opening",
        "invalid-opening",
        "invalid-history",
        "overlap",
    ] {
        let mut m = mapping();
        match case {
            "missing" => m.staged = None,
            "unapproved" => m.staged.as_mut().unwrap().approved = false,
            "checkpoint" => m.cursor = Some("old".into()),
            "expired" => m.staged.as_mut().unwrap().fetched_at -= Duration::minutes(31),
            "opening" => m.staged.as_mut().unwrap().opening_balance = None,
            "invalid-opening" => m.staged.as_mut().unwrap().opening_balance = Some("bad".into()),
            "invalid-history" => m.source_history = json!([]),
            _ => m.staged.as_mut().unwrap().removed.push("one".into()),
        }
        assert!(bank_commit(m).is_err(), "{case}");
    }
    for amount in ["0", "-20"] {
        let mut m = mapping();
        m.staged.as_mut().unwrap().opening_balance = Some(amount.into());
        let batch = bank_commit(m).unwrap();
        assert_eq!(batch.activities.len(), if amount == "0" { 1 } else { 2 });
        if amount != "0" {
            assert_eq!(batch.activities[1].activity_type, "WITHDRAWAL");
        }
    }
}
#[test]
fn cutover_retains_source_history_but_only_imports_new_records_without_opening() {
    let mut m = mapping();
    m.cutover_date = Some("2026-11-02".parse().unwrap());
    m.staged.as_mut().unwrap().opening_balance = None;
    let mut newer = tx("-20", "INCOME");
    newer.transaction_id = "new".into();
    newer.date = "2026-11-02".parse().unwrap();
    m.staged.as_mut().unwrap().records.push(newer);
    let batch = bank_commit(m).unwrap();
    assert_eq!(batch.activities.len(), 1);
    assert_eq!(batch.activities[0].source_record_id.as_deref(), Some("new"));
    assert!(batch.mapping.source_history.get("one").is_some());
}
#[test]
fn pending_replacements_remove_only_pending_source_and_digest_binds_review_data() {
    let mut m = mapping();
    let stage = m.staged.as_mut().unwrap();
    let mut pending = tx("8", "TRANSFER_OUT");
    pending.pending = true;
    pending.transaction_id = "pending".into();
    stage.records.push(pending);
    stage.records[0].pending_transaction_id = Some("pending".into());
    assert_eq!(
        suggested_opening(&m, m.staged.as_ref().unwrap())
            .unwrap()
            .to_string(),
        "110"
    );
    let first = digest(m.staged.as_ref().unwrap(), &m.decisions).unwrap();
    m.staged.as_mut().unwrap().opening_balance = Some("20".into());
    assert_eq!(
        first,
        digest(m.staged.as_ref().unwrap(), &m.decisions).unwrap()
    );
    m.staged.as_mut().unwrap().records[0].amount += rust_decimal::Decimal::ONE;
    assert_ne!(
        first,
        digest(m.staged.as_ref().unwrap(), &m.decisions).unwrap()
    );
    let batch = bank_commit(m).unwrap();
    assert_eq!(batch.removed_ids, vec!["pending"]);
    assert_eq!(batch.activities.len(), 2);
    assert_ne!(stable_id("account", "Case"), stable_id("account", "case"));
    assert_ne!(stable_id("a", "bc"), stable_id("ab", "c"));
}
#[test]
fn currency_and_reserved_secret_keys_reject_aliasing() {
    let mut a:Account=serde_json::from_value(json!({"account_id":"a","name":"Synthetic","type":"credit","balances":{"iso_currency_code":"USD"}})).unwrap();
    assert_eq!(currency(&a).unwrap(), "USD");
    for value in [None, Some("usd"), Some("US"), Some("USDD")] {
        a.balances.iso_currency_code = value.map(str::to_owned);
        assert!(currency(&a).is_err());
    }
    a.balances.iso_currency_code = Some("USD".into());
    a.balances.unofficial_currency_code = Some("USD".into());
    assert!(currency(&a).is_err());
    assert!(decision(&tx("10", "INCOME"), &mapping()).is_err());
    let mut c = Connection {
        id: uuid::Uuid::new_v4().to_string(),
        owner: "owner".into(),
        environment: PlaidEnvironment::Sandbox,
        product: PlaidProduct::Transactions,
        institution: "chase".into(),
        institution_id: "ins_1".into(),
        item_id: None,
        status: "linking".into(),
        expires_at: Utc::now(),
        update_mode: false,
    };
    for purpose in ["access", "link", "revocation"] {
        assert!(secret_key(&c, purpose)
            .unwrap()
            .starts_with("plaid:sandbox:"));
    }
    c.environment = PlaidEnvironment::Production;
    assert!(secret_key(&c, "access")
        .unwrap()
        .starts_with("plaid:production:"));
    c.environment = PlaidEnvironment::Sandbox;
    assert!(secret_key(&c, "unknown").is_err());
    let canonical = secret_key(&c, "access").unwrap();
    c.id = c.id.to_uppercase();
    assert_eq!(secret_key(&c, "access").unwrap(), canonical);
    c.id = "provider-case-sensitive-id".into();
    assert!(secret_key(&c, "access").is_err());
    for (error, expected) in [
        (PlaidError::NotReady, SyncError::NotReady),
        (PlaidError::Credentials, SyncError::Attention),
        (PlaidError::ReconnectRequired, SyncError::Attention),
        (PlaidError::Configuration, SyncError::Configuration),
        (PlaidError::RateLimited, SyncError::RateLimited),
        (PlaidError::Unavailable, SyncError::Provider),
    ] {
        assert_eq!(SyncError::from(error), expected);
    }
}

#[test]
fn institution_products_and_amex_account_restrictions_are_explicit() {
    use super::service::{eligible, product_for};
    for name in ["chase", "boa", "usbank", "amex"] {
        assert_eq!(product_for(name).unwrap(), PlaidProduct::Transactions);
    }
    for name in ["merrill_edge", "schwab", "robinhood"] {
        assert_eq!(product_for(name).unwrap(), PlaidProduct::Investments);
    }
    assert!(product_for("unknown").is_err());
    let mut c = Connection {
        id: uuid::Uuid::new_v4().to_string(),
        owner: "owner".into(),
        environment: PlaidEnvironment::Sandbox,
        product: PlaidProduct::Transactions,
        institution: "amex".into(),
        institution_id: "ins_1".into(),
        item_id: None,
        status: "connected".into(),
        expires_at: Utc::now(),
        update_mode: false,
    };
    for (kind, subtype, amex, bank, investment) in [
        ("credit", "credit card", true, true, false),
        ("credit", "other", false, false, false),
        ("depository", "savings", false, true, false),
        ("depository", "checking", false, true, false),
        ("depository", "other", false, false, false),
        ("investment", "brokerage", false, false, true),
    ] {
        let a:Account=serde_json::from_value(json!({"account_id":"a","name":"Synthetic","type":kind,"subtype":subtype,"balances":{}})).unwrap();
        c.institution = "amex".into();
        assert_eq!(eligible(&c, &a), amex);
        c.institution = "chase".into();
        assert_eq!(eligible(&c, &a), bank);
        c.product = PlaidProduct::Investments;
        assert_eq!(eligible(&c, &a), investment);
        c.product = PlaidProduct::Transactions;
    }
}

#[test]
fn brokerage_daily_quotes_follow_snapshot_calendar_across_utc_midnight() {
    let fetched = chrono::DateTime::parse_from_rfc3339("2026-11-06T02:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let timezone = chrono::FixedOffset::west_opt(8 * 3600).unwrap();
    let mut holding:Holding=serde_json::from_value(json!({"account_id":"external","security_id":"security","quantity":"2.5","institution_price":"40","institution_value":"100","iso_currency_code":"USD"})).unwrap();
    let expected = "2026-11-05T12:00:00Z"
        .parse::<chrono::DateTime<Utc>>()
        .unwrap();
    assert_eq!(
        holdings_quote_timestamp(&holding, fetched, &timezone).unwrap(),
        expected
    );
    holding.institution_price_datetime = Some(fetched);
    assert_eq!(
        holdings_quote_timestamp(&holding, fetched, &timezone).unwrap(),
        expected
    );
    assert_eq!(holding.institution_price_datetime, Some(fetched));
    holding.institution_price_as_of = Some("2026-11-04".parse().unwrap());
    assert_eq!(
        holdings_quote_timestamp(&holding, fetched, &timezone)
            .unwrap()
            .date_naive(),
        "2026-11-04".parse::<chrono::NaiveDate>().unwrap()
    );
    holding.institution_price_as_of = Some("2026-11-06".parse().unwrap());
    assert!(matches!(
        holdings_quote_timestamp(&holding, fetched, &timezone),
        Err(SyncError::Review)
    ));
    holding.institution_price_as_of = None;
    holding.institution_price_datetime = Some(fetched + Duration::seconds(1));
    assert!(matches!(
        holdings_quote_timestamp(&holding, fetched, &timezone),
        Err(SyncError::Review)
    ));
}

#[test]
fn corrections_crossing_cutover_remove_old_ledger_rows_and_retain_corrected_history() {
    let mut m = mapping();
    m.cutover_date = Some(chrono::NaiveDate::from_ymd_opt(2026, 11, 2).unwrap());
    m.cursor = Some("committed".into());
    let mut old = tx("10", "FOOD_AND_DRINK");
    old.date = chrono::NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
    let mut removed = old.clone();
    removed.transaction_id = "removed".into();
    m.source_history = json!({"one": old, "removed": removed});
    let stage = m.staged.as_mut().unwrap();
    stage.checkpoint_in = m.cursor.clone();
    stage.opening_balance = None;
    stage.removed.push("removed".into());
    stage.records[0].amount = "12".parse().unwrap();
    let batch = bank_commit(m.clone()).unwrap();
    assert_eq!(batch.removed_ids, vec!["removed", "one"]);
    assert!(batch.mapping.source_history.get("removed").is_none());
    assert!(cutover_removals(&batch.mapping, m.staged.as_ref().unwrap())
        .unwrap()
        .is_empty());
    assert_eq!(batch.mapping.cursor.as_deref(), Some("next"));
    assert_eq!(batch.mapping.source_history["one"]["date"], "2026-11-01");
    assert!(batch.activities.is_empty());
    // An initial pre-cutover record remains intentionally excluded.
    m.source_history = json!({});
    assert!(bank_commit(m).unwrap().activities.is_empty());
}
