import { HoldingType } from "@/lib/constants";
import type { Holding, MonetaryValue } from "@/lib/types";

// Holdings lists keep open and closed positions separate. The asset overview
// shows the current position with lifetime metrics across both lifecycle rows.
export function getAssetProfileHolding(holdings: Holding[], assetId: string): Holding | null {
  const positions = holdings.filter(
    (holding) => holding.instrument?.id === assetId || holding.id === assetId,
  );
  const current = positions.find((holding) => !holding.isClosed) ?? positions[0];
  if (!current) return null;
  if (positions.length === 1) return current;

  const sum = (
    field: "income" | "realizedGain" | "totalGain" | "totalReturn" | "returnBasis",
  ): MonetaryValue | null => {
    const values = positions.flatMap((holding) => (holding[field] ? [holding[field]] : []));
    if (values.length === 0) return null;
    return values.reduce(
      (total, value) => ({
        local: total.local + Number(value.local),
        base: total.base + Number(value.base),
      }),
      { local: 0, base: 0 },
    );
  };
  const percentage = (value: MonetaryValue | null, basis: number): number | null => {
    if (value == null) return null;
    return basis !== 0 ? value.local / Math.abs(basis) : value.local === 0 ? 0 : null;
  };

  const income = sum("income");
  const realizedGain = sum("realizedGain");
  const unknownOpenBasis = positions.some(
    (holding) =>
      holding.holdingType === HoldingType.SECURITY &&
      !holding.isClosed &&
      holding.costBasis == null,
  );
  const totalGain = unknownOpenBasis ? null : sum("totalGain");
  const totalReturn = unknownOpenBasis ? null : sum("totalReturn");
  const returnBasis = unknownOpenBasis ? null : sum("returnBasis");
  const basis = returnBasis?.local ?? 0;

  return {
    ...current,
    income,
    realizedGain,
    realizedGainPct: unknownOpenBasis
      ? null
      : percentage(realizedGain, basis - Number(current.costBasis?.local ?? 0)),
    totalGain,
    totalGainPct: percentage(totalGain, basis),
    totalReturn,
    totalReturnPct: percentage(totalReturn, basis),
    returnBasis,
  };
}
