import type { Holding } from "@/lib/types";
import type {
  BacktestAssetInput,
  BacktestStrategy,
  DividendMode,
  RecurringContribution,
} from "@/lib/types/backtest";

export interface BacktestPreset {
  id: string;
  labelKey: string;
  descriptionKey: string;
  assets: BacktestAssetInput[];
  strategy: BacktestStrategy;
  benchmarkSymbol: string;
  dividendMode: DividendMode;
  contribution?: RecurringContribution | null;
}

export const BACKTEST_PRESETS: BacktestPreset[] = [
  {
    id: "classic-60-40",
    labelKey: "backtest:presets.classic_60_40.name",
    descriptionKey: "backtest:presets.classic_60_40.description",
    assets: [
      { symbol: "VTI", weight: 60 },
      { symbol: "BND", weight: 40 },
    ],
    strategy: { type: "periodicRebalance", frequency: "quarterly" },
    benchmarkSymbol: "SPY",
    dividendMode: "reinvest",
  },
  {
    id: "three-fund",
    labelKey: "backtest:presets.three_fund.name",
    descriptionKey: "backtest:presets.three_fund.description",
    assets: [
      { symbol: "VTI", weight: 50 },
      { symbol: "VXUS", weight: 20 },
      { symbol: "BND", weight: 30 },
    ],
    strategy: { type: "driftBandRebalance", thresholdPct: 5 },
    benchmarkSymbol: "SPY",
    dividendMode: "reinvest",
  },
  {
    id: "all-weather",
    labelKey: "backtest:presets.all_weather.name",
    descriptionKey: "backtest:presets.all_weather.description",
    assets: [
      { symbol: "VTI", weight: 30 },
      { symbol: "TLT", weight: 40 },
      { symbol: "IEF", weight: 15 },
      { symbol: "GLD", weight: 7.5 },
      { symbol: "DBC", weight: 7.5 },
    ],
    strategy: { type: "periodicRebalance", frequency: "annually" },
    benchmarkSymbol: "SPY",
    dividendMode: "reinvest",
  },
  {
    id: "trend-200sma",
    labelKey: "backtest:presets.trend_200sma.name",
    descriptionKey: "backtest:presets.trend_200sma.description",
    assets: [
      { symbol: "SPY", weight: 60 },
      { symbol: "QQQ", weight: 40 },
    ],
    strategy: {
      type: "smaTrendFilter",
      smaDays: 200,
      checkFrequency: "monthly",
      defensiveSymbol: "SHY",
    },
    benchmarkSymbol: "SPY",
    dividendMode: "reinvest",
  },
  {
    id: "dual-momentum",
    labelKey: "backtest:presets.dual_momentum.name",
    descriptionKey: "backtest:presets.dual_momentum.description",
    assets: [
      { symbol: "SPY", weight: 25 },
      { symbol: "QQQ", weight: 25 },
      { symbol: "IWM", weight: 25 },
      { symbol: "GLD", weight: 25 },
    ],
    strategy: {
      type: "momentumRotation",
      lookbackDays: 126,
      topN: 2,
      rebalanceFrequency: "monthly",
      weighting: "equal",
      absoluteMomentum: true,
      defensiveSymbol: "SHY",
    },
    benchmarkSymbol: "SPY",
    dividendMode: "reinvest",
  },
];

const US_TICKER_RE = /^[A-Z0-9.\-^]{1,16}$/;

/**
 * Extract up to 10 USD-denominated market holdings from the user's current portfolio
 * and normalize their market-value weights to 100%.
 */
export function extractUsdHoldingsWeights(holdings: Holding[]): BacktestAssetInput[] {
  const bySymbol = new Map<string, number>();

  for (const holding of holdings) {
    if (holding.isClosed || holding.holdingType === "cash") continue;
    const instrument = holding.instrument;
    if (!instrument || instrument.quoteMode === "MANUAL") continue;
    if ((instrument.currency || holding.localCurrency || "").toUpperCase() !== "USD") continue;

    const rawSymbol = instrument.symbol?.trim().toUpperCase() ?? "";
    if (!rawSymbol || !US_TICKER_RE.test(rawSymbol)) continue;

    const value = holding.marketValue?.local ?? holding.marketValue?.base ?? 0;
    if (!Number.isFinite(value) || value <= 0) continue;

    bySymbol.set(rawSymbol, (bySymbol.get(rawSymbol) ?? 0) + value);
  }

  const topEntries = [...bySymbol.entries()].sort((a, b) => b[1] - a[1]).slice(0, 10);

  const total = topEntries.reduce((sum, [, val]) => sum + val, 0);
  if (total <= 0 || topEntries.length === 0) {
    return [];
  }

  const weights = topEntries.map(([symbol, val]) => ({
    symbol,
    weight: Math.round((val / total) * 1000) / 10,
  }));

  // Adjust largest holding so total equals 100.0 cleanly after rounding.
  const roundedSum = weights.reduce((sum, item) => sum + item.weight, 0);
  const diff = Math.round((100 - roundedSum) * 10) / 10;
  if (weights.length > 0 && Math.abs(diff) > 0) {
    weights[0].weight = Math.max(0.1, Math.round((weights[0].weight + diff) * 10) / 10);
  }

  return weights;
}
