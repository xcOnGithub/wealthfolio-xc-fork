//! Async orchestration service for on-demand backtest experiments.
//!
//! Fetches historical quotes and optional cash dividend events via the existing
//! `QuoteServiceTrait` (preserving provider settings and never mutating SQLite),
//! then executes the deterministic simulation off the async worker pool via
//! `portfolio::coordinator::blocking`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use futures::future::join_all;
use rust_decimal::prelude::ToPrimitive;

use crate::errors::{Error, Result, ValidationError};
use crate::portfolio::coordinator::blocking;
use crate::quotes::{FetchDividendsParams, QuoteServiceTrait};

use super::engine::{simulate, SymbolMarketSeries};
use super::model::{BacktestRequest, BacktestResponse, BacktestStrategy, DividendMode};

/// Run an in-session USD / US-market backtest experiment using on-demand
/// historical market data and optional dividend events.
pub async fn run_backtest(
    quote_service: Arc<dyn QuoteServiceTrait>,
    request: BacktestRequest,
) -> Result<BacktestResponse> {
    let mut req = request.normalized_and_validated()?;
    let today = Utc::now().date_naive();
    if req.end_date > today {
        req.end_date = today;
        if req.end_date <= req.start_date {
            return Err(Error::Validation(ValidationError::InvalidInput(
                "Start date must be before today.".to_string(),
            )));
        }
    }

    let warmup_trading_days = req.warmup_trading_days();
    let warmup_calendar_days = if warmup_trading_days > 0 {
        ((warmup_trading_days as i64) * 7 / 5) + 45
    } else {
        7
    };
    let fetch_start = req.start_date - Duration::days(warmup_calendar_days);
    let fetch_end = req.end_date;

    let mut required_symbols: Vec<String> = req.assets.iter().map(|a| a.symbol.clone()).collect();
    if let Some(def_sym) = defensive_symbol(&req.strategy) {
        if !required_symbols.iter().any(|s| s == def_sym) {
            required_symbols.push(def_sym.to_string());
        }
    }
    let required_set: HashSet<String> = required_symbols.iter().cloned().collect();

    let mut all_symbols = required_symbols.clone();
    if let Some(bench) = &req.benchmark_symbol {
        if !all_symbols.iter().any(|s| s == bench) {
            all_symbols.push(bench.clone());
        }
    }

    let include_dividends = req.dividend_mode != DividendMode::Exclude;

    let fetch_futures = all_symbols.into_iter().map(|symbol| {
        let qs = Arc::clone(&quote_service);
        async move {
            let quotes_res = qs
                .fetch_quotes_for_symbol(&symbol, "USD", fetch_start, fetch_end)
                .await;

            let dividends_res = if include_dividends && quotes_res.is_ok() {
                Some(
                    qs.fetch_dividends(FetchDividendsParams {
                        symbol: symbol.clone(),
                        exchange_mic: None,
                        instrument_type: None,
                        quote_ccy: Some("USD".to_string()),
                        preferred_provider: None,
                        start: Some(fetch_start),
                        end: Some(fetch_end),
                    })
                    .await,
                )
            } else {
                None
            };

            (symbol, quotes_res, dividends_res)
        }
    });

    let fetched = join_all(fetch_futures).await;

    let mut series_map: HashMap<String, SymbolMarketSeries> = HashMap::new();
    let mut warnings: Vec<String> = Vec::new();

    for (symbol, quotes_res, dividends_res) in fetched {
        let is_required = required_set.contains(&symbol);
        let quotes = match quotes_res {
            Ok(quotes) if !quotes.is_empty() => quotes,
            Ok(_) => {
                if is_required {
                    return Err(Error::Validation(ValidationError::InvalidInput(format!(
                        "No historical price data found for '{symbol}' in the selected range."
                    ))));
                }
                warnings.push(format!(
                    "No historical quotes found for benchmark '{symbol}'; benchmark comparison was omitted."
                ));
                continue;
            }
            Err(err) => {
                if is_required {
                    return Err(err);
                }
                warnings.push(format!(
                    "Failed to fetch benchmark '{symbol}'; benchmark comparison was omitted."
                ));
                continue;
            }
        };

        // Validate USD-only market scope for v1.
        let detected_currency = quotes
            .iter()
            .rev()
            .map(|q| q.currency.trim())
            .find(|c| !c.is_empty())
            .unwrap_or("USD")
            .to_uppercase();

        if detected_currency != "USD" {
            if is_required {
                return Err(Error::Validation(ValidationError::InvalidInput(format!(
                    "Ticker '{symbol}' is quoted in {detected_currency}. Backtest v1 supports USD / US-market securities only."
                ))));
            }
            warnings.push(format!(
                "Benchmark '{symbol}' is quoted in {detected_currency} (USD only in v1); benchmark comparison was omitted."
            ));
            continue;
        }

        let mut prices = BTreeMap::new();
        for q in quotes {
            let day = q.timestamp.date_naive();
            if let Some(close) = q.close.to_f64() {
                if close.is_finite() && close > 0.0 {
                    prices.insert(day, close);
                }
            }
        }

        if prices.len() < 2 {
            if is_required {
                return Err(Error::Validation(ValidationError::InvalidInput(format!(
                    "Not enough valid daily quotes for '{symbol}'."
                ))));
            }
            warnings.push(format!(
                "Not enough valid daily quotes for benchmark '{symbol}'; benchmark comparison was omitted."
            ));
            continue;
        }

        let mut dividends = BTreeMap::new();
        let dividends_available = match dividends_res {
            None => !include_dividends,
            Some(Ok(events)) => {
                for ev in events {
                    if ev.amount.is_finite() && ev.amount > 0.0 {
                        if let Some(dt) = DateTime::<Utc>::from_timestamp(ev.date, 0) {
                            let day: NaiveDate = dt.date_naive();
                            *dividends.entry(day).or_insert(0.0) += ev.amount;
                        }
                    }
                }
                true
            }
            Some(Err(_)) => {
                warnings.push(format!(
                    "Dividend history unavailable for '{symbol}'; simulated price return only for this symbol."
                ));
                false
            }
        };

        series_map.insert(
            symbol.clone(),
            SymbolMarketSeries {
                symbol,
                currency: detected_currency,
                prices,
                dividends,
                dividends_available,
            },
        );
    }

    blocking(move || simulate(&req, &series_map, warnings)).await
}

fn defensive_symbol(strategy: &BacktestStrategy) -> Option<&str> {
    match strategy {
        BacktestStrategy::SmaTrendFilter {
            defensive_symbol, ..
        }
        | BacktestStrategy::MomentumRotation {
            defensive_symbol, ..
        } => defensive_symbol.as_deref(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::Asset;
    use crate::errors::Error;
    use crate::portfolio::backtest::model::{BacktestAssetInput, RebalanceFrequency};
    use crate::quotes::{
        FetchDividendsParams, LatestQuotePair, LatestQuoteSnapshot, MarketDataError, ProviderInfo,
        Quote, QuoteImport, QuoteServiceTrait, QuoteSyncState, SparseAssetMarketFacts,
        SymbolSearchResult, SymbolSyncPlan, SyncMode, SyncResult,
    };
    use crate::ProviderProfile;
    use async_trait::async_trait;
    use chrono::TimeZone;
    use rust_decimal::Decimal;
    use std::str::FromStr;
    use wealthfolio_market_data::DividendEvent;

    struct StubQuoteService {
        quotes_by_symbol: HashMap<String, Result<Vec<Quote>>>,
        dividends_by_symbol: HashMap<String, Result<Vec<DividendEvent>>>,
    }

    fn stub_quote(symbol: &str, day: NaiveDate, close: &str, currency: &str) -> Quote {
        let ts = Utc.from_utc_datetime(&day.and_hms_opt(16, 0, 0).unwrap());
        let dec = Decimal::from_str(close).unwrap();
        Quote {
            id: format!("{symbol}_{day}_STUB"),
            asset_id: symbol.to_string(),
            timestamp: ts,
            open: dec,
            high: dec,
            low: dec,
            close: dec,
            adjclose: dec,
            volume: Decimal::ZERO,
            currency: currency.to_string(),
            data_source: "STUB".to_string(),
            created_at: ts,
            notes: None,
        }
    }

    #[async_trait]
    impl QuoteServiceTrait for StubQuoteService {
        fn get_latest_quote(&self, _symbol: &str) -> Result<Quote> {
            unimplemented!()
        }
        fn get_latest_quotes(&self, _symbols: &[String]) -> Result<HashMap<String, Quote>> {
            unimplemented!()
        }
        fn get_latest_quotes_as_of(
            &self,
            _symbols: &[String],
            _as_of: NaiveDate,
        ) -> Result<HashMap<String, Quote>> {
            unimplemented!()
        }
        fn get_sparse_asset_market_facts(
            &self,
            _requests: &[(String, NaiveDate)],
        ) -> Result<SparseAssetMarketFacts> {
            unimplemented!()
        }
        fn get_latest_quotes_snapshot(
            &self,
            _asset_ids: &[String],
        ) -> Result<HashMap<String, LatestQuoteSnapshot>> {
            unimplemented!()
        }
        fn get_latest_quotes_pair(
            &self,
            _symbols: &[String],
        ) -> Result<HashMap<String, LatestQuotePair>> {
            unimplemented!()
        }
        fn get_historical_quotes(&self, _symbol: &str) -> Result<Vec<Quote>> {
            unimplemented!()
        }
        fn get_all_historical_quotes(&self) -> Result<HashMap<String, Vec<(NaiveDate, Quote)>>> {
            unimplemented!()
        }
        fn get_quotes_in_range(
            &self,
            _symbols: &HashSet<String>,
            _start: NaiveDate,
            _end: NaiveDate,
        ) -> Result<Vec<Quote>> {
            unimplemented!()
        }
        fn get_quotes_in_range_filled(
            &self,
            _symbols: &HashSet<String>,
            _start: NaiveDate,
            _end: NaiveDate,
        ) -> Result<Vec<Quote>> {
            unimplemented!()
        }
        async fn get_daily_quotes(
            &self,
            _asset_ids: &HashSet<String>,
            _start: NaiveDate,
            _end: NaiveDate,
        ) -> Result<HashMap<NaiveDate, HashMap<String, Quote>>> {
            unimplemented!()
        }
        async fn add_quote(&self, _quote: &Quote) -> Result<Quote> {
            unimplemented!()
        }
        async fn update_quote(&self, _quote: Quote) -> Result<Quote> {
            unimplemented!()
        }
        async fn delete_quote(&self, _quote_id: &str) -> Result<()> {
            unimplemented!()
        }
        async fn bulk_upsert_quotes(&self, _quotes: Vec<Quote>) -> Result<usize> {
            unimplemented!()
        }
        async fn search_symbol(&self, _query: &str) -> Result<Vec<SymbolSearchResult>> {
            Ok(vec![])
        }
        async fn search_symbol_with_currency(
            &self,
            _query: &str,
            _account_currency: Option<&str>,
        ) -> Result<Vec<SymbolSearchResult>> {
            Ok(vec![])
        }
        async fn get_asset_profile(&self, _asset: &Asset) -> Result<ProviderProfile> {
            unimplemented!()
        }
        async fn fetch_quotes_from_provider(
            &self,
            _asset_id: &str,
            _start: NaiveDate,
            _end: NaiveDate,
        ) -> Result<Vec<Quote>> {
            unimplemented!()
        }
        async fn fetch_quotes_for_symbol(
            &self,
            asset_id: &str,
            _currency: &str,
            _start: NaiveDate,
            _end: NaiveDate,
        ) -> Result<Vec<Quote>> {
            match self.quotes_by_symbol.get(asset_id) {
                Some(Ok(v)) => Ok(v.clone()),
                Some(Err(e)) => Err(Error::MarketData(MarketDataError::ProviderError(
                    e.to_string(),
                ))),
                None => Err(Error::MarketData(MarketDataError::NotFound(
                    asset_id.to_string(),
                ))),
            }
        }
        async fn fetch_dividends(
            &self,
            params: FetchDividendsParams,
        ) -> Result<Vec<DividendEvent>> {
            match self.dividends_by_symbol.get(&params.symbol) {
                Some(Ok(v)) => Ok(v.clone()),
                Some(Err(e)) => Err(Error::MarketData(MarketDataError::ProviderError(
                    e.to_string(),
                ))),
                None => Ok(vec![]),
            }
        }
        async fn sync(
            &self,
            _mode: SyncMode,
            _asset_ids: Option<Vec<String>>,
        ) -> Result<SyncResult> {
            unimplemented!()
        }
        async fn resync(&self, _asset_ids: Option<Vec<String>>) -> Result<SyncResult> {
            unimplemented!()
        }
        async fn refresh_sync_state(&self) -> Result<()> {
            unimplemented!()
        }
        fn get_sync_plan(&self) -> Result<Vec<SymbolSyncPlan>> {
            unimplemented!()
        }
        async fn handle_activity_created(
            &self,
            _symbol: &str,
            _activity_date: NaiveDate,
        ) -> Result<()> {
            unimplemented!()
        }
        async fn handle_activity_deleted(&self, _symbol: &str) -> Result<()> {
            unimplemented!()
        }
        async fn delete_sync_state(&self, _symbol: &str) -> Result<()> {
            unimplemented!()
        }
        fn get_symbols_needing_sync(&self) -> Result<Vec<QuoteSyncState>> {
            unimplemented!()
        }
        fn get_sync_state(&self, _symbol: &str) -> Result<Option<QuoteSyncState>> {
            unimplemented!()
        }
        async fn mark_profile_enriched(&self, _symbol: &str) -> Result<()> {
            unimplemented!()
        }
        fn get_assets_needing_profile_enrichment(&self) -> Result<Vec<QuoteSyncState>> {
            unimplemented!()
        }
        fn get_sync_states_with_errors(&self) -> Result<Vec<QuoteSyncState>> {
            Ok(vec![])
        }
        async fn reset_sync_errors(&self, _asset_ids: &[String]) -> Result<()> {
            unimplemented!()
        }
        async fn reset_sync_state_for_profile_change(&self, _asset_id: &str) -> Result<()> {
            unimplemented!()
        }
        async fn update_position_status_from_holdings(
            &self,
            _current_holdings: &HashMap<String, Decimal>,
        ) -> Result<()> {
            Ok(())
        }
        async fn get_providers_info(&self) -> Result<Vec<ProviderInfo>> {
            Ok(vec![])
        }
        async fn update_provider_settings(
            &self,
            _provider_id: &str,
            _priority: i32,
            _enabled: bool,
        ) -> Result<()> {
            Ok(())
        }
        async fn check_quotes_import(
            &self,
            _content: &[u8],
            _has_header_row: bool,
        ) -> Result<Vec<QuoteImport>> {
            unimplemented!()
        }
        async fn import_quotes(
            &self,
            _quotes: Vec<QuoteImport>,
            _overwrite: bool,
        ) -> Result<Vec<QuoteImport>> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn rejects_non_usd_required_ticker() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();
        let mut quotes = HashMap::new();
        quotes.insert(
            "SHOP.TO".to_string(),
            Ok(vec![
                stub_quote("SHOP.TO", d1, "100", "CAD"),
                stub_quote("SHOP.TO", d2, "105", "CAD"),
            ]),
        );

        let qs = Arc::new(StubQuoteService {
            quotes_by_symbol: quotes,
            dividends_by_symbol: HashMap::new(),
        });

        let req = BacktestRequest {
            assets: vec![BacktestAssetInput {
                symbol: "SHOP.TO".to_string(),
                weight: 100.0,
            }],
            start_date: d1,
            end_date: d2,
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: None,
            strategy: BacktestStrategy::BuyAndHold,
        };

        let err = run_backtest(qs, req).await.unwrap_err();
        assert!(err.to_string().contains("CAD"));
    }

    #[tokio::test]
    async fn isolates_dividend_and_benchmark_provider_failures() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();
        let d3 = NaiveDate::from_ymd_opt(2024, 1, 4).unwrap();

        let mut quotes = HashMap::new();
        quotes.insert(
            "VTI".to_string(),
            Ok(vec![
                stub_quote("VTI", d1, "100", "USD"),
                stub_quote("VTI", d2, "110", "USD"),
                stub_quote("VTI", d3, "120", "USD"),
            ]),
        );
        quotes.insert(
            "BADBENCH".to_string(),
            Err(Error::MarketData(MarketDataError::ProviderError(
                "timeout".to_string(),
            ))),
        );

        let mut divs = HashMap::new();
        divs.insert(
            "VTI".to_string(),
            Err(Error::MarketData(MarketDataError::ProviderError(
                "dividends unsupported".to_string(),
            ))),
        );

        let qs = Arc::new(StubQuoteService {
            quotes_by_symbol: quotes,
            dividends_by_symbol: divs,
        });

        let req = BacktestRequest {
            assets: vec![BacktestAssetInput {
                symbol: "VTI".to_string(),
                weight: 100.0,
            }],
            start_date: d1,
            end_date: d3,
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Reinvest,
            fee_bps: 0.0,
            benchmark_symbol: Some("BADBENCH".to_string()),
            strategy: BacktestStrategy::PeriodicRebalance {
                frequency: RebalanceFrequency::Quarterly,
            },
        };

        let res = run_backtest(qs, req).await.unwrap();
        assert!((res.strategy.metrics.final_value - 12_000.0).abs() < 1e-6);
        assert!(res.benchmark.is_none());
        assert!(res.warnings.iter().any(|w| w.contains("BADBENCH")));
        assert!(res
            .warnings
            .iter()
            .any(|w| w.contains("Dividend history unavailable")));
        assert!(!res.symbol_coverage[0].dividends_available);
    }
}
