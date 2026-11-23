export type ContributionFrequency = "biweekly" | "monthly" | "quarterly" | "annually";

export type DividendMode = "reinvest" | "cash" | "exclude";

export type RebalanceFrequency = "monthly" | "quarterly" | "semiannually" | "annually";

export type SignalFrequency = "daily" | "monthly";

export type MomentumWeighting = "equal" | "inverseVolatility";

export interface BacktestAssetInput {
  symbol: string;
  weight: number;
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
    }
  | {
      type: "momentumRotation";
      lookbackDays: number;
      topN: number;
      rebalanceFrequency: RebalanceFrequency;
      weighting: MomentumWeighting;
      absoluteMomentum: boolean;
      defensiveSymbol?: string | null;
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
  strategy: BacktestStrategy;
}

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
  label: string;
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
  warnings: string[];
}
