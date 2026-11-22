//! Read-only activity from reviewed statements, independent of pricing and ledger replay.
use super::reconstruction::StatementEvent;
use super::{daily::StoredDailyStatement, invalid, live_boundary, StatementService};
use crate::{accounts::account_types, errors::Result, portfolio::coordinator::blocking};
use chrono::NaiveDate;
use serde::Serialize;
use std::{
    cmp::Reverse,
    collections::{BTreeSet, HashMap},
};

const PAGE_SIZE: usize = 50;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementActivityRow {
    #[serde(flatten)]
    pub event: StatementEvent,
    pub statement_date: NaiveDate,
    pub currency: String,
    pub asset_label: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementActivityPage {
    pub rows: Vec<StatementActivityRow>,
    pub next_offset: Option<usize>,
}

fn activity_page(
    mut statements: Vec<StoredDailyStatement>,
    boundary: Option<NaiveDate>,
    offset: usize,
) -> StatementActivityPage {
    // Reviewed periods cannot overlap. Sort periods and then their rows so
    // a settlement day has a stable order even when several fills are equal.
    statements.sort_unstable_by_key(|review| Reverse(review.interval.through));
    let mut rows = statements
        .into_iter()
        .filter(|review| boundary.is_none_or(|date| review.interval.from < date))
        .flat_map(|review| {
            let mut interval = review.interval;
            interval
                .events
                .retain(|event| boundary.is_none_or(|date| event.date < date));
            interval.events.sort_unstable_by(|a, b| {
                b.date
                    .cmp(&a.date)
                    .then_with(|| a.source_page.cmp(&b.source_page))
                    .then_with(|| a.source_line.cmp(&b.source_line))
                    .then_with(|| a.source_id.cmp(&b.source_id))
            });
            interval
                .events
                .into_iter()
                .map(move |event| StatementActivityRow {
                    event,
                    statement_date: interval.through,
                    currency: interval.currency.clone(),
                    asset_label: None,
                })
        })
        .skip(offset)
        .take(PAGE_SIZE + 1)
        .collect::<Vec<_>>();
    let next_offset = (rows.len() > PAGE_SIZE).then(|| offset + PAGE_SIZE);
    rows.truncate(PAGE_SIZE);
    StatementActivityPage { rows, next_offset }
}

impl StatementService {
    pub async fn list_activity(
        &self,
        account_id: String,
        offset: usize,
    ) -> Result<StatementActivityPage> {
        let this = self.clone();
        blocking(move || {
            let account = this.accounts.get_by_id(&account_id)?;
            if account.account_type != account_types::SECURITIES {
                return Err(invalid());
            }
            let statements = this.repository.list_daily(&account_id)?;
            let boundary = if statements.is_empty() {
                None
            } else {
                let valuations =
                    this.valuations
                        .get_historical_valuations(&account_id, None, None)?;
                live_boundary(&valuations, true)
            };
            let mut page = activity_page(statements, boundary, offset);
            let ids: BTreeSet<_> = page
                .rows
                .iter()
                .filter_map(|row| row.event.asset_id.clone())
                .collect();
            if let Some(pricing) = this.pricing.as_ref().filter(|_| !ids.is_empty()) {
                let ids: Vec<_> = ids.into_iter().collect();
                let labels: HashMap<_, _> = pricing
                    .assets
                    .list_by_asset_ids(&ids)?
                    .into_iter()
                    .filter_map(|asset| {
                        asset
                            .display_code
                            .or(asset.name)
                            .map(|label| (asset.id, label))
                    })
                    .collect();
                for row in &mut page.rows {
                    row.asset_label = row
                        .event
                        .asset_id
                        .as_ref()
                        .and_then(|id| labels.get(id))
                        .cloned();
                }
            }
            Ok(page)
        })
        .await
    }
}
