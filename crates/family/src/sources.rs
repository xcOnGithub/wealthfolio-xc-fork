use crate::{engine, model::*};
use chrono::{DateTime, Datelike, Duration, Utc};
use rust_decimal::prelude::ToPrimitive;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use wealthfolio_connect::{broker::BrokerSyncServiceTrait, plaid::state::PlaidRepository};
use wealthfolio_core::{
    accounts::{AccountServiceTrait, TrackingMode},
    activities::ActivityServiceTrait,
    assets::{AssetServiceTrait, QuoteMode},
    goals::GoalServiceTrait,
    portfolio::{
        allocation_targets::{AllocationTargetServiceTrait, DriftServiceTrait, ScopeType},
        net_worth::NetWorthServiceTrait,
        snapshot::SnapshotServiceTrait,
        valuation::ValuationStatus,
    },
    portfolios::{AccountScope, PortfolioServiceTrait},
    quotes::QuoteServiceTrait,
    settings::SettingsServiceTrait,
    Error, Result,
};
use wealthfolio_spending::{
    analytics::{AnalyticsService, ReportRequest},
    budget::BudgetService,
    cash_activities::{CashActivitySearchRequest, CashActivityService, CashActivityStatusFilter},
};

#[derive(Clone)]
pub struct FamilySources {
    pub settings: Arc<dyn SettingsServiceTrait>,
    pub taxonomies: Arc<dyn wealthfolio_core::taxonomies::TaxonomyServiceTrait>,
    pub accounts: Arc<dyn AccountServiceTrait>,
    pub activities: Arc<dyn ActivityServiceTrait>,
    pub assets: Arc<dyn AssetServiceTrait>,
    pub quotes: Arc<dyn QuoteServiceTrait>,
    pub snapshots: Arc<dyn SnapshotServiceTrait>,
    pub net_worth: Arc<dyn NetWorthServiceTrait>,
    pub goals: Arc<dyn GoalServiceTrait>,
    pub targets: Arc<dyn AllocationTargetServiceTrait>,
    pub drift: Arc<dyn DriftServiceTrait>,
    pub portfolios: Arc<dyn PortfolioServiceTrait>,
    pub cash: Arc<CashActivityService>,
    pub analytics: Arc<AnalyticsService>,
    pub budget: Arc<BudgetService>,
    pub brokers: Arc<dyn BrokerSyncServiceTrait>,
    pub plaid: Arc<dyn PlaidRepository>,
}
async fn read<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| Error::Unexpected("Family data read failed".into()))?
}
fn candidate(
    key: String,
    source: &str,
    kind: &str,
    title: String,
    detail: String,
    href: String,
    notify: bool,
) -> Candidate {
    Candidate {
        key,
        source: source.into(),
        kind: kind.into(),
        severity: if notify { "warning" } else { "info" }.into(),
        title: title.chars().take(200).collect(),
        detail: detail.chars().take(1000).collect(),
        href,
        amount: None,
        currency: None,
        notify,
    }
}
fn append(e: &mut Evaluation, source: &str, result: Result<Vec<Candidate>>) {
    match result {
        Ok(mut items) => {
            e.candidates.append(&mut items);
            e.complete_sources.push(source.into());
        }
        Err(_) => e.warnings.push(format!(
            "{source}: data is temporarily unavailable; existing inbox items were preserved."
        )),
    }
}
pub(crate) fn escaped(value: &str) -> String {
    // IDs are opaque route/query components, never user-controlled navigation URLs.
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
impl FamilySources {
    pub async fn evaluate(
        &self,
        settings: &FamilySettings,
        now: DateTime<Utc>,
    ) -> Result<Evaluation> {
        let app = self.settings.get_settings()?;
        let tz: chrono_tz::Tz = app.timezone.parse().unwrap_or(chrono_tz::UTC);
        let day = now.with_timezone(&tz).date_naive();
        let mut e = Evaluation::default();
        for (filter, source, kind) in [
            (
                CashActivityStatusFilter::Uncategorized,
                "uncategorized",
                "uncategorized",
            ),
            (
                CashActivityStatusFilter::NeedsReview,
                "cash_review",
                "flagged_transaction",
            ),
        ] {
            let result = async {
                let mut candidates = vec![];
                let mut offset = 0;
                loop {
                    let request: CashActivitySearchRequest =
                        serde_json::from_value(serde_json::json!({
                            "status": filter, "offset": offset, "limit": 200,
                        }))
                        .map_err(|_| engine::invalid("Invalid cash query"))?;
                    let page = self
                        .cash
                        .search(request, Some(&app.base_currency), &app.timezone)
                        .await
                        .map_err(|_| engine::invalid("Cash data unavailable"))?;
                    let count = page.items.len();
                    for row in page.items {
                        let a = row.activity;
                        candidates.push(candidate(
                            format!("{source}:{}", a.id),
                            source,
                            kind,
                            if source == "uncategorized" {
                                "What was this charge?"
                            } else {
                                "Review flagged transaction"
                            }
                            .into(),
                            a.notes.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| {
                                format!("{} — {}", a.activity_type, a.activity_date.date_naive())
                            }),
                            format!(
                                "/activities?account={}&activity={}",
                                escaped(&a.account_id),
                                escaped(&a.id)
                            ),
                            false,
                        ));
                    }
                    offset += count;
                    if offset >= page.total_count || count == 0 {
                        break;
                    }
                    if offset >= 10_000 {
                        return Err(engine::invalid(
                            "Too many pending transactions; narrow the spending scope",
                        ));
                    }
                }
                Ok(candidates)
            }
            .await;
            append(&mut e, source, result);
        }
        let activities = self.activities.clone();
        append(
            &mut e,
            "flagged",
            read(move || {
                let response = activities.search_activities(
                    1,
                    2000,
                    None,
                    None,
                    None,
                    None,
                    Some(true),
                    None,
                    None,
                    None,
                    None,
                )?;
                if response.meta.total_row_count > 2000 {
                    return Err(engine::invalid("Too many flagged transactions"));
                }
                Ok(response
                    .data
                    .into_iter()
                    .map(|a| {
                        candidate(
                            format!("flagged:{}", a.id),
                            "flagged",
                            "flagged_transaction",
                            "Review flagged transaction".into(),
                            format!("{} — {}", a.account_name, a.date),
                            format!(
                                "/activities?account={}&activity={}",
                                escaped(&a.account_id),
                                escaped(&a.id)
                            ),
                            false,
                        )
                    })
                    .collect())
            })
            .await,
        );
        append(
            &mut e,
            "charges",
            self.charges(settings, now, &app.base_currency, &app.timezone)
                .await,
        );
        let period = day.format("%Y-%m").to_string();
        append(
            &mut e,
            "budget",
            async {
                let budget = self
                    .budget
                    .get_readonly(Some(period.clone()), &app.base_currency, &app.timezone)
                    .await
                    .map_err(|_| engine::invalid("Budget unavailable"))?;
                if budget.computed.calculation_quality.missing_fx_transactions > 0 {
                    return Err(engine::invalid("Budget FX is incomplete"));
                }
                let mut rows = budget.computed.ungrouped_rows;
                rows.extend(
                    budget
                        .computed
                        .group_rows
                        .into_iter()
                        .flat_map(|g| g.categories),
                );
                Ok(rows
                    .into_iter()
                    .filter(|r| {
                        r.taxonomy_id == "spending_categories"
                            && r.target + r.rollover_in > 0.0
                            && r.actual / (r.target + r.rollover_in) >= settings.budget_threshold
                    })
                    .map(|r| {
                        let level = if r.actual >= r.target + r.rollover_in {
                            "over"
                        } else {
                            "threshold"
                        };
                        let mut c = candidate(
                            format!("budget:{period}:{}:{level}", r.category_id),
                            "budget",
                            "budget_threshold",
                            format!(
                                "{} budget: {:.0}% used",
                                r.name,
                                100.0 * r.actual / (r.target + r.rollover_in)
                            ),
                            format!(
                                "{} {:.2} spent of {:.2}",
                                app.base_currency,
                                r.actual,
                                r.target + r.rollover_in
                            ),
                            "/spending/budget".into(),
                            true,
                        );
                        c.amount = Some(r.actual);
                        c.currency = Some(app.base_currency.clone());
                        c
                    })
                    .collect())
            }
            .await,
        );
        append(&mut e, "plaid", self.plaid_attention(settings, now).await);
        let source = self.clone();
        let stale = settings.stale_valuation_days;
        append(
            &mut e,
            "manual",
            read(move || {
                let accounts = source.accounts.get_non_archived_accounts()?;
                let assets = source.assets.get_assets()?;
                let account_ids: Vec<_> = accounts.iter().map(|a| a.id.clone()).collect();
                let snapshots = source
                    .snapshots
                    .get_latest_snapshots_as_of(&account_ids, day)?;
                let held: HashSet<String> = snapshots
                    .values()
                    .flat_map(|s| s.positions.values())
                    .filter(|p| p.quantity != rust_decimal::Decimal::ZERO)
                    .map(|p| p.asset_id.clone())
                    .collect();
                let manual: Vec<_> = assets
                    .into_iter()
                    .filter(|a| {
                        a.is_active
                            && a.quote_mode == QuoteMode::Manual
                            && (a.kind.is_alternative() || held.contains(&a.id))
                    })
                    .collect();
                let ids: Vec<_> = manual.iter().map(|a| a.id.clone()).collect();
                let quotes = source.quotes.get_latest_quotes_as_of(&ids, day)?;
                let mut result = vec![];
                for a in manual {
                    let date = quotes.get(&a.id).map(|q| q.timestamp.date_naive());
                    if date.is_none_or(|d| (day - d).num_days() >= stale) {
                        result.push(candidate(
                            format!("manual:asset:{}", a.id),
                            "manual",
                            "stale_valuation",
                            format!(
                                "Update {} valuation",
                                a.name
                                    .or(a.display_code)
                                    .unwrap_or_else(|| "manual asset".into())
                            ),
                            date.map(|d| format!("Last valuation: {d}"))
                                .unwrap_or_else(|| "No valuation is recorded.".into()),
                            format!("/holdings/{}", escaped(&a.id)),
                            false,
                        ));
                    }
                }
                for a in accounts.iter().filter(|a| {
                    a.is_active
                        && a.tracking_mode == TrackingMode::Holdings
                        && a.provider.as_deref().is_none_or(|p| p == "MANUAL")
                }) {
                    let latest = source.snapshots.get_latest_holdings_keyframe(&a.id, day)?;
                    if latest
                        .as_ref()
                        .is_none_or(|s| (day - s.snapshot_date).num_days() >= stale)
                    {
                        result.push(candidate(
                            format!("manual:account:{}", a.id),
                            "manual",
                            "stale_valuation",
                            format!("Update {} holdings", a.name),
                            latest
                                .map(|s| format!("Last snapshot: {}", s.snapshot_date))
                                .unwrap_or_else(|| "No holdings snapshot is recorded.".into()),
                            format!("/accounts/{}", escaped(&a.id)),
                            false,
                        ));
                    }
                }
                Ok(result)
            })
            .await,
        );
        let source = self.clone();
        append(
            &mut e,
            "broker",
            read(move || {
                let mut result = vec![];
                for a in source.accounts.get_non_archived_accounts()? {
                    if let Some(sync) = source
                        .brokers
                        .get_activity_sync_state(&a.id)
                        .map_err(|_| engine::invalid("Broker state unavailable"))?
                    {
                        if matches!(
                            sync.sync_status,
                            wealthfolio_connect::broker_ingest::SyncStatus::Failed
                                | wealthfolio_connect::broker_ingest::SyncStatus::NeedsReview
                        ) {
                            result.push(candidate(
                                format!("broker:{}", a.id),
                                "broker",
                                "connection_attention",
                                format!("{} needs a sync review", a.name),
                                "Check the institution connection and pending import review."
                                    .into(),
                                "/settings/accounts".into(),
                                true,
                            ));
                        }
                    }
                }
                Ok(result)
            })
            .await,
        );
        append(
            &mut e,
            "drift",
            self.drift_attention(day, &app.base_currency).await,
        );
        let mut bills = vec![];
        for bill in &settings.bills {
            if let Some(next) = engine::next_bill(bill, day) {
                if (next.due_date - day).num_days() <= settings.bill_notice_days {
                    let mut c = candidate(
                        format!("bill:{}:{}", bill.id, next.due_date),
                        "bills",
                        "upcoming_bill",
                        format!("{} is due {}", bill.name, next.due_date),
                        format!("{} {:.2}", bill.currency, bill.amount),
                        "/family?tab=settings".into(),
                        true,
                    );
                    c.amount = Some(bill.amount);
                    c.currency = Some(bill.currency.clone());
                    bills.push(c);
                }
            }
        }
        append(&mut e, "bills", Ok(bills));
        engine::coalesce_transactions(&mut e);
        Ok(e)
    }

    async fn charges(
        &self,
        s: &FamilySettings,
        now: DateTime<Utc>,
        base: &str,
        timezone: &str,
    ) -> Result<Vec<Candidate>> {
        let mut rows = vec![];
        loop {
            let page = self
                .cash
                .search(
                    CashActivitySearchRequest {
                        start_date: Some((now - Duration::days(120)).to_rfc3339()),
                        end_date: Some(now.to_rfc3339()),
                        offset: rows.len(),
                        limit: 500,
                        ..Default::default()
                    },
                    Some(base),
                    timezone,
                )
                .await
                .map_err(|_| engine::invalid("Charge data unavailable"))?;
            let complete = rows.len() + page.items.len() >= page.total_count;
            rows.extend(page.items);
            if complete {
                break;
            }
            if rows.len() >= 10_000 {
                return Err(engine::invalid("Too many charges to evaluate"));
            }
        }
        let mut result = vec![];
        for row in &rows {
            if row.visible_spending_amount <= 0.0
                || row.activity.activity_date < now - Duration::days(7)
            {
                continue;
            }
            let amount = if row.activity.currency == base {
                Some(row.visible_spending_amount)
            } else {
                // Convert the visible consumption portion using the canonical row FX.
                row.net_amount_base
                    .filter(|_| row.net_amount != 0.0)
                    .map(|b| row.visible_spending_amount * (b / row.net_amount).abs())
            };
            if amount.is_none() {
                return Err(engine::invalid("Recent charge FX is unavailable"));
            }
            let merchant = row
                .activity
                .notes
                .as_deref()
                .unwrap_or("")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            let history: Vec<f64> = rows
                .iter()
                .filter(|r| {
                    !merchant.is_empty()
                        && r.activity.currency == row.activity.currency
                        && r.activity.activity_date < row.activity.activity_date
                        && r.activity.activity_date
                            >= row.activity.activity_date - Duration::days(90)
                        && r.visible_spending_amount > 0.0
                        && r.activity
                            .notes
                            .as_deref()
                            .unwrap_or("")
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ")
                            .to_lowercase()
                            == merchant
                })
                .map(|r| r.visible_spending_amount)
                .collect();
            let unusual = if history.len() >= 3 {
                let mean = history.iter().sum::<f64>() / history.len() as f64;
                row.visible_spending_amount > mean * s.unusual_multiplier
                    && amount.is_some_and(|v| v >= s.unusual_minimum)
            } else {
                false
            };
            let large = amount.is_some_and(|a| a >= s.large_charge_amount);
            if large || unusual {
                let kind = if unusual {
                    "unusual_charge"
                } else {
                    "large_charge"
                };
                let mut c = candidate(
                    format!("charge:{}", row.activity.id),
                    "charges",
                    kind,
                    if unusual {
                        "Unusual charge"
                    } else {
                        "Large charge"
                    }
                    .into(),
                    row.activity.notes.clone().unwrap_or_else(|| {
                        format!("Charge on {}", row.activity.activity_date.date_naive())
                    }),
                    format!(
                        "/activities?account={}&activity={}",
                        escaped(&row.activity.account_id),
                        escaped(&row.activity.id)
                    ),
                    true,
                );
                c.amount = amount;
                c.currency = Some(base.into());
                result.push(c);
            }
        }
        Ok(result)
    }

    async fn plaid_attention(
        &self,
        s: &FamilySettings,
        now: DateTime<Utc>,
    ) -> Result<Vec<Candidate>> {
        let connections = self
            .plaid
            .connections()
            .await
            .map_err(|_| engine::invalid("Plaid state unavailable"))?;
        let mappings = self
            .plaid
            .mappings()
            .await
            .map_err(|_| engine::invalid("Plaid mapping state unavailable"))?;
        let mut result = vec![];
        for c in connections {
            if matches!(
                c.status.as_str(),
                "attention" | "expired" | "pending_revocation"
            ) {
                result.push(candidate(
                    format!("plaid:connection:{}", c.id),
                    "plaid",
                    "connection_attention",
                    format!("{} connection needs attention", c.institution),
                    "Reconnect or review the institution in account settings.".into(),
                    "/settings/accounts".into(),
                    true,
                ));
            }
            if let Some(expiry) = c.consent_expires_at {
                if c.status == "connected" && expiry <= now + Duration::days(s.consent_notice_days)
                {
                    result.push(candidate(
                        format!("plaid:consent:{}:{expiry}", c.id),
                        "plaid",
                        "consent_expiring",
                        format!("{} consent expires soon", c.institution),
                        format!("Consent expiration: {}", expiry.date_naive()),
                        "/settings/accounts".into(),
                        true,
                    ));
                }
            }
        }
        for m in mappings {
            if m.staged.as_ref().is_some_and(|s| !s.approved)
                || m.holdings_staged.is_some()
                || matches!(m.status.as_str(), "initial_review" | "attention")
            {
                result.push(candidate(
                    format!("plaid:review:{}", m.id),
                    "plaid",
                    "reconciliation",
                    "Review a pending account reconciliation".into(),
                    "A broker or statement-assisted import is waiting for review.".into(),
                    "/settings/accounts".into(),
                    false,
                ));
            }
        }
        Ok(result)
    }

    async fn drift_attention(&self, day: chrono::NaiveDate, base: &str) -> Result<Vec<Candidate>> {
        if self.net_worth.get_net_worth(day).await?.value_status != ValuationStatus::Complete {
            return Err(engine::invalid("Portfolio values incomplete"));
        }
        let mut result = vec![];
        for target in self
            .targets
            .list_targets()?
            .into_iter()
            .filter(|t| t.archived_at.is_none())
        {
            let filter = match target.scope_type {
                ScopeType::All => AccountScope::All,
                ScopeType::Account => AccountScope::Account {
                    account_id: target
                        .scope_id
                        .clone()
                        .ok_or_else(|| engine::invalid("Missing target account"))?,
                },
                ScopeType::Portfolio => AccountScope::Portfolio {
                    portfolio_id: target
                        .scope_id
                        .clone()
                        .ok_or_else(|| engine::invalid("Missing target portfolio"))?,
                },
            };
            let scope = self.portfolios.resolve_account_scope(&filter, base)?;
            if scope.account_ids.is_empty() {
                continue;
            }
            let report = self
                .drift
                .get_drift_report_for_target(&target.id, &scope.account_ids, base, &scope.scope_id)
                .await?;
            if report.out_of_band_count > 0 {
                result.push(candidate(format!("drift:{}",target.id),"drift","portfolio_drift",format!("{} is outside its target bands",target.name),
                    format!("{} allocation categories need review; largest drift {:.2} percentage points.",report.out_of_band_count,report.max_drift_bps as f64/100.0),"/insights?tab=overview".into(),true));
            }
        }
        Ok(result)
    }

    pub async fn digest(
        &self,
        month: &str,
        settings: &FamilySettings,
        now: DateTime<Utc>,
    ) -> Result<MonthlyDigest> {
        let app = self.settings.get_settings()?;
        let (start, end) = engine::month_bounds(month)?;
        let timezone: chrono_tz::Tz = app.timezone.parse().unwrap_or(chrono_tz::UTC);
        if end >= now.with_timezone(&timezone).date_naive() {
            return Err(engine::invalid("Choose a completed calendar month"));
        }
        let previous_end = start - Duration::days(1);
        let previous_start = previous_end
            .with_day(1)
            .ok_or_else(|| engine::invalid("Invalid month"))?;
        let tz = wealthfolio_core::utils::time_utils::parse_user_timezone_or_default(&app.timezone);
        let (from, to) = wealthfolio_core::utils::time_utils::local_date_range_utc_bounds(
            Some(start),
            Some(end),
            tz,
        )?;
        let (old_from, old_to) = wealthfolio_core::utils::time_utils::local_date_range_utc_bounds(
            Some(previous_start),
            Some(previous_end),
            tz,
        )?;
        let from = from.ok_or_else(|| engine::invalid("Missing report date"))?;
        let to = to.ok_or_else(|| engine::invalid("Missing report date"))?;
        let old_from = old_from.ok_or_else(|| engine::invalid("Missing report date"))?;
        let old_to = old_to.ok_or_else(|| engine::invalid("Missing report date"))?;
        let mut d = MonthlyDigest {
            month: month.into(),
            generated_at: now,
            currency: app.base_currency.clone(),
            net_worth: None,
            net_worth_change: None,
            income: None,
            spending: None,
            savings_rate: None,
            movers: vec![],
            goals: vec![],
            upcoming_bills: vec![],
            warnings: vec![],
            text: String::new(),
        };
        let current = self.net_worth.get_net_worth(end).await;
        let old = self.net_worth.get_net_worth(previous_end).await;
        match (current, old) {
            (Ok(c), Ok(p))
                if c.value_status == ValuationStatus::Complete
                    && p.value_status == ValuationStatus::Complete =>
            {
                d.net_worth = c.net_worth.to_f64();
                d.net_worth_change = (c.net_worth - p.net_worth).to_f64();
            }
            _ => d
                .warnings
                .push("Net-worth comparison is unavailable or incomplete.".into()),
        }
        let spending_configured = self
            .cash
            .spending_account_ids()
            .await
            .is_ok_and(|ids| !ids.is_empty());
        let report = self
            .analytics
            .monthly_report(
                ReportRequest {
                    start_date: from.to_rfc3339(),
                    end_date: (to - Duration::milliseconds(1)).to_rfc3339(),
                    account_ids: None,
                },
                &app.timezone,
                &app.base_currency,
            )
            .await;
        let old_report = self
            .analytics
            .monthly_report(
                ReportRequest {
                    start_date: old_from.to_rfc3339(),
                    end_date: (old_to - Duration::milliseconds(1)).to_rfc3339(),
                    account_ids: None,
                },
                &app.timezone,
                &app.base_currency,
            )
            .await;
        match (report, old_report) {
            (Ok(c), Ok(p))
                if spending_configured
                    && c.current.calculation_quality.missing_fx_transactions == 0
                    && p.current.calculation_quality.missing_fx_transactions == 0 =>
            {
                d.income = Some(c.current.income);
                d.spending = Some(c.current.outflow);
                d.savings_rate = (c.current.income > 0.0)
                    .then(|| (c.current.income - c.current.outflow) / c.current.income);
                let mut categories: HashMap<String, (String, f64, f64)> = HashMap::new();
                for row in c.spending_breakdown {
                    let name = self
                        .taxonomies
                        .get_taxonomy(&row.taxonomy_id)
                        .ok()
                        .flatten()
                        .and_then(|t| {
                            t.categories
                                .into_iter()
                                .find(|c| c.id == row.category_id)
                                .map(|c| c.name)
                        })
                        .unwrap_or_else(|| "Uncategorized".into());
                    let entry = categories
                        .entry(row.category_id.clone())
                        .or_insert((name, 0.0, 0.0));
                    entry.1 += row.amount;
                }
                for row in p.spending_breakdown {
                    let name = self
                        .taxonomies
                        .get_taxonomy(&row.taxonomy_id)
                        .ok()
                        .flatten()
                        .and_then(|t| {
                            t.categories
                                .into_iter()
                                .find(|c| c.id == row.category_id)
                                .map(|c| c.name)
                        })
                        .unwrap_or_else(|| "Uncategorized".into());
                    let entry = categories
                        .entry(row.category_id.clone())
                        .or_insert((name, 0.0, 0.0));
                    entry.2 += row.amount;
                }
                d.movers = categories
                    .into_values()
                    .map(|(name, current, previous)| SpendingMover {
                        name,
                        current,
                        previous,
                        change: current - previous,
                    })
                    .collect();
                d.movers
                    .sort_by(|a, b| b.change.abs().total_cmp(&a.change.abs()));
                d.movers.truncate(5);
            }
            _ => d
                .warnings
                .push("Spending comparison is unavailable or missing currency conversions.".into()),
        }
        match self.goals.get_goals() {
            Ok(goals) => {
                d.goals = goals
                    .into_iter()
                    .filter(|g| g.status_lifecycle != "archived")
                    .map(|g| FamilyGoal {
                        title: g.title,
                        progress: g.summary_progress,
                        current: g.summary_current_value,
                        target: g.summary_target_amount.or(g.target_amount),
                        currency: g.currency,
                    })
                    .collect()
            }
            Err(_) => d.warnings.push("Goal progress is unavailable.".into()),
        }
        let next_start = end + Duration::days(1);
        let (_, next_end) = engine::month_bounds(&next_start.format("%Y-%m").to_string())?;
        d.upcoming_bills = settings
            .bills
            .iter()
            .filter_map(|b| engine::next_bill(b, next_start))
            .filter(|b| b.due_date <= next_end)
            .collect();
        d.upcoming_bills.sort_by_key(|b| b.due_date);
        d.warnings.push("Net worth uses recorded valuations; manual assets may need updating. Goal progress is the latest recorded value, not a historical month-end snapshot.".into());
        d.text = engine::render_digest(&d);
        Ok(d)
    }
}

#[async_trait::async_trait]
pub trait FamilyFactSource: Send + Sync {
    async fn evaluate(&self, settings: &FamilySettings, now: DateTime<Utc>) -> Result<Evaluation>;
    async fn digest(
        &self,
        month: &str,
        settings: &FamilySettings,
        now: DateTime<Utc>,
    ) -> Result<MonthlyDigest>;
    fn timezone(&self) -> Result<String>;
    fn validate_account(&self, id: &str) -> Result<()>;
}
#[async_trait::async_trait]
impl FamilyFactSource for FamilySources {
    async fn evaluate(&self, settings: &FamilySettings, now: DateTime<Utc>) -> Result<Evaluation> {
        FamilySources::evaluate(self, settings, now).await
    }
    async fn digest(
        &self,
        month: &str,
        settings: &FamilySettings,
        now: DateTime<Utc>,
    ) -> Result<MonthlyDigest> {
        FamilySources::digest(self, month, settings, now).await
    }
    fn timezone(&self) -> Result<String> {
        Ok(self.settings.get_settings()?.timezone)
    }
    fn validate_account(&self, id: &str) -> Result<()> {
        self.accounts.get_account(id).map(|_| ())
    }
}
