import { render, screen } from "@/test/render";
import { describe, expect, it, vi } from "vitest";
import { MemoryRouter } from "react-router-dom";
import type { Holding } from "@/lib/types";
import { HoldingsGroupedTable } from "./holdings-grouped-table";
vi.mock("@/components/ticker-avatar", () => ({ TickerAvatar: () => null }));
vi.mock("@/hooks/use-balance-privacy", () => ({
  useBalancePrivacy: () => ({ isBalanceHidden: false }),
}));
vi.mock("@/lib/settings-provider", () => ({
  useSettingsContext: () => ({ settings: { baseCurrency: "USD" } }),
}));
describe("grouped holding gain availability", () => {
  it.each([null, 0])("preserves unknown gains and real zero (%s)", (value) => {
    const monetary = value == null ? null : { local: value, base: value };
    const holding: Holding = {
      id: "stock",
      accountId: "a",
      holdingType: "security",
      instrument: { id: "stock", symbol: "STOCK", currency: "USD", quoteMode: "MARKET" },
      quantity: 1,
      localCurrency: "USD",
      baseCurrency: "USD",
      marketValue: { local: 100, base: 100 },
      costBasis: monetary,
      totalGain: monetary,
      totalGainPct: value,
      weight: 1,
      asOfDate: "2026-11-01",
    };
    render(
      <MemoryRouter>
        <HoldingsGroupedTable
          holdings={[holding]}
          accountTypeMap={new Map()}
          linkedLiabilities={new Map()}
          performanceMode="pnl"
          showConvertedValues={false}
          isLoading={false}
        />
      </MemoryRouter>,
    );
    if (value == null) expect(screen.getAllByText("—").length).toBeGreaterThan(0);
    else {
      expect(screen.queryByText("—")).not.toBeInTheDocument();
      expect(screen.getByText("$0.00")).toBeInTheDocument();
    }
  });
});

it.each([false, true])(
  "preserves explicit unknown returns and legacy fallback, converted=%s",
  (showConvertedValues) => {
    const holding: Holding = {
      id: "return-stock",
      accountId: "a",
      holdingType: "security",
      instrument: { id: "return-stock", symbol: "RETURN", currency: "USD", quoteMode: "MARKET" },
      quantity: 1,
      localCurrency: "USD",
      baseCurrency: "USD",
      marketValue: { local: 500, base: 500 },
      totalReturn: null,
      totalGain: { local: 10, base: 10 },
      weight: 1,
      asOfDate: "2026-11-01",
    };
    const view = (row: Holding) => (
      <MemoryRouter>
        <HoldingsGroupedTable
          holdings={[row]}
          accountTypeMap={new Map()}
          linkedLiabilities={new Map()}
          performanceMode="return"
          showConvertedValues={showConvertedValues}
          isLoading={false}
        />
      </MemoryRouter>
    );
    const { rerender } = render(view(holding));
    expect(screen.queryByText("$10.00")).not.toBeInTheDocument();
    expect(screen.getAllByText("—").length).toBeGreaterThan(0);
    rerender(view({ ...holding, totalReturn: undefined }));
    expect(screen.getByText("$10.00")).toBeInTheDocument();
  },
);
