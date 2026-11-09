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
        let stage = self.prepare_holdings_range(&m, &c, true).await?;
        let plan = investments::plan(&m, &stage)?;
        let openings: Vec<_> = plan.opening_quantities.iter()
            .filter(|(_, qty)| **qty != Decimal::ZERO)
            .map(|(id, qty)| json!({
                "securityId": id,
                "name": plan.securities.get(id).and_then(|s| s.name.as_ref().or(s.ticker_symbol.as_ref())),
                "quantity": qty.to_string(),
                "unitCost": m.investment_ledger.as_ref().and_then(|l| l.opening_positions.get(id)).map(|p| &p.unit_cost),
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
                json!({"recordId": issue.record_id, "reason": issue.reason, "name": name})
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
        self.apply_investments(m, stage, opening_costs, admit).await
    }

    async fn apply_investments(
        &self,
        mut m: Mapping,
        stage: HoldingsStage,
        opening_costs: BTreeMap<String, String>,
        admit: Admission<'_>,
    ) -> state::Result<()> {
        let expected = m.clone();
        let plan = investments::plan(&m, &stage)?;
        if !plan.issues.is_empty() {
            return Err(SyncError::Review);
        }
        let ledger = if let Some(ledger) = &m.investment_ledger {
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
            }
        };
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
        let stage = self.prepare_holdings(&m, c).await?;
        self.apply_investments(m, stage, BTreeMap::new(), admit)
            .await
    }
}
