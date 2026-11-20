import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it, vi, beforeEach } from "vitest";
import type { TaxYearReport } from "@/lib/types";
import { TaxCenterPage } from "./tax-center-page";

const mockGetTaxYearReport = vi.fn();
const mockExportTaxPacket = vi.fn();

vi.mock("@/adapters", () => ({
  getTaxYearReport: (...args: unknown[]) => mockGetTaxYearReport(...args),
  exportTaxPacket: (...args: unknown[]) => mockExportTaxPacket(...args),
  createContributionLimit: vi.fn(),
  updateContributionLimit: vi.fn(),
  deleteContributionLimit: vi.fn(),
  calculateDepositsForLimit: vi.fn(),
  logger: { error: vi.fn(), warn: vi.fn(), info: vi.fn(), debug: vi.fn() },
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, opts?: Record<string, unknown>) =>
      typeof opts?.defaultValue === "string" ? opts.defaultValue : key,
  }),
}));

vi.mock("@wealthfolio/ui", async () => {
  const actual = await vi.importActual<typeof import("@wealthfolio/ui")>("@wealthfolio/ui");
  return {
    ...actual,
    useLocalizationSettings: () => ({ timezone: "UTC" }),
    useAmountFormatting: () => ({
      formatAmount: (value: number, currency: string) => `${currency} ${value.toFixed(2)}`,
    }),
    useDateFormatting: () => ({
      formatCalendarDate: (date: string) => date,
      formatDate: (date: string) => date,
    }),
  };
});

const sampleReport: TaxYearReport = {
  taxYear: 2026,
  asOfDate: "2026-11-20",
  baseCurrency: "USD",
  memberFilter: null,
  availableMembers: ["Alex", "Jordan"],
  disclaimer: "Informational estimates only",
  realizedGains: {
    shortTermGainsBase: "250.00",
    shortTermLossesBase: "-100.00",
    netShortTermBase: "150.00",
    longTermGainsBase: "1000.00",
    longTermLossesBase: "0.00",
    netLongTermBase: "1000.00",
    unknownTermNetBase: "0.00",
    totalNetRealizedBase: "1150.00",
    taxableDisposalCount: 2,
    unclassifiedRealizedPnlBase: "0.00",
    unclassifiedDisposalCount: 0,
    taxAdvantagedRealizedPnlBase: "0.00",
    taxAdvantagedDisposalCount: 0,
    disposals: [
      {
        disposalId: "disp-1",
        lotId: "lot-1",
        disposalActivityId: "act-1",
        accountId: "acc-1",
        accountName: "Alex Brokerage",
        ownerName: "Alex",
        taxTreatment: "TAXABLE",
        assetId: "asset-vti",
        symbol: "VTI",
        assetName: "Vanguard Total Stock",
        openDate: "2024-01-10",
        disposalDate: "2026-06-15",
        holdingDays: 887,
        term: "LONG_TERM",
        kind: "SALE",
        isShortSale: false,
        quantity: "10",
        proceedsBase: "2800.00",
        costBasisBase: "1800.00",
        realizedPnlBase: "1000.00",
        currency: "USD",
        costBasisMethod: "FIFO",
        washSaleRisk: null,
        estimatedDisallowedLossBase: "0.00",
      },
    ],
  },
  washSales: [
    {
      disposalId: "disp-2",
      disposalActivityId: "act-2",
      disposalAccountId: "acc-1",
      disposalAccountName: "Alex Brokerage",
      disposalOwnerName: "Alex",
      disposalDate: "2026-08-01",
      assetId: "asset-spy",
      symbol: "SPY",
      disposedQuantity: "5",
      realizedLossBase: "100.00",
      replacementActivityId: "act-3",
      replacementAccountId: "acc-2",
      replacementAccountName: "Jordan Roth IRA",
      replacementOwnerName: "Jordan",
      replacementTaxTreatment: "TAX_FREE",
      replacementDate: "2026-08-10",
      matchedQuantity: "5",
      estimatedDisallowedLossBase: "100.00",
      severity: "IRA_PERMANENT_DISALLOWANCE",
      confidence: "HIGH",
      note: "Rev. Rul. 2008-5",
    },
  ],
  income: {
    taxable: {
      grossDividendsBase: "400.00",
      grossInterestBase: "100.00",
      grossOtherIncomeBase: "0.00",
      returnOfCapitalBase: "0.00",
      incomeWithholdingTaxBase: "25.00",
      incomeFeesBase: "0.00",
      netInvestmentIncomeBase: "475.00",
      standaloneTaxPaidBase: "0.00",
    },
    taxDeferred: {
      grossDividendsBase: "0.00",
      grossInterestBase: "0.00",
      grossOtherIncomeBase: "0.00",
      returnOfCapitalBase: "0.00",
      incomeWithholdingTaxBase: "0.00",
      incomeFeesBase: "0.00",
      netInvestmentIncomeBase: "0.00",
      standaloneTaxPaidBase: "0.00",
    },
    taxFree: {
      grossDividendsBase: "0.00",
      grossInterestBase: "0.00",
      grossOtherIncomeBase: "0.00",
      returnOfCapitalBase: "0.00",
      incomeWithholdingTaxBase: "0.00",
      incomeFeesBase: "0.00",
      netInvestmentIncomeBase: "0.00",
      standaloneTaxPaidBase: "0.00",
    },
    unclassified: {
      grossDividendsBase: "0.00",
      grossInterestBase: "0.00",
      grossOtherIncomeBase: "0.00",
      returnOfCapitalBase: "0.00",
      incomeWithholdingTaxBase: "0.00",
      incomeFeesBase: "0.00",
      netInvestmentIncomeBase: "0.00",
      standaloneTaxPaidBase: "0.00",
    },
    byAccount: [],
  },
  tlhCandidates: [],
  assetLocation: {
    totals: {
      taxableBase: "10000.00",
      taxDeferredBase: "20000.00",
      taxFreeBase: "15000.00",
      unclassifiedBase: "0.00",
      totalBase: "45000.00",
    },
    byCategory: [],
  },
  contributionLimits: {
    totalLimitBase: "23500.00",
    totalContributedBase: "15000.00",
    totalRemainingBase: "8500.00",
    overLimitCount: 0,
    unmaterializedRecurringCount: 0,
    byMember: [],
  },
  readiness: [],
  warnings: {
    unclassifiedAccounts: [],
    holdingsModeTaxableAccounts: [],
    staleAccountIds: [],
    invalidTaxMetaAccountIds: [],
    unconvertedCurrencies: [],
    unpricedTlhLotCount: 0,
    unassignedAccountCount: 0,
    unassignedLimitCount: 0,
  },
};

describe("TaxCenterPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetTaxYearReport.mockResolvedValue(sampleReport);
  });

  it("renders KPI totals, wash-sale alerts, and realized disposals", async () => {
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });

    render(
      <QueryClientProvider client={queryClient}>
        <MemoryRouter>
          <TaxCenterPage />
        </MemoryRouter>
      </QueryClientProvider>,
    );

    await waitFor(() => {
      expect(screen.getByText("USD 1150.00")).toBeTruthy();
    });
    expect(screen.getByText("VTI")).toBeTruthy();
    expect(screen.getByText("SPY")).toBeTruthy();
    expect(screen.getByText("USD 500.00")).toBeTruthy();
    expect(screen.getByText("USD 8500.00")).toBeTruthy();
  });
});
