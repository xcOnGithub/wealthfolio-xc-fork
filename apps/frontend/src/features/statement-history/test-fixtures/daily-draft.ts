import type { MerrillHistoryDraft } from "../merrill-history";
export function dailyDraft(): MerrillHistoryDraft {
  return {
    format: "merrill-cma-history-v1",
    from: "2025-05-02",
    through: "2025-05-05",
    datePolicy: "settlement",
    activityHoldingLabels: {},
    openingCash: { value: "100", source: { page: 1, line: 10 } },
    closing: {
      date: "2025-05-05",
      currency: "USD",
      totalValue: "114",
      cashValue: "90",
      positionsComplete: true,
      format: "merrill-cma-monthly-v1",
      sourcePage: 1,
      positions: [{ label: "DEMO", quantity: "2", marketValue: "24", costBasis: null }],
    },
    cashReconciliation: { knownMovementTotal: "-10", difference: "0", matches: true },
    issues: [],
    evidence: {
      from: "2025-05-02",
      through: "2025-05-05",
      sections: { securities: 2 },
      dispositions: [],
      activities: [
        {
          id: "row-1",
          section: "securities",
          description: "Private transaction memo",
          security: "DEMO",
          date: "2025-05-02",
          dateKind: "settlement",
          action: "BUY",
          quantity: "1",
          cash: "-10",
          fee: "0.02",
          transferValue: null,
          page: 2,
          line: 10,
        },
      ],
    },
  };
}
