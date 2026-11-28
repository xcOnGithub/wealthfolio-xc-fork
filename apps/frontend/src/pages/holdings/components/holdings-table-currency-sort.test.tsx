import { HoldingType, QuoteMode } from "@/lib/constants";
import type { Holding } from "@/lib/types";
import { fireEvent, render, screen, within } from "@/test/render";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import { HoldingsTable } from "./holdings-table";

vi.mock("@/components/ticker-avatar", () => ({ TickerAvatar: () => null }));
vi.mock("@/hooks/use-balance-privacy", () => ({
  useBalancePrivacy: () => ({ isBalanceHidden: false }),
}));
vi.mock("@/lib/settings-provider", () => ({
  useSettingsContext: () => ({ settings: { baseCurrency: "EUR" } }),
}));

describe("HoldingsTable real DataTable currency-toggle sorting", () => {
  it("reapplies the active Total Return sort when toggling between native and base currencies", () => {
    const baseOnlyExactHolding: Holding = {
      id: "eur-base-only-exact-100",
      accountId: "account-1",
      holdingType: HoldingType.SECURITY,
      quantity: 1,
      localCurrency: "USD",
      baseCurrency: "EUR",
      marketValue: { local: 110, base: 100 },
      costBasis: { local: 100, base: 90 },
      unrealizedGain: { local: 10, base: 10 },
      unrealizedGainPct: 0.1,
      totalReturn: { local: undefined as unknown as number, base: 100 },
      totalReturnPct: null,
      weight: 0.5,
      asOfDate: "2026-11-27",
      instrument: {
        id: "asset-alpha",
        symbol: "ALPH",
        name: "Alpha Corp",
        currency: "USD",
        quoteMode: QuoteMode.MARKET,
      },
    };
    const eurExact18Holding: Holding = {
      id: "eur-exact-18",
      accountId: "account-1",
      holdingType: HoldingType.SECURITY,
      quantity: 1,
      localCurrency: "EUR",
      baseCurrency: "EUR",
      marketValue: { local: 118, base: 118 },
      costBasis: { local: 100, base: 100 },
      unrealizedGain: { local: 18, base: 18 },
      unrealizedGainPct: 0.18,
      totalReturn: { local: 18, base: 18 },
      totalReturnPct: 0.18,
      weight: 0.5,
      asOfDate: "2026-11-27",
      instrument: {
        id: "asset-beta",
        symbol: "BETA",
        name: "Beta GmbH",
        currency: "EUR",
        quoteMode: QuoteMode.MARKET,
      },
    };

    render(
      <MemoryRouter>
        <HoldingsTable
          holdings={[eurExact18Holding, baseOnlyExactHolding]}
          isLoading={false}
          visibilityFilters={["open"]}
        />
      </MemoryRouter>,
    );

    const getRowOrder = () => {
      const rows = screen.getAllByRole("row").slice(1);
      return rows.map((row) => (within(row).queryByText("ALPH") ? "ALPH" : "BETA"));
    };

    // Sort ascending by Total Return in native currency mode:
    // ALPH displays estimated unrealized gain ($10 -> €10 base) < BETA (€18 base).
    fireEvent.click(screen.getByRole("button", { name: /Total Return/i }));
    fireEvent.click(screen.getByRole("menuitem", { name: /Asc/i }));
    expect(getRowOrder()).toEqual(["ALPH", "BETA"]);

    // Toggle to base currency (EUR): ALPH now displays exact €100 > BETA €18.
    // TanStack must reapply the active ascending sort so BETA moves ahead of ALPH.
    fireEvent.click(screen.getByRole("button", { name: /Show values in base currency/i }));
    expect(getRowOrder()).toEqual(["BETA", "ALPH"]);

    // Toggle back to native currency: ALPH returns to estimated €10 base < BETA €18.
    fireEvent.click(screen.getByRole("button", { name: /Show values in asset currency/i }));
    expect(getRowOrder()).toEqual(["ALPH", "BETA"]);
  });
});
