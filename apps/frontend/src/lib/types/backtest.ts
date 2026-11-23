export type ContributionFrequency = "biweekly" | "monthly" | "quarterly" | "annually";

export type DividendMode = "reinvest" | "cash" | "exclude";

export type RebalanceFrequency = "monthly" | "quarterly" | "semiannually" | "annually";

export type SignalFrequency = "daily" | "monthly";

export type MomentumWeighting = "equal" | "inverseVolatility";

export interface BacktestAssetInput {
  symbol: string;
  weight: number;
  exchangeMic?: string | null;
  instrumentType?: string | null;
}

export interface RecurringContribution {
  amount: number;
  frequency: ContributionFrequency;
}

export type BacktestStrategy =
  | { type: "buyAndHold" }
  | { type: "periodicRebalance"; frequency: RebalanceFrequency }
  | { type: "driftBandRebalance"; thresholdPct: number }
  | {
      type: "smaTrendFilter";
      smaDays: number;
      checkFrequency: SignalFrequency;
      defensiveSymbol?: string | null;
      defensiveExchangeMic?: string | null;
      defensiveInstrumentType?: string | null;
    }
  | {
      type: "momentumRotation";
      lookbackDays: number;
      topN: number;
      rebalanceFrequency: RebalanceFrequency;
      weighting: MomentumWeighting;
      absoluteMomentum: boolean;
      defensiveSymbol?: string | null;
      defensiveExchangeMic?: string | null;
      defensiveInstrumentType?: string | null;
    };

export interface BacktestRequest {
  assets: BacktestAssetInput[];
  startDate: string;
  endDate: string;
  initialCapital: number;
  contribution?: RecurringContribution | null;
  dividendMode: DividendMode;
  feeBps: number;
  benchmarkSymbol?: string | null;
  benchmarkExchangeMic?: string | null;
  benchmarkInstrumentType?: string | null;
  strategy: BacktestStrategy;
}

export type BacktestSeriesDescriptor =
  | { kind: "buyAndHold" }
  | { kind: "periodicRebalance"; frequency: RebalanceFrequency }
  | { kind: "driftBandRebalance"; thresholdPct: number }
  | { kind: "smaTrendFilter"; smaDays: number }
  | { kind: "momentumRotation"; lookbackDays: number; topN: number }
  | { kind: "benchmark"; symbol: string };

export type BacktestWarning =
  | { code: "effectiveStartAdjusted"; requestedDate: string; effectiveDate: string }
  | { code: "effectiveEndAdjusted"; requestedDate: string; effectiveDate: string }
  | { code: "dividendsUnavailable"; symbol: string }
  | { code: "isolatedGapsForwardFilled"; symbol: string; count: number }
  | { code: "benchmarkUnavailable"; symbol: string }
  | { code: "benchmarkNotUsListing"; symbol: string }
  | { code: "benchmarkNonUsd"; symbol: string; currency: string }
  | {
      code: "benchmarkStartsLate";
      symbol: string;
      benchmarkStartDate: string;
      effectiveStartDate: string;
    }
  | {
      code: "benchmarkEndsEarly";
      symbol: string;
      benchmarkEndDate: string;
      effectiveEndDate: string;
    }
  | { code: "benchmarkDataGap"; symbol: string }
  | { code: "benchmarkSplitAdjustmentUnavailable"; symbol: string; source: string };

export interface BacktestPoint {
  date: string;
  value: number;
  invested: number;
  cumulativeReturn: number;
  drawdown: number;
}

export interface AnnualReturnPoint {
  year: number;
  returnPct: number;
}

export interface BacktestEndingHolding {
  symbol: string;
  isCash: boolean;
  weight: number;
  value: number;
}

export interface BacktestMetrics {
  finalValue: number;
  totalInvested: number;
  totalProfit: number;
  totalReturn: number;
  twr: number;
  cagr: number | null;
  annualizedVolatility: number | null;
  sharpeRatio: number | null;
  sortinoRatio: number | null;
  maxDrawdown: number;
  maxDrawdownPeakDate: string | null;
  maxDrawdownTroughDate: string | null;
  maxDrawdownRecoveryDate: string | null;
  bestYear: AnnualReturnPoint | null;
  worstYear: AnnualReturnPoint | null;
  totalDividends: number;
  totalFees: number;
  rebalanceCount: number;
  annualizedTurnover: number;
}

export interface BacktestSeriesResult {
  descriptor: BacktestSeriesDescriptor;
  metrics: BacktestMetrics;
  series: BacktestPoint[];
  annualReturns: AnnualReturnPoint[];
  endingHoldings: BacktestEndingHolding[];
}

export interface BacktestSymbolCoverage {
  symbol: string;
  currency: string;
  firstDate: string;
  lastDate: string;
  quoteCount: number;
  dividendEventsCount: number;
  dividendsAvailable: boolean;
}

export interface BacktestResponse {
  currency: string;
  requestedStartDate: string;
  requestedEndDate: string;
  effectiveStartDate: string;
  effectiveEndDate: string;
  tradingDays: number;
  dividendMode: DividendMode;
  strategy: BacktestSeriesResult;
  buyAndHold: BacktestSeriesResult | null;
  benchmark: BacktestSeriesResult | null;
  symbolCoverage: BacktestSymbolCoverage[];
  warnings: BacktestWarning[];
}
