import type { BacktestSeriesDescriptor, BacktestWarning } from "@/lib/types/backtest";
import type { TFunction } from "i18next";

export function formatBacktestSeriesLabel(
  descriptor: BacktestSeriesDescriptor,
  t: TFunction,
): string {
  switch (descriptor.kind) {
    case "buyAndHold":
      return t("backtest:strategies.buy_and_hold");
    case "periodicRebalance":
      return t("backtest:series_labels.periodic_rebalance", {
        frequency: t(`backtest:frequencies.${descriptor.frequency}`),
      });
    case "driftBandRebalance":
      return t("backtest:series_labels.drift_band", {
        threshold: descriptor.thresholdPct.toFixed(1),
      });
    case "smaTrendFilter":
      return t("backtest:series_labels.sma_trend", {
        days: descriptor.smaDays,
      });
    case "momentumRotation":
      return t("backtest:series_labels.momentum_rotation", {
        topN: descriptor.topN,
        days: descriptor.lookbackDays,
      });
    case "benchmark":
      return descriptor.symbol;
  }
}

export function formatBacktestWarning(
  warning: BacktestWarning,
  t: TFunction,
  formatDate: (date: string) => string,
): string {
  switch (warning.code) {
    case "effectiveStartAdjusted":
      return t("backtest:warnings.effective_start_adjusted", {
        requestedDate: formatDate(warning.requestedDate),
        effectiveDate: formatDate(warning.effectiveDate),
      });
    case "effectiveEndAdjusted":
      return t("backtest:warnings.effective_end_adjusted", {
        requestedDate: formatDate(warning.requestedDate),
        effectiveDate: formatDate(warning.effectiveDate),
      });
    case "dividendsUnavailable":
      return t("backtest:warnings.dividends_unavailable", {
        symbol: warning.symbol,
      });
    case "isolatedGapsForwardFilled":
      return t("backtest:warnings.isolated_gaps_forward_filled", {
        symbol: warning.symbol,
        count: warning.count,
      });
    case "benchmarkUnavailable":
      return t("backtest:warnings.benchmark_unavailable", {
        symbol: warning.symbol,
      });
    case "benchmarkNotUsListing":
      return t("backtest:warnings.benchmark_not_us_listing", {
        symbol: warning.symbol,
      });
    case "benchmarkNonUsd":
      return t("backtest:warnings.benchmark_non_usd", {
        symbol: warning.symbol,
        currency: warning.currency,
      });
    case "benchmarkStartsLate":
      return t("backtest:warnings.benchmark_starts_late", {
        symbol: warning.symbol,
        benchmarkStartDate: formatDate(warning.benchmarkStartDate),
        effectiveStartDate: formatDate(warning.effectiveStartDate),
      });
    case "benchmarkEndsEarly":
      return t("backtest:warnings.benchmark_ends_early", {
        symbol: warning.symbol,
        benchmarkEndDate: formatDate(warning.benchmarkEndDate),
        effectiveEndDate: formatDate(warning.effectiveEndDate),
      });
    case "benchmarkDataGap":
      return t("backtest:warnings.benchmark_data_gap", {
        symbol: warning.symbol,
      });
    case "benchmarkSplitAdjustmentUnavailable":
      return t("backtest:warnings.benchmark_split_adjustment_unavailable", {
        symbol: warning.symbol,
        source: warning.source,
      });
  }
}
