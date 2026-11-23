use crate::model::*;
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use std::collections::HashSet;
use wealthfolio_core::{Error, Result};

pub fn invalid(message: &str) -> Error {
    Error::InvalidConfigValue(message.into())
}

pub fn validate_settings(s: &FamilySettings) -> Result<()> {
    if !s.large_charge_amount.is_finite()
        || s.large_charge_amount <= 0.0
        || !s.unusual_multiplier.is_finite()
        || !(1.5..=20.0).contains(&s.unusual_multiplier)
        || !s.unusual_minimum.is_finite()
        || s.unusual_minimum <= 0.0
        || !s.budget_threshold.is_finite()
        || !(0.1..=2.0).contains(&s.budget_threshold)
        || !(1..=90).contains(&s.consent_notice_days)
        || !(1..=730).contains(&s.stale_valuation_days)
        || !(1..=90).contains(&s.bill_notice_days)
    {
        return Err(invalid("Invalid family alert thresholds"));
    }
    if s.recipients.len() > 5 || s.bills.len() > 100 {
        return Err(invalid("Too many recipients or bills"));
    }
    let email = |v: &str| {
        v.len() <= 254
            && !v.contains(['\r', '\n', ' '])
            && v.split_once('@')
                .is_some_and(|(a, b)| !a.is_empty() && b.contains('.') && !b.ends_with('.'))
    };
    if s.recipients.iter().any(|v| !email(v))
        || s.email_from.len() > 320
        || s.email_from.contains(['\r', '\n'])
    {
        return Err(invalid("Invalid family email address"));
    }
    if (s.email_alerts || s.email_digest)
        && (s.recipients.is_empty() || s.email_from.trim().is_empty())
    {
        return Err(invalid(
            "Email delivery needs a sender and at least one recipient",
        ));
    }
    let mut ids = HashSet::new();
    for bill in &s.bills {
        if bill.id.is_empty()
            || bill.id.len() > 80
            || !ids.insert(&bill.id)
            || bill.name.trim().is_empty()
            || bill.name.len() > 160
            || !bill.amount.is_finite()
            || bill.amount <= 0.0
            || bill.currency.len() != 3
            || !bill.currency.bytes().all(|b| b.is_ascii_uppercase())
        {
            return Err(invalid("Invalid annual bill"));
        }
    }
    Ok(())
}

pub fn month_bounds(month: &str) -> Result<(NaiveDate, NaiveDate)> {
    if month.len() != 7 {
        return Err(invalid("Expected YYYY-MM"));
    }
    let start = NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d")
        .map_err(|_| invalid("Invalid month"))?;
    if !(1900..=9998).contains(&start.year()) {
        return Err(invalid("Month is outside the supported range"));
    }
    let next = if start.month() == 12 {
        NaiveDate::from_ymd_opt(start.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(start.year(), start.month() + 1, 1)
    }
    .ok_or_else(|| invalid("Invalid month"))?;
    Ok((start, next - Duration::days(1)))
}

pub fn due_month(now: DateTime<Utc>, timezone: &str) -> Option<String> {
    let tz: chrono_tz::Tz = timezone.parse().unwrap_or(chrono_tz::UTC);
    let local = now.with_timezone(&tz);
    let first = tz
        .with_ymd_and_hms(local.year(), local.month(), 1, 9, 0, 0)
        .earliest()?;
    if local < first {
        return None;
    }
    Some(
        (local.date_naive().with_day(1)? - Duration::days(1))
            .format("%Y-%m")
            .to_string(),
    )
}

pub fn next_bill(bill: &FamilyBill, from: NaiveDate) -> Option<FamilyBill> {
    if !bill.annual {
        return (bill.due_date >= from).then(|| bill.clone());
    }
    let mut result = bill.clone();
    let occurrence = |year| {
        NaiveDate::from_ymd_opt(year, bill.due_date.month(), bill.due_date.day())
            .or_else(|| NaiveDate::from_ymd_opt(year, bill.due_date.month(), 28))
    };
    result.due_date = occurrence(from.year().max(bill.due_date.year()))?;
    if result.due_date < from {
        result.due_date = occurrence(result.due_date.year() + 1)?;
    }
    Some(result)
}

pub fn reconcile(
    state: &mut FamilyState,
    evaluation: Evaluation,
    now: DateTime<Utc>,
) -> Vec<String> {
    let seen: HashSet<&str> = evaluation
        .candidates
        .iter()
        .map(|c| c.key.as_str())
        .collect();
    for item in &mut state.items {
        if item.active
            && evaluation.complete_sources.contains(&item.fact.source)
            && !seen.contains(item.fact.key.as_str())
        {
            item.active = false;
            item.status = InboxStatus::Resolved;
            item.updated_at = now;
            item.snoozed_until = None;
        }
        if item.status == InboxStatus::Snoozed
            && item.snoozed_until.is_some_and(|until| until <= now)
        {
            item.status = if item.active {
                InboxStatus::Open
            } else {
                InboxStatus::Resolved
            };
            item.snoozed_until = None;
        }
    }
    let mut notify = vec![];
    for candidate in evaluation.candidates {
        if let Some(item) = state.items.iter_mut().find(|i| i.fact.key == candidate.key) {
            if !item.active {
                item.id = uuid::Uuid::now_v7().to_string();
                item.first_seen = now;
                item.status = InboxStatus::Open;
                item.snoozed_until = None;
                if candidate.notify {
                    notify.push(item.id.clone());
                }
            }
            item.active = true;
            item.fact = candidate;
            item.updated_at = now;
        } else {
            let id = uuid::Uuid::now_v7().to_string();
            if candidate.notify {
                notify.push(id.clone());
            }
            state.items.push(InboxItem {
                id,
                fact: candidate,
                status: InboxStatus::Open,
                active: true,
                first_seen: now,
                updated_at: now,
                snoozed_until: None,
                note: String::new(),
            });
        }
    }
    state
        .items
        .retain(|i| i.active || i.updated_at > now - Duration::days(180));
    state.warnings = evaluation.warnings;
    state.last_evaluated = Some(now);
    notify
}

pub fn apply_action(
    state: &mut FamilyState,
    action: InboxAction,
    now: DateTime<Utc>,
) -> Result<()> {
    let item = state
        .items
        .iter_mut()
        .find(|i| i.id == action.id)
        .ok_or_else(|| invalid("Inbox item not found"))?;
    if let Some(note) = action.note {
        if note.len() > 2000 {
            return Err(invalid("Family note is too long"));
        }
        item.note = note;
    }
    match action.action.as_str() {
        "note" => {}
        "acknowledge" => item.status = InboxStatus::Acknowledged,
        "resolve" => item.status = InboxStatus::Resolved,
        "reopen" => item.status = InboxStatus::Open,
        "snooze" => {
            let until = action
                .snoozed_until
                .ok_or_else(|| invalid("Snooze date is required"))?;
            if until <= now || until > now + Duration::days(365) {
                return Err(invalid("Invalid snooze date"));
            }
            item.status = InboxStatus::Snoozed;
            item.snoozed_until = Some(until);
        }
        _ => return Err(invalid("Unknown inbox action")),
    }
    if item.status != InboxStatus::Snoozed {
        item.snoozed_until = None;
    }
    item.updated_at = now;
    Ok(())
}

pub fn render_digest(d: &MonthlyDigest) -> String {
    let money = |v: Option<f64>| {
        v.map(|v| format!("{} {:.2}", d.currency, v))
            .unwrap_or_else(|| "unavailable".into())
    };
    let rate = d
        .savings_rate
        .map(|v| format!("{:.1}%", v * 100.0))
        .unwrap_or_else(|| "unavailable".into());
    let mut text = format!("Family Money Meeting — {}\n\nNet worth: {} (change {})\nIncome: {}\nSpending: {}\nSavings rate: {}\n",d.month,money(d.net_worth),money(d.net_worth_change),money(d.income),money(d.spending),rate);
    text.push_str("\nTop spending movers versus the previous calendar month:\n");
    for m in &d.movers {
        text.push_str(&format!(
            "- {}: {} {:.2} ({:+.2})\n",
            m.name, d.currency, m.current, m.change
        ));
    }
    if d.movers.is_empty() {
        text.push_str("- No comparable category data.\n");
    }
    text.push_str("\nGoal progress (latest recorded values):\n");
    for goal in d.goals.iter().take(5) {
        text.push_str(&format!(
            "- {}: {}\n",
            goal.title,
            goal.progress
                .map(|v| format!("{:.1}%", v * 100.0))
                .unwrap_or_else(|| "unavailable".into())
        ));
    }
    if d.goals.len() > 5 {
        text.push_str("- More goals are available in the app.\n");
    }
    if d.goals.is_empty() {
        text.push_str("- No active goals.\n");
    }
    text.push_str("\nUpcoming big expenses:\n");
    for b in d.upcoming_bills.iter().take(5) {
        text.push_str(&format!(
            "- {}: {} {:.2} on {}\n",
            b.name, b.currency, b.amount, b.due_date
        ));
    }
    if d.upcoming_bills.len() > 5 {
        text.push_str("- More upcoming expenses are available in the app.\n");
    }
    if d.upcoming_bills.is_empty() {
        text.push_str("- No configured bills due next month.\n");
    }
    text.push_str("\nTalk together: What changed? What needs a decision? What is one action for next month?\n");
    for warning in &d.warnings {
        text.push_str(&format!("\nData note: {warning}\n"));
    }
    text
}

/// One task per transaction even when it is both uncategorized and flagged.
/// All constituent reads must succeed before a missing transaction is cleared.
pub fn coalesce_transactions(e: &mut Evaluation) {
    let sources = ["uncategorized", "cash_review", "flagged"];
    let complete = sources
        .iter()
        .all(|s| e.complete_sources.iter().any(|v| v == s));
    e.complete_sources
        .retain(|s| !sources.contains(&s.as_str()));
    if complete {
        e.complete_sources.push("transactions".into());
    }
    let mut transactions = std::collections::BTreeMap::<String, Candidate>::new();
    let mut others = vec![];
    for mut c in e.candidates.drain(..) {
        if sources.contains(&c.source.as_str()) {
            c.key = format!(
                "transaction:{}",
                c.key.split_once(':').map(|(_, id)| id).unwrap_or(&c.key)
            );
            c.source = "transactions".into();
            if !transactions
                .get(&c.key)
                .is_some_and(|old| old.kind == "flagged_transaction")
            {
                transactions.insert(c.key.clone(), c);
            }
        } else {
            others.push(c);
        }
    }
    others.extend(transactions.into_values());
    e.candidates = others;
}

#[cfg(test)]
mod tests {
    use super::*;
    fn now() -> DateTime<Utc> {
        "2026-05-15T17:00:00Z".parse().unwrap()
    }
    fn fact(key: &str, source: &str) -> Candidate {
        Candidate {
            key: key.into(),
            source: source.into(),
            kind: "large_charge".into(),
            severity: "warning".into(),
            title: "Charge".into(),
            detail: "Synthetic".into(),
            href: "/family".into(),
            amount: Some(1000.0),
            currency: Some("USD".into()),
            notify: true,
        }
    }
    fn evaluation(candidates: Vec<Candidate>, complete: bool) -> Evaluation {
        Evaluation {
            candidates,
            complete_sources: if complete {
                vec!["charges".into()]
            } else {
                vec![]
            },
            warnings: vec![],
        }
    }
    #[test]
    fn populated_inbox_round_trips_fractional_and_zero_amounts() {
        for amount in [None, Some(0.0), Some(1200.25)] {
            let mut state = FamilyState::default();
            let mut candidate = fact("one", "charges");
            candidate.amount = amount;
            reconcile(&mut state, evaluation(vec![candidate], true), now());
            state.items[0].note = "Synthetic family note".into();
            let json = serde_json::to_string(&state).unwrap();
            let restored: FamilyState =
                serde_json::from_str(&json).expect("Populated inbox must survive a restart");
            assert_eq!(restored.items[0].fact.amount, amount);
            assert_eq!(restored.items[0].note, "Synthetic family note");
        }
    }

    #[test]
    fn digest_keeps_unknown_values_distinct_from_genuine_zeroes() {
        let mut d: MonthlyDigest = serde_json::from_value(serde_json::json!({
            "month": "2026-04", "generatedAt": now(), "currency": "USD",
            "netWorth": null, "netWorthChange": null, "income": null,
            "spending": 0.0, "savingsRate": null, "movers": [], "goals": [],
            "upcomingBills": [], "warnings": ["Synthetic incomplete data"], "text": ""
        }))
        .unwrap();
        let text = render_digest(&d);
        assert!(text.contains("Net worth: unavailable (change unavailable)"));
        assert!(text.contains("Income: unavailable"));
        assert!(text.contains("Spending: USD 0.00"));
        assert!(text.contains("Savings rate: unavailable"));
        assert!(text.contains("Data note: Synthetic incomplete data"));
        d.net_worth = Some(0.0);
        d.net_worth_change = Some(0.0);
        assert!(render_digest(&d).contains("Net worth: USD 0.00 (change USD 0.00)"));
    }

    #[test]
    fn repeated_detection_keeps_notes_acknowledgements_and_episode() {
        let mut s = FamilyState::default();
        let ids = reconcile(
            &mut s,
            evaluation(vec![fact("one", "charges")], true),
            now(),
        );
        apply_action(
            &mut s,
            InboxAction {
                id: ids[0].clone(),
                action: "acknowledge".into(),
                note: Some("Ask together".into()),
                snoozed_until: None,
            },
            now(),
        )
        .unwrap();
        assert!(reconcile(
            &mut s,
            evaluation(vec![fact("one", "charges")], true),
            now() + Duration::minutes(5)
        )
        .is_empty());
        assert_eq!(s.items[0].id, ids[0]);
        assert_eq!(s.items[0].status, InboxStatus::Acknowledged);
        assert_eq!(s.items[0].note, "Ask together");
    }
    #[test]
    fn failed_sources_do_not_clear_items_but_real_recovery_does() {
        let mut s = FamilyState::default();
        reconcile(
            &mut s,
            evaluation(vec![fact("one", "charges")], true),
            now(),
        );
        reconcile(&mut s, evaluation(vec![], false), now());
        assert!(s.items[0].active);
        reconcile(&mut s, evaluation(vec![], true), now());
        assert!(!s.items[0].active);
        assert_eq!(s.items[0].status, InboxStatus::Resolved);
        let previous = s.items[0].id.clone();
        let reopened = reconcile(
            &mut s,
            evaluation(vec![fact("one", "charges")], true),
            now() + Duration::days(1),
        );
        assert_eq!(reopened.len(), 1);
        assert_ne!(previous, reopened[0]);
    }
    #[test]
    fn dismiss_is_retained_until_condition_really_clears() {
        let mut s = FamilyState::default();
        reconcile(
            &mut s,
            evaluation(vec![fact("one", "charges")], true),
            now(),
        );
        let id = s.items[0].id.clone();
        apply_action(
            &mut s,
            InboxAction {
                id,
                action: "resolve".into(),
                note: None,
                snoozed_until: None,
            },
            now(),
        )
        .unwrap();
        assert!(reconcile(
            &mut s,
            evaluation(vec![fact("one", "charges")], true),
            now()
        )
        .is_empty());
        assert_eq!(s.items[0].status, InboxStatus::Resolved);
    }
    #[test]
    fn snooze_expires_without_generating_another_notification() {
        let mut s = FamilyState::default();
        reconcile(
            &mut s,
            evaluation(vec![fact("one", "charges")], true),
            now(),
        );
        let id = s.items[0].id.clone();
        apply_action(
            &mut s,
            InboxAction {
                id,
                action: "snooze".into(),
                note: None,
                snoozed_until: Some(now() + Duration::days(7)),
            },
            now(),
        )
        .unwrap();
        assert!(reconcile(
            &mut s,
            evaluation(vec![fact("one", "charges")], true),
            now() + Duration::days(8)
        )
        .is_empty());
        assert_eq!(s.items[0].status, InboxStatus::Open);
    }
    #[test]
    fn one_transaction_is_not_queued_twice_and_partial_reads_preserve_it() {
        let mut e = Evaluation {
            candidates: vec![
                fact("uncategorized:id", "uncategorized"),
                fact("cash_review:id", "cash_review"),
            ],
            complete_sources: vec!["uncategorized".into(), "cash_review".into()],
            warnings: vec![],
        };
        e.candidates[0].kind = "uncategorized".into();
        e.candidates[1].kind = "flagged_transaction".into();
        coalesce_transactions(&mut e);
        assert_eq!(e.candidates.len(), 1);
        assert_eq!(e.candidates[0].key, "transaction:id");
        assert_eq!(e.candidates[0].kind, "flagged_transaction");
        assert!(!e.complete_sources.contains(&"transactions".into()));
    }
    #[test]
    fn months_are_calendar_months_including_leap_and_year_boundaries() {
        assert_eq!(month_bounds("2024-02").unwrap().1.to_string(), "2024-02-29");
        assert_eq!(month_bounds("2026-12").unwrap().1.to_string(), "2026-12-31");
        assert!(month_bounds("2026-13").is_err());
        assert!(month_bounds("2026-1").is_err());
    }
    #[test]
    fn monthly_due_honors_profile_timezone_not_server_midnight() {
        assert_eq!(
            due_month(
                "2026-04-01T15:59:00Z".parse().unwrap(),
                "America/Los_Angeles"
            ),
            None
        );
        assert_eq!(
            due_month(
                "2026-04-01T16:00:00Z".parse().unwrap(),
                "America/Los_Angeles"
            ),
            Some("2026-03".into())
        );
        assert_eq!(
            due_month(
                "2026-01-01T17:00:00Z".parse().unwrap(),
                "America/Los_Angeles"
            ),
            Some("2025-12".into())
        );
        assert_eq!(
            due_month(
                "2026-05-20T17:00:00Z".parse().unwrap(),
                "America/Los_Angeles"
            ),
            Some("2026-04".into())
        );
    }
    #[test]
    fn annual_bill_rolls_forward_and_leap_anniversaries_use_february_28() {
        let b = FamilyBill {
            id: "annual".into(),
            name: "Insurance".into(),
            due_date: "2024-02-29".parse().unwrap(),
            amount: 1200.0,
            currency: "USD".into(),
            annual: true,
        };
        assert_eq!(
            next_bill(&b, "2025-01-01".parse().unwrap())
                .unwrap()
                .due_date
                .to_string(),
            "2025-02-28"
        );
        assert_eq!(
            next_bill(&b, "2025-03-01".parse().unwrap())
                .unwrap()
                .due_date
                .to_string(),
            "2026-02-28"
        );
        assert!(next_bill(
            &FamilyBill { annual: false, ..b },
            "2025-03-01".parse().unwrap()
        )
        .is_none());
    }
    #[test]
    fn thresholds_reject_nonfinite_values_and_outbound_defaults_are_off() {
        let s = FamilySettings::default();
        assert!(!s.email_alerts && !s.email_digest && !s.webhook_alerts && !s.webhook_digest);
        validate_settings(&s).unwrap();
        assert!(validate_settings(&FamilySettings {
            large_charge_amount: f64::NAN,
            ..s.clone()
        })
        .is_err());
        assert!(validate_settings(&FamilySettings {
            budget_threshold: 0.0,
            ..s
        })
        .is_err());
    }
    #[test]
    fn note_validation_does_not_hide_an_unrecognized_action() {
        let mut s = FamilyState::default();
        reconcile(
            &mut s,
            evaluation(vec![fact("one", "charges")], true),
            now(),
        );
        let id = s.items[0].id.clone();
        assert!(apply_action(
            &mut s,
            InboxAction {
                id,
                action: "delete_everything".into(),
                note: None,
                snoozed_until: None
            },
            now()
        )
        .is_err());
    }
}
