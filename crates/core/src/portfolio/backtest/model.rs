//! Backtest experiment request, configuration, and result models.

use std::collections::HashSet;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use wealthfolio_market_data::resolver::{canonicalize_exchange_mic, is_us_exchange_mic};

use crate::errors::{Error, Result, ValidationError};

/// Maximum number of assets in a single backtest experiment.
pub const MAX_BACKTEST_ASSETS: usize = 10;

/// Maximum allowed fee/slippage drag in basis points (5%).
pub const MAX_FEE_BPS: f64 = 500.0;

/// Maximum SMA or momentum lookback window in trading days.
pub const MAX_LOOKBACK_DAYS: usize = 400;

/// Minimum SMA or momentum lookback window in trading days.
pub const MIN_LOOKBACK_DAYS: usize = 5;

/// Allowed market instrument types for US-listed equity/fund backtesting in v1.
pub fn is_allowed_us_security_type(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_uppercase().as_str(),
        "EQUITY" | "ETF" | "MUTUALFUND" | "MUTUAL_FUND" | "FUND" | "TRUST" | "CLOSED_END_FUND"
    )
}

fn normalize_optional_us_mic(raw: Option<&str>, symbol: &str) -> Result<Option<String>> {
    let Some(trimmed) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let canonical = canonicalize_exchange_mic(trimmed);
    if !is_us_exchange_mic(&canonical) {
        return Err(Error::Validation(ValidationError::InvalidInput(format!(
            "Ticker '{symbol}' on exchange '{canonical}' is outside the US-listed securities scope."
        ))));
    }
    Ok(Some(canonical))
}

fn normalize_optional_security_type(raw: Option<&str>, symbol: &str) -> Result<Option<String>> {
    let Some(trimmed) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let upper = trimmed.to_ascii_uppercase();
    if !is_allowed_us_security_type(&upper) {
        return Err(Error::Validation(ValidationError::InvalidInput(format!(
            "Ticker '{symbol}' has instrument type '{upper}', which is outside the US-listed equity/ETF scope."
        ))));
    }
    Ok(Some(upper))
}

/// Input asset weight specification with optional listing identity from search/holdings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BacktestAssetInput {
    pub symbol: String,
    /// Target allocation percentage (0.0 to 100.0).
    pub weight: f64,
    #[serde(default)]
    pub exchange_mic: Option<String>,
    #[serde(default)]
    pub instrument_type: Option<String>,
}

/// Recurring contribution schedule (DCA).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ContributionFrequency {
    Biweekly,
    Monthly,
    Quarterly,
    Annually,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecurringContribution {
    pub amount: f64,
    pub frequency: ContributionFrequency,
}

/// How cash dividends are handled during the simulation.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DividendMode {
    /// Reinvest each cash dividend immediately into the paying security at close (DRIP).
    #[default]
    Reinvest,
    /// Credit cash dividends to the portfolio cash balance (deployed on rebalance).
    Cash,
    /// Ignore dividends (price return only).
    Exclude,
}

/// Periodic rebalance cadence.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RebalanceFrequency {
    Monthly,
    #[default]
    Quarterly,
    Semiannually,
    Annually,
}

/// Evaluation frequency for tactical trend signals.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SignalFrequency {
    Daily,
    #[default]
    Monthly,
}

/// Weighting scheme among selected leaders in momentum rotation.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MomentumWeighting {
    #[default]
    Equal,
    InverseVolatility,
}

/// Backtest strategy definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum BacktestStrategy {
    /// Invest according to target weights and never sell to rebalance.
    BuyAndHold,
    /// Rebalance back to target weights on a fixed calendar schedule.
    PeriodicRebalance { frequency: RebalanceFrequency },
    /// Rebalance back to target weights whenever any asset drifts beyond `threshold_pct` (percentage points).
    DriftBandRebalance {
        #[serde(rename = "thresholdPct")]
        threshold_pct: f64,
    },
    /// Hold each asset when its close is above its `sma_days` simple moving average;
    /// otherwise park that slice in `defensive_symbol` (or cash if omitted).
    SmaTrendFilter {
        #[serde(rename = "smaDays")]
        sma_days: usize,
        #[serde(rename = "checkFrequency", default)]
        check_frequency: SignalFrequency,
        #[serde(rename = "defensiveSymbol", default)]
        defensive_symbol: Option<String>,
        #[serde(rename = "defensiveExchangeMic", default)]
        defensive_exchange_mic: Option<String>,
        #[serde(rename = "defensiveInstrumentType", default)]
        defensive_instrument_type: Option<String>,
    },
    /// On each rebalance date, rank universe assets by trailing `lookback_days` return
    /// and hold the top `top_n` assets.
    MomentumRotation {
        #[serde(rename = "lookbackDays")]
        lookback_days: usize,
        #[serde(rename = "topN")]
        top_n: usize,
        #[serde(rename = "rebalanceFrequency", default)]
        rebalance_frequency: RebalanceFrequency,
        #[serde(default)]
        weighting: MomentumWeighting,
        #[serde(rename = "absoluteMomentum", default)]
        absolute_momentum: bool,
        #[serde(rename = "defensiveSymbol", default)]
        defensive_symbol: Option<String>,
        #[serde(rename = "defensiveExchangeMic", default)]
        defensive_exchange_mic: Option<String>,
        #[serde(rename = "defensiveInstrumentType", default)]
        defensive_instrument_type: Option<String>,
    },
}

/// Request payload for running an in-session backtest experiment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BacktestRequest {
    pub assets: Vec<BacktestAssetInput>,
    pub start_date: NaiveDate,
    pub end_date: NaiveDate,
    pub initial_capital: f64,
    #[serde(default)]
    pub contribution: Option<RecurringContribution>,
    #[serde(default)]
    pub dividend_mode: DividendMode,
    #[serde(default)]
    pub fee_bps: f64,
    #[serde(default)]
    pub benchmark_symbol: Option<String>,
    #[serde(default)]
    pub benchmark_exchange_mic: Option<String>,
    #[serde(default)]
    pub benchmark_instrument_type: Option<String>,
    pub strategy: BacktestStrategy,
}

impl BacktestRequest {
    /// Normalize ticker strings and validate all request bounds.
    pub fn normalized_and_validated(&self) -> Result<Self> {
        if self.assets.is_empty() {
            return Err(Error::Validation(ValidationError::InvalidInput(
                "At least one asset is required.".to_string(),
            )));
        }
        if self.assets.len() > MAX_BACKTEST_ASSETS {
            return Err(Error::Validation(ValidationError::InvalidInput(format!(
                "At most {MAX_BACKTEST_ASSETS} assets are supported in a backtest."
            ))));
        }
        if self.end_date <= self.start_date {
            return Err(Error::Validation(ValidationError::InvalidInput(
                "End date must be after start date.".to_string(),
            )));
        }
        if !self.initial_capital.is_finite() || self.initial_capital <= 0.0 {
            return Err(Error::Validation(ValidationError::InvalidInput(
                "Initial capital must be a positive number.".to_string(),
            )));
        }
        if !self.fee_bps.is_finite() || !(0.0..=MAX_FEE_BPS).contains(&self.fee_bps) {
            return Err(Error::Validation(ValidationError::InvalidInput(format!(
                "Transaction fee must be between 0 and {MAX_FEE_BPS} bps."
            ))));
        }

        if let Some(contrib) = &self.contribution {
            if !contrib.amount.is_finite() || contrib.amount < 0.0 {
                return Err(Error::Validation(ValidationError::InvalidInput(
                    "Recurring contribution amount cannot be negative.".to_string(),
                )));
            }
        }

        let mut seen = HashSet::new();
        let mut normalized_assets = Vec::with_capacity(self.assets.len());
        let mut weight_sum = 0.0;

        for asset in &self.assets {
            let symbol = normalize_symbol(&asset.symbol)?;
            if !seen.insert(symbol.clone()) {
                return Err(Error::Validation(ValidationError::InvalidInput(format!(
                    "Duplicate ticker '{symbol}' in portfolio."
                ))));
            }
            if !asset.weight.is_finite() || asset.weight < 0.0 || asset.weight > 100.0 {
                return Err(Error::Validation(ValidationError::InvalidInput(format!(
                    "Weight for '{symbol}' must be between 0% and 100%."
                ))));
            }
            let exchange_mic = normalize_optional_us_mic(asset.exchange_mic.as_deref(), &symbol)?;
            let instrument_type =
                normalize_optional_security_type(asset.instrument_type.as_deref(), &symbol)?;
            weight_sum += asset.weight;
            normalized_assets.push(BacktestAssetInput {
                symbol,
                weight: asset.weight,
                exchange_mic,
                instrument_type,
            });
        }

        let requires_target_weights =
            !matches!(self.strategy, BacktestStrategy::MomentumRotation { .. });
        if requires_target_weights && !(99.9..=100.1).contains(&weight_sum) {
            return Err(Error::Validation(ValidationError::InvalidInput(format!(
                "Asset weights must sum to 100% (currently {weight_sum:.2}%)."
            ))));
        }

        let (benchmark_symbol, benchmark_exchange_mic, benchmark_instrument_type) = match self
            .benchmark_symbol
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(sym) => {
                let normalized = normalize_symbol(sym)?;
                let mic =
                    normalize_optional_us_mic(self.benchmark_exchange_mic.as_deref(), &normalized)?;
                let inst_type = normalize_optional_security_type(
                    self.benchmark_instrument_type.as_deref(),
                    &normalized,
                )?;
                (Some(normalized), mic, inst_type)
            }
            None => (None, None, None),
        };

        let strategy = match &self.strategy {
            BacktestStrategy::BuyAndHold => BacktestStrategy::BuyAndHold,
            BacktestStrategy::PeriodicRebalance { frequency } => {
                BacktestStrategy::PeriodicRebalance {
                    frequency: *frequency,
                }
            }
            BacktestStrategy::DriftBandRebalance { threshold_pct } => {
                if !threshold_pct.is_finite() || *threshold_pct <= 0.0 || *threshold_pct >= 100.0 {
                    return Err(Error::Validation(ValidationError::InvalidInput(
                        "Drift band threshold must be between 0% and 100%.".to_string(),
                    )));
                }
                BacktestStrategy::DriftBandRebalance {
                    threshold_pct: *threshold_pct,
                }
            }
            BacktestStrategy::SmaTrendFilter {
                sma_days,
                check_frequency,
                defensive_symbol,
                defensive_exchange_mic,
                defensive_instrument_type,
            } => {
                if !(MIN_LOOKBACK_DAYS..=MAX_LOOKBACK_DAYS).contains(sma_days) {
                    return Err(Error::Validation(ValidationError::InvalidInput(format!(
                        "SMA window must be between {MIN_LOOKBACK_DAYS} and {MAX_LOOKBACK_DAYS} trading days."
                    ))));
                }
                let (def_sym, def_mic, def_type) = match defensive_symbol
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    Some(sym) => {
                        let normalized = normalize_symbol(sym)?;
                        let mic = normalize_optional_us_mic(
                            defensive_exchange_mic.as_deref(),
                            &normalized,
                        )?;
                        let inst_type = normalize_optional_security_type(
                            defensive_instrument_type.as_deref(),
                            &normalized,
                        )?;
                        (Some(normalized), mic, inst_type)
                    }
                    None => (None, None, None),
                };
                BacktestStrategy::SmaTrendFilter {
                    sma_days: *sma_days,
                    check_frequency: *check_frequency,
                    defensive_symbol: def_sym,
                    defensive_exchange_mic: def_mic,
                    defensive_instrument_type: def_type,
                }
            }
            BacktestStrategy::MomentumRotation {
                lookback_days,
                top_n,
                rebalance_frequency,
                weighting,
                absolute_momentum,
                defensive_symbol,
                defensive_exchange_mic,
                defensive_instrument_type,
            } => {
                if !(MIN_LOOKBACK_DAYS..=MAX_LOOKBACK_DAYS).contains(lookback_days) {
                    return Err(Error::Validation(ValidationError::InvalidInput(format!(
                        "Momentum lookback must be between {MIN_LOOKBACK_DAYS} and {MAX_LOOKBACK_DAYS} trading days."
                    ))));
                }
                if *top_n == 0 || *top_n > normalized_assets.len() {
                    return Err(Error::Validation(ValidationError::InvalidInput(format!(
                        "Top N must be between 1 and {} (number of universe assets).",
                        normalized_assets.len()
                    ))));
                }
                let (def_sym, def_mic, def_type) = match defensive_symbol
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    Some(sym) => {
                        let normalized = normalize_symbol(sym)?;
                        let mic = normalize_optional_us_mic(
                            defensive_exchange_mic.as_deref(),
                            &normalized,
                        )?;
                        let inst_type = normalize_optional_security_type(
                            defensive_instrument_type.as_deref(),
                            &normalized,
                        )?;
                        (Some(normalized), mic, inst_type)
                    }
                    None => (None, None, None),
                };
                BacktestStrategy::MomentumRotation {
                    lookback_days: *lookback_days,
                    top_n: *top_n,
                    rebalance_frequency: *rebalance_frequency,
                    weighting: *weighting,
                    absolute_momentum: *absolute_momentum,
                    defensive_symbol: def_sym,
                    defensive_exchange_mic: def_mic,
                    defensive_instrument_type: def_type,
                }
            }
        };

        let contribution = self
            .contribution
            .as_ref()
            .filter(|c| c.amount > 0.0)
            .cloned();

        Ok(Self {
            assets: normalized_assets,
            start_date: self.start_date,
            end_date: self.end_date,
            initial_capital: self.initial_capital,
            contribution,
            dividend_mode: self.dividend_mode,
            fee_bps: self.fee_bps,
            benchmark_symbol,
            benchmark_exchange_mic,
            benchmark_instrument_type,
            strategy,
        })
    }

    /// Extra trading days needed prior to `start_date` to warm up indicators.
    pub fn warmup_trading_days(&self) -> usize {
        match &self.strategy {
            BacktestStrategy::SmaTrendFilter { sma_days, .. } => *sma_days,
            BacktestStrategy::MomentumRotation {
                lookback_days,
                weighting,
                ..
            } => {
                let vol_window = match weighting {
                    MomentumWeighting::InverseVolatility => 63,
                    MomentumWeighting::Equal => 0,
                };
                (*lookback_days).max(vol_window)
            }
            _ => 0,
        }
    }
}

/// Normalize and validate a US ticker symbol.
pub fn normalize_symbol(raw: &str) -> Result<String> {
    let trimmed = raw.trim().to_uppercase();
    if trimmed.is_empty() || trimmed.len() > 16 {
        return Err(Error::Validation(ValidationError::InvalidInput(format!(
            "Invalid ticker symbol '{raw}'."
        ))));
    }
    if !trimmed
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'^'))
    {
        return Err(Error::Validation(ValidationError::InvalidInput(format!(
            "Ticker symbol '{trimmed}' contains unsupported characters."
        ))));
    }
    Ok(trimmed)
}

/// Structured, locale-independent descriptor for a simulated series label.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum BacktestSeriesDescriptor {
    BuyAndHold,
    PeriodicRebalance {
        frequency: RebalanceFrequency,
    },
    DriftBandRebalance {
        #[serde(rename = "thresholdPct")]
        threshold_pct: f64,
    },
    SmaTrendFilter {
        #[serde(rename = "smaDays")]
        sma_days: usize,
    },
    MomentumRotation {
        #[serde(rename = "lookbackDays")]
        lookback_days: usize,
        #[serde(rename = "topN")]
        top_n: usize,
    },
    Benchmark {
        symbol: String,
    },
}

/// Structured, locale-independent warning emitted during data loading or simulation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "code", rename_all = "camelCase")]
pub enum BacktestWarning {
    EffectiveStartAdjusted {
        #[serde(rename = "requestedDate")]
        requested_date: NaiveDate,
        #[serde(rename = "effectiveDate")]
        effective_date: NaiveDate,
    },
    EffectiveEndAdjusted {
        #[serde(rename = "requestedDate")]
        requested_date: NaiveDate,
        #[serde(rename = "effectiveDate")]
        effective_date: NaiveDate,
    },
    DividendsUnavailable {
        symbol: String,
    },
    IsolatedGapsForwardFilled {
        symbol: String,
        count: usize,
    },
    BenchmarkUnavailable {
        symbol: String,
    },
    BenchmarkNotUsListing {
        symbol: String,
    },
    BenchmarkNonUsd {
        symbol: String,
        currency: String,
    },
    BenchmarkStartsLate {
        symbol: String,
        #[serde(rename = "benchmarkStartDate")]
        benchmark_start_date: NaiveDate,
        #[serde(rename = "effectiveStartDate")]
        effective_start_date: NaiveDate,
    },
    BenchmarkEndsEarly {
        symbol: String,
        #[serde(rename = "benchmarkEndDate")]
        benchmark_end_date: NaiveDate,
        #[serde(rename = "effectiveEndDate")]
        effective_end_date: NaiveDate,
    },
    BenchmarkDataGap {
        symbol: String,
    },
    BenchmarkSplitAdjustmentUnavailable {
        symbol: String,
        source: String,
    },
}

/// Single daily data point on a simulated equity curve.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BacktestPoint {
    pub date: NaiveDate,
    /// Total portfolio market value in USD.
    pub value: f64,
    /// Cumulative capital invested (initial + DCA) in USD.
    pub invested: f64,
    /// Cumulative time-weighted return as a decimal (e.g. 0.25 = +25%).
    pub cumulative_return: f64,
    /// Drawdown from running TWR peak as a negative or zero decimal (e.g. -0.15 = -15%).
    pub drawdown: f64,
}

/// Calendar-year TWR return.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnnualReturnPoint {
    pub year: i32,
    /// Return for the year as a decimal (e.g. 0.12 = +12%).
    pub return_pct: f64,
}

/// Final holding breakdown at the end of the simulation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BacktestEndingHolding {
    pub symbol: String,
    pub is_cash: bool,
    pub weight: f64,
    pub value: f64,
}

/// Summary risk/return metrics for a simulated series.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BacktestMetrics {
    pub final_value: f64,
    pub total_invested: f64,
    pub total_profit: f64,
    /// Money-on-money profit percentage: (final_value - total_invested) / total_invested.
    pub total_return: f64,
    /// Cumulative time-weighted return over the effective window.
    pub twr: f64,
    /// Annualized time-weighted return (CAGR).
    pub cagr: Option<f64>,
    /// Annualized daily TWR standard deviation.
    pub annualized_volatility: Option<f64>,
    /// Annualized Sharpe ratio (0% risk-free rate).
    pub sharpe_ratio: Option<f64>,
    /// Annualized Sortino ratio (0% target return).
    pub sortino_ratio: Option<f64>,
    /// Worst peak-to-trough drawdown as a negative or zero decimal (e.g. -0.24).
    pub max_drawdown: f64,
    pub max_drawdown_peak_date: Option<NaiveDate>,
    pub max_drawdown_trough_date: Option<NaiveDate>,
    pub max_drawdown_recovery_date: Option<NaiveDate>,
    pub best_year: Option<AnnualReturnPoint>,
    pub worst_year: Option<AnnualReturnPoint>,
    pub total_dividends: f64,
    pub total_fees: f64,
    pub rebalance_count: usize,
    /// Annualized one-way portfolio turnover as a decimal (e.g. 0.35 = 35%/yr).
    pub annualized_turnover: f64,
}

/// Full simulation output for one curve (strategy, buy-and-hold baseline, or benchmark).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BacktestSeriesResult {
    pub descriptor: BacktestSeriesDescriptor,
    pub metrics: BacktestMetrics,
    pub series: Vec<BacktestPoint>,
    pub annual_returns: Vec<AnnualReturnPoint>,
    pub ending_holdings: Vec<BacktestEndingHolding>,
}

/// Per-symbol market data coverage summary returned to the UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BacktestSymbolCoverage {
    pub symbol: String,
    pub currency: String,
    pub first_date: NaiveDate,
    pub last_date: NaiveDate,
    pub quote_count: usize,
    pub dividend_events_count: usize,
    pub dividends_available: bool,
}

/// Complete response from running a backtest experiment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BacktestResponse {
    pub currency: String,
    pub requested_start_date: NaiveDate,
    pub requested_end_date: NaiveDate,
    pub effective_start_date: NaiveDate,
    pub effective_end_date: NaiveDate,
    pub trading_days: usize,
    pub dividend_mode: DividendMode,
    pub strategy: BacktestSeriesResult,
    /// Buy & Hold baseline with the same assets, weights, cash flows, and dividend mode.
    /// Omitted when the selected strategy is already `BuyAndHold`.
    pub buy_and_hold: Option<BacktestSeriesResult>,
    /// Optional benchmark simulated with the same initial capital, DCA, and dividend mode.
    pub benchmark: Option<BacktestSeriesResult>,
    pub symbol_coverage: Vec<BacktestSymbolCoverage>,
    pub warnings: Vec<BacktestWarning>,
}
