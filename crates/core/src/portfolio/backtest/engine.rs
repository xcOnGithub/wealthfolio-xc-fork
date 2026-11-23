//! Pure, deterministic daily backtest simulation engine.
//!
//! All strategies evaluate signals strictly using prices available on or before
//! trading day `t` and apply trades at day `t`'s close (affecting returns from
//! `t` to `t + 1`), avoiding lookahead bias.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::{Datelike, NaiveDate};
use rust_decimal::prelude::ToPrimitive;
use wealthfolio_market_data::SplitEvent;

use crate::errors::{Error, Result, ValidationError};

use super::model::{
    AnnualReturnPoint, BacktestEndingHolding, BacktestMetrics, BacktestPoint, BacktestRequest,
    BacktestResponse, BacktestSeriesDescriptor, BacktestSeriesResult, BacktestStrategy,
    BacktestSymbolCoverage, BacktestWarning, ContributionFrequency, DividendMode,
    MomentumWeighting, RebalanceFrequency, SignalFrequency,
};

const TRADING_DAYS_PER_YEAR: f64 = 252.0;
const CALENDAR_DAYS_PER_YEAR: f64 = 365.25;
const INVERSE_VOL_WINDOW: usize = 63;

/// Dedicated series-map key used when an optional benchmark has the same ticker
/// symbol as a required holding but a separately resolved venue.
pub const BENCHMARK_SERIES_KEY: &str = "__benchmark__";

/// Backward-adjust an unadjusted close series across stock splits so all closes
/// share the post-split basis (matching split-adjusted dividends).
///
/// For each split event with positive `ratio` (e.g. `2.0` for a 2-for-1 split),
/// every close strictly before `split.date` is divided by `ratio`.
pub fn apply_splits_backward(prices: &mut BTreeMap<NaiveDate, f64>, splits: &[SplitEvent]) {
    let all_dates: HashSet<NaiveDate> = prices.keys().copied().collect();
    apply_splits_backward_for_dates(prices, &all_dates, splits);
}

/// Backward-adjust only the specified unadjusted close dates across stock splits,
/// leaving already split-adjusted closes untouched in mixed-source series.
pub fn apply_splits_backward_for_dates(
    prices: &mut BTreeMap<NaiveDate, f64>,
    unadjusted_dates: &HashSet<NaiveDate>,
    splits: &[SplitEvent],
) {
    if unadjusted_dates.is_empty() {
        return;
    }
    for split in splits {
        let Some(ratio) = split.ratio.to_f64() else {
            continue;
        };
        if !ratio.is_finite() || ratio <= 0.0 || (ratio - 1.0).abs() < 1e-12 {
            continue;
        }
        for (&day, price) in prices.iter_mut() {
            if day < split.date && unadjusted_dates.contains(&day) {
                *price /= ratio;
            }
        }
    }
}

/// Market data series for a single symbol supplied to the simulation engine.
#[derive(Debug, Clone)]
pub struct SymbolMarketSeries {
    pub symbol: String,
    pub currency: String,
    pub prices: BTreeMap<NaiveDate, f64>,
    pub dividends: BTreeMap<NaiveDate, f64>,
    pub dividends_available: bool,
}

/// Pre-aligned dense matrix of prices and dividends on a shared trading calendar.
struct AlignedMarketData {
    dates: Vec<NaiveDate>,
    symbols: Vec<String>,
    symbol_index: HashMap<String, usize>,
    /// `prices[sym_idx][day_idx]`
    prices: Vec<Vec<f64>>,
    /// `dividends[sym_idx][day_idx]` (cash dividend per share paid on day_idx)
    dividends: Vec<Vec<f64>>,
    /// Total-return price index for ranking momentum when dividends are included.
    tr_prices: Vec<Vec<f64>>,
    sim_start_idx: usize,
    sim_end_idx: usize,
}

/// Run the full backtest simulation on pre-fetched market series.
pub fn simulate(
    request: &BacktestRequest,
    series_map: &HashMap<String, SymbolMarketSeries>,
    mut warnings: Vec<BacktestWarning>,
) -> Result<BacktestResponse> {
    let req = request.normalized_and_validated()?;

    // Collect required portfolio symbols (universe + optional defensive asset).
    let mut required_symbols: Vec<String> = req.assets.iter().map(|a| a.symbol.clone()).collect();
    if let Some(def_sym) = defensive_symbol_of(&req.strategy) {
        if !required_symbols.iter().any(|s| s == def_sym) {
            required_symbols.push(def_sym.to_string());
        }
    }

    let aligned = align_market_data(&req, &required_symbols, series_map, &mut warnings)?;

    let effective_start_date = aligned.dates[aligned.sim_start_idx];
    let effective_end_date = aligned.dates[aligned.sim_end_idx];
    let trading_days = aligned.sim_end_idx - aligned.sim_start_idx + 1;

    // Build normalized target weights for the universe assets.
    let base_weights = normalized_universe_weights(&req);

    // Run primary strategy.
    let strategy_descriptor = strategy_descriptor_of(&req.strategy);
    let strategy_result = run_single_simulation(
        &aligned,
        &req,
        &base_weights,
        &req.strategy,
        strategy_descriptor,
    )?;

    // Run Buy & Hold baseline unless already selected.
    let buy_and_hold = if matches!(req.strategy, BacktestStrategy::BuyAndHold) {
        None
    } else {
        Some(run_single_simulation(
            &aligned,
            &req,
            &base_weights,
            &BacktestStrategy::BuyAndHold,
            BacktestSeriesDescriptor::BuyAndHold,
        )?)
    };

    // Run optional benchmark over the exact same effective window.
    let benchmark = if let Some(bench_sym) = &req.benchmark_symbol {
        match series_map
            .get(BENCHMARK_SERIES_KEY)
            .or_else(|| series_map.get(bench_sym))
        {
            Some(bench_series) => run_benchmark_simulation(
                bench_series,
                &aligned.dates[aligned.sim_start_idx..=aligned.sim_end_idx],
                &req,
                &mut warnings,
            ),
            None => {
                // Only emit BenchmarkUnavailable if the service did not already record a specific omission warning.
                let already_warned = warnings.iter().any(|w| match w {
                    BacktestWarning::BenchmarkUnavailable { symbol }
                    | BacktestWarning::BenchmarkNotUsListing { symbol }
                    | BacktestWarning::BenchmarkNonUsd { symbol, .. }
                    | BacktestWarning::BenchmarkSplitAdjustmentUnavailable { symbol, .. } => {
                        symbol == bench_sym
                    }
                    _ => false,
                });
                if !already_warned {
                    warnings.push(BacktestWarning::BenchmarkUnavailable {
                        symbol: bench_sym.clone(),
                    });
                }
                None
            }
        }
    } else {
        None
    };

    // Build coverage metadata for all fetched symbols.
    let mut coverage_symbols = required_symbols;
    if let Some(bench) = &req.benchmark_symbol {
        if !coverage_symbols.iter().any(|s| s == bench) {
            coverage_symbols.push(bench.clone());
        }
    }

    let mut symbol_coverage = Vec::with_capacity(coverage_symbols.len());
    for sym in coverage_symbols {
        let series_opt = if req.benchmark_symbol.as_deref() == Some(sym.as_str()) {
            series_map
                .get(&sym)
                .or_else(|| series_map.get(BENCHMARK_SERIES_KEY))
        } else {
            series_map.get(&sym)
        };
        if let Some(s) = series_opt {
            if let (Some((&first_date, _)), Some((&last_date, _))) =
                (s.prices.first_key_value(), s.prices.last_key_value())
            {
                let div_count = s
                    .dividends
                    .range(effective_start_date..=effective_end_date)
                    .count();
                symbol_coverage.push(BacktestSymbolCoverage {
                    symbol: s.symbol.clone(),
                    currency: s.currency.clone(),
                    first_date,
                    last_date,
                    quote_count: s.prices.len(),
                    dividend_events_count: div_count,
                    dividends_available: s.dividends_available,
                });
            }
        }
    }

    Ok(BacktestResponse {
        currency: "USD".to_string(),
        requested_start_date: req.start_date,
        requested_end_date: req.end_date,
        effective_start_date,
        effective_end_date,
        trading_days,
        dividend_mode: req.dividend_mode,
        strategy: strategy_result,
        buy_and_hold,
        benchmark,
        symbol_coverage,
        warnings,
    })
}

fn defensive_symbol_of(strategy: &BacktestStrategy) -> Option<&str> {
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

fn strategy_descriptor_of(strategy: &BacktestStrategy) -> BacktestSeriesDescriptor {
    match strategy {
        BacktestStrategy::BuyAndHold => BacktestSeriesDescriptor::BuyAndHold,
        BacktestStrategy::PeriodicRebalance { frequency } => {
            BacktestSeriesDescriptor::PeriodicRebalance {
                frequency: *frequency,
            }
        }
        BacktestStrategy::DriftBandRebalance { threshold_pct } => {
            BacktestSeriesDescriptor::DriftBandRebalance {
                threshold_pct: *threshold_pct,
            }
        }
        BacktestStrategy::SmaTrendFilter { sma_days, .. } => {
            BacktestSeriesDescriptor::SmaTrendFilter {
                sma_days: *sma_days,
            }
        }
        BacktestStrategy::MomentumRotation {
            lookback_days,
            top_n,
            ..
        } => BacktestSeriesDescriptor::MomentumRotation {
            lookback_days: *lookback_days,
            top_n: *top_n,
        },
    }
}

fn normalized_universe_weights(req: &BacktestRequest) -> Vec<f64> {
    let sum: f64 = req.assets.iter().map(|a| a.weight).sum();
    if (99.9..=100.1).contains(&sum) && sum > 0.0 {
        req.assets.iter().map(|a| a.weight / sum).collect()
    } else {
        let eq = 1.0 / (req.assets.len() as f64);
        vec![eq; req.assets.len()]
    }
}

fn align_market_data(
    req: &BacktestRequest,
    required_symbols: &[String],
    series_map: &HashMap<String, SymbolMarketSeries>,
    warnings: &mut Vec<BacktestWarning>,
) -> Result<AlignedMarketData> {
    let mut common_first: Option<NaiveDate> = None;
    let mut common_last: Option<NaiveDate> = None;
    let mut union_dates = BTreeSet::new();

    for sym in required_symbols {
        let series = series_map.get(sym).ok_or_else(|| {
            Error::Validation(ValidationError::InvalidInput(format!(
                "No historical price data available for '{sym}'."
            )))
        })?;
        let (Some((&first, _)), Some((&last, _))) = (
            series.prices.first_key_value(),
            series.prices.last_key_value(),
        ) else {
            return Err(Error::Validation(ValidationError::InvalidInput(format!(
                "No historical price data available for '{sym}'."
            ))));
        };

        common_first = Some(common_first.map_or(first, |curr| curr.max(first)));
        common_last = Some(common_last.map_or(last, |curr| curr.min(last)));
        for &d in series.prices.keys() {
            union_dates.insert(d);
        }
    }

    let (Some(common_first), Some(common_last)) = (common_first, common_last) else {
        return Err(Error::Validation(ValidationError::InvalidInput(
            "No overlapping price history found.".to_string(),
        )));
    };

    if common_last <= common_first {
        return Err(Error::Validation(ValidationError::InvalidInput(
            "Selected tickers do not have an overlapping historical date range.".to_string(),
        )));
    }

    let dates: Vec<NaiveDate> = union_dates
        .into_iter()
        .filter(|d| *d >= common_first && *d <= common_last && *d <= req.end_date)
        .collect();

    if dates.len() < 2 {
        return Err(Error::Validation(ValidationError::InvalidInput(
            "Not enough overlapping trading days in the selected range.".to_string(),
        )));
    }

    let warmup = req.warmup_trading_days();
    let requested_start_idx = dates
        .iter()
        .position(|&d| d >= req.start_date)
        .ok_or_else(|| {
            Error::Validation(ValidationError::InvalidInput(
                "No historical price data available on or after the requested start date."
                    .to_string(),
            ))
        })?;

    let sim_start_idx = requested_start_idx.max(warmup);
    let sim_end_idx = dates.len() - 1;

    if sim_end_idx <= sim_start_idx {
        return Err(Error::Validation(ValidationError::InvalidInput(format!(
            "Not enough historical data to satisfy the {warmup}-day indicator warmup and backtest window."
        ))));
    }

    let effective_start = dates[sim_start_idx];
    let effective_end = dates[sim_end_idx];

    if (effective_start - req.start_date).num_days() > 7 {
        warnings.push(BacktestWarning::EffectiveStartAdjusted {
            requested_date: req.start_date,
            effective_date: effective_start,
        });
    }
    if (req.end_date - effective_end).num_days() > 7 {
        warnings.push(BacktestWarning::EffectiveEndAdjusted {
            requested_date: req.end_date,
            effective_date: effective_end,
        });
    }

    let num_syms = required_symbols.len();
    let num_days = dates.len();
    let mut symbol_index = HashMap::with_capacity(num_syms);
    let mut prices = Vec::with_capacity(num_syms);
    let mut dividends = Vec::with_capacity(num_syms);
    let mut tr_prices = Vec::with_capacity(num_syms);

    for (sym_idx, sym) in required_symbols.iter().enumerate() {
        symbol_index.insert(sym.clone(), sym_idx);
        let series = &series_map[sym];

        // Forward-fill at most a single isolated missing session; reject consecutive
        // missing sessions so stale prices never drive signals or simulated trades.
        let mut sym_prices = Vec::with_capacity(num_days);
        let mut last_valid = series
            .prices
            .range(..=dates[0])
            .next_back()
            .map(|(_, &p)| p)
            .unwrap_or(0.0);
        let mut consecutive_missing = 0_usize;
        let mut isolated_fills = 0_usize;

        for (day_idx, &d) in dates.iter().enumerate() {
            if let Some(&p) = series.prices.get(&d) {
                if p.is_finite() && p > 0.0 {
                    last_valid = p;
                    consecutive_missing = 0;
                    sym_prices.push(last_valid);
                    continue;
                }
            }

            consecutive_missing += 1;
            if consecutive_missing > 1 {
                return Err(Error::Validation(ValidationError::InvalidInput(format!(
                    "Ticker '{sym}' is missing consecutive trading sessions around {d}, exceeding the single-session forward-fill limit."
                ))));
            }
            if last_valid <= 0.0 {
                return Err(Error::Validation(ValidationError::InvalidInput(format!(
                    "Invalid non-positive price encountered for '{sym}' on {d}."
                ))));
            }
            if day_idx >= sim_start_idx {
                isolated_fills += 1;
            }
            sym_prices.push(last_valid);
        }

        if isolated_fills > 0 {
            warnings.push(BacktestWarning::IsolatedGapsForwardFilled {
                symbol: sym.clone(),
                count: isolated_fills,
            });
        }

        // Map dividend events onto the first trading day >= ex-date.
        let mut sym_divs = vec![0.0_f64; num_days];
        if req.dividend_mode != DividendMode::Exclude {
            for (&div_date, &amount) in &series.dividends {
                if amount <= 0.0 || !amount.is_finite() || div_date > effective_end {
                    continue;
                }
                match dates.binary_search(&div_date) {
                    Ok(pos) => sym_divs[pos] += amount,
                    Err(pos) if pos < num_days && pos > 0 => sym_divs[pos] += amount,
                    _ => {}
                }
            }
        }

        // Build total-return price index (reinvesting dividends) for momentum ranking.
        let mut sym_tr = Vec::with_capacity(num_days);
        let mut virtual_shares = 1.0_f64;
        for day_idx in 0..num_days {
            let p = sym_prices[day_idx];
            if day_idx > 0 && sym_divs[day_idx] > 0.0 {
                virtual_shares += (virtual_shares * sym_divs[day_idx]) / p;
            }
            sym_tr.push(virtual_shares * p);
        }

        prices.push(sym_prices);
        dividends.push(sym_divs);
        tr_prices.push(sym_tr);
    }

    Ok(AlignedMarketData {
        dates,
        symbols: required_symbols.to_vec(),
        symbol_index,
        prices,
        dividends,
        tr_prices,
        sim_start_idx,
        sim_end_idx,
    })
}

fn run_single_simulation(
    data: &AlignedMarketData,
    req: &BacktestRequest,
    base_weights: &[f64],
    strategy: &BacktestStrategy,
    descriptor: BacktestSeriesDescriptor,
) -> Result<BacktestSeriesResult> {
    let num_syms = data.symbols.len();
    let universe_len = req.assets.len();
    let fee_rate = req.fee_bps / 10_000.0;

    let mut shares = vec![0.0_f64; num_syms];
    let mut cash = req.initial_capital;
    let mut total_invested = req.initial_capital;
    let mut total_dividends = 0.0_f64;
    let mut total_fees = 0.0_f64;
    let mut rebalance_count = 0_usize;
    let mut cumulative_turnover = 0.0_f64;

    let start_date = data.dates[data.sim_start_idx];
    let sim_len = data.sim_end_idx - data.sim_start_idx + 1;
    let mut points = Vec::with_capacity(sim_len);
    let mut daily_twr_returns = Vec::with_capacity(sim_len.saturating_sub(1));

    let mut twr_index = 1.0_f64;
    let mut prev_equity_after_trade = req.initial_capital;
    let mut running_peak_twr = 1.0_f64;

    let mut current_target_weights = vec![0.0_f64; num_syms];

    for day_idx in data.sim_start_idx..=data.sim_end_idx {
        let date = data.dates[day_idx];
        let is_first_day = day_idx == data.sim_start_idx;

        // 1. Collect dividends on shares held entering today (day > 0).
        if !is_first_day && req.dividend_mode != DividendMode::Exclude {
            for (sym_idx, share_qty) in shares.iter_mut().enumerate() {
                let div = data.dividends[sym_idx][day_idx];
                if div > 0.0 && *share_qty > 0.0 {
                    let div_cash = *share_qty * div;
                    total_dividends += div_cash;
                    match req.dividend_mode {
                        DividendMode::Reinvest => {
                            let price = data.prices[sym_idx][day_idx];
                            *share_qty += div_cash / price;
                        }
                        DividendMode::Cash => {
                            cash += div_cash;
                        }
                        DividendMode::Exclude => {}
                    }
                }
            }
        }

        // 2. Mark-to-market before external contribution and before today's trades.
        let equity_before_flow = portfolio_value(cash, &shares, &data.prices, day_idx);

        // Update daily TWR before adding external DCA cash.
        if !is_first_day && prev_equity_after_trade > 0.0 {
            let day_ret = (equity_before_flow / prev_equity_after_trade) - 1.0;
            daily_twr_returns.push(day_ret);
            twr_index *= 1.0 + day_ret;
        }

        // 3. Apply scheduled recurring DCA contribution.
        let mut contributed_today = 0.0_f64;
        if !is_first_day {
            if let Some(contrib) = &req.contribution {
                let prev_date = data.dates[day_idx - 1];
                if is_contribution_day(start_date, prev_date, date, contrib.frequency) {
                    cash += contrib.amount;
                    total_invested += contrib.amount;
                    contributed_today = contrib.amount;
                }
            }
        }

        // 4. Evaluate strategy target weights and decide whether to rebalance.
        let (should_rebalance, new_targets) = evaluate_strategy(
            strategy,
            data,
            day_idx,
            universe_len,
            base_weights,
            &current_target_weights,
            cash,
            &shares,
        );

        if should_rebalance {
            current_target_weights = new_targets;
            let equity_before_trade = portfolio_value(cash, &shares, &data.prices, day_idx);

            let mut gross_traded = 0.0_f64;
            for sym_idx in 0..num_syms {
                let current_val = shares[sym_idx] * data.prices[sym_idx][day_idx];
                let target_val = current_target_weights[sym_idx] * equity_before_trade;
                gross_traded += (target_val - current_val).abs();
            }

            let fee = gross_traded * fee_rate;
            total_fees += fee;
            let equity_after_fee = (equity_before_trade - fee).max(0.0);

            let mut weight_total = 0.0_f64;
            for sym_idx in 0..num_syms {
                let w = current_target_weights[sym_idx];
                weight_total += w;
                shares[sym_idx] = (w * equity_after_fee) / data.prices[sym_idx][day_idx];
            }
            cash = ((1.0 - weight_total).max(0.0)) * equity_after_fee;

            if !is_first_day && gross_traded > 1e-6 && equity_before_trade > 0.0 {
                rebalance_count += 1;
                cumulative_turnover += (gross_traded * 0.5) / equity_before_trade;
            }

            // Deduct transaction fee drag from TWR on every trade day (including day 0).
            if equity_before_trade > 0.0 && fee > 0.0 {
                let fee_factor = equity_after_fee / equity_before_trade;
                if let Some(last_ret) = daily_twr_returns.last_mut() {
                    *last_ret = (1.0 + *last_ret) * fee_factor - 1.0;
                } else {
                    daily_twr_returns.push(fee_factor - 1.0);
                }
                twr_index *= fee_factor;
            }
        } else if contributed_today > 0.0 {
            // Deploy fresh DCA cash into currently active target weights without selling existing holdings.
            let deploy_total: f64 = current_target_weights.iter().sum();
            if deploy_total > 0.0 {
                let equity_before_trade = portfolio_value(cash, &shares, &data.prices, day_idx);
                let deploy_cash = contributed_today * deploy_total.min(1.0);
                let fee = deploy_cash * fee_rate;
                total_fees += fee;
                let net_deploy = (deploy_cash - fee).max(0.0);
                cash -= deploy_cash;
                for sym_idx in 0..num_syms {
                    let w = current_target_weights[sym_idx];
                    if w > 0.0 {
                        let alloc = net_deploy * (w / deploy_total);
                        shares[sym_idx] += alloc / data.prices[sym_idx][day_idx];
                    }
                }

                // Fold DCA trade fee drag into TWR while excluding the external cash inflow itself.
                if equity_before_trade > 0.0 && fee > 0.0 {
                    let fee_factor = (equity_before_trade - fee).max(0.0) / equity_before_trade;
                    if let Some(last_ret) = daily_twr_returns.last_mut() {
                        *last_ret = (1.0 + *last_ret) * fee_factor - 1.0;
                    }
                    twr_index *= fee_factor;
                }
            }
        }

        if twr_index > running_peak_twr {
            running_peak_twr = twr_index;
        }

        let equity_after_trade = portfolio_value(cash, &shares, &data.prices, day_idx);
        prev_equity_after_trade = equity_after_trade;

        let drawdown = if running_peak_twr > 0.0 {
            (twr_index / running_peak_twr) - 1.0
        } else {
            0.0
        };

        points.push(BacktestPoint {
            date,
            value: equity_after_trade,
            invested: total_invested,
            cumulative_return: twr_index - 1.0,
            drawdown,
        });
    }

    // Build ending holdings breakdown.
    let final_day_idx = data.sim_end_idx;
    let final_value = points.last().map_or(0.0, |p| p.value);
    let mut ending_holdings = Vec::new();
    for (sym_idx, &share_qty) in shares.iter().enumerate() {
        let val = share_qty * data.prices[sym_idx][final_day_idx];
        if val > 1e-6 {
            ending_holdings.push(BacktestEndingHolding {
                symbol: data.symbols[sym_idx].clone(),
                is_cash: false,
                weight: if final_value > 0.0 {
                    val / final_value
                } else {
                    0.0
                },
                value: val,
            });
        }
    }
    if cash > 1e-4 {
        ending_holdings.push(BacktestEndingHolding {
            symbol: "USD".to_string(),
            is_cash: true,
            weight: if final_value > 0.0 {
                cash / final_value
            } else {
                0.0
            },
            value: cash,
        });
    }
    ending_holdings.sort_by(|a, b| b.value.total_cmp(&a.value));

    let annual_returns = compute_annual_returns(&points);
    let metrics = compute_metrics(
        &points,
        &daily_twr_returns,
        &annual_returns,
        total_dividends,
        total_fees,
        rebalance_count,
        cumulative_turnover,
    );

    Ok(BacktestSeriesResult {
        descriptor,
        metrics,
        series: points,
        annual_returns,
        ending_holdings,
    })
}

#[allow(clippy::too_many_arguments)]
fn evaluate_strategy(
    strategy: &BacktestStrategy,
    data: &AlignedMarketData,
    day_idx: usize,
    universe_len: usize,
    base_weights: &[f64],
    prev_targets: &[f64],
    cash: f64,
    shares: &[f64],
) -> (bool, Vec<f64>) {
    let num_syms = data.symbols.len();
    let is_first_day = day_idx == data.sim_start_idx;

    let mut base_full = vec![0.0_f64; num_syms];
    base_full[..universe_len].copy_from_slice(base_weights);

    match strategy {
        BacktestStrategy::BuyAndHold => (is_first_day, base_full),
        BacktestStrategy::PeriodicRebalance { frequency } => {
            if is_first_day {
                return (true, base_full);
            }
            let prev_date = data.dates[day_idx - 1];
            let curr_date = data.dates[day_idx];
            (
                is_period_boundary(prev_date, curr_date, *frequency),
                base_full,
            )
        }
        BacktestStrategy::DriftBandRebalance { threshold_pct } => {
            if is_first_day {
                return (true, base_full);
            }
            let total = portfolio_value(cash, shares, &data.prices, day_idx);
            if total <= 0.0 {
                return (false, base_full);
            }
            let threshold = threshold_pct / 100.0;
            let mut max_drift = (cash / total).abs();
            for sym_idx in 0..num_syms {
                let current_w = (shares[sym_idx] * data.prices[sym_idx][day_idx]) / total;
                let drift = (current_w - base_full[sym_idx]).abs();
                if drift > max_drift {
                    max_drift = drift;
                }
            }
            (max_drift >= threshold, base_full)
        }
        BacktestStrategy::SmaTrendFilter {
            sma_days,
            check_frequency,
            defensive_symbol,
            ..
        } => {
            let should_check = is_first_day
                || match check_frequency {
                    SignalFrequency::Daily => true,
                    SignalFrequency::Monthly => is_period_boundary(
                        data.dates[day_idx - 1],
                        data.dates[day_idx],
                        RebalanceFrequency::Monthly,
                    ),
                };

            if !should_check {
                return (false, prev_targets.to_vec());
            }

            let defensive_idx = defensive_symbol
                .as_ref()
                .and_then(|s| data.symbol_index.get(s).copied());

            let mut targets = vec![0.0_f64; num_syms];
            for sym_idx in 0..universe_len {
                let w = base_weights[sym_idx];
                if w <= 0.0 {
                    continue;
                }
                let price = data.prices[sym_idx][day_idx];
                let sma = simple_moving_average(&data.prices[sym_idx], day_idx, *sma_days);
                if price > sma {
                    targets[sym_idx] += w;
                } else if let Some(def_idx) = defensive_idx {
                    targets[def_idx] += w;
                }
                // Otherwise slice stays in USD cash.
            }

            let should_trade = is_first_day
                || match check_frequency {
                    SignalFrequency::Monthly => true,
                    SignalFrequency::Daily => targets
                        .iter()
                        .zip(prev_targets.iter())
                        .any(|(a, b)| (a - b).abs() > 1e-6),
                };

            (should_trade, targets)
        }
        BacktestStrategy::MomentumRotation {
            lookback_days,
            top_n,
            rebalance_frequency,
            weighting,
            absolute_momentum,
            defensive_symbol,
            ..
        } => {
            let should_check = is_first_day
                || is_period_boundary(
                    data.dates[day_idx - 1],
                    data.dates[day_idx],
                    *rebalance_frequency,
                );
            if !should_check {
                return (false, prev_targets.to_vec());
            }

            let mut scores: Vec<(usize, f64)> = (0..universe_len)
                .map(|sym_idx| {
                    let tr_now = data.tr_prices[sym_idx][day_idx];
                    let tr_past = data.tr_prices[sym_idx][day_idx - lookback_days];
                    let ret = if tr_past > 0.0 {
                        (tr_now / tr_past) - 1.0
                    } else {
                        f64::NEG_INFINITY
                    };
                    (sym_idx, ret)
                })
                .collect();

            // Deterministic descending sort by momentum score.
            scores.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

            let selected: Vec<(usize, f64)> = scores.into_iter().take(*top_n).collect();
            let raw_weights: Vec<f64> = match weighting {
                MomentumWeighting::Equal => {
                    let eq = 1.0 / (selected.len() as f64);
                    vec![eq; selected.len()]
                }
                MomentumWeighting::InverseVolatility => {
                    let inv_vols: Vec<f64> = selected
                        .iter()
                        .map(|(sym_idx, _)| {
                            let vol = trailing_volatility(
                                &data.tr_prices[*sym_idx],
                                day_idx,
                                INVERSE_VOL_WINDOW,
                            );
                            1.0 / vol.max(1e-6)
                        })
                        .collect();
                    let sum: f64 = inv_vols.iter().sum();
                    if sum > 0.0 {
                        inv_vols.into_iter().map(|v| v / sum).collect()
                    } else {
                        vec![1.0 / (selected.len() as f64); selected.len()]
                    }
                }
            };

            let defensive_idx = defensive_symbol
                .as_ref()
                .and_then(|s| data.symbol_index.get(s).copied());

            let mut targets = vec![0.0_f64; num_syms];
            for ((sym_idx, ret), w) in selected.into_iter().zip(raw_weights) {
                if *absolute_momentum && ret <= 0.0 {
                    if let Some(def_idx) = defensive_idx {
                        targets[def_idx] += w;
                    }
                    // Otherwise keep slice in USD cash.
                } else {
                    targets[sym_idx] += w;
                }
            }

            (true, targets)
        }
    }
}

fn run_benchmark_simulation(
    bench: &SymbolMarketSeries,
    calendar: &[NaiveDate],
    req: &BacktestRequest,
    warnings: &mut Vec<BacktestWarning>,
) -> Option<BacktestSeriesResult> {
    if calendar.len() < 2 || bench.prices.is_empty() {
        return None;
    }

    let (&first_bench, _) = bench.prices.first_key_value()?;
    let (&last_bench, _) = bench.prices.last_key_value()?;
    let start_date = calendar[0];
    let end_date = *calendar.last()?;

    if first_bench > start_date {
        warnings.push(BacktestWarning::BenchmarkStartsLate {
            symbol: bench.symbol.clone(),
            benchmark_start_date: first_bench,
            effective_start_date: start_date,
        });
        return None;
    }
    if last_bench < end_date {
        warnings.push(BacktestWarning::BenchmarkEndsEarly {
            symbol: bench.symbol.clone(),
            benchmark_end_date: last_bench,
            effective_end_date: end_date,
        });
        return None;
    }

    let mut sym_prices = Vec::with_capacity(calendar.len());
    let mut last_valid = bench
        .prices
        .range(..=start_date)
        .next_back()
        .map(|(_, &p)| p)
        .unwrap_or(0.0);
    let mut consecutive_missing = 0_usize;

    for &d in calendar {
        if let Some(&p) = bench.prices.get(&d) {
            if p.is_finite() && p > 0.0 {
                last_valid = p;
                consecutive_missing = 0;
                sym_prices.push(last_valid);
                continue;
            }
        }

        consecutive_missing += 1;
        if consecutive_missing > 1 {
            warnings.push(BacktestWarning::BenchmarkDataGap {
                symbol: bench.symbol.clone(),
            });
            return None;
        }
        if last_valid <= 0.0 {
            return None;
        }
        sym_prices.push(last_valid);
    }

    let mut sym_divs = vec![0.0_f64; calendar.len()];
    if req.dividend_mode != DividendMode::Exclude {
        for (&div_date, &amount) in &bench.dividends {
            if amount <= 0.0 || !amount.is_finite() || div_date > end_date {
                continue;
            }
            match calendar.binary_search(&div_date) {
                Ok(pos) => sym_divs[pos] += amount,
                Err(pos) if pos < calendar.len() && pos > 0 => sym_divs[pos] += amount,
                _ => {}
            }
        }
    }

    let mut symbol_index = HashMap::new();
    symbol_index.insert(bench.symbol.clone(), 0);

    let aligned = AlignedMarketData {
        dates: calendar.to_vec(),
        symbols: vec![bench.symbol.clone()],
        symbol_index,
        prices: vec![sym_prices.clone()],
        dividends: vec![sym_divs],
        tr_prices: vec![sym_prices],
        sim_start_idx: 0,
        sim_end_idx: calendar.len() - 1,
    };

    let bench_req = BacktestRequest {
        assets: vec![super::model::BacktestAssetInput {
            symbol: bench.symbol.clone(),
            weight: 100.0,
            exchange_mic: None,
            instrument_type: None,
        }],
        start_date,
        end_date,
        initial_capital: req.initial_capital,
        contribution: req.contribution.clone(),
        dividend_mode: req.dividend_mode,
        fee_bps: req.fee_bps,
        benchmark_symbol: None,
        benchmark_exchange_mic: None,
        benchmark_instrument_type: None,
        strategy: BacktestStrategy::BuyAndHold,
    };

    run_single_simulation(
        &aligned,
        &bench_req,
        &[1.0],
        &BacktestStrategy::BuyAndHold,
        BacktestSeriesDescriptor::Benchmark {
            symbol: bench.symbol.clone(),
        },
    )
    .ok()
}

fn portfolio_value(cash: f64, shares: &[f64], prices: &[Vec<f64>], day_idx: usize) -> f64 {
    let holdings: f64 = shares
        .iter()
        .enumerate()
        .map(|(i, &q)| q * prices[i][day_idx])
        .sum();
    cash + holdings
}

fn simple_moving_average(prices: &[f64], end_idx: usize, window: usize) -> f64 {
    let start_idx = end_idx + 1 - window;
    let sum: f64 = prices[start_idx..=end_idx].iter().sum();
    sum / (window as f64)
}

fn trailing_volatility(prices: &[f64], end_idx: usize, window: usize) -> f64 {
    let start_idx = end_idx.saturating_sub(window);
    if end_idx <= start_idx + 1 {
        return 0.0;
    }
    let mut returns = Vec::with_capacity(end_idx - start_idx);
    for i in (start_idx + 1)..=end_idx {
        let prev = prices[i - 1];
        if prev > 0.0 {
            returns.push((prices[i] / prev) - 1.0);
        }
    }
    sample_std_dev(&returns)
}

fn is_period_boundary(prev: NaiveDate, curr: NaiveDate, frequency: RebalanceFrequency) -> bool {
    match frequency {
        RebalanceFrequency::Monthly => curr.year() != prev.year() || curr.month() != prev.month(),
        RebalanceFrequency::Quarterly => {
            curr.year() != prev.year() || ((curr.month() - 1) / 3) != ((prev.month() - 1) / 3)
        }
        RebalanceFrequency::Semiannually => {
            curr.year() != prev.year() || ((curr.month() - 1) / 6) != ((prev.month() - 1) / 6)
        }
        RebalanceFrequency::Annually => curr.year() != prev.year(),
    }
}

fn is_contribution_day(
    start_date: NaiveDate,
    prev: NaiveDate,
    curr: NaiveDate,
    frequency: ContributionFrequency,
) -> bool {
    match frequency {
        ContributionFrequency::Biweekly => {
            let prev_bucket = (prev - start_date).num_days() / 14;
            let curr_bucket = (curr - start_date).num_days() / 14;
            curr_bucket > prev_bucket
        }
        ContributionFrequency::Monthly => {
            is_period_boundary(prev, curr, RebalanceFrequency::Monthly)
        }
        ContributionFrequency::Quarterly => {
            is_period_boundary(prev, curr, RebalanceFrequency::Quarterly)
        }
        ContributionFrequency::Annually => {
            is_period_boundary(prev, curr, RebalanceFrequency::Annually)
        }
    }
}

fn compute_annual_returns(points: &[BacktestPoint]) -> Vec<AnnualReturnPoint> {
    if points.is_empty() {
        return Vec::new();
    }

    let mut result = Vec::new();
    let mut current_year = points[0].date.year();
    let mut year_start_index = 1.0;
    let mut last_index = 1.0 + points[0].cumulative_return;

    for point in points.iter().skip(1) {
        let yr = point.date.year();
        let idx_val = 1.0 + point.cumulative_return;
        if yr != current_year {
            if year_start_index > 0.0 {
                result.push(AnnualReturnPoint {
                    year: current_year,
                    return_pct: (last_index / year_start_index) - 1.0,
                });
            }
            current_year = yr;
            year_start_index = last_index;
        }
        last_index = idx_val;
    }

    if year_start_index > 0.0 {
        result.push(AnnualReturnPoint {
            year: current_year,
            return_pct: (last_index / year_start_index) - 1.0,
        });
    }

    result
}

fn compute_metrics(
    points: &[BacktestPoint],
    daily_returns: &[f64],
    annual_returns: &[AnnualReturnPoint],
    total_dividends: f64,
    total_fees: f64,
    rebalance_count: usize,
    cumulative_turnover: f64,
) -> BacktestMetrics {
    let first = points.first();
    let last = points.last();

    let final_value = last.map_or(0.0, |p| p.value);
    let total_invested = last.map_or(0.0, |p| p.invested);
    let total_profit = final_value - total_invested;
    let total_return = if total_invested > 0.0 {
        total_profit / total_invested
    } else {
        0.0
    };
    let twr = last.map_or(0.0, |p| p.cumulative_return);

    let calendar_days = match (first, last) {
        (Some(f), Some(l)) => (l.date - f.date).num_days(),
        _ => 0,
    };
    let years = (calendar_days as f64) / CALENDAR_DAYS_PER_YEAR;

    let cagr = if years > 0.0 && (1.0 + twr) > 0.0 {
        Some((1.0 + twr).powf(1.0 / years) - 1.0)
    } else {
        None
    };

    let (annualized_volatility, sharpe_ratio, sortino_ratio) = if daily_returns.len() >= 2 {
        let mean_daily = daily_returns.iter().sum::<f64>() / (daily_returns.len() as f64);
        let std_daily = sample_std_dev(daily_returns);
        let ann_vol = std_daily * TRADING_DAYS_PER_YEAR.sqrt();
        let ann_mean = mean_daily * TRADING_DAYS_PER_YEAR;

        let sharpe = if ann_vol > 1e-12 {
            Some(ann_mean / ann_vol)
        } else {
            None
        };

        let downside_sq_sum: f64 = daily_returns
            .iter()
            .map(|&r| if r < 0.0 { r * r } else { 0.0 })
            .sum();
        let downside_dev =
            (downside_sq_sum / (daily_returns.len() as f64)).sqrt() * TRADING_DAYS_PER_YEAR.sqrt();
        let sortino = if downside_dev > 1e-12 {
            Some(ann_mean / downside_dev)
        } else {
            None
        };

        (Some(ann_vol), sharpe, sortino)
    } else {
        (None, None, None)
    };

    // Find max drawdown peak, trough, and recovery dates on TWR index.
    let mut peak_twr = 1.0_f64;
    let mut peak_date: Option<NaiveDate> = first.map(|p| p.date);
    let mut max_dd = 0.0_f64;
    let mut max_dd_peak: Option<NaiveDate> = None;
    let mut max_dd_trough: Option<NaiveDate> = None;
    let mut max_dd_peak_val = 1.0_f64;
    let mut max_dd_trough_idx = 0_usize;

    for (idx, point) in points.iter().enumerate() {
        let val = 1.0 + point.cumulative_return;
        if val > peak_twr {
            peak_twr = val;
            peak_date = Some(point.date);
        }
        if peak_twr > 0.0 {
            let dd = (val / peak_twr) - 1.0;
            if dd < max_dd {
                max_dd = dd;
                max_dd_peak = peak_date;
                max_dd_trough = Some(point.date);
                max_dd_peak_val = peak_twr;
                max_dd_trough_idx = idx;
            }
        }
    }

    let max_dd_recovery = if max_dd < 0.0 {
        points
            .iter()
            .skip(max_dd_trough_idx + 1)
            .find(|p| (1.0 + p.cumulative_return) >= max_dd_peak_val - 1e-12)
            .map(|p| p.date)
    } else {
        None
    };

    let best_year = annual_returns
        .iter()
        .max_by(|a, b| a.return_pct.total_cmp(&b.return_pct))
        .cloned();
    let worst_year = annual_returns
        .iter()
        .min_by(|a, b| a.return_pct.total_cmp(&b.return_pct))
        .cloned();

    let annualized_turnover = if years > 0.0 {
        cumulative_turnover / years
    } else {
        cumulative_turnover
    };

    BacktestMetrics {
        final_value,
        total_invested,
        total_profit,
        total_return,
        twr,
        cagr,
        annualized_volatility,
        sharpe_ratio,
        sortino_ratio,
        max_drawdown: max_dd,
        max_drawdown_peak_date: max_dd_peak,
        max_drawdown_trough_date: max_dd_trough,
        max_drawdown_recovery_date: max_dd_recovery,
        best_year,
        worst_year,
        total_dividends,
        total_fees,
        rebalance_count,
        annualized_turnover,
    }
}

fn sample_std_dev(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
    var.max(0.0).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portfolio::backtest::model::{BacktestAssetInput, RecurringContribution};
    use chrono::Duration;
    use rust_decimal_macros::dec;

    fn trading_days_from(start: NaiveDate, count: usize) -> Vec<NaiveDate> {
        let mut out = Vec::with_capacity(count);
        let mut d = start;
        while out.len() < count {
            let wd = d.weekday();
            if wd != chrono::Weekday::Sat && wd != chrono::Weekday::Sun {
                out.push(d);
            }
            d += Duration::days(1);
        }
        out
    }

    fn make_series(
        symbol: &str,
        dates: &[NaiveDate],
        prices: &[f64],
        dividends: Vec<(NaiveDate, f64)>,
    ) -> SymbolMarketSeries {
        let price_map: BTreeMap<NaiveDate, f64> =
            dates.iter().copied().zip(prices.iter().copied()).collect();
        let div_map: BTreeMap<NaiveDate, f64> = dividends.into_iter().collect();
        SymbolMarketSeries {
            symbol: symbol.to_string(),
            currency: "USD".to_string(),
            prices: price_map,
            dividends: div_map,
            dividends_available: true,
        }
    }

    fn asset(symbol: &str, weight: f64) -> BacktestAssetInput {
        BacktestAssetInput {
            symbol: symbol.to_string(),
            weight,
            exchange_mic: None,
            instrument_type: None,
        }
    }

    #[test]
    fn buy_and_hold_computes_exact_return_and_drawdown() {
        let dates = trading_days_from(NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(), 5);
        // 100 -> 120 -> 90 (25% drawdown from 120) -> 120 (recovery) -> 150 (+50%)
        let prices = vec![100.0, 120.0, 90.0, 120.0, 150.0];
        let mut map = HashMap::new();
        map.insert(
            "SPY".to_string(),
            make_series("SPY", &dates, &prices, vec![]),
        );

        let req = BacktestRequest {
            assets: vec![asset("SPY", 100.0)],
            start_date: dates[0],
            end_date: dates[4],
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: None,
            benchmark_exchange_mic: None,
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::BuyAndHold,
        };

        let res = simulate(&req, &map, vec![]).unwrap();
        assert!(res.buy_and_hold.is_none());
        assert!((res.strategy.metrics.final_value - 15_000.0).abs() < 1e-6);
        assert!((res.strategy.metrics.total_return - 0.5).abs() < 1e-6);
        assert!((res.strategy.metrics.max_drawdown - (-0.25)).abs() < 1e-6);
        assert_eq!(res.strategy.metrics.max_drawdown_peak_date, Some(dates[1]));
        assert_eq!(
            res.strategy.metrics.max_drawdown_trough_date,
            Some(dates[2])
        );
        assert_eq!(
            res.strategy.metrics.max_drawdown_recovery_date,
            Some(dates[3])
        );
    }

    #[test]
    fn dividend_modes_differentiate_reinvest_cash_and_exclude() {
        let dates = trading_days_from(NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(), 4);
        let prices = vec![100.0, 100.0, 110.0, 110.0];
        let divs = vec![(dates[1], 10.0)];

        let mut map = HashMap::new();
        map.insert("VYM".to_string(), make_series("VYM", &dates, &prices, divs));

        let run = |mode| {
            let req = BacktestRequest {
                assets: vec![asset("VYM", 100.0)],
                start_date: dates[0],
                end_date: dates[3],
                initial_capital: 1_000.0,
                contribution: None,
                dividend_mode: mode,
                fee_bps: 0.0,
                benchmark_symbol: None,
                benchmark_exchange_mic: None,
                benchmark_instrument_type: None,
                strategy: BacktestStrategy::BuyAndHold,
            };
            simulate(&req, &map, vec![]).unwrap()
        };

        let reinvest = run(DividendMode::Reinvest);
        let cash = run(DividendMode::Cash);
        let exclude = run(DividendMode::Exclude);

        assert!((exclude.strategy.metrics.final_value - 1_100.0).abs() < 1e-6);
        assert!((exclude.strategy.metrics.total_dividends - 0.0).abs() < 1e-6);
        assert!((cash.strategy.metrics.final_value - 1_200.0).abs() < 1e-6);
        assert!((cash.strategy.metrics.total_dividends - 100.0).abs() < 1e-6);
        assert!((reinvest.strategy.metrics.final_value - 1_210.0).abs() < 1e-6);
        assert!((reinvest.strategy.metrics.total_dividends - 100.0).abs() < 1e-6);
    }

    #[test]
    fn drift_band_rebalance_captures_rebalancing_bonus_and_tracks_rebalance_count() {
        let dates = trading_days_from(NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(), 4);
        let prices_a = vec![100.0, 200.0, 100.0, 100.0];
        let prices_b = vec![100.0, 100.0, 100.0, 100.0];
        let mut map = HashMap::new();
        map.insert("A".to_string(), make_series("A", &dates, &prices_a, vec![]));
        map.insert("B".to_string(), make_series("B", &dates, &prices_b, vec![]));

        let req = BacktestRequest {
            assets: vec![asset("A", 50.0), asset("B", 50.0)],
            start_date: dates[0],
            end_date: dates[3],
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: None,
            benchmark_exchange_mic: None,
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::DriftBandRebalance {
                threshold_pct: 10.0,
            },
        };

        let res = simulate(&req, &map, vec![]).unwrap();
        assert!((res.strategy.metrics.final_value - 11_250.0).abs() < 1e-6);
        assert_eq!(res.strategy.metrics.rebalance_count, 2);
        let bh = res.buy_and_hold.unwrap();
        assert!((bh.metrics.final_value - 10_000.0).abs() < 1e-6);
    }

    #[test]
    fn periodic_rebalance_and_dca_keep_twr_unpolluted_by_inflows() {
        let dates = trading_days_from(NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(), 45);
        let prices = vec![100.0; dates.len()];
        let mut map = HashMap::new();
        map.insert(
            "VTI".to_string(),
            make_series("VTI", &dates, &prices, vec![]),
        );

        let req = BacktestRequest {
            assets: vec![asset("VTI", 100.0)],
            start_date: dates[0],
            end_date: *dates.last().unwrap(),
            initial_capital: 10_000.0,
            contribution: Some(RecurringContribution {
                amount: 1_000.0,
                frequency: ContributionFrequency::Monthly,
            }),
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: None,
            benchmark_exchange_mic: None,
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::PeriodicRebalance {
                frequency: RebalanceFrequency::Monthly,
            },
        };

        let res = simulate(&req, &map, vec![]).unwrap();
        assert!((res.strategy.metrics.total_invested - 12_000.0).abs() < 1e-6);
        assert!((res.strategy.metrics.final_value - 12_000.0).abs() < 1e-6);
        assert!(res.strategy.metrics.twr.abs() < 1e-9);
        assert!(res.strategy.metrics.max_drawdown.abs() < 1e-9);
    }

    #[test]
    fn initial_purchase_and_dca_only_fees_are_reflected_in_twr() {
        // 25 trading days spanning Jan -> Feb 2024 (1 monthly DCA contribution on Feb 1).
        let dates = trading_days_from(NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(), 25);
        let prices = vec![100.0; dates.len()];
        let mut map = HashMap::new();
        map.insert(
            "VTI".to_string(),
            make_series("VTI", &dates, &prices, vec![]),
        );

        let req = BacktestRequest {
            assets: vec![asset("VTI", 100.0)],
            start_date: dates[0],
            end_date: *dates.last().unwrap(),
            initial_capital: 1_000.0,
            contribution: Some(RecurringContribution {
                amount: 100.0,
                frequency: ContributionFrequency::Monthly,
            }),
            dividend_mode: DividendMode::Exclude,
            fee_bps: 100.0, // 1% fee
            benchmark_symbol: None,
            benchmark_exchange_mic: None,
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::BuyAndHold,
        };

        let res = simulate(&req, &map, vec![]).unwrap();
        // Day 0: $1,000 * 1% = $10 fee -> $990 value, TWR factor 0.99.
        // Feb 1 DCA: +$100 inflow ($1,090 pre-trade), $100 * 1% = $1 fee -> $1,089 value,
        // fee factor 1089 / 1090. Total fees = $11.
        assert!((res.strategy.metrics.total_fees - 11.0).abs() < 1e-6);
        assert!((res.strategy.metrics.final_value - 1_089.0).abs() < 1e-6);
        let expected_twr = 0.99 * (1_089.0 / 1_090.0) - 1.0;
        assert!((res.strategy.metrics.twr - expected_twr).abs() < 1e-9);
        assert!(res.strategy.metrics.twr < -0.01);
        assert!((res.strategy.metrics.max_drawdown - expected_twr).abs() < 1e-9);

        // Flat-price Buy & Hold with 100 bps entry fee only (no DCA) must reflect
        // the -1% entry fee in volatility, Sharpe, and Sortino (not 0% / None).
        let entry_only_req = BacktestRequest {
            assets: vec![asset("VTI", 100.0)],
            start_date: dates[0],
            end_date: *dates.last().unwrap(),
            initial_capital: 1_000.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 100.0,
            benchmark_symbol: None,
            benchmark_exchange_mic: None,
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::BuyAndHold,
        };
        let entry_only = simulate(&entry_only_req, &map, vec![]).unwrap();
        assert!((entry_only.strategy.metrics.twr - (-0.01)).abs() < 1e-9);
        assert!(entry_only.strategy.metrics.annualized_volatility.unwrap() > 0.0);
        assert!(entry_only.strategy.metrics.sharpe_ratio.unwrap() < 0.0);
        assert!(entry_only.strategy.metrics.sortino_ratio.unwrap() < 0.0);
    }

    #[test]
    fn consecutive_missing_closes_fail_for_asset_and_omit_benchmark() {
        let dates = trading_days_from(NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(), 10);
        let prices_full = vec![100.0; 10];
        // Asset B is missing 5 consecutive sessions [2..7] bracketed by valid prices.
        let mut gapped_dates = Vec::new();
        let mut gapped_prices = Vec::new();
        for (idx, &d) in dates.iter().enumerate() {
            if !(2..7).contains(&idx) {
                gapped_dates.push(d);
                gapped_prices.push(100.0);
            }
        }

        // 1. Required portfolio asset with 5 missing sessions must error.
        let mut map = HashMap::new();
        map.insert(
            "A".to_string(),
            make_series("A", &dates, &prices_full, vec![]),
        );
        map.insert(
            "B".to_string(),
            make_series("B", &gapped_dates, &gapped_prices, vec![]),
        );

        let req = BacktestRequest {
            assets: vec![asset("A", 50.0), asset("B", 50.0)],
            start_date: dates[0],
            end_date: dates[9],
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: None,
            benchmark_exchange_mic: None,
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::BuyAndHold,
        };
        let err = simulate(&req, &map, vec![]).unwrap_err();
        assert!(err.to_string().contains("consecutive trading sessions"));

        // 2. Benchmark with 5 missing sessions is omitted with BenchmarkDataGap warning.
        let mut bench_map = HashMap::new();
        bench_map.insert(
            "A".to_string(),
            make_series("A", &dates, &prices_full, vec![]),
        );
        bench_map.insert(
            "BENCH".to_string(),
            make_series("BENCH", &gapped_dates, &gapped_prices, vec![]),
        );
        let bench_req = BacktestRequest {
            assets: vec![asset("A", 100.0)],
            start_date: dates[0],
            end_date: dates[9],
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: Some("BENCH".to_string()),
            benchmark_exchange_mic: None,
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::BuyAndHold,
        };
        let res = simulate(&bench_req, &bench_map, vec![]).unwrap();
        assert!(res.benchmark.is_none());
        assert!(res.warnings.contains(&BacktestWarning::BenchmarkDataGap {
            symbol: "BENCH".to_string()
        }));

        // 3. Single isolated missing session is allowed and emits IsolatedGapsForwardFilled warning.
        let mut single_gap_dates = Vec::new();
        let mut single_gap_prices = Vec::new();
        for (idx, &d) in dates.iter().enumerate() {
            if idx != 4 {
                single_gap_dates.push(d);
                single_gap_prices.push(100.0);
            }
        }
        let mut single_map = HashMap::new();
        single_map.insert(
            "A".to_string(),
            make_series("A", &dates, &prices_full, vec![]),
        );
        single_map.insert(
            "B".to_string(),
            make_series("B", &single_gap_dates, &single_gap_prices, vec![]),
        );
        let single_res = simulate(&req, &single_map, vec![]).unwrap();
        assert!(single_res
            .warnings
            .contains(&BacktestWarning::IsolatedGapsForwardFilled {
                symbol: "B".to_string(),
                count: 1,
            }));
    }

    #[test]
    fn benchmark_ending_before_effective_end_is_omitted_with_warning() {
        let dates = trading_days_from(NaiveDate::from_ymd_opt(2024, 1, 25).unwrap(), 10);
        let prices = vec![100.0; 10];
        let mut map = HashMap::new();
        map.insert(
            "VTI".to_string(),
            make_series("VTI", &dates, &prices, vec![]),
        );
        // Benchmark stops at index 4 (e.g. Jan 31) while VTI continues to index 9 (Feb 7).
        map.insert(
            "STALE".to_string(),
            make_series("STALE", &dates[..5], &prices[..5], vec![]),
        );

        let req = BacktestRequest {
            assets: vec![asset("VTI", 100.0)],
            start_date: dates[0],
            end_date: dates[9],
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: Some("STALE".to_string()),
            benchmark_exchange_mic: None,
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::BuyAndHold,
        };

        let res = simulate(&req, &map, vec![]).unwrap();
        assert!(res.benchmark.is_none());
        assert!(res.warnings.contains(&BacktestWarning::BenchmarkEndsEarly {
            symbol: "STALE".to_string(),
            benchmark_end_date: dates[4],
            effective_end_date: dates[9],
        }));
    }

    #[test]
    fn apply_splits_backward_neutralizes_two_for_one_split() {
        let dates = trading_days_from(NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(), 4);
        // Unadjusted closes: $200, $200 before split on dates[2], then $100, $100 after 2:1 split.
        let mut prices: BTreeMap<NaiveDate, f64> = [
            (dates[0], 200.0),
            (dates[1], 200.0),
            (dates[2], 100.0),
            (dates[3], 100.0),
        ]
        .into_iter()
        .collect();

        apply_splits_backward(
            &mut prices,
            &[SplitEvent {
                date: dates[2],
                ratio: dec!(2),
            }],
        );

        assert!((prices[&dates[0]] - 100.0).abs() < 1e-9);
        assert!((prices[&dates[1]] - 100.0).abs() < 1e-9);
        assert!((prices[&dates[2]] - 100.0).abs() < 1e-9);
        assert!((prices[&dates[3]] - 100.0).abs() < 1e-9);
    }

    #[test]
    fn sma_trend_filter_exits_to_cash_without_lookahead() {
        let dates = trading_days_from(NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(), 12);
        let prices = vec![
            100.0, 100.0, 100.0, 100.0, 100.0, 105.0, 90.0, 80.0, 70.0, 60.0, 50.0, 50.0,
        ];
        let mut map = HashMap::new();
        map.insert(
            "RISK".to_string(),
            make_series("RISK", &dates, &prices, vec![]),
        );

        let req = BacktestRequest {
            assets: vec![asset("RISK", 100.0)],
            start_date: dates[5],
            end_date: dates[11],
            initial_capital: 10_500.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: None,
            benchmark_exchange_mic: None,
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::SmaTrendFilter {
                sma_days: 5,
                check_frequency: SignalFrequency::Daily,
                defensive_symbol: None,
                defensive_exchange_mic: None,
                defensive_instrument_type: None,
            },
        };

        let res = simulate(&req, &map, vec![]).unwrap();
        assert!((res.strategy.metrics.final_value - 9_000.0).abs() < 1e-6);
        let bh = res.buy_and_hold.unwrap();
        assert!((bh.metrics.final_value - 5_000.0).abs() < 1e-6);
    }

    #[test]
    fn momentum_rotation_selects_top_performer_and_respects_absolute_momentum() {
        let dates = trading_days_from(NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(), 30);
        let winner: Vec<f64> = (0..30).map(|i| 100.0 + (i as f64) * 2.0).collect();
        let loser: Vec<f64> = (0..30).map(|i| 100.0 - (i as f64) * 1.5).collect();
        let safe = vec![100.0; 30];

        let mut map = HashMap::new();
        map.insert(
            "WIN".to_string(),
            make_series("WIN", &dates, &winner, vec![]),
        );
        map.insert(
            "LOSE".to_string(),
            make_series("LOSE", &dates, &loser, vec![]),
        );
        map.insert("SHY".to_string(), make_series("SHY", &dates, &safe, vec![]));

        let req = BacktestRequest {
            assets: vec![asset("WIN", 50.0), asset("LOSE", 50.0)],
            start_date: dates[5],
            end_date: *dates.last().unwrap(),
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: None,
            benchmark_exchange_mic: None,
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::MomentumRotation {
                lookback_days: 5,
                top_n: 1,
                rebalance_frequency: RebalanceFrequency::Monthly,
                weighting: MomentumWeighting::Equal,
                absolute_momentum: true,
                defensive_symbol: Some("SHY".to_string()),
                defensive_exchange_mic: None,
                defensive_instrument_type: None,
            },
        };

        let res = simulate(&req, &map, vec![]).unwrap();
        let expected = 10_000.0 * (winner[29] / winner[5]);
        assert!((res.strategy.metrics.final_value - expected).abs() < 1e-6);
        assert_eq!(res.strategy.ending_holdings.len(), 1);
        assert_eq!(res.strategy.ending_holdings[0].symbol, "WIN");
    }

    #[test]
    fn benchmark_missing_history_emits_warning_without_failing_backtest() {
        let dates = trading_days_from(NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(), 10);
        let prices = vec![100.0; 10];
        let mut map = HashMap::new();
        map.insert(
            "VTI".to_string(),
            make_series("VTI", &dates, &prices, vec![]),
        );
        map.insert(
            "NEWETF".to_string(),
            make_series("NEWETF", &dates[5..], &prices[5..], vec![]),
        );

        let req = BacktestRequest {
            assets: vec![asset("VTI", 100.0)],
            start_date: dates[0],
            end_date: dates[9],
            initial_capital: 10_000.0,
            contribution: None,
            dividend_mode: DividendMode::Exclude,
            fee_bps: 0.0,
            benchmark_symbol: Some("NEWETF".to_string()),
            benchmark_exchange_mic: None,
            benchmark_instrument_type: None,
            strategy: BacktestStrategy::BuyAndHold,
        };

        let res = simulate(&req, &map, vec![]).unwrap();
        assert!(res.benchmark.is_none());
        assert!(res
            .warnings
            .contains(&BacktestWarning::BenchmarkStartsLate {
                symbol: "NEWETF".to_string(),
                benchmark_start_date: dates[5],
                effective_start_date: dates[0],
            }));
    }
}
