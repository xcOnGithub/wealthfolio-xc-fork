use super::*;
use crate::plaid::investments::{self, InvestmentLedger, OpeningPosition};

impl PlaidService {
    pub async fn preview_investments(
        &self,
        id: &str,
        admit: Admission<'_>,
    ) -> state::Result<Value> {
        admit()?;
        let mut m = self.mapping(id).await?;
        let c = self.connection(&m.connection_id).await?;
        let stage = self.prepare_holdings_range(&m, &c, true, admit).await?;
        let plan = investments::plan(&m, &stage)?;
        let suggested_costs = if m.investment_ledger.is_none() {
            self.suggest_opening_unit_costs(&m, &stage, &plan).await
        } else {
            BTreeMap::new()
        };
        let openings: Vec<_> = plan.opening_quantities.iter()
            .filter(|(_, qty)| **qty != Decimal::ZERO)
            .map(|(id, qty)| json!({
                "securityId": id,
                "name": plan.securities.get(id).and_then(|s| s.name.as_ref().or(s.ticker_symbol.as_ref())),
                "quantity": qty.to_string(),
                "unitCost": m.investment_ledger.as_ref().and_then(|l| l.opening_positions.get(id)).map(|p| &p.unit_cost),
                "suggestedUnitCost": suggested_costs.get(id),
            })).collect();
        let records: Vec<_> = plan
            .drafts
            .iter()
            .map(|row| {
                json!({
                    "id": row.source, "date": row.date, "type": row.kind,
                    "securityId": row.security,
                    "name": row.security.as_ref().and_then(|id| plan.securities.get(id))
                        .and_then(|s| s.name.as_ref().or(s.ticker_symbol.as_ref())),
                    "quantity": row.quantity.to_string(), "price": row.price.to_string(),
                    "cash": row.cash.to_string(), "fee": row.fee.to_string(),
                    "requiresTransferCost": row.kind == "TRANSFER_IN",
                    "transferUnitCost": m.investment_ledger.as_ref().and_then(|l| l.transfer_unit_costs.get(&row.source)),
                })
            })
            .collect();
        let issues: Vec<_> = plan
            .issues
            .iter()
            .map(|issue| {
                let name = stage
                    .history
                    .iter()
                    .find(|tx| {
                        tx.investment_transaction_id == issue.record_id
                            || format!("investment:{}", tx.investment_transaction_id)
                                == issue.record_id
                    })
                    .map(|tx| &tx.name)
                    .or_else(|| {
                        plan.securities
                            .get(&issue.record_id)
                            .and_then(|s| s.name.as_ref().or(s.ticker_symbol.as_ref()))
                    });
                json!({"recordId": issue.record_id, "securityId": issue.security_id, "reason": issue.reason, "name": name})
            })
            .collect();
        let value = json!({
            "mappingId": m.id, "digest": stage.digest, "initial": m.investment_ledger.is_none(),
            "startDate": plan.start_date, "currency": m.currency,
            "openingCash": plan.opening_cash.to_string(), "currentCash": plan.current_cash.to_string(),
            "openingPositions": openings, "records": records, "issues": issues,
            "sourceTransactionCount": stage.history.len(),
        });
        m.holdings_staged = Some(stage);
        admit()?;
        self.repository.put_mappings(vec![m]).await?;
        Ok(value)
    }

    pub async fn approve_investments(
        &self,
        id: &str,
        digest: &str,
        opening_costs: BTreeMap<String, String>,
        confirmed: bool,
        admit: Admission<'_>,
    ) -> state::Result<()> {
        self.approve_investments_with_transfer_costs(
            id,
            digest,
            opening_costs,
            BTreeMap::new(),
            confirmed,
            admit,
        )
        .await
    }

    pub async fn approve_investments_with_transfer_costs(
        &self,
        id: &str,
        digest: &str,
        opening_costs: BTreeMap<String, String>,
        transfer_costs: BTreeMap<String, String>,
        confirmed: bool,
        admit: Admission<'_>,
    ) -> state::Result<()> {
        admit()?;
        if !confirmed {
            return Err(SyncError::Review);
        }
        let m = self.mapping(id).await?;
        let c = self.connection(&m.connection_id).await?;
        if c.status != "connected" || m.product(&c) != PlaidProduct::Investments {
            return Err(SyncError::Review);
        }
        let stage = m.holdings_staged.clone().ok_or(SyncError::Review)?;
        if stage.digest != digest || (Utc::now() - stage.fetched_at).num_minutes() > 30 {
            return Err(SyncError::Changed);
        }
        self.apply_investments(m, stage, opening_costs, transfer_costs, admit)
            .await
    }

    async fn apply_investments(
        &self,
        mut m: Mapping,
        stage: HoldingsStage,
        opening_costs: BTreeMap<String, String>,
        transfer_costs: BTreeMap<String, String>,
        admit: Admission<'_>,
    ) -> state::Result<()> {
        let expected = m.clone();
        let plan = investments::plan(&m, &stage)?;
        if !plan.issues.is_empty() {
            return Err(SyncError::Review);
        }
        let mut ledger = if let Some(ledger) = &m.investment_ledger {
            ledger.clone()
        } else {
            let mut positions = BTreeMap::new();
            for (id, qty) in &plan.opening_quantities {
                if *qty == Decimal::ZERO {
                    continue;
                }
                let cost = opening_costs.get(id).ok_or(SyncError::Review)?;
                let parsed = cost.parse::<Decimal>().map_err(|_| SyncError::Review)?;
                if parsed < Decimal::ZERO {
                    return Err(SyncError::Review);
                }
                positions.insert(
                    id.clone(),
                    OpeningPosition {
                        quantity: qty.to_string(),
                        unit_cost: parsed.to_string(),
                    },
                );
            }
            InvestmentLedger {
                start_date: plan.start_date,
                opening_cash: plan.opening_cash.to_string(),
                opening_positions: positions,
                transfer_unit_costs: BTreeMap::new(),
            }
        };
        let incoming: HashSet<_> = plan
            .drafts
            .iter()
            .filter(|row| row.kind == "TRANSFER_IN")
            .map(|row| &row.source)
            .collect();
        if transfer_costs.keys().any(|id| !incoming.contains(id)) {
            return Err(SyncError::Review);
        }
        for id in incoming {
            let existing = ledger.transfer_unit_costs.get(id);
            if existing
                .zip(transfer_costs.get(id))
                .is_some_and(|(old, new)| old != new)
            {
                return Err(SyncError::Review);
            }
            let value = existing
                .or_else(|| transfer_costs.get(id))
                .ok_or(SyncError::Review)?;
            if value.parse::<Decimal>().map_err(|_| SyncError::Review)? < Decimal::ZERO {
                return Err(SyncError::Review);
            }
            ledger.transfer_unit_costs.insert(id.clone(), value.clone());
        }
        let used: HashSet<_> = plan
            .drafts
            .iter()
            .filter_map(|r| r.security.as_ref())
            .chain(plan.current_quantities.keys())
            .chain(ledger.opening_positions.keys())
            .collect();
        let mut specs = Vec::new();
        let mut keys = BTreeMap::new();
        for id in used {
            let security = plan.securities.get(id).ok_or(SyncError::Review)?;
            let symbol = security.ticker_symbol.clone().ok_or(SyncError::Review)?;
            let spec = AssetSpec::market_instrument(
                symbol.clone(),
                symbol,
                None,
                InstrumentType::Equity,
                m.currency.clone(),
            );
            keys.insert(id.clone(), spec.instrument_key().ok_or(SyncError::Review)?);
            specs.push(spec);
        }
        admit()?;
        let ensured = self
            .assets
            .ensure_assets(specs, self.activities.as_ref())
            .await
            .map_err(|_| SyncError::Storage)?;
        let assets: BTreeMap<_, _> = keys
            .into_iter()
            .map(|(id, key)| {
                Ok((
                    id,
                    ensured
                        .input_to_asset_id
                        .get(&key)
                        .cloned()
                        .ok_or(SyncError::Review)?,
                ))
            })
            .collect::<state::Result<_>>()?;
        // Two provider IDs resolving to one asset are ambiguous until explicitly reconciled.
        if assets.values().collect::<HashSet<_>>().len() != assets.len() {
            return Err(SyncError::Review);
        }
        let timezone = self
            .timezone
            .read()
            .map_err(|_| SyncError::Storage)?
            .clone();
        let activities = investments::activities(&m, &plan, &ledger, &assets, &timezone)?;
        let mut quotes = Vec::new();
        let tz = wealthfolio_core::utils::time_utils::parse_user_timezone_or_default(&timezone);
        for h in &stage.response.holdings {
            let Some(id) = assets.get(&h.security_id) else {
                continue;
            };
            if h.quantity.is_zero() {
                continue;
            }
            let asset = ensured.assets.get(id).ok_or(SyncError::Review)?;
            if asset.instrument_type != Some(InstrumentType::Equity)
                || asset.quote_ccy != m.currency
            {
                return Err(SyncError::Review);
            }
            let price = h
                .institution_value
                .checked_div(h.quantity)
                .ok_or(SyncError::Review)?;
            if price < Decimal::ZERO {
                return Err(SyncError::Review);
            }
            let timestamp = ingest::holdings_quote_timestamp(h, stage.fetched_at, &tz)?;
            quotes.push(wealthfolio_core::quotes::Quote {
                id: format!("{}_{}_BROKER", id, timestamp.date_naive()),
                asset_id: id.clone(),
                timestamp,
                open: price,
                high: price,
                low: price,
                close: price,
                adjclose: price,
                volume: Decimal::ZERO,
                currency: m.currency.clone(),
                data_source: "BROKER".into(),
                created_at: stage.fetched_at,
                notes: None,
            });
        }
        let expected_quantities = plan
            .current_quantities
            .iter()
            .map(|(id, q)| Ok((assets.get(id).cloned().ok_or(SyncError::Review)?, *q)))
            .collect::<state::Result<_>>()?;
        let mut expected_basis = BTreeMap::new();
        for h in &stage.response.holdings {
            if let (Some(asset), Some(basis)) = (assets.get(&h.security_id), h.cost_basis) {
                expected_basis.insert(asset.clone(), basis);
            }
        }
        let as_of = stage
            .fetched_at
            .with_timezone(
                &wealthfolio_core::utils::time_utils::parse_user_timezone_or_default(&timezone),
            )
            .date_naive();
        m.source_history = json!({"transactions":stage.history,"securities":stage.response.securities,"holdings":stage.response.holdings,"asOf":stage.fetched_at,"performanceHistoryComplete":false,"coverageStart":ledger.start_date});
        m.investment_ledger = Some(ledger);
        m.holdings_staged = None;
        m.status = "idle".into();
        m.last_error = None;
        m.last_success = Some(stage.fetched_at);
        admit()?;
        self.repository
            .commit_investments(InvestmentCommit {
                mapping: m.clone(),
                expected,
                activities,
                quotes,
                expected_quantities,
                expected_basis,
                expected_cash: plan.current_cash,
                timezone,
                as_of,
            })
            .await?;
        let start = m
            .investment_ledger
            .as_ref()
            .and_then(|l| l.start_date.pred_opt())
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| d.and_utc());
        self.events.emit(DomainEvent::activities_changed(
            vec![m.account_id],
            ensured.assets.keys().cloned().collect(),
            vec![m.currency],
            start,
        ));
        Ok(())
    }

    pub(super) async fn sync_investments(
        &self,
        m: Mapping,
        c: &Connection,
        admit: Admission<'_>,
    ) -> state::Result<()> {
        let stage = self.prepare_holdings(&m, c, admit).await?;
        self.apply_investments(m, stage, BTreeMap::new(), BTreeMap::new(), admit)
            .await
    }

    async fn suggest_opening_unit_costs(
        &self,
        m: &Mapping,
        stage: &HoldingsStage,
        plan: &investments::Plan,
    ) -> BTreeMap<String, String> {
        let Some(repo) = self.statements.clone() else {
            return BTreeMap::new();
        };
        let positive: Vec<_> = plan
            .opening_quantities
            .iter()
            .filter(|(_, qty)| **qty > Decimal::ZERO)
            .map(|(id, qty)| {
                let symbol = plan
                    .securities
                    .get(id)
                    .and_then(|s| s.ticker_symbol.as_ref())
                    .filter(|s| !s.is_empty())
                    .cloned();
                (id.clone(), *qty, symbol)
            })
            .collect();
        if positive.is_empty() {
            return BTreeMap::new();
        }
        let assets = self.assets.clone();
        let activities = self.activities.clone();
        let timezone = self.timezone.clone();
        let connection_id = m.connection_id.clone();
        let account_id = m.account_id.clone();
        let currency = m.currency.clone();
        let opening_day = plan.start_date - chrono::TimeDelta::days(1);
        let fetch_end = stage.fetched_at.date_naive();
        let fetch_start = fetch_end - chrono::TimeDelta::days(730);
        let stage_txs: Vec<_> = stage
            .history
            .iter()
            .map(
                |tx| wealthfolio_core::portfolio::statements::daily::StageProviderTx {
                    id: tx.investment_transaction_id.as_str(),
                    account_id: tx.account_id.as_str(),
                    security_id: tx.security_id.as_deref(),
                    cancel_target: tx.cancel_transaction_id.as_deref(),
                    date: tx.date,
                    transaction_type: tx.transaction_type.as_str(),
                    subtype: tx.subtype.as_str(),
                    quantity_nonzero: !tx.quantity.is_zero(),
                },
            )
            .collect();
        let plan_symbols: BTreeMap<String, Option<String>> = plan
            .securities
            .iter()
            .map(|(k, s)| (k.clone(), s.ticker_symbol.clone()))
            .collect();
        let Some(reconciled) =
            wealthfolio_core::portfolio::statements::daily::reconcile_source_history_and_stage_trades(
                &m.external_id,
                &m.currency,
                &m.source_history,
                &stage_txs,
                &plan_symbols,
                fetch_start,
                fetch_end,
            )
        else {
            return BTreeMap::new();
        };
        wealthfolio_core::portfolio::coordinator::blocking(move || {
            let statements = repo.list_daily(&account_id)?;
            if statements.is_empty() {
                return Ok(BTreeMap::new());
            }
            let stored_trades = repo.provider_security_trade_dates(&account_id)?;
            if stored_trades.verification_incomplete {
                return Ok(BTreeMap::new());
            }
            let trade_coverage = Some(reconciled.coverage);
            let observed_snapshots = repo.observed_snapshots(&account_id)?;
            let resolve = |symbol: &str| {
                let listing = wealthfolio_core::assets::AssetMetadata {
                    instrument_symbol: Some(symbol.to_owned()),
                    display_code: Some(symbol.to_owned()),
                    instrument_type: Some(InstrumentType::Equity),
                    requested_quote_ccy: Some(currency.clone()),
                    ..Default::default()
                };
                assets.find_asset_by_listing(&listing, Some(&currency))
            };
            let mut resolved_assets = BTreeMap::new();
            let mut counts = BTreeMap::<String, usize>::new();
            for (id, _, symbol) in &positive {
                if let Some(asset) = symbol.as_deref().and_then(resolve) {
                    *counts.entry(asset.id.clone()).or_default() += 1;
                    resolved_assets.insert(id.as_str(), asset.id);
                }
            }
            let mut events_by_asset: BTreeMap<
                String,
                Vec<wealthfolio_core::portfolio::statements::ProviderTradeEvent>,
            > = BTreeMap::new();
            for item in reconciled.by_security {
                let mut targets =
                    BTreeSet::from([ingest::stable_id(&connection_id, &item.security_id)]);
                if let Some(id) = resolved_assets
                    .get(item.security_id.as_str())
                    .cloned()
                    .or_else(|| {
                        item.ticker_symbol
                            .as_deref()
                            .and_then(resolve)
                            .map(|a| a.id)
                    })
                {
                    targets.insert(id);
                }
                targets.extend(item.explicit_asset_ids);
                for target_id in targets {
                    events_by_asset
                        .entry(target_id)
                        .or_default()
                        .extend(item.events.iter().copied());
                }
            }
            for events in events_by_asset.values_mut() {
                events.sort_unstable();
            }
            let asset_ids: Vec<String> = resolved_assets.values().cloned().collect();
            let tz = timezone
                .read()
                .map_err(|_| wealthfolio_core::Error::Repository("timezone lock poisoned".into()))?
                .clone();
            let split_activities = activities.get_split_activities_by_asset_ids(&asset_ids)?;
            let splits =
                wealthfolio_core::portfolio::statements::daily::split_events_for_activities(
                    &split_activities,
                    &tz,
                );
            let mut out = BTreeMap::new();
            for (security_id, qty, _) in &positive {
                let Some(asset_id) = resolved_assets
                    .get(security_id.as_str())
                    .filter(|id| counts.get(id.as_str()) == Some(&1))
                else {
                    continue;
                };
                let events = events_by_asset
                    .get(asset_id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                if let Some(unit_cost) =
                    wealthfolio_core::portfolio::statements::daily::suggested_opening_unit_cost(
                        &statements,
                        &currency,
                        asset_id,
                        opening_day,
                        *qty,
                        &observed_snapshots,
                        &splits,
                        events,
                        trade_coverage,
                    )
                {
                    out.insert(security_id.clone(), unit_cost.to_string());
                }
            }
            Ok(out)
        })
        .await
        .unwrap_or_default()
    }
}
