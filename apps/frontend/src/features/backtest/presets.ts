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
  benchmarkExchangeMic: string;
  benchmarkInstrumentType: string;
  dividendMode: DividendMode;
  contribution?: RecurringContribution | null;
}

export const ALLOWED_US_INSTRUMENT_TYPES = new Set([
  "EQUITY",
  "ETF",
  "MUTUALFUND",
  "MUTUAL_FUND",
  "FUND",
  "TRUST",
  "CLOSED_END_FUND",
]);

export const KNOWN_US_EXCHANGE_MICS = new Set([
  "XNYS",
  "XNAS",
  "ARCX",
  "XASE",
  "BATS",
  "IEXG",
  "XNGS",
  "XNMS",
  "XNCM",
  "OTCM",
  "OOTC",
]);

export const BACKTEST_PRESETS: BacktestPreset[] = [
  {
    id: "classic-60-40",
    labelKey: "backtest:presets.classic_60_40.name",
    descriptionKey: "backtest:presets.classic_60_40.description",
    assets: [
      { symbol: "VTI", weight: 60, exchangeMic: "ARCX", instrumentType: "ETF" },
      { symbol: "BND", weight: 40, exchangeMic: "XNAS", instrumentType: "ETF" },
    ],
    strategy: { type: "periodicRebalance", frequency: "quarterly" },
    benchmarkSymbol: "SPY",
    benchmarkExchangeMic: "ARCX",
    benchmarkInstrumentType: "ETF",
    dividendMode: "reinvest",
  },
  {
    id: "three-fund",
    labelKey: "backtest:presets.three_fund.name",
    descriptionKey: "backtest:presets.three_fund.description",
    assets: [
      { symbol: "VTI", weight: 50, exchangeMic: "ARCX", instrumentType: "ETF" },
      { symbol: "VXUS", weight: 20, exchangeMic: "XNAS", instrumentType: "ETF" },
      { symbol: "BND", weight: 30, exchangeMic: "XNAS", instrumentType: "ETF" },
    ],
    strategy: { type: "driftBandRebalance", thresholdPct: 5 },
    benchmarkSymbol: "SPY",
    benchmarkExchangeMic: "ARCX",
    benchmarkInstrumentType: "ETF",
    dividendMode: "reinvest",
  },
  {
    id: "all-weather",
    labelKey: "backtest:presets.all_weather.name",
    descriptionKey: "backtest:presets.all_weather.description",
    assets: [
      { symbol: "VTI", weight: 30, exchangeMic: "ARCX", instrumentType: "ETF" },
      { symbol: "TLT", weight: 40, exchangeMic: "XNAS", instrumentType: "ETF" },
      { symbol: "IEF", weight: 15, exchangeMic: "XNAS", instrumentType: "ETF" },
      { symbol: "GLD", weight: 7.5, exchangeMic: "ARCX", instrumentType: "ETF" },
      { symbol: "DBC", weight: 7.5, exchangeMic: "ARCX", instrumentType: "ETF" },
    ],
    strategy: { type: "periodicRebalance", frequency: "annually" },
    benchmarkSymbol: "SPY",
    benchmarkExchangeMic: "ARCX",
    benchmarkInstrumentType: "ETF",
    dividendMode: "reinvest",
  },
  {
    id: "trend-200sma",
    labelKey: "backtest:presets.trend_200sma.name",
    descriptionKey: "backtest:presets.trend_200sma.description",
    assets: [
      { symbol: "SPY", weight: 60, exchangeMic: "ARCX", instrumentType: "ETF" },
      { symbol: "QQQ", weight: 40, exchangeMic: "XNAS", instrumentType: "ETF" },
    ],
    strategy: {
      type: "smaTrendFilter",
      smaDays: 200,
      checkFrequency: "monthly",
      defensiveSymbol: "SHY",
      defensiveExchangeMic: "XNAS",
      defensiveInstrumentType: "ETF",
    },
    benchmarkSymbol: "SPY",
    benchmarkExchangeMic: "ARCX",
    benchmarkInstrumentType: "ETF",
    dividendMode: "reinvest",
  },
  {
    id: "dual-momentum",
    labelKey: "backtest:presets.dual_momentum.name",
    descriptionKey: "backtest:presets.dual_momentum.description",
    assets: [
      { symbol: "SPY", weight: 25, exchangeMic: "ARCX", instrumentType: "ETF" },
      { symbol: "QQQ", weight: 25, exchangeMic: "XNAS", instrumentType: "ETF" },
      { symbol: "IWM", weight: 25, exchangeMic: "ARCX", instrumentType: "ETF" },
      { symbol: "GLD", weight: 25, exchangeMic: "ARCX", instrumentType: "ETF" },
    ],
    strategy: {
      type: "momentumRotation",
      lookbackDays: 126,
      topN: 2,
      rebalanceFrequency: "monthly",
      weighting: "equal",
      absoluteMomentum: true,
      defensiveSymbol: "SHY",
      defensiveExchangeMic: "XNAS",
      defensiveInstrumentType: "ETF",
    },
    benchmarkSymbol: "SPY",
    benchmarkExchangeMic: "ARCX",
    benchmarkInstrumentType: "ETF",
    dividendMode: "reinvest",
  },
];

const US_TICKER_RE = /^[A-Z0-9.-]{1,16}$/;

/**
 * Extract up to 10 US-listed, USD-denominated market holdings from the user's
 * current portfolio and normalize their market-value weights to 100%.
 */
export function extractUsdHoldingsWeights(holdings: Holding[]): BacktestAssetInput[] {
  const bySymbol = new Map<
    string,
    { value: number; exchangeMic: string | null; instrumentType: string | null }
  >();

  for (const holding of holdings) {
    if (holding.isClosed || holding.holdingType === "cash") continue;
    const instrument = holding.instrument;
    if (!instrument || instrument.quoteMode === "MANUAL") continue;
    if ((instrument.currency || holding.localCurrency || "").toUpperCase() !== "USD") continue;

    const exchangeMic = instrument.exchangeMic?.trim().toUpperCase() || null;
    if (exchangeMic && !KNOWN_US_EXCHANGE_MICS.has(exchangeMic)) continue;

    const instrumentType = instrument.instrumentType?.trim().toUpperCase() || null;
    if (instrumentType && !ALLOWED_US_INSTRUMENT_TYPES.has(instrumentType)) continue;

    const rawSymbol = instrument.symbol?.trim().toUpperCase() ?? "";
    if (!rawSymbol || !US_TICKER_RE.test(rawSymbol)) continue;

    const value = holding.marketValue?.local ?? holding.marketValue?.base ?? 0;
    if (!Number.isFinite(value) || value <= 0) continue;

    const existing = bySymbol.get(rawSymbol);
    bySymbol.set(rawSymbol, {
      value: (existing?.value ?? 0) + value,
      exchangeMic: existing?.exchangeMic ?? exchangeMic,
      instrumentType: existing?.instrumentType ?? instrumentType,
    });
  }

  const topEntries = [...bySymbol.entries()].sort((a, b) => b[1].value - a[1].value).slice(0, 10);

  const total = topEntries.reduce((sum, [, entry]) => sum + entry.value, 0);
  if (total <= 0 || topEntries.length === 0) {
    return [];
  }

  const weights = topEntries.map(([symbol, entry]) => ({
    symbol,
    weight: Math.round((entry.value / total) * 1000) / 10,
    exchangeMic: entry.exchangeMic,
    instrumentType: entry.instrumentType,
  }));

  // Adjust largest holding so total equals 100.0 cleanly after rounding.
  const roundedSum = weights.reduce((sum, item) => sum + item.weight, 0);
  const diff = Math.round((100 - roundedSum) * 10) / 10;
  if (weights.length > 0 && Math.abs(diff) > 0) {
    weights[0].weight = Math.max(0.1, Math.round((weights[0].weight + diff) * 10) / 10);
  }

  return weights;
}
