import { fireEvent, render, screen } from "@/test/render";
import { HoldingType, QuoteMode } from "@/lib/constants";
import type { Holding } from "@/lib/types";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import { HoldingsTableMobile } from "./holdings-table-mobile";

vi.mock("@/components/ticker-avatar", () => ({
  TickerAvatar: () => <div data-testid="ticker-avatar" />,
}));
vi.mock("@/hooks/use-balance-privacy", () => ({
  useBalancePrivacy: () => ({ isBalanceHidden: false }),
}));
vi.mock("./holdings-mobile-filter-sheet", () => ({ HoldingsMobileFilterSheet: () => null }));

const security = {
  id: "holding-1",
  holdingType: "security",
  localCurrency: "USD",
  instrument: { id: "asset/with space", symbol: "TEST", name: "Test investment" },
} as Holding;
const cash = {
  id: "$CASH-USD",
  holdingType: "cash",
  localCurrency: "USD",
  instrument: { id: "$CASH-USD", symbol: "$CASH-USD" },
} as Holding;

function Detail() {
  const { state } = useLocation();
  const { holding } = state as { holding: Holding };
  return <div role="status">{holding.id}</div>;
}

it("exposes a real holding link, preserves its context, and leaves cash non-interactive", () => {
  render(
    <MemoryRouter initialEntries={["/holdings"]}>
      <Routes>
        <Route
          path="/holdings"
          element={
            <HoldingsTableMobile
              holdings={[security, cash]}
              isLoading={false}
              selectedTypes={[]}
              setSelectedTypes={vi.fn()}
              accountFilter={{ type: "all" }}
              onAccountScopeChange={vi.fn()}
              accounts={[]}
              portfolios={[]}
              showAccountScope={false}
              showSearch={false}
              showFilterButton={false}
            />
          }
        />
        <Route path="/holdings/:id" element={<Detail />} />
      </Routes>
    </MemoryRouter>,
  );
  const link = screen.getByRole("link", { name: /TEST/ });
  expect(link).toHaveAttribute("href", "/holdings/asset%2Fwith%20space");
  expect(screen.getAllByRole("link")).toHaveLength(1);
  fireEvent.click(link);
  expect(screen.getByRole("status")).toHaveTextContent("holding-1");
});

const cashHolding: Holding = {
  id: "cash-usd",
  accountId: "account-1",
  holdingType: HoldingType.CASH,
  instrument: {
    id: "cash:USD",
    symbol: "USD",
    name: "Cash (USD)",
    currency: "USD",
    quoteMode: QuoteMode.MANUAL,
  },
  quantity: 250,
  localCurrency: "USD",
  baseCurrency: "USD",
  marketValue: { local: 250, base: 250 },
  weight: 0.25,
  asOfDate: "2026-08-18",
};

describe("HoldingsTableMobile", () => {
  it("keeps its controls available while holdings load", () => {
    render(
      <HoldingsTableMobile
        holdings={[]}
        isLoading
        selectedTypes={[]}
        setSelectedTypes={vi.fn()}
        accountFilter={{ type: "all" }}
        onAccountScopeChange={vi.fn()}
        accounts={[]}
        portfolios={[]}
        toolbarActions={<button type="button">Open actions</button>}
      />,
    );

    expect(screen.getByPlaceholderText("Search...")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open holdings filters" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open actions" })).toBeInTheDocument();
    expect(screen.queryByText("No positions found")).not.toBeInTheDocument();
  });

  it("shows portfolio weight below a cash balance", () => {
    render(
      <HoldingsTableMobile
        holdings={[cashHolding]}
        isLoading={false}
        selectedTypes={[]}
        setSelectedTypes={vi.fn()}
        accountFilter={{ type: "all" }}
        onAccountScopeChange={vi.fn()}
        accounts={[]}
        portfolios={[]}
        showSearch={false}
        showFilterButton={false}
      />,
    );

    expect(screen.getByText("Weight 25.00%")).toBeInTheDocument();
  });

  it("suggests changing filters when closed positions are hidden", () => {
    render(
      <HoldingsTableMobile
        holdings={[]}
        isLoading={false}
        selectedTypes={[]}
        setSelectedTypes={vi.fn()}
        accountFilter={{ type: "all" }}
        onAccountScopeChange={vi.fn()}
        accounts={[]}
        portfolios={[]}
        hasHiddenPositions
      />,
    );

    expect(screen.getByText("Try adjusting your search or filter criteria.")).toBeInTheDocument();
    expect(
      screen.queryByText("Add activities to see your positions here."),
    ).not.toBeInTheDocument();
  });
});

it.each(["pnl", "return"] as const)("shows unavailable %s without manufacturing zero", (mode) => {
  const holding: Holding = {
    ...cashHolding,
    id: "stock",
    holdingType: HoldingType.SECURITY,
    instrument: { id: "stock", symbol: "STOCK", currency: "USD", quoteMode: QuoteMode.MARKET },
    costBasis: null,
    totalGain: null,
    totalGainPct: null,
    totalReturn: null,
    totalReturnPct: null,
  };
  const props = {
    isLoading: false,
    selectedTypes: [],
    setSelectedTypes: vi.fn(),
    accountFilter: { type: "all" as const },
    onAccountScopeChange: vi.fn(),
    accounts: [],
    portfolios: [],
    showSearch: false,
    showFilterButton: false,
    performanceMode: mode,
  };
  const { rerender } = render(
    <MemoryRouter>
      <HoldingsTableMobile {...props} holdings={[holding]} />
    </MemoryRouter>,
  );
  expect(screen.getAllByText("—")).toHaveLength(2);
  const zero = {
    ...holding,
    costBasis: { local: 0, base: 0 },
    totalGain: { local: 0, base: 0 },
    totalGainPct: 0,
    totalReturn: { local: 0, base: 0 },
    totalReturnPct: 0,
  };
  rerender(
    <MemoryRouter>
      <HoldingsTableMobile {...props} holdings={[zero]} />
    </MemoryRouter>,
  );
  expect(screen.queryByText("—")).not.toBeInTheDocument();
});
