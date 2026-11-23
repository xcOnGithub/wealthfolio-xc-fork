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
    are_compatible_us_exchange_mics, canonicalize_exchange_mic, is_us_exchange_mic,
    operating_exchange_mic, reconcile_us_exchange_mics, resolve_exchange_label_to_mic,
};

use crate::assets::{Asset, AssetKind, InstrumentType, QuoteMode};
use crate::errors::{Error, Result, ValidationError};
use crate::portfolio::coordinator::blocking;
use crate::quotes::{FetchDividendsParams, Quote, QuoteServiceTrait, SymbolSearchResult};

use super::engine::{
    apply_splits_backward_for_dates, simulate, SymbolMarketSeries, BENCHMARK_SERIES_KEY,
    DEFENSIVE_SERIES_KEY,
};
use super::model::{
    is_allowed_us_security_type, BacktestRequest, BacktestResponse, BacktestStrategy,
    BacktestWarning, DividendMode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuoteSplitBasis {
    /// Historical daily closes are already split-adjusted by the provider.
    AlreadyAdjusted,
    /// Historical daily closes are as-traded (unadjusted) and require split normalization.
    NeedsSplitAdjustment,
    /// Source adjustment semantics are unknown; cannot safely normalize or assume adjusted.
    Unknown,
}

fn classify_quote_split_basis(source: &str) -> QuoteSplitBasis {
    match source.trim().to_ascii_uppercase().as_str() {
        "YAHOO" | "MARKETDATA_APP" | "FINNHUB" => QuoteSplitBasis::AlreadyAdjusted,
        "ALPHA_VANTAGE" => QuoteSplitBasis::NeedsSplitAdjustment,
        _ => QuoteSplitBasis::Unknown,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SymbolRole {
    Universe,
    Defensive,
    Benchmark,
}

#[derive(Debug, Clone)]
struct SymbolRequestMeta {
    role: SymbolRole,
    symbol: String,
    hint_mic: Option<String>,
    hint_type: Option<String>,
}

#[derive(Debug, Clone)]
struct VerifiedUsListing {
    /// Specific verified US MIC (e.g. `XNGS` when a segment hint is backed by `XNAS`).
    exchange_mic: String,
    /// Provider-reported MIC used to query quotes, splits, and dividends.
    provider_mic: String,
}

#[derive(Debug, Clone)]
struct FetchTarget {
    map_key: String,
    symbol: String,
    is_required: bool,
    listing: VerifiedUsListing,
}

/// Resolve a candidate exchange string (ISO 10383 MIC, Yahoo exchange code, or
/// known provider venue name/alias) into a canonical ISO 10383 MIC if recognized.
fn resolve_exchange_to_mic(raw: &str) -> Option<String> {
    resolve_exchange_label_to_mic(raw)
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
    // Reject empty/overlong tickers, non-equity characters (`EURUSD=X`, `^GSPC`,
    // `GC=F`, `BTC/USD`), and common crypto/FX pairs (`BTC-USD`, `ETH-USD`, etc.).
    if symbol.is_empty()
        || symbol.len() > 16
        || !symbol
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return true;
    }
    if let Some((_, quote_part)) = symbol.rsplit_once('-') {
        if matches!(quote_part, "USD" | "EUR" | "GBP" | "CAD" | "USDT" | "USDC") {
            return true;
        }
    }
    false
}

/// If a returned quote carries a canonical `SEC:<symbol>:<mic>` asset ID whose MIC
/// conflicts with `verified_mic`, return the conflicting MIC.
fn quote_venue_conflict(quotes: &[Quote], verified_mic: &str) -> Option<String> {
    for q in quotes {
        let parts: Vec<&str> = q.asset_id.split(':').collect();
        if parts.len() == 3 && parts[0].eq_ignore_ascii_case("SEC") {
            let raw_mic = parts[2].trim();
            if !raw_mic.is_empty() {
                let canonical = canonicalize_exchange_mic(raw_mic);
                if !are_compatible_us_exchange_mics(&canonical, verified_mic) {
                    return Some(canonical);
                }
            }
        }
    }
    None
}

/// Server-side verification that `symbol` refers to a US-listed, USD-denominated
/// equity/ETF/fund security, returning the authoritative US exchange MIC.
async fn verify_us_listing(
    quote_service: &Arc<dyn QuoteServiceTrait>,
    symbol: &str,
    hint_mic: Option<&str>,
    hint_type: Option<&str>,
) -> std::result::Result<VerifiedUsListing, String> {
    if is_obviously_non_us_equity_symbol(symbol) {
        return Err(format!(
            "Ticker '{symbol}' is outside the US-listed equity/ETF scope."
        ));
    }

    if let Some(hint_mic) = hint_mic {
        if !is_us_exchange_mic(hint_mic) {
            return Err(format!(
                "Ticker '{symbol}' on exchange '{hint_mic}' is not a US listing."
            ));
        }
    }
    if let Some(hint_type) = hint_type {
        if !is_allowed_us_security_type(hint_type) {
            return Err(format!(
                "Ticker '{symbol}' ({hint_type}) is outside the US-listed equity/ETF scope."
            ));
        }
    }

    let mut saw_incomplete_us_search_candidate = false;

    // 1. Check authoritative symbol search results from enabled providers.
    if let Ok(results) = quote_service.search_symbol(symbol).await {
        let exact: Vec<&SymbolSearchResult> = results
            .iter()
            .filter(|r| search_result_matches_symbol(r, symbol))
            .collect();

        if !exact.is_empty() {
            let mut matching_us_mics: Vec<String> = Vec::new();

            for candidate in &exact {
                let quote_type_ok = is_allowed_us_security_type(&candidate.quote_type);
                let currency_ok = candidate
                    .currency
                    .as_deref()
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                    .is_none_or(|c| c.eq_ignore_ascii_case("USD"));
                if !quote_type_ok || !currency_ok {
                    continue;
                }

                match search_result_mic(candidate) {
                    Some(mic) if is_us_exchange_mic(&mic) => {
                        if !matching_us_mics.contains(&mic) {
                            matching_us_mics.push(mic);
                        }
                    }
                    Some(_) => {
                        // Explicit non-US venue on this candidate; ignore.
                    }
                    None => {
                        // Exact match with valid type/currency but missing venue metadata
                        // (e.g. Finnhub symbol search). Continue to profile verification.
                        saw_incomplete_us_search_candidate = true;
                    }
                }
            }

            if let Some(hint_mic) = hint_mic {
                if matching_us_mics.iter().any(|m| m == hint_mic) {
                    return Ok(VerifiedUsListing {
                        exchange_mic: hint_mic.to_string(),
                        provider_mic: hint_mic.to_string(),
                    });
                }

                let hint_operating = operating_exchange_mic(hint_mic);
                let has_explicit_segment_conflict = hint_operating.as_deref().is_some_and(|op| {
                    matching_us_mics.iter().any(|m| {
                        operating_exchange_mic(m).as_deref() == Some(op)
                            && !are_compatible_us_exchange_mics(hint_mic, m)
                    })
                });
                if !has_explicit_segment_conflict {
                    if let Some((provider_mic, reconciled_mic)) =
                        matching_us_mics.iter().find_map(|m| {
                            reconcile_us_exchange_mics(hint_mic, m).map(|rec| (m.clone(), rec))
                        })
                    {
                        return Ok(VerifiedUsListing {
                            exchange_mic: reconciled_mic,
                            provider_mic,
                        });
                    }
                }

                if !matching_us_mics.is_empty() && !saw_incomplete_us_search_candidate {
                    return Err(format!(
                        "Ticker '{symbol}' exchange hint '{hint_mic}' conflicts with verified listing metadata."
                    ));
                }
                if !saw_incomplete_us_search_candidate {
                    return Err(format!(
                        "Ticker '{symbol}' is not a US-listed USD equity or ETF."
                    ));
                }
            } else if let Some(verified_mic) = matching_us_mics.into_iter().next() {
                return Ok(VerifiedUsListing {
                    exchange_mic: verified_mic.clone(),
                    provider_mic: verified_mic,
                });
            } else if !saw_incomplete_us_search_candidate {
                return Err(format!(
                    "Ticker '{symbol}' is not a US-listed USD equity or ETF."
                ));
            }
        }
    }

    // 2. Follow up with asset profile lookup through enabled providers, requiring
    //    authoritative provider venue evidence (never trusting an unverified client hint).
    let temp_asset = Asset {
        id: symbol.to_string(),
        instrument_symbol: Some(symbol.to_string()),
        display_code: Some(symbol.to_string()),
        kind: AssetKind::Investment,
        instrument_type: Some(InstrumentType::Equity),
        instrument_exchange_mic: hint_mic.map(str::to_string),
        quote_ccy: "USD".to_string(),
        quote_mode: QuoteMode::Market,
        ..Default::default()
    };

    if let Ok(profile) = quote_service
        .get_asset_profile_with_venue(&temp_asset)
        .await
    {
        let quote_type_ok = match profile.asset_type.as_deref() {
            Some(ty) => is_allowed_us_security_type(ty),
            None => saw_incomplete_us_search_candidate,
        };
        let profile_ccy = profile
            .listing_currency
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .or_else(|| {
                let c = profile.currency.trim();
                (!c.is_empty()).then_some(c)
            });
        let currency_ok = profile_ccy.is_none_or(|c| c.eq_ignore_ascii_case("USD"));
        let provider_mic = profile
            .exchange
            .as_deref()
            .and_then(resolve_exchange_to_mic);

        if let Some(verified_mic) = provider_mic {
            if !is_us_exchange_mic(&verified_mic) {
                return Err(format!(
                    "Ticker '{symbol}' on exchange '{verified_mic}' is not a US listing."
                ));
            }
            let reconciled_mic = if let Some(hint_mic) = hint_mic {
                let Some(reconciled) = reconcile_us_exchange_mics(hint_mic, &verified_mic) else {
                    return Err(format!(
                        "Ticker '{symbol}' exchange hint '{hint_mic}' conflicts with verified listing '{verified_mic}'."
                    ));
                };
                reconciled
            } else {
                verified_mic.clone()
            };
            if quote_type_ok && currency_ok {
                return Ok(VerifiedUsListing {
                    exchange_mic: reconciled_mic,
                    provider_mic: verified_mic,
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

enum SplitRequirement {
    AlreadyAdjusted,
    NeedsSplits {
        source_label: String,
        unadjusted_dates: HashSet<NaiveDate>,
    },
    UnknownBasis {
        source_label: String,
    },
}

/// Parse daily closes from raw quotes while preserving each date's split-adjustment basis.
fn extract_prices_and_split_requirement(
    quotes: &[Quote],
) -> (BTreeMap<NaiveDate, f64>, SplitRequirement) {
    let mut prices = BTreeMap::new();
    let mut basis_by_day: BTreeMap<NaiveDate, (QuoteSplitBasis, String)> = BTreeMap::new();

    for q in quotes {
        let day = q.timestamp.date_naive();
        if let Some(close) = q.close.to_f64() {
            if close.is_finite() && close > 0.0 {
                prices.insert(day, close);
                let src = q.data_source.trim().to_ascii_uppercase();
                let basis = classify_quote_split_basis(&src);
                basis_by_day.insert(day, (basis, src));
            }
        }
    }

    let mut unknown_sources: HashSet<String> = HashSet::new();
    let mut unadjusted_sources: HashSet<String> = HashSet::new();
    let mut unadjusted_dates: HashSet<NaiveDate> = HashSet::new();

    for (&day, (basis, src)) in &basis_by_day {
        match basis {
            QuoteSplitBasis::AlreadyAdjusted => {}
            QuoteSplitBasis::NeedsSplitAdjustment => {
                unadjusted_dates.insert(day);
                unadjusted_sources.insert(src.clone());
            }
            QuoteSplitBasis::Unknown => {
                unknown_sources.insert(if src.is_empty() {
                    "UNKNOWN".to_string()
                } else {
                    src.clone()
                });
            }
        }
    }

    let requirement = if !unknown_sources.is_empty() {
        let mut sorted: Vec<String> = unknown_sources.into_iter().collect();
        sorted.sort();
        SplitRequirement::UnknownBasis {
            source_label: sorted.join("+"),
        }
    } else if !unadjusted_dates.is_empty() {
        let mut sorted: Vec<String> = unadjusted_sources.into_iter().collect();
        sorted.sort();
        SplitRequirement::NeedsSplits {
            source_label: sorted.join("+"),
            unadjusted_dates,
        }
    } else {
        SplitRequirement::AlreadyAdjusted
    };

    (prices, requirement)
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

    // Verify universe holdings, optional defensive ticker, and optional benchmark
    // independently so same-symbol listings on distinct US venues never overwrite
    // or silently share each other's venue metadata.
    let mut verify_metas: Vec<SymbolRequestMeta> = req
        .assets
        .iter()
        .map(|asset| SymbolRequestMeta {
            role: SymbolRole::Universe,
            symbol: asset.symbol.clone(),
            hint_mic: asset.exchange_mic.clone(),
            hint_type: asset.instrument_type.clone(),
        })
        .collect();

    if let Some((def_sym, def_mic, def_type)) = defensive_listing(&req.strategy) {
        verify_metas.push(SymbolRequestMeta {
            role: SymbolRole::Defensive,
            symbol: def_sym.to_string(),
            hint_mic: def_mic.map(str::to_string),
            hint_type: def_type.map(str::to_string),
        });
    }

    if let Some(bench) = &req.benchmark_symbol {
        verify_metas.push(SymbolRequestMeta {
            role: SymbolRole::Benchmark,
            symbol: bench.clone(),
            hint_mic: req.benchmark_exchange_mic.clone(),
            hint_type: req.benchmark_instrument_type.clone(),
        });
    }

    type VerifyKey = (String, Option<String>, Option<String>);
    let mut unique_verify_keys: Vec<VerifyKey> = Vec::new();
    for meta in &verify_metas {
        let key = (
            meta.symbol.clone(),
            meta.hint_mic.clone(),
            meta.hint_type.clone(),
        );
        if !unique_verify_keys.contains(&key) {
            unique_verify_keys.push(key);
        }
    }

    let verify_futures = unique_verify_keys.into_iter().map(|key| {
        let qs = Arc::clone(&quote_service);
        async move {
            let listing_res =
                verify_us_listing(&qs, &key.0, key.1.as_deref(), key.2.as_deref()).await;
            (key, listing_res)
        }
    });
    let verified_by_key: HashMap<VerifyKey, std::result::Result<VerifiedUsListing, String>> =
        join_all(verify_futures).await.into_iter().collect();

    let mut warnings: Vec<BacktestWarning> = Vec::new();
    let mut fetch_targets: Vec<FetchTarget> = Vec::new();
    let mut benchmark_alias_from_key: Option<String> = None;

    for meta in verify_metas {
        let key = (
            meta.symbol.clone(),
            meta.hint_mic.clone(),
            meta.hint_type.clone(),
        );
        let listing_res = verified_by_key
            .get(&key)
            .cloned()
            .expect("every verify_meta key was queued in unique_verify_keys");

        match (meta.role, listing_res) {
            (SymbolRole::Universe, Ok(verified_listing)) => {
                fetch_targets.push(FetchTarget {
                    map_key: meta.symbol.clone(),
                    symbol: meta.symbol,
                    is_required: true,
                    listing: verified_listing,
                });
            }
            (SymbolRole::Universe, Err(reason)) | (SymbolRole::Defensive, Err(reason)) => {
                return Err(Error::Validation(ValidationError::InvalidInput(reason)));
            }
            (SymbolRole::Defensive, Ok(def_listing)) => {
                if let Some(existing) = fetch_targets
                    .iter_mut()
                    .find(|t| t.is_required && t.symbol == meta.symbol)
                {
                    if let Some(reconciled) = reconcile_us_exchange_mics(
                        &def_listing.exchange_mic,
                        &existing.listing.exchange_mic,
                    ) {
                        existing.listing.exchange_mic = reconciled;
                    } else if existing.listing.provider_mic == def_listing.provider_mic {
                        return Err(Error::Validation(ValidationError::InvalidInput(format!(
                            "Defensive ticker '{}' on '{}' conflicts with portfolio holding on '{}'.",
                            meta.symbol, def_listing.exchange_mic, existing.listing.exchange_mic
                        ))));
                    } else {
                        fetch_targets.push(FetchTarget {
                            map_key: DEFENSIVE_SERIES_KEY.to_string(),
                            symbol: meta.symbol,
                            is_required: true,
                            listing: def_listing,
                        });
                    }
                } else {
                    fetch_targets.push(FetchTarget {
                        map_key: meta.symbol.clone(),
                        symbol: meta.symbol,
                        is_required: true,
                        listing: def_listing,
                    });
                }
            }
            (SymbolRole::Benchmark, Ok(bench_listing)) => {
                if let Some(matched) = fetch_targets.iter().find(|t| {
                    t.symbol == meta.symbol
                        && are_compatible_us_exchange_mics(
                            &t.listing.exchange_mic,
                            &bench_listing.exchange_mic,
                        )
                }) {
                    if matched.map_key != meta.symbol {
                        benchmark_alias_from_key = Some(matched.map_key.clone());
                    }
                } else if fetch_targets.iter().any(|t| {
                    t.symbol == meta.symbol && t.listing.provider_mic == bench_listing.provider_mic
                }) {
                    warnings.push(BacktestWarning::BenchmarkNotUsListing {
                        symbol: meta.symbol,
                    });
                    req.benchmark_symbol = None;
                } else {
                    let map_key = if fetch_targets.iter().any(|t| t.symbol == meta.symbol) {
                        BENCHMARK_SERIES_KEY.to_string()
                    } else {
                        meta.symbol.clone()
                    };
                    fetch_targets.push(FetchTarget {
                        map_key,
                        symbol: meta.symbol,
                        is_required: false,
                        listing: bench_listing,
                    });
                }
            }
            (SymbolRole::Benchmark, Err(_)) => {
                warnings.push(BacktestWarning::BenchmarkNotUsListing {
                    symbol: meta.symbol,
                });
                req.benchmark_symbol = None;
            }
        }
    }

    let include_dividends = req.dividend_mode != DividendMode::Exclude;

    let fetch_futures = fetch_targets.into_iter().map(|target| {
        let qs = Arc::clone(&quote_service);
        async move {
            // Use the verified provider listing MIC across quotes, splits, and dividends.
            let quotes_res = qs
                .fetch_quotes_for_listing(
                    &target.symbol,
                    Some(&target.listing.provider_mic),
                    "USD",
                    fetch_start,
                    fetch_end,
                )
                .await;

            // Inspect per-quote split-adjustment basis and fetch splits only when
            // at least one retained quote came from an unadjusted source.
            let split_fetch = match &quotes_res {
                Ok(quotes) if !quotes.is_empty() => {
                    let (prices, requirement) = extract_prices_and_split_requirement(quotes);
                    match requirement {
                        SplitRequirement::AlreadyAdjusted => Some((prices, None, None)),
                        SplitRequirement::UnknownBasis { source_label } => {
                            Some((prices, Some(source_label), None))
                        }
                        SplitRequirement::NeedsSplits {
                            source_label,
                            unadjusted_dates,
                        } => {
                            let fetched = qs
                                .fetch_splits_for_symbol(
                                    &target.symbol,
                                    Some(&target.listing.provider_mic),
                                    fetch_start,
                                    today,
                                )
                                .await;
                            Some((
                                prices,
                                None,
                                Some((source_label, unadjusted_dates, fetched)),
                            ))
                        }
                    }
                }
                _ => None,
            };

            let dividends_res = if include_dividends && quotes_res.is_ok() {
                Some(
                    qs.fetch_dividends(FetchDividendsParams {
                        symbol: target.symbol.clone(),
                        exchange_mic: Some(target.listing.provider_mic.clone()),
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

            (target, quotes_res, split_fetch, dividends_res)
        }
    });

    let fetched = join_all(fetch_futures).await;

    let mut series_map: HashMap<String, SymbolMarketSeries> = HashMap::new();

    for (target, quotes_res, split_fetch_opt, dividends_res) in fetched {
        let symbol = target.symbol;
        let is_required = target.is_required;
        let verified_listing = target.listing;

        let quotes = match quotes_res {
            Ok(quotes) if !quotes.is_empty() => quotes,
            Ok(_) => {
                if is_required {
                    return Err(Error::Validation(ValidationError::InvalidInput(format!(
                        "No historical price data found for '{symbol}' in the selected range."
                    ))));
                }
                warnings.push(BacktestWarning::BenchmarkUnavailable { symbol });
                req.benchmark_symbol = None;
                continue;
            }
            Err(err) => {
                if is_required {
                    return Err(err);
                }
                warnings.push(BacktestWarning::BenchmarkUnavailable { symbol });
                req.benchmark_symbol = None;
                continue;
            }
        };

        // Reject quotes whose canonical asset ID carries a conflicting exchange MIC.
        if let Some(conflict_mic) = quote_venue_conflict(&quotes, &verified_listing.exchange_mic) {
            if is_required {
                return Err(Error::Validation(ValidationError::InvalidInput(format!(
                    "Quotes for '{symbol}' belonged to exchange '{conflict_mic}', conflicting with verified US listing '{}'.",
                    verified_listing.exchange_mic
                ))));
            }
            warnings.push(BacktestWarning::BenchmarkNotUsListing { symbol });
            req.benchmark_symbol = None;
            continue;
        }

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
            req.benchmark_symbol = None;
            continue;
        }

        let Some((mut prices, unknown_source_opt, splits_opt)) = split_fetch_opt else {
            continue;
        };

        if let Some(unknown_source) = unknown_source_opt {
            if is_required {
                return Err(Error::Validation(ValidationError::InvalidInput(format!(
                    "Provider '{unknown_source}' has an unverified split-adjustment basis for '{symbol}'."
                ))));
            }
            warnings.push(BacktestWarning::BenchmarkSplitAdjustmentUnavailable {
                symbol,
                source: unknown_source,
            });
            req.benchmark_symbol = None;
            continue;
        }

        // Apply split normalization strictly to unadjusted quote dates, leaving
        // already split-adjusted dates untouched in mixed-source series.
        if let Some((source_label, unadjusted_dates, splits_res)) = splits_opt {
            match splits_res {
                Ok(splits) => {
                    apply_splits_backward_for_dates(&mut prices, &unadjusted_dates, &splits);
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
                    req.benchmark_symbol = None;
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
            req.benchmark_symbol = None;
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
            target.map_key,
            SymbolMarketSeries {
                symbol,
                exchange_mic: Some(verified_listing.exchange_mic),
                currency: detected_currency,
                prices,
                dividends,
                dividends_available,
            },
        );
    }

    if let Some(src_key) = benchmark_alias_from_key {
        if let Some(cloned) = series_map.get(&src_key).cloned() {
            series_map.insert(BENCHMARK_SERIES_KEY.to_string(), cloned);
        }
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

    use std::sync::Mutex;

    #[derive(Default)]
    struct StubQuoteService {
        quotes_by_symbol: HashMap<String, Result<Vec<Quote>>>,
        dividends_by_symbol: HashMap<String, Result<Vec<DividendEvent>>>,
        splits_by_symbol: HashMap<String, Result<Vec<SplitEvent>>>,
        search_by_symbol: HashMap<String, Vec<SymbolSearchResult>>,
        profiles_by_symbol: HashMap<String, ProviderProfile>,
        recorded_quote_listings: Mutex<Vec<(String, Option<String>)>>,
        recorded_split_listings: Mutex<Vec<(String, Option<String>)>>,
        recorded_dividend_listings: Mutex<Vec<(String, Option<String>)>>,
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
        async fn get_asset_profile(&self, asset: &Asset) -> Result<ProviderProfile> {
            let key = asset
                .instrument_symbol
                .as_deref()
                .unwrap_or(asset.id.as_str());
            self.profiles_by_symbol
                .get(key)
                .cloned()
                .ok_or_else(|| Error::MarketData(MarketDataError::NotFound(key.to_string())))
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
        async fn fetch_quotes_for_listing(
            &self,
            symbol: &str,
            exchange_mic: Option<&str>,
            currency: &str,
            start: NaiveDate,
            end: NaiveDate,
        ) -> Result<Vec<Quote>> {
            self.recorded_quote_listings
                .lock()
                .unwrap()
                .push((symbol.to_string(), exchange_mic.map(str::to_string)));
            if let Some(mic) = exchange_mic {
                let scoped_key = format!("{symbol}:{mic}");
                if self.quotes_by_symbol.contains_key(&scoped_key) {
                    return self
                        .fetch_quotes_for_symbol(&scoped_key, currency, start, end)
                        .await;
                }
            }
            self.fetch_quotes_for_symbol(symbol, currency, start, end)
                .await
        }
        async fn fetch_dividends(
            &self,
            params: FetchDividendsParams,
        ) -> Result<Vec<DividendEvent>> {
            self.recorded_dividend_listings
                .lock()
                .unwrap()
                .push((params.symbol.clone(), params.exchange_mic.clone()));
            let lookup_key = params
                .exchange_mic
                .as_deref()
                .map(|mic| format!("{}:{mic}", params.symbol))
                .filter(|k| self.dividends_by_symbol.contains_key(k));
            let res = lookup_key
                .as_deref()
                .and_then(|k| self.dividends_by_symbol.get(k))
                .or_else(|| self.dividends_by_symbol.get(&params.symbol));
            match res {
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
            exchange_mic: Option<&str>,
            _start: NaiveDate,
            _end: NaiveDate,
        ) -> Result<Vec<SplitEvent>> {
            self.recorded_split_listings
                .lock()
                .unwrap()
                .push((symbol.to_string(), exchange_mic.map(str::to_string)));
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
            splits_by_symbol: splits,
            search_by_symbol: search,
            ..Default::default()
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
    async fn mixed_yahoo_and_alpha_vantage_history_does_not_double_adjust_splits() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();
        let d3 = NaiveDate::from_ymd_opt(2024, 1, 4).unwrap();
        let d4 = NaiveDate::from_ymd_opt(2024, 1, 5).unwrap();

        // Day 1 is already split-adjusted from YAHOO ($100), Day 2 is unadjusted from
        // ALPHA_VANTAGE ($200), Day 3 is a 2-for-1 split ($100) with zero economic change.
        let mut quotes = HashMap::new();
        quotes.insert(
            "NVDA".to_string(),
            Ok(vec![
                stub_quote_with_source("NVDA", d1, "100", "USD", "YAHOO"),
                stub_quote_with_source("NVDA", d2, "200", "USD", "ALPHA_VANTAGE"),
                stub_quote_with_source("NVDA", d3, "100", "USD", "ALPHA_VANTAGE"),
                stub_quote_with_source("NVDA", d4, "100", "USD", "YAHOO"),
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
            splits_by_symbol: splits,
            search_by_symbol: search,
            ..Default::default()
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
            search_by_symbol: search,
            ..Default::default()
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
    async fn uses_verified_listing_mic_for_quotes_splits_and_dividends_and_rejects_venue_conflict()
    {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();

        let mut quotes = HashMap::new();
        quotes.insert(
            "AAPL".to_string(),
            Ok(vec![
                stub_quote_with_source("AAPL", d1, "100", "USD", "ALPHA_VANTAGE"),
                stub_quote_with_source("AAPL", d2, "105", "USD", "ALPHA_VANTAGE"),
            ]),
        );
        let mut splits = HashMap::new();
        splits.insert("AAPL".to_string(), Ok(vec![]));
        let mut search = HashMap::new();
        search.insert(
            "AAPL".to_string(),
            vec![stub_search_result("AAPL", "XNAS", "EQUITY", "USD")],
        );

        let stub = Arc::new(StubQuoteService {
            quotes_by_symbol: quotes,
            splits_by_symbol: splits,
            search_by_symbol: search,
            ..Default::default()
        });
        let qs: Arc<dyn QuoteServiceTrait> = stub.clone();

        let req = BacktestRequest {
            assets: vec![req_asset("AAPL", 100.0)],
            start_date: d1,
            end_date: d2,
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Reinvest,
            fee_bps: 0.0,
            benchmark_symbol: None,
            benchmark_exchange_mic: None,
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::BuyAndHold,
        };

        run_backtest(qs, req).await.unwrap();
        assert_eq!(
            *stub.recorded_quote_listings.lock().unwrap(),
            vec![("AAPL".to_string(), Some("XNAS".to_string()))]
        );
        assert_eq!(
            *stub.recorded_split_listings.lock().unwrap(),
            vec![("AAPL".to_string(), Some("XNAS".to_string()))]
        );
        assert_eq!(
            *stub.recorded_dividend_listings.lock().unwrap(),
            vec![("AAPL".to_string(), Some("XNAS".to_string()))]
        );

        // If quotes carry a conflicting canonical venue (e.g. SEC:AAPL:XTSE), reject.
        let mut bad_quote_1 = stub_quote("AAPL", d1, "100", "USD");
        bad_quote_1.asset_id = "SEC:AAPL:XTSE".to_string();
        let mut bad_quote_2 = stub_quote("AAPL", d2, "105", "USD");
        bad_quote_2.asset_id = "SEC:AAPL:XTSE".to_string();
        let mut conflict_quotes = HashMap::new();
        conflict_quotes.insert("AAPL".to_string(), Ok(vec![bad_quote_1, bad_quote_2]));
        let mut conflict_search = HashMap::new();
        conflict_search.insert(
            "AAPL".to_string(),
            vec![stub_search_result("AAPL", "XNAS", "EQUITY", "USD")],
        );
        let conflict_qs = Arc::new(StubQuoteService {
            quotes_by_symbol: conflict_quotes,
            search_by_symbol: conflict_search,
            ..Default::default()
        });
        let conflict_req = BacktestRequest {
            assets: vec![req_asset("AAPL", 100.0)],
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
        let err = run_backtest(conflict_qs, conflict_req).await.unwrap_err();
        assert!(err
            .to_string()
            .contains("conflicting with verified US listing"));
    }

    #[tokio::test]
    async fn continues_to_profile_when_search_lacks_venue_and_requires_authoritative_venue() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();

        // Finnhub-style exact match: quote_type "Stock" and no MIC/exchange.
        let finnhub_search = SymbolSearchResult {
            symbol: "MSFT".to_string(),
            canonical_symbol: Some("MSFT".to_string()),
            exchange: String::new(),
            exchange_mic: None,
            canonical_exchange_mic: None,
            quote_type: "Stock".to_string(),
            currency: None,
            ..Default::default()
        };

        let make_req = || BacktestRequest {
            assets: vec![BacktestAssetInput {
                symbol: "MSFT".to_string(),
                weight: 100.0,
                exchange_mic: Some("XNAS".to_string()),
                instrument_type: Some("Stock".to_string()),
            }],
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

        // 1. Succeeds when profile fallback supplies authoritative US exchange ("NMS" -> XNAS).
        let make_quotes = || {
            let mut quotes = HashMap::new();
            quotes.insert(
                "MSFT".to_string(),
                Ok(vec![
                    stub_quote("MSFT", d1, "400", "USD"),
                    stub_quote("MSFT", d2, "420", "USD"),
                ]),
            );
            quotes
        };
        let mut search = HashMap::new();
        search.insert("MSFT".to_string(), vec![finnhub_search.clone()]);
        let mut profiles = HashMap::new();
        profiles.insert(
            "MSFT".to_string(),
            ProviderProfile {
                symbol: "MSFT".to_string(),
                asset_type: Some("EQUITY".to_string()),
                listing_currency: Some("USD".to_string()),
                exchange: Some("NMS".to_string()),
                data_source: "YAHOO".to_string(),
                ..Default::default()
            },
        );

        let qs_ok = Arc::new(StubQuoteService {
            quotes_by_symbol: make_quotes(),
            search_by_symbol: search.clone(),
            profiles_by_symbol: profiles,
            ..Default::default()
        });
        let res = run_backtest(qs_ok, make_req()).await.unwrap();
        assert!((res.strategy.metrics.final_value - 10_500.0).abs() < 1e-6);

        // 2. Rejects when profile fallback lacks authoritative venue evidence (exchange: None),
        //    even though the request passed an XNAS hint.
        let mut no_venue_profiles = HashMap::new();
        no_venue_profiles.insert(
            "MSFT".to_string(),
            ProviderProfile {
                symbol: "MSFT".to_string(),
                asset_type: Some("EQUITY".to_string()),
                listing_currency: Some("USD".to_string()),
                exchange: None,
                data_source: "FINNHUB".to_string(),
                ..Default::default()
            },
        );
        let qs_no_venue = Arc::new(StubQuoteService {
            quotes_by_symbol: make_quotes(),
            search_by_symbol: search,
            profiles_by_symbol: no_venue_profiles,
            ..Default::default()
        });
        assert!(run_backtest(qs_no_venue, make_req()).await.is_err());
    }

    #[tokio::test]
    async fn omits_out_of_scope_benchmark_with_non_us_mic_without_failing_run() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();

        let mut quotes = HashMap::new();
        quotes.insert(
            "VTI".to_string(),
            Ok(vec![
                stub_quote("VTI", d1, "100", "USD"),
                stub_quote("VTI", d2, "110", "USD"),
            ]),
        );
        let mut search = HashMap::new();
        search.insert(
            "VTI".to_string(),
            vec![stub_search_result("VTI", "ARCX", "ETF", "USD")],
        );
        search.insert(
            "SHOP.TO".to_string(),
            vec![stub_search_result("SHOP.TO", "XTSE", "EQUITY", "CAD")],
        );

        let qs = Arc::new(StubQuoteService {
            quotes_by_symbol: quotes,
            search_by_symbol: search,
            ..Default::default()
        });

        let req = BacktestRequest {
            assets: vec![req_asset("VTI", 100.0)],
            start_date: d1,
            end_date: d2,
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: Some("SHOP.TO".to_string()),
            benchmark_exchange_mic: Some("XTSE".to_string()),
            benchmark_instrument_type: Some("EQUITY".to_string()),
            strategy: BacktestStrategy::BuyAndHold,
        };

        let res = run_backtest(qs, req).await.unwrap();
        assert!((res.strategy.metrics.final_value - 11_000.0).abs() < 1e-6);
        assert!(res.benchmark.is_none());
        assert!(res
            .warnings
            .contains(&BacktestWarning::BenchmarkNotUsListing {
                symbol: "SHOP.TO".to_string()
            }));
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
            search_by_symbol: search,
            ..Default::default()
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
            search_by_symbol: search,
            ..Default::default()
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

    #[tokio::test]
    async fn keeps_same_symbol_benchmark_metadata_independent_of_required_holdings() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();

        let make_qs = || {
            let mut quotes = HashMap::new();
            quotes.insert(
                "MSFT".to_string(),
                Ok(vec![
                    stub_quote("MSFT", d1, "400", "USD"),
                    stub_quote("MSFT", d2, "420", "USD"),
                ]),
            );
            let mut search = HashMap::new();
            search.insert(
                "MSFT".to_string(),
                vec![stub_search_result("MSFT", "XNAS", "EQUITY", "USD")],
            );
            Arc::new(StubQuoteService {
                quotes_by_symbol: quotes,
                search_by_symbol: search,
                ..Default::default()
            })
        };

        // 1. Unqualified MSFT holding + optional MSFT benchmark pinned to XNYS:
        //    required holding still resolves on XNAS and succeeds; benchmark is omitted with warning.
        let req_unqualified = BacktestRequest {
            assets: vec![req_asset("MSFT", 100.0)],
            start_date: d1,
            end_date: d2,
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: Some("MSFT".to_string()),
            benchmark_exchange_mic: Some("XNYS".to_string()),
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::BuyAndHold,
        };
        let res_unqualified = run_backtest(make_qs(), req_unqualified).await.unwrap();
        assert!((res_unqualified.strategy.metrics.final_value - 10_500.0).abs() < 1e-6);
        assert!(res_unqualified.benchmark.is_none());
        assert!(res_unqualified
            .warnings
            .contains(&BacktestWarning::BenchmarkNotUsListing {
                symbol: "MSFT".to_string()
            }));

        // 2. XNAS-pinned MSFT holding + optional MSFT benchmark pinned to XNYS:
        //    must omit the conflicting XNYS benchmark with warning rather than reusing Nasdaq prices.
        let req_pinned = BacktestRequest {
            assets: vec![BacktestAssetInput {
                symbol: "MSFT".to_string(),
                weight: 100.0,
                exchange_mic: Some("XNAS".to_string()),
                instrument_type: Some("EQUITY".to_string()),
            }],
            start_date: d1,
            end_date: d2,
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: Some("MSFT".to_string()),
            benchmark_exchange_mic: Some("XNYS".to_string()),
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::BuyAndHold,
        };
        let res_pinned = run_backtest(make_qs(), req_pinned).await.unwrap();
        assert!((res_pinned.strategy.metrics.final_value - 10_500.0).abs() < 1e-6);
        assert!(res_pinned.benchmark.is_none());
        assert!(res_pinned
            .warnings
            .contains(&BacktestWarning::BenchmarkNotUsListing {
                symbol: "MSFT".to_string()
            }));

        // 3. Matching MSFT/XNAS benchmark reuses history with a single quote fetch.
        let stub_shared = make_qs();
        let qs_shared: Arc<dyn QuoteServiceTrait> = stub_shared.clone();
        let req_matching = BacktestRequest {
            assets: vec![req_asset("MSFT", 100.0)],
            start_date: d1,
            end_date: d2,
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: Some("MSFT".to_string()),
            benchmark_exchange_mic: Some("XNAS".to_string()),
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::BuyAndHold,
        };
        let res_matching = run_backtest(qs_shared, req_matching).await.unwrap();
        assert!(res_matching.benchmark.is_some());
        assert_eq!(stub_shared.recorded_quote_listings.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn omits_non_standard_benchmark_symbol_like_fx_pair_without_failing_run() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();

        let mut quotes = HashMap::new();
        quotes.insert(
            "VTI".to_string(),
            Ok(vec![
                stub_quote("VTI", d1, "100", "USD"),
                stub_quote("VTI", d2, "110", "USD"),
            ]),
        );
        let mut search = HashMap::new();
        search.insert(
            "VTI".to_string(),
            vec![stub_search_result("VTI", "ARCX", "ETF", "USD")],
        );

        let qs = Arc::new(StubQuoteService {
            quotes_by_symbol: quotes,
            search_by_symbol: search,
            ..Default::default()
        });

        let req = BacktestRequest {
            assets: vec![req_asset("VTI", 100.0)],
            start_date: d1,
            end_date: d2,
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: Some("EURUSD=X".to_string()),
            benchmark_exchange_mic: None,
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::BuyAndHold,
        };

        let res = run_backtest(qs, req).await.unwrap();
        assert!((res.strategy.metrics.final_value - 11_000.0).abs() < 1e-6);
        assert!(res.benchmark.is_none());
        assert!(res
            .warnings
            .contains(&BacktestWarning::BenchmarkNotUsListing {
                symbol: "EURUSD=X".to_string()
            }));
    }

    #[tokio::test]
    async fn rejects_foreign_nasdaq_profile_and_unrecognized_exchange_labels() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();

        let make_quotes = || {
            let mut quotes = HashMap::new();
            quotes.insert(
                "VOLV".to_string(),
                Ok(vec![
                    stub_quote("VOLV", d1, "100", "USD"),
                    stub_quote("VOLV", d2, "105", "USD"),
                ]),
            );
            quotes
        };

        let incomplete_search = SymbolSearchResult {
            symbol: "VOLV".to_string(),
            canonical_symbol: Some("VOLV".to_string()),
            exchange: "UNFAMILIAR VENUE LABEL".to_string(),
            exchange_mic: None,
            canonical_exchange_mic: None,
            quote_type: "Stock".to_string(),
            currency: Some("USD".to_string()),
            ..Default::default()
        };
        let mut search = HashMap::new();
        search.insert("VOLV".to_string(), vec![incomplete_search]);

        // 1. Foreign Nasdaq ("NASDAQ STOCKHOLM") maps to XSTO (SE), never XNAS, and is rejected.
        let mut stockholm_profiles = HashMap::new();
        stockholm_profiles.insert(
            "VOLV".to_string(),
            ProviderProfile {
                symbol: "VOLV".to_string(),
                asset_type: Some("EQUITY".to_string()),
                listing_currency: Some("USD".to_string()),
                exchange: Some("NASDAQ STOCKHOLM".to_string()),
                data_source: "FINNHUB".to_string(),
                ..Default::default()
            },
        );
        let qs_stockholm = Arc::new(StubQuoteService {
            quotes_by_symbol: make_quotes(),
            search_by_symbol: search.clone(),
            profiles_by_symbol: stockholm_profiles,
            ..Default::default()
        });
        let req = BacktestRequest {
            assets: vec![req_asset("VOLV", 100.0)],
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
        let err_stockholm = run_backtest(qs_stockholm, req.clone()).await.unwrap_err();
        assert!(err_stockholm.to_string().contains("XSTO"));

        // 2. Unrecognized exchange label in search continues to profile fallback; if profile
        //    supplies US Nasdaq ("NASDAQ NMS - GLOBAL MARKET"), verification succeeds.
        let mut us_profiles = HashMap::new();
        us_profiles.insert(
            "VOLV".to_string(),
            ProviderProfile {
                symbol: "VOLV".to_string(),
                asset_type: Some("EQUITY".to_string()),
                listing_currency: Some("USD".to_string()),
                exchange: Some("NASDAQ NMS - GLOBAL MARKET".to_string()),
                data_source: "FINNHUB".to_string(),
                ..Default::default()
            },
        );
        let qs_us = Arc::new(StubQuoteService {
            quotes_by_symbol: make_quotes(),
            search_by_symbol: search,
            profiles_by_symbol: us_profiles,
            ..Default::default()
        });
        let ok_res = run_backtest(qs_us, req).await.unwrap();
        assert!((ok_res.strategy.metrics.final_value - 10_500.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn reports_separate_coverage_for_same_symbol_benchmark_on_different_venue() {
        let d0 = NaiveDate::from_ymd_opt(2023, 12, 29).unwrap();
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();

        let mut quotes = HashMap::new();
        // Strategy holding on XNAS: flat 0% return over [d1, d2]
        quotes.insert(
            "DUAL:XNAS".to_string(),
            Ok(vec![
                stub_quote("DUAL", d1, "100", "USD"),
                stub_quote("DUAL", d2, "100", "USD"),
            ]),
        );
        // Benchmark on XNYS: +25% return over [d0, d2] with an earlier first_date
        quotes.insert(
            "DUAL:XNYS".to_string(),
            Ok(vec![
                stub_quote("DUAL", d0, "95", "USD"),
                stub_quote("DUAL", d1, "100", "USD"),
                stub_quote("DUAL", d2, "125", "USD"),
            ]),
        );

        let mut divs = HashMap::new();
        divs.insert(
            "DUAL:XNAS".to_string(),
            Err(Error::MarketData(MarketDataError::ProviderError(
                "no dividends on XNAS".to_string(),
            ))),
        );
        divs.insert(
            "DUAL:XNYS".to_string(),
            Ok(vec![DividendEvent {
                date: Utc
                    .from_utc_datetime(&d2.and_hms_opt(12, 0, 0).unwrap())
                    .timestamp(),
                amount: 1.5,
            }]),
        );

        let mut search = HashMap::new();
        search.insert(
            "DUAL".to_string(),
            vec![
                stub_search_result("DUAL", "XNAS", "EQUITY", "USD"),
                stub_search_result("DUAL", "XNYS", "EQUITY", "USD"),
            ],
        );

        let qs = Arc::new(StubQuoteService {
            quotes_by_symbol: quotes,
            dividends_by_symbol: divs,
            search_by_symbol: search,
            ..Default::default()
        });

        let req = BacktestRequest {
            assets: vec![BacktestAssetInput {
                symbol: "DUAL".to_string(),
                weight: 100.0,
                exchange_mic: Some("XNAS".to_string()),
                instrument_type: Some("EQUITY".to_string()),
            }],
            start_date: d1,
            end_date: d2,
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Cash,
            fee_bps: 0.0,
            benchmark_symbol: Some("DUAL".to_string()),
            benchmark_exchange_mic: Some("XNYS".to_string()),
            benchmark_instrument_type: Some("EQUITY".to_string()),
            strategy: BacktestStrategy::BuyAndHold,
        };

        let res = run_backtest(qs, req).await.unwrap();
        assert!(res.strategy.metrics.twr.abs() < 1e-9);
        let bench = res.benchmark.expect("XNYS benchmark should be present");
        // 100 shares bought at $100 -> $125 price + $1.50 cash dividend = $126.50 (+26.5% TWR)
        assert!((bench.metrics.twr - 0.265).abs() < 1e-9);

        assert_eq!(res.symbol_coverage.len(), 2);
        let holding_cov = &res.symbol_coverage[0];
        assert_eq!(holding_cov.symbol, "DUAL");
        assert_eq!(holding_cov.exchange_mic.as_deref(), Some("XNAS"));
        assert!(!holding_cov.is_benchmark);
        assert_eq!(holding_cov.first_date, d1);
        assert!(!holding_cov.dividends_available);

        let bench_cov = &res.symbol_coverage[1];
        assert_eq!(bench_cov.symbol, "DUAL");
        assert_eq!(bench_cov.exchange_mic.as_deref(), Some("XNYS"));
        assert!(bench_cov.is_benchmark);
        assert_eq!(bench_cov.first_date, d0);
        assert!(bench_cov.dividends_available);
        assert_eq!(bench_cov.dividend_events_count, 1);
    }

    #[tokio::test]
    async fn keeps_same_symbol_defensive_listing_separate_from_required_holding_in_sma() {
        use crate::portfolio::backtest::model::SignalFrequency;

        let dates: Vec<NaiveDate> = [
            (2025, 1, 27),
            (2025, 1, 28),
            (2025, 1, 29),
            (2025, 1, 30),
            (2025, 1, 31),
            (2025, 2, 3),
            (2025, 2, 4),
        ]
        .into_iter()
        .map(|(y, m, d)| NaiveDate::from_ymd_opt(y, m, d).unwrap())
        .collect();

        // MSFT on XNAS: 100 across warmup, then drops to 90 on Feb 3 (below 5-day SMA = 98) and stays at 90.
        let xnas_closes = ["100", "100", "100", "100", "100", "90", "90"];
        // MSFT on XNYS (defensive listing): 100 through Feb 3, then rallies to 130 on Feb 4 (+30%).
        let xnys_closes = ["100", "100", "100", "100", "100", "100", "130"];

        let mut quotes = HashMap::new();
        quotes.insert(
            "MSFT:XNAS".to_string(),
            Ok(dates
                .iter()
                .zip(xnas_closes)
                .map(|(&d, c)| stub_quote("SEC:MSFT:XNAS", d, c, "USD"))
                .collect()),
        );
        quotes.insert(
            "MSFT:XNYS".to_string(),
            Ok(dates
                .iter()
                .zip(xnys_closes)
                .map(|(&d, c)| stub_quote("SEC:MSFT:XNYS", d, c, "USD"))
                .collect()),
        );

        let mut search = HashMap::new();
        search.insert(
            "MSFT".to_string(),
            vec![
                stub_search_result("MSFT", "XNAS", "EQUITY", "USD"),
                stub_search_result("MSFT", "XNYS", "EQUITY", "USD"),
            ],
        );

        let stub = Arc::new(StubQuoteService {
            quotes_by_symbol: quotes,
            search_by_symbol: search,
            ..Default::default()
        });
        let qs: Arc<dyn QuoteServiceTrait> = stub.clone();

        let req = BacktestRequest {
            assets: vec![BacktestAssetInput {
                symbol: "MSFT".to_string(),
                weight: 100.0,
                exchange_mic: Some("XNAS".to_string()),
                instrument_type: Some("EQUITY".to_string()),
            }],
            start_date: dates[5],
            end_date: dates[6],
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: None,
            benchmark_exchange_mic: None,
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::SmaTrendFilter {
                sma_days: 5,
                check_frequency: SignalFrequency::Daily,
                defensive_symbol: Some("MSFT".to_string()),
                defensive_exchange_mic: Some("XNYS".to_string()),
                defensive_instrument_type: Some("EQUITY".to_string()),
            },
        };

        let res = run_backtest(qs, req).await.unwrap();
        let recorded = stub.recorded_quote_listings.lock().unwrap().clone();
        assert_eq!(
            recorded,
            vec![
                ("MSFT".to_string(), Some("XNAS".to_string())),
                ("MSFT".to_string(), Some("XNYS".to_string())),
            ]
        );
        // Defensive allocation invested in MSFT/XNYS (+30%), while Buy & Hold stayed in MSFT/XNAS (0%).
        assert!((res.strategy.metrics.final_value - 13_000.0).abs() < 1e-6);
        let bh = res.buy_and_hold.expect("buy & hold baseline");
        assert!((bh.metrics.final_value - 10_000.0).abs() < 1e-6);
        assert_eq!(res.strategy.ending_holdings.len(), 1);
        assert_eq!(
            res.strategy.ending_holdings[0].exchange_mic.as_deref(),
            Some("XNYS")
        );
        assert_eq!(res.symbol_coverage.len(), 2);
        assert_eq!(res.symbol_coverage[0].exchange_mic.as_deref(), Some("XNAS"));
        assert_eq!(res.symbol_coverage[1].exchange_mic.as_deref(), Some("XNYS"));
    }

    #[tokio::test]
    async fn accepts_segment_mic_with_operating_provider_result_and_rejects_segment_conflict() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();

        let make_quotes = || {
            let mut quotes = HashMap::new();
            quotes.insert(
                "AAPL:XNAS".to_string(),
                Ok(vec![
                    stub_quote("SEC:AAPL:XNAS", d1, "100", "USD"),
                    stub_quote("SEC:AAPL:XNAS", d2, "110", "USD"),
                ]),
            );
            quotes
        };

        let base_req = BacktestRequest {
            assets: vec![BacktestAssetInput {
                symbol: "AAPL".to_string(),
                weight: 100.0,
                exchange_mic: Some("XNGS".to_string()),
                instrument_type: Some("EQUITY".to_string()),
            }],
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

        // 1. Holding with segment MIC `XNGS` succeeds when provider search reports operating MIC `XNAS`.
        let mut ok_search = HashMap::new();
        ok_search.insert(
            "AAPL".to_string(),
            vec![stub_search_result("AAPL", "XNAS", "EQUITY", "USD")],
        );
        let qs_ok = Arc::new(StubQuoteService {
            quotes_by_symbol: make_quotes(),
            search_by_symbol: ok_search.clone(),
            ..Default::default()
        });
        let res = run_backtest(qs_ok, base_req.clone()).await.unwrap();
        assert!((res.strategy.metrics.final_value - 11_000.0).abs() < 1e-6);
        assert_eq!(res.symbol_coverage[0].exchange_mic.as_deref(), Some("XNGS"));

        // 2. Holding with `XNGS` also succeeds via profile fallback reporting "NASDAQ GLOBAL SELECT MARKET" (-> XNAS).
        let mut profiles = HashMap::new();
        profiles.insert(
            "AAPL".to_string(),
            ProviderProfile {
                symbol: "AAPL".to_string(),
                asset_type: Some("EQUITY".to_string()),
                listing_currency: Some("USD".to_string()),
                exchange: Some("NASDAQ GLOBAL SELECT MARKET".to_string()),
                data_source: "FINNHUB".to_string(),
                ..Default::default()
            },
        );
        let qs_profile = Arc::new(StubQuoteService {
            quotes_by_symbol: make_quotes(),
            profiles_by_symbol: profiles,
            ..Default::default()
        });
        let res_profile = run_backtest(qs_profile, base_req.clone()).await.unwrap();
        assert_eq!(
            res_profile.symbol_coverage[0].exchange_mic.as_deref(),
            Some("XNGS")
        );

        // 3. Explicit segment conflict (`XNGS` holding vs `XNCM` provider result) is rejected.
        let mut conflict_search = HashMap::new();
        conflict_search.insert(
            "AAPL".to_string(),
            vec![stub_search_result("AAPL", "XNCM", "EQUITY", "USD")],
        );
        let qs_conflict = Arc::new(StubQuoteService {
            quotes_by_symbol: make_quotes(),
            search_by_symbol: conflict_search,
            ..Default::default()
        });
        let err = run_backtest(qs_conflict, base_req).await.unwrap_err();
        assert!(err.to_string().contains("conflicts with verified listing"));
    }
}
