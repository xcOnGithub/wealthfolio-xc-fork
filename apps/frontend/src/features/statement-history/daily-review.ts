import Decimal from "decimal.js";
import type { DailyStatementWrite } from "@/adapters/shared/statements";
import type { StatementHistoryDraft } from "./merrill-history";

/** No source identity, filenames or counterparty descriptions cross this boundary. */
export function dailyStatementWrite(
  draft: StatementHistoryDraft,
  links: Record<string, string>,
  expectedRevision: string | null,
  expectedSnapshotRevision: string | null,
): DailyStatementWrite {
  if (draft.issues.length || !draft.openingCash || draft.closing.cashValue === null)
    throw new Error("incomplete");
  const link = (key: string) => {
    if (!links[key]) throw new Error("security");
    return links[key];
  };
  const Money = Decimal.clone({ precision: 40 });
  const closingAssetIds = draft.closing.positions.map((_, i) => link(`holding:${i}`));
  const grouped = new Map<string, DailyStatementWrite["interval"]["closingPositions"][number]>();
  for (const [index, p] of draft.closing.positions.entries()) {
    if (p.marketValue === null) throw new Error("market");
    const assetId = closingAssetIds[index];
    const old = grouped.get(assetId);
    grouped.set(assetId, {
      assetId,
      quantity: new Money(old?.quantity ?? 0).plus(p.quantity).toFixed(),
      marketValue: new Money(old?.marketValue ?? 0).plus(p.marketValue).toFixed(),
      costBasis:
        p.costBasis === null || old?.costBasis === null
          ? null
          : new Money(old?.costBasis ?? 0).plus(p.costBasis).toFixed(),
    });
  }
  return {
    closingAssetIds,
    snapshot: draft.closing,
    expectedRevision,
    expectedSnapshotRevision,
    interval: {
      datePolicy: "settlement",
      from: draft.from,
      through: draft.through,
      currency: draft.closing.currency,
      openingCash: draft.openingCash.value,
      closingCash: draft.closing.cashValue,
      closingValue: draft.closing.totalValue,
      closingPositionsComplete: draft.closing.positionsComplete,
      activityComplete: true,
      closingPositions: [...grouped.values()].sort((a, b) => a.assetId.localeCompare(b.assetId)),
      events: draft.evidence.activities
        .filter((e) => !["sweep", "pending"].includes(e.section))
        .map((e) => {
          if (!e.date || e.action === "UNSUPPORTED") throw new Error("event");
          const isCash = e.action === "CASH" || e.action === "INCOME";
          const isTransfer = e.action === "TRANSFER_IN" || e.action === "TRANSFER_OUT";
          if ((!isCash && e.quantity === null) || (!isTransfer && e.cash === null))
            throw new Error("event");
          return {
            sourceId: e.id,
            date: e.date,
            kind: isCash
              ? ("CASH" as const)
              : (e.action as "BUY" | "SELL" | "TRANSFER_IN" | "TRANSFER_OUT"),
            assetId: isCash ? null : link(`activity:${e.id}`),
            quantity: isCash ? null : e.quantity,
            cash: isTransfer ? "0" : e.cash!,
            sourcePage: e.page,
            sourceLine: e.line,
          };
        }),
    },
  };
}
