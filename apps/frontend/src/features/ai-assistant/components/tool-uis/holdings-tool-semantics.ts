export type ReturnType = "daily" | "pnl" | "return";

interface BaseHoldingPerformanceAmounts {
  holdingType?: string;
  costBasisBase?: number | null;
  totalGainBase?: number | null;
  totalReturnBase?: number | null;
  returnBasisBase?: number | null;
}

export interface BasePortfolioPerformance {
  totalPnl: number | null;
  totalPnlPct: number | null;
  totalReturn: number | null;
  totalReturnPct: number | null;
}

interface DailyHoldingPerformanceAmounts {
  dayChangePct?: number | null;
  marketValueBase: number | null;
}

export interface DailyPortfolioPerformance {
  changeAmount: number | null;
  changePct: number | null;
}

function percentageFromGrossBasis(amount: number, grossBasis: number): number | null {
  if (grossBasis > 0) return amount / grossBasis;
  return amount === 0 ? 0 : null;
}

export function calculateBasePortfolioPerformance(
  holdings: BaseHoldingPerformanceAmounts[],
): BasePortfolioPerformance {
  let basis = 0;
  let totalPnl = 0;
  let totalReturn = 0;
  let hasUnbasedPnl = false;
  let hasUnbasedReturn = false;
  let missingPnl = false;
  let missingReturn = false;
  let missingBasis = false;

  holdings.forEach((holding) => {
    const isCash = holding.holdingType?.toLowerCase() === "cash";
    // Omitted fields are legacy; explicit null is unknown. Cash uses its balance.
    const rawBasis =
      holding.returnBasisBase === undefined || isCash
        ? (holding.returnBasisBase ?? holding.costBasisBase)
        : holding.returnBasisBase;
    const holdingBasis = Math.abs(rawBasis ?? 0);
    const holdingPnl = holding.totalGainBase ?? (isCash ? 0 : null);
    const holdingReturn =
      holding.totalReturnBase === undefined
        ? (holding.totalGainBase ?? (isCash ? 0 : null))
        : (holding.totalReturnBase ?? (isCash ? 0 : null));
    missingPnl ||= holdingPnl == null;
    missingReturn ||= holdingReturn == null;
    missingBasis ||= rawBasis == null;

    basis += holdingBasis;
    totalPnl += holdingPnl ?? 0;
    totalReturn += holdingReturn ?? 0;
    hasUnbasedPnl ||= holdingBasis === 0 && holdingPnl != null && holdingPnl !== 0;
    hasUnbasedReturn ||= holdingBasis === 0 && holdingReturn != null && holdingReturn !== 0;
  });

  return {
    totalPnl: missingPnl ? null : totalPnl,
    totalPnlPct:
      missingPnl || missingBasis || hasUnbasedPnl
        ? null
        : percentageFromGrossBasis(totalPnl, basis),
    totalReturn: missingReturn ? null : totalReturn,
    totalReturnPct:
      missingReturn || missingBasis || hasUnbasedReturn
        ? null
        : percentageFromGrossBasis(totalReturn, basis),
  };
}

export function calculateDailyPortfolioPerformance(
  holdings: DailyHoldingPerformanceAmounts[],
): DailyPortfolioPerformance {
  if (holdings.some((holding) => holding.marketValueBase == null || holding.dayChangePct == null)) {
    return { changeAmount: null, changePct: null };
  }
  let grossExposure = 0;
  let changeAmount = 0;
  let hasDailyPerformance = false;

  holdings.forEach((holding) => {
    if (holding.dayChangePct == null || holding.marketValueBase == null) return;

    const exposure = Math.abs(holding.marketValueBase);
    hasDailyPerformance = true;
    grossExposure += exposure;
    changeAmount += exposure * holding.dayChangePct;
  });

  return {
    changeAmount,
    changePct: grossExposure > 0 ? changeAmount / grossExposure : hasDailyPerformance ? 0 : null,
  };
}

export function getPortfolioChangePercent(
  returnType: ReturnType,
  dailyChangePct: number | null,
  totalPnlPct: number | null,
  totalReturnPct: number | null,
): number | null {
  if (returnType === "daily") return dailyChangePct;
  return returnType === "return" ? totalReturnPct : totalPnlPct;
}
