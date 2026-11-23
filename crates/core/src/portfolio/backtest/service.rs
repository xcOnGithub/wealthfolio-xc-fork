//! Async orchestration service for on-demand backtest experiments.
//!
//! Fetches historical quotes, optional cash dividend events, and (when needed
//! for unadjusted sources) split events via the existing `QuoteServiceTrait`
//! (preserving provider settings and never mutating SQLite), verifies US-listed
//! USD security eligibility server-side, then executes the deterministic
//! simulation off the async worker pool via `portfolio::coordinator::blocking`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use futures::future::join_all;
use rust_decimal::prelude::ToPrimitive;
use wealthfolio_market_data::resolver::{
    canonicalize_exchange_mic, is_us_exchange_mic, yahoo_exchange_to_mic,
};

use crate::assets::{Asset, AssetKind, InstrumentType, QuoteMode};
use crate::errors::{Error, Result, ValidationError};
use crate::portfolio::coordinator::blocking;
use crate::quotes::{FetchDividendsParams, Quote, QuoteServiceTrait, SymbolSearchResult};

use super::engine::{apply_splits_backward, simulate, SymbolMarketSeries};
use super::model::{
    is_allowed_us_security_type, BacktestRequest, BacktestResponse, BacktestStrategy,
    BacktestWarning, DividendMode,
};

/// Providers whose historical daily `close` values are already split-adjusted
/// in `crates/market-data`.
fn is_verified_split_adjusted_source(source: &str) -> bool {
    matches!(
        source.trim().to_ascii_uppercase().as_str(),
        "YAHOO" | "MARKETDATA_APP" | "FINNHUB"
    )
}

#[derive(Debug, Clone)]
struct SymbolRequestMeta {
    symbol: String,
    hint_mic: Option<String>,
    hint_type: Option<String>,
    is_required: bool,
}

#[derive(Debug, Clone)]
struct VerifiedUsListing {
    exchange_mic: String,
}

/// Resolve a candidate exchange string (either an ISO 10383 MIC or a Yahoo exchange
/// code like `NMS`/`NYQ`/`PCX`) into a canonical MIC if recognized.
fn resolve_exchange_to_mic(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let canonical = canonicalize_exchange_mic(trimmed);
    if is_us_exchange_mic(&canonical) {
        return Some(canonical);
    }
    if let Some(mapped) = yahoo_exchange_to_mic(trimmed) {
        return Some(mapped.into_owned());
    }
    Some(canonical)
}

fn search_result_mic(r: &SymbolSearchResult) -> Option<String> {
    r.canonical_exchange_mic
        .as_deref()
        .or(r.exchange_mic.as_deref())
        .and_then(resolve_exchange_to_mic)
        .or_else(|| resolve_exchange_to_mic(&r.exchange))
}

fn search_result_matches_symbol(r: &SymbolSearchResult, symbol: &str) -> bool {
    r.symbol.trim().eq_ignore_ascii_case(symbol)
        || r.canonical_symbol
            .as_deref()
            .is_some_and(|c| c.trim().eq_ignore_ascii_case(symbol))
}

fn is_obviously_non_us_equity_symbol(symbol: &str) -> bool {
    // Reject FX pairs (`EURUSD=X`), futures (`GC=F`), indices (`^GSPC`),
    // and common crypto pairs (`BTC-USD`, `ETH-USD`, etc.).
    if symbol.contains('=') || symbol.starts_with('^') {
        return true;
    }
    if let Some((_, quote_part)) = symbol.rsplit_once('-') {
        if matches!(quote_part, "USD" | "EUR" | "GBP" | "CAD" | "USDT" | "USDC") {
            return true;
        }
    }
    false
}

/// Server-side verification that `symbol` refers to a US-listed, USD-denominated
/// equity/ETF/fund security.
async fn verify_us_listing(
    quote_service: &Arc<dyn QuoteServiceTrait>,
    meta: &SymbolRequestMeta,
) -> std::result::Result<VerifiedUsListing, String> {
    let symbol = &meta.symbol;
    if is_obviously_non_us_equity_symbol(symbol) {
        return Err(format!(
            "Ticker '{symbol}' is outside the US-listed equity/ETF scope."
        ));
    }

    if let Some(hint_mic) = &meta.hint_mic {
        if !is_us_exchange_mic(hint_mic) {
            return Err(format!(
                "Ticker '{symbol}' on exchange '{hint_mic}' is not a US listing."
            ));
        }
    }
    if let Some(hint_type) = &meta.hint_type {
        if !is_allowed_us_security_type(hint_type) {
            return Err(format!(
                "Ticker '{symbol}' ({hint_type}) is outside the US-listed equity/ETF scope."
            ));
        }
    }

    // 1. Check authoritative symbol search results from enabled providers.
    if let Ok(results) = quote_service.search_symbol(symbol).await {
        let exact: Vec<&SymbolSearchResult> = results
            .iter()
            .filter(|r| search_result_matches_symbol(r, symbol))
            .collect();

        if !exact.is_empty() {
            // If caller specified a MIC, narrow to matching listing(s) when present.
            let candidates: Vec<&SymbolSearchResult> = if let Some(hint_mic) = &meta.hint_mic {
                let by_mic: Vec<&SymbolSearchResult> = exact
                    .iter()
                    .copied()
                    .filter(|r| search_result_mic(r).as_deref() == Some(hint_mic.as_str()))
                    .collect();
                if by_mic.is_empty() {
                    exact
                } else {
                    by_mic
                }
            } else {
                exact
            };

            for candidate in &candidates {
                let quote_type_ok = is_allowed_us_security_type(&candidate.quote_type);
                let currency_ok = candidate
                    .currency
                    .as_deref()
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                    .is_none_or(|c| c.eq_ignore_ascii_case("USD"));
                let mic = search_result_mic(candidate);
                let mic_ok = mic.as_deref().is_some_and(is_us_exchange_mic);

                if quote_type_ok && currency_ok && mic_ok {
                    if let Some(verified_mic) = mic {
                        return Ok(VerifiedUsListing {
                            exchange_mic: verified_mic,
                        });
                    }
                }
            }

            return Err(format!(
                "Ticker '{symbol}' is not a US-listed USD equity or ETF."
            ));
        }
    }

    // 2. Fall back to asset profile lookup (for providers without search coverage).
    let temp_asset = Asset {
        id: symbol.clone(),
        instrument_symbol: Some(symbol.clone()),
        display_code: Some(symbol.clone()),
        kind: AssetKind::Investment,
        instrument_type: Some(InstrumentType::Equity),
        instrument_exchange_mic: meta.hint_mic.clone(),
        quote_ccy: "USD".to_string(),
        quote_mode: QuoteMode::Market,
        ..Default::default()
    };

    if let Ok(profile) = quote_service.get_asset_profile(&temp_asset).await {
        let quote_type_ok = profile
            .asset_type
            .as_deref()
            .is_some_and(is_allowed_us_security_type);
        let currency_trimmed = profile.currency.trim();
        let currency_ok =
            currency_trimmed.is_empty() || currency_trimmed.eq_ignore_ascii_case("USD");
        let mic = meta.hint_mic.clone();
        let mic_ok = mic.as_deref().is_some_and(is_us_exchange_mic);

        if quote_type_ok && currency_ok && mic_ok {
            if let Some(verified_mic) = mic {
                return Ok(VerifiedUsListing {
                    exchange_mic: verified_mic,
                });
            }
        }

        return Err(format!(
            "Ticker '{symbol}' could not be verified as a US-listed USD equity or ETF."
        ));
    }

    Err(format!(
        "Ticker '{symbol}' could not be verified as a US-listed USD security."
    ))
}

/// Detect whether every quote in `quotes` came from a single verified split-adjusted source.
/// Returns `Ok(())` if already split-adjusted, or `Err(source_label)` if split normalization is required.
fn check_split_adjustment_status(quotes: &[Quote]) -> std::result::Result<(), String> {
    let mut sources: HashSet<String> = HashSet::new();
    for q in quotes {
        let src = q.data_source.trim().to_ascii_uppercase();
        if !src.is_empty() {
            sources.insert(src);
        }
    }
    if sources.is_empty() {
        return Err("UNKNOWN".to_string());
    }
    if sources.len() == 1 {
        if let Some(only) = sources.iter().next() {
            if is_verified_split_adjusted_source(only) {
                return Ok(());
            }
            return Err(only.clone());
        }
    }
    // Mixed quote sources may mix adjusted and unadjusted closes.
    let mut sorted: Vec<String> = sources.into_iter().collect();
    sorted.sort();
    Err(sorted.join("+"))
}

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

    let mut symbol_metas: Vec<SymbolRequestMeta> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();

    for asset in &req.assets {
        seen.insert(asset.symbol.clone(), symbol_metas.len());
        symbol_metas.push(SymbolRequestMeta {
            symbol: asset.symbol.clone(),
            hint_mic: asset.exchange_mic.clone(),
            hint_type: asset.instrument_type.clone(),
            is_required: true,
        });
    }

    if let Some((def_sym, def_mic, def_type)) = defensive_listing(&req.strategy) {
        if let Some(&idx) = seen.get(def_sym) {
            symbol_metas[idx].is_required = true;
            if symbol_metas[idx].hint_mic.is_none() {
                symbol_metas[idx].hint_mic = def_mic.map(str::to_string);
            }
            if symbol_metas[idx].hint_type.is_none() {
                symbol_metas[idx].hint_type = def_type.map(str::to_string);
            }
        } else {
            seen.insert(def_sym.to_string(), symbol_metas.len());
            symbol_metas.push(SymbolRequestMeta {
                symbol: def_sym.to_string(),
                hint_mic: def_mic.map(str::to_string),
                hint_type: def_type.map(str::to_string),
                is_required: true,
            });
        }
    }

    if let Some(bench) = &req.benchmark_symbol {
        if let Some(&idx) = seen.get(bench) {
            if symbol_metas[idx].hint_mic.is_none() {
                symbol_metas[idx].hint_mic = req.benchmark_exchange_mic.clone();
            }
            if symbol_metas[idx].hint_type.is_none() {
                symbol_metas[idx].hint_type = req.benchmark_instrument_type.clone();
            }
        } else {
            seen.insert(bench.clone(), symbol_metas.len());
            symbol_metas.push(SymbolRequestMeta {
                symbol: bench.clone(),
                hint_mic: req.benchmark_exchange_mic.clone(),
                hint_type: req.benchmark_instrument_type.clone(),
                is_required: false,
            });
        }
    }

    let include_dividends = req.dividend_mode != DividendMode::Exclude;

    let fetch_futures = symbol_metas.into_iter().map(|meta| {
        let qs = Arc::clone(&quote_service);
        async move {
            let listing_res = verify_us_listing(&qs, &meta).await;
            let Ok(verified_listing) = listing_res else {
                return (
                    meta,
                    Err(listing_res.err().unwrap_or_default()),
                    None,
                    None,
                    None,
                );
            };

            let quotes_res = qs
                .fetch_quotes_for_symbol(&meta.symbol, "USD", fetch_start, fetch_end)
                .await;

            // Normalize unadjusted sources (e.g. Alpha Vantage TIME_SERIES_DAILY) using
            // split events up through `today` so closes share the post-split basis with dividends.
            let splits_res = match &quotes_res {
                Ok(quotes) if !quotes.is_empty() => match check_split_adjustment_status(quotes) {
                    Ok(()) => None,
                    Err(source_label) => {
                        let fetched = qs
                            .fetch_splits_for_symbol(
                                &meta.symbol,
                                Some(&verified_listing.exchange_mic),
                                fetch_start,
                                today,
                            )
                            .await;
                        Some((source_label, fetched))
                    }
                },
                _ => None,
            };

            let dividends_res = if include_dividends && quotes_res.is_ok() {
                Some(
                    qs.fetch_dividends(FetchDividendsParams {
                        symbol: meta.symbol.clone(),
                        exchange_mic: Some(verified_listing.exchange_mic.clone()),
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

            (
                meta,
                Ok(verified_listing),
                Some(quotes_res),
                splits_res,
                dividends_res,
            )
        }
    });

    let fetched = join_all(fetch_futures).await;

    let mut series_map: HashMap<String, SymbolMarketSeries> = HashMap::new();
    let mut warnings: Vec<BacktestWarning> = Vec::new();

    for (meta, listing_res, quotes_opt, splits_opt, dividends_res) in fetched {
        let symbol = meta.symbol;
        let is_required = meta.is_required;

        if let Err(reason) = listing_res {
            if is_required {
                return Err(Error::Validation(ValidationError::InvalidInput(reason)));
            }
            warnings.push(BacktestWarning::BenchmarkNotUsListing { symbol });
            continue;
        }

        let Some(quotes_res) = quotes_opt else {
            continue;
        };

        let quotes = match quotes_res {
            Ok(quotes) if !quotes.is_empty() => quotes,
            Ok(_) => {
                if is_required {
                    return Err(Error::Validation(ValidationError::InvalidInput(format!(
                        "No historical price data found for '{symbol}' in the selected range."
                    ))));
                }
                warnings.push(BacktestWarning::BenchmarkUnavailable { symbol });
                continue;
            }
            Err(err) => {
                if is_required {
                    return Err(err);
                }
                warnings.push(BacktestWarning::BenchmarkUnavailable { symbol });
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
            warnings.push(BacktestWarning::BenchmarkNonUsd {
                symbol,
                currency: detected_currency,
            });
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

        // Apply split normalization for unadjusted sources, or reject if split history is unavailable.
        if let Some((source_label, splits_res)) = splits_opt {
            match splits_res {
                Ok(splits) => {
                    apply_splits_backward(&mut prices, &splits);
                }
                Err(_) => {
                    if is_required {
                        return Err(Error::Validation(ValidationError::InvalidInput(format!(
                            "Provider '{source_label}' returns unadjusted closes for '{symbol}', and no enabled provider could supply split history to normalize them."
                        ))));
                    }
                    warnings.push(BacktestWarning::BenchmarkSplitAdjustmentUnavailable {
                        symbol,
                        source: source_label,
                    });
                    continue;
                }
            }
        }

        if prices.len() < 2 {
            if is_required {
                return Err(Error::Validation(ValidationError::InvalidInput(format!(
                    "Not enough valid daily quotes for '{symbol}'."
                ))));
            }
            warnings.push(BacktestWarning::BenchmarkUnavailable { symbol });
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
                warnings.push(BacktestWarning::DividendsUnavailable {
                    symbol: symbol.clone(),
                });
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

fn defensive_listing(strategy: &BacktestStrategy) -> Option<(&str, Option<&str>, Option<&str>)> {
    match strategy {
        BacktestStrategy::SmaTrendFilter {
            defensive_symbol,
            defensive_exchange_mic,
            defensive_instrument_type,
            ..
        }
        | BacktestStrategy::MomentumRotation {
            defensive_symbol,
            defensive_exchange_mic,
            defensive_instrument_type,
            ..
        } => defensive_symbol.as_deref().map(|s| {
            (
                s,
                defensive_exchange_mic.as_deref(),
                defensive_instrument_type.as_deref(),
            )
        }),
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
    use rust_decimal_macros::dec;
    use std::str::FromStr;
    use wealthfolio_market_data::{DividendEvent, SplitEvent};

    struct StubQuoteService {
        quotes_by_symbol: HashMap<String, Result<Vec<Quote>>>,
        dividends_by_symbol: HashMap<String, Result<Vec<DividendEvent>>>,
        splits_by_symbol: HashMap<String, Result<Vec<SplitEvent>>>,
        search_by_symbol: HashMap<String, Vec<SymbolSearchResult>>,
    }

    fn stub_quote_with_source(
        symbol: &str,
        day: NaiveDate,
        close: &str,
        currency: &str,
        source: &str,
    ) -> Quote {
        let ts = Utc.from_utc_datetime(&day.and_hms_opt(16, 0, 0).unwrap());
        let dec = Decimal::from_str(close).unwrap();
        Quote {
            id: format!("{symbol}_{day}_{source}"),
            asset_id: symbol.to_string(),
            timestamp: ts,
            open: dec,
            high: dec,
            low: dec,
            close: dec,
            adjclose: dec,
            volume: Decimal::ZERO,
            currency: currency.to_string(),
            data_source: source.to_string(),
            created_at: ts,
            notes: None,
        }
    }

    fn stub_quote(symbol: &str, day: NaiveDate, close: &str, currency: &str) -> Quote {
        stub_quote_with_source(symbol, day, close, currency, "YAHOO")
    }

    fn stub_search_result(
        symbol: &str,
        exchange_mic: &str,
        quote_type: &str,
        currency: &str,
    ) -> SymbolSearchResult {
        SymbolSearchResult {
            symbol: symbol.to_string(),
            canonical_symbol: Some(symbol.to_string()),
            exchange_mic: Some(exchange_mic.to_string()),
            canonical_exchange_mic: Some(exchange_mic.to_string()),
            quote_type: quote_type.to_string(),
            currency: Some(currency.to_string()),
            ..Default::default()
        }
    }

    fn req_asset(symbol: &str, weight: f64) -> BacktestAssetInput {
        BacktestAssetInput {
            symbol: symbol.to_string(),
            weight,
            exchange_mic: None,
            instrument_type: None,
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
        async fn search_symbol(&self, query: &str) -> Result<Vec<SymbolSearchResult>> {
            Ok(self
                .search_by_symbol
                .get(query)
                .cloned()
                .unwrap_or_default())
        }
        async fn search_symbol_with_currency(
            &self,
            query: &str,
            _account_currency: Option<&str>,
        ) -> Result<Vec<SymbolSearchResult>> {
            self.search_symbol(query).await
        }
        async fn get_asset_profile(&self, _asset: &Asset) -> Result<ProviderProfile> {
            Err(Error::MarketData(MarketDataError::NotFound(
                "no stub profile".to_string(),
            )))
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
        async fn fetch_splits_for_symbol(
            &self,
            symbol: &str,
            _exchange_mic: Option<&str>,
            _start: NaiveDate,
            _end: NaiveDate,
        ) -> Result<Vec<SplitEvent>> {
            match self.splits_by_symbol.get(symbol) {
                Some(Ok(v)) => Ok(v.clone()),
                Some(Err(e)) => Err(Error::MarketData(MarketDataError::ProviderError(
                    e.to_string(),
                ))),
                None => Err(Error::MarketData(MarketDataError::ProviderError(
                    "splits not supported".to_string(),
                ))),
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
    async fn normalizes_alpha_vantage_unadjusted_closes_across_two_for_one_split() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();
        let d3 = NaiveDate::from_ymd_opt(2024, 1, 4).unwrap();
        let d4 = NaiveDate::from_ymd_opt(2024, 1, 5).unwrap();

        let mut quotes = HashMap::new();
        quotes.insert(
            "NVDA".to_string(),
            Ok(vec![
                stub_quote_with_source("NVDA", d1, "200", "USD", "ALPHA_VANTAGE"),
                stub_quote_with_source("NVDA", d2, "200", "USD", "ALPHA_VANTAGE"),
                // 2-for-1 split on d3 with zero economic gain/loss
                stub_quote_with_source("NVDA", d3, "100", "USD", "ALPHA_VANTAGE"),
                stub_quote_with_source("NVDA", d4, "100", "USD", "ALPHA_VANTAGE"),
            ]),
        );

        let mut splits = HashMap::new();
        splits.insert(
            "NVDA".to_string(),
            Ok(vec![SplitEvent {
                date: d3,
                ratio: dec!(2),
            }]),
        );

        let mut search = HashMap::new();
        search.insert(
            "NVDA".to_string(),
            vec![stub_search_result("NVDA", "XNAS", "EQUITY", "USD")],
        );

        let qs = Arc::new(StubQuoteService {
            quotes_by_symbol: quotes,
            dividends_by_symbol: HashMap::new(),
            splits_by_symbol: splits,
            search_by_symbol: search,
        });

        let req = BacktestRequest {
            assets: vec![req_asset("NVDA", 100.0)],
            start_date: d1,
            end_date: d4,
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: None,
            benchmark_exchange_mic: None,
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::BuyAndHold,
        };

        let res = run_backtest(qs, req).await.unwrap();
        assert!(res.strategy.metrics.twr.abs() < 1e-9);
        assert!((res.strategy.metrics.final_value - 10_000.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn rejects_unadjusted_source_when_no_split_provider_is_available() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();

        let mut quotes = HashMap::new();
        quotes.insert(
            "NVDA".to_string(),
            Ok(vec![
                stub_quote_with_source("NVDA", d1, "200", "USD", "ALPHA_VANTAGE"),
                stub_quote_with_source("NVDA", d2, "100", "USD", "ALPHA_VANTAGE"),
            ]),
        );

        let mut search = HashMap::new();
        search.insert(
            "NVDA".to_string(),
            vec![stub_search_result("NVDA", "XNAS", "EQUITY", "USD")],
        );

        let qs = Arc::new(StubQuoteService {
            quotes_by_symbol: quotes,
            dividends_by_symbol: HashMap::new(),
            splits_by_symbol: HashMap::new(),
            search_by_symbol: search,
        });

        let req = BacktestRequest {
            assets: vec![req_asset("NVDA", 100.0)],
            start_date: d1,
            end_date: d2,
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: None,
            benchmark_exchange_mic: None,
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::BuyAndHold,
        };

        let err = run_backtest(qs, req).await.unwrap_err();
        assert!(err.to_string().contains("unadjusted closes"));
    }

    #[tokio::test]
    async fn rejects_non_us_usd_listing_and_usd_crypto() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();

        let mut quotes = HashMap::new();
        quotes.insert(
            "VWRA.L".to_string(),
            Ok(vec![
                stub_quote("VWRA.L", d1, "100", "USD"),
                stub_quote("VWRA.L", d2, "105", "USD"),
            ]),
        );
        quotes.insert(
            "BTC-USD".to_string(),
            Ok(vec![
                stub_quote("BTC-USD", d1, "50000", "USD"),
                stub_quote("BTC-USD", d2, "52000", "USD"),
            ]),
        );

        let mut search = HashMap::new();
        search.insert(
            "VWRA.L".to_string(),
            vec![stub_search_result("VWRA.L", "XLON", "ETF", "USD")],
        );
        search.insert(
            "BTC-USD".to_string(),
            vec![stub_search_result(
                "BTC-USD",
                "CCC",
                "CRYPTOCURRENCY",
                "USD",
            )],
        );

        let qs: Arc<dyn QuoteServiceTrait> = Arc::new(StubQuoteService {
            quotes_by_symbol: quotes,
            dividends_by_symbol: HashMap::new(),
            splits_by_symbol: HashMap::new(),
            search_by_symbol: search,
        });

        // 1. London-listed USD ETF rejected
        let london_req = BacktestRequest {
            assets: vec![req_asset("VWRA.L", 100.0)],
            start_date: d1,
            end_date: d2,
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: None,
            benchmark_exchange_mic: None,
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::BuyAndHold,
        };
        assert!(run_backtest(Arc::clone(&qs), london_req).await.is_err());

        // 2. USD crypto rejected
        let crypto_req = BacktestRequest {
            assets: vec![req_asset("BTC-USD", 100.0)],
            start_date: d1,
            end_date: d2,
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: None,
            benchmark_exchange_mic: None,
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::BuyAndHold,
        };
        assert!(run_backtest(qs, crypto_req).await.is_err());
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

        let mut search = HashMap::new();
        search.insert(
            "VTI".to_string(),
            vec![stub_search_result("VTI", "ARCX", "ETF", "USD")],
        );
        search.insert(
            "BADBENCH".to_string(),
            vec![stub_search_result("BADBENCH", "ARCX", "ETF", "USD")],
        );

        let qs = Arc::new(StubQuoteService {
            quotes_by_symbol: quotes,
            dividends_by_symbol: divs,
            splits_by_symbol: HashMap::new(),
            search_by_symbol: search,
        });

        let req = BacktestRequest {
            assets: vec![req_asset("VTI", 100.0)],
            start_date: d1,
            end_date: d3,
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Reinvest,
            fee_bps: 0.0,
            benchmark_symbol: Some("BADBENCH".to_string()),
            benchmark_exchange_mic: None,
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::PeriodicRebalance {
                frequency: RebalanceFrequency::Quarterly,
            },
        };

        let res = run_backtest(qs, req).await.unwrap();
        assert!((res.strategy.metrics.final_value - 12_000.0).abs() < 1e-6);
        assert!(res.benchmark.is_none());
        assert!(res
            .warnings
            .contains(&BacktestWarning::BenchmarkUnavailable {
                symbol: "BADBENCH".to_string()
            }));
        assert!(res
            .warnings
            .contains(&BacktestWarning::DividendsUnavailable {
                symbol: "VTI".to_string()
            }));
        assert!(!res.symbol_coverage[0].dividends_available);
    }
}
