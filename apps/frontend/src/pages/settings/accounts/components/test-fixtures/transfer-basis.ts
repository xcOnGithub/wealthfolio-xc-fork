import type { PlaidInvestmentPreview } from "@/adapters/shared/plaid";
import type { TransferStatement } from "../transfer-basis";

export function transferPreview(): PlaidInvestmentPreview {
  return {
    mappingId: "mapping",
    digest: "digest",
    initial: true,
    startDate: "2025-05-13",
    currency: "USD",
    openingCash: "0",
    currentCash: "0",
    openingPositions: [],
    issues: [],
    sourceTransactionCount: 1,
    records: [
      {
        id: "incoming",
        date: "2025-10-10",
        type: "TRANSFER_IN",
        securityId: "demo",
        name: "Synthetic equity",
        quantity: "4",
        cash: "-7",
        fee: "7",
        price: "999",
        requiresTransferCost: true,
        transferUnitCost: null,
      },
    ],
  };
}
export function transferStatement(): TransferStatement {
  return {
    name: "synthetic.pdf",
    asOf: "2025-10-31",
    exclusions: [],
    sources: { 1: { page: "2", line: "3" } },
    candidates: [{ security: "DEMO", quantity: "4", totalCost: "100", unitCost: "25", rows: [1] }],
  };
}
