import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { HoldingType, QuoteMode } from "@/lib/constants";
import type { Holding } from "@/lib/types";
import type { ReactNode } from "react";

import { HoldingsTable } from "./holdings-table";

vi.mock("@wealthfolio/ui/components/ui/data-table", () => ({
  DataTable: ({
    columns,
    data,
    defaultColumnVisibility,
    pinRowsToTop,
  }: {
    columns: {
      id?: string;
      accessorFn?: (row: Holding, index: number) => unknown;
      cell?: (context: { row: { original: Holding } }) => ReactNode;
      meta?: { label?: string };
      filterFn?: (
        row: { getValue: (id: string) => unknown },
        id: string,
        value: string[],
      ) => boolean;
      sortingFn?: (rowA: { original: Holding }, rowB: { original: Holding }) => number;
    }[];
    data: Holding[];
    defaultColumnVisibility?: Record<string, boolean>;
    pinRowsToTop?: (row: Holding) => boolean;
  }) => {
    const getValue = (id: string) =>
      data[0] == null
        ? undefined
        : columns.find((column) => column.id === id)?.accessorFn?.(data[0], 0);
    const getNumericValue = (id: string) => {
      const value = getValue(id);
      return typeof value === "number" ? value : "";
    };
    const holdingTypeColumn = columns.find((column) => column.id === "holdingType");
    const holdingTypeCell =
      data[0] && holdingTypeColumn?.cell
        ? holdingTypeColumn.cell({ row: { original: data[0] } })
        : null;
    const realizedReturnColumn = columns.find((column) => column.id === "realizedReturn");
    const realizedReturnCell =
      data[0] && realizedReturnColumn?.cell
        ? realizedReturnColumn.cell({ row: { original: data[0] } })
        : null;
    const totalReturnColumn = columns.find((column) => column.id === "totalReturn");
    const sortedByTotalReturn = totalReturnColumn?.sortingFn
      ? [...data].sort((a, b) => totalReturnColumn.sortingFn!({ original: a }, { original: b }))
      : data;
    const matchesHoldingType = (value: string) =>
      holdingTypeColumn?.filterFn?.({ getValue: () => getValue("holdingType") }, "holdingType", [
        value,
      ]) ?? false;

    return (
      <div>
        {data[0] &&
          columns.find((column) => column.id === "symbol")?.cell?.({ row: { original: data[0] } })}
        {["bookValue", "totalPnl", "totalReturn", "unrealizedPnl"].map((id) => (
          <div key={id} data-testid={`basis-cell-${id}`}>
            {data[0] &&
              columns.find((column) => column.id === id)?.cell?.({ row: { original: data[0] } })}
          </div>
        ))}
        <div data-testid="total-return-sorted-ids">
          {sortedByTotalReturn.map((h) => h.id).join(",")}
        </div>
        <div data-testid="column-ids">{columns.map((column) => column.id).join(",")}</div>
        <div data-testid="symbol-name-hidden">
          {String(defaultColumnVisibility?.symbolName === false)}
        </div>
        <div data-testid="closed-cost-basis">{getNumericValue("closedCostBasis")}</div>
        <div data-testid="sale-proceeds">{getNumericValue("saleProceeds")}</div>
        <div data-testid="closing-cash-flow-label">
          {columns.find((column) => column.id === "saleProceeds")?.meta?.label}
        </div>
        <div data-testid="realized-return-cell">{realizedReturnCell}</div>
        <div data-testid="holding-type-cell">{holdingTypeCell}</div>
        <div data-testid="parent-type-match">{matchesHoldingType("FUND") ? "true" : "false"}</div>
        <div data-testid="exact-type-match">
          {matchesHoldingType("FUND_MUTUAL") ? "true" : "false"}
        </div>
        <div data-testid="cash-pinning">{String(data[0] && pinRowsToTop?.(data[0]))}</div>
      </div>
    );
  },
}));

vi.mock("@/components/ticker-avatar", () => ({ TickerAvatar: () => null }));
vi.mock("./cash-holdings-sheet", () => ({
  CashHoldingsSheet: ({ currency, onClose }: { currency: string; onClose: () => void }) => (
    <div role="dialog" aria-label={`Cash (${currency})`}>
      <button onClick={onClose}>Close cash</button>
    </div>
  ),
}));

vi.mock("@/hooks/use-balance-privacy", () => ({
  useBalancePrivacy: () => ({ isBalanceHidden: false }),
}));

vi.mock("@/lib/settings-provider", () => ({
  useSettingsContext: () => ({ settings: { baseCurrency: "USD" } }),
}));

vi.mock("react-router-dom", async (importOriginal) => ({
  ...(await importOriginal<typeof import("react-router-dom")>()),
  useNavigate: () => vi.fn(),
}));

describe("HoldingsTable columns", () => {
  it("uses market columns for open positions", () => {
    render(<HoldingsTable holdings={[]} isLoading={false} visibilityFilters={["open"]} />);

    const columnIds = screen.getByTestId("column-ids").textContent;
    expect(columnIds).toContain("marketPrice");
    expect(columnIds).toContain("marketValue");
    expect(columnIds).not.toContain("saleProceeds");
  });

  it("uses realized-performance columns for closed positions", () => {
    render(<HoldingsTable holdings={[]} isLoading={false} visibilityFilters={["closed"]} />);

    expect(screen.getByTestId("column-ids")).toHaveTextContent(
      [
        "symbol",
        "closedCostBasis",
        "saleProceeds",
        "closedRealizedPnl",
        "realizedReturn",
        "symbolName",
        "holdingType",
        "currency",
        "actions",
      ].join(","),
    );
    expect(screen.getByTestId("symbol-name-hidden")).toHaveTextContent("true");
  });

  it("derives closing cash flow from disposed cost basis and realized gain", () => {
    const holding: Holding = {
      id: "closed-position",
      accountId: "account-1",
      holdingType: HoldingType.SECURITY,
      isClosed: true,
      quantity: 0,
      localCurrency: "USD",
      baseCurrency: "USD",
      marketValue: { local: 0, base: 0 },
      costBasis: { local: 0, base: 0 },
      returnBasis: { local: 100, base: 125 },
      realizedGain: { local: 20, base: 25 },
      weight: 0,
      asOfDate: "2026-08-18",
    };

    render(<HoldingsTable holdings={[holding]} isLoading={false} visibilityFilters={["closed"]} />);

    expect(screen.getByTestId("closed-cost-basis")).toHaveTextContent("125");
    expect(screen.getByTestId("sale-proceeds")).toHaveTextContent("150");
    expect(screen.getByTestId("closing-cash-flow-label")).toHaveTextContent("Closing Cash Flow");
    expect(screen.getByTestId("realized-return-cell")).toHaveTextContent("—");
  });

  it("keeps the closing cash flow signed for a closed short position", () => {
    const holding: Holding = {
      id: "closed-short-position",
      accountId: "account-1",
      holdingType: HoldingType.SECURITY,
      isClosed: true,
      quantity: 0,
      localCurrency: "USD",
      baseCurrency: "USD",
      marketValue: { local: 0, base: 0 },
      costBasis: { local: 0, base: 0 },
      returnBasis: { local: -1000, base: -1000 },
      realizedGain: { local: 200, base: 200 },
      realizedGainPct: 0.2,
      weight: 0,
      asOfDate: "2026-08-18",
    };

    render(<HoldingsTable holdings={[holding]} isLoading={false} visibilityFilters={["closed"]} />);

    expect(screen.getByTestId("closed-cost-basis")).toHaveTextContent("-1000");
    expect(screen.getByTestId("sale-proceeds")).toHaveTextContent("-800");
  });

  it("renders a localized asset type and matches its taxonomy key exactly", () => {
    const holding: Holding = {
      id: "mutual-fund",
      accountId: "account-1",
      holdingType: HoldingType.SECURITY,
      quantity: 1,
      localCurrency: "USD",
      baseCurrency: "USD",
      marketValue: { local: 100, base: 100 },
      weight: 1,
      asOfDate: "2026-08-18",
      instrument: {
        id: "asset-mutual-fund",
        symbol: "FUND",
        currency: "USD",
        quoteMode: QuoteMode.MARKET,
        classifications: {
          assetType: {
            id: "FUND_MUTUAL",
            taxonomyId: "instrument_type",
            name: "Mutual Fund",
            key: "FUND_MUTUAL",
            color: "#000000",
            sortOrder: 1,
            createdAt: "2026-08-18",
            updatedAt: "2026-08-18",
          },
          assetClasses: [],
          sectors: [],
          regions: [],
          customGroups: [],
        },
      },
    };

    render(<HoldingsTable holdings={[holding]} isLoading={false} visibilityFilters={["open"]} />);

    expect(screen.getByTestId("holding-type-cell")).toHaveTextContent("Mutual Fund");
    expect(screen.getByTestId("parent-type-match")).toHaveTextContent("false");
    expect(screen.getByTestId("exact-type-match")).toHaveTextContent("true");
  });

  it("keeps cash sortable and opens its account breakdown", () => {
    const cashHolding: Holding = {
      id: "cash-usd",
      accountId: "account-1",
      holdingType: HoldingType.CASH,
      quantity: 100,
      localCurrency: "USD",
      baseCurrency: "USD",
      marketValue: { local: 100, base: 100 },
      weight: 0.1,
      asOfDate: "2026-08-20",
    };

    render(
      <HoldingsTable holdings={[cashHolding]} isLoading={false} visibilityFilters={["open"]} />,
    );

    expect(screen.getByTestId("cash-pinning")).toHaveTextContent("undefined");
    fireEvent.click(screen.getByRole("button", { name: "Details for Cash (USD)" }));
    expect(screen.getByRole("dialog", { name: "Cash (USD)" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Close cash" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("does not reopen cash details when a removed balance returns", () => {
    const cash: Holding = {
      id: "cash-usd",
      accountId: "account-1",
      holdingType: HoldingType.CASH,
      quantity: 100,
      localCurrency: "USD",
      baseCurrency: "USD",
      marketValue: { local: 100, base: 100 },
      weight: 0.1,
      asOfDate: "2026-08-20",
    };
    const view = (holdings: Holding[]) => <HoldingsTable holdings={holdings} isLoading={false} />;
    const { rerender } = render(view([cash]));
    fireEvent.click(screen.getByRole("button", { name: "Details for Cash (USD)" }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    rerender(view([]));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    rerender(view([cash]));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Details for Cash (USD)" }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });
});

it.each([null, 0])(
  "renders unavailable basis and gains distinctly from real zero (%s)",
  (value) => {
    const monetary = value == null ? null : { local: value, base: value };
    const holding: Holding = {
      id: "plaid",
      accountId: "a",
      holdingType: HoldingType.SECURITY,
      quantity: 1,
      localCurrency: "USD",
      baseCurrency: "USD",
      marketValue: { local: 100, base: 100 },
      costBasis: monetary,
      unrealizedGain: monetary,
      totalGain: monetary,
      totalReturn: monetary,
      weight: 1,
      asOfDate: "2026-11-01",
    };
    render(<HoldingsTable holdings={[holding]} isLoading={false} visibilityFilters={["open"]} />);
    for (const id of ["bookValue", "totalPnl", "totalReturn", "unrealizedPnl"]) {
      const cell = screen.getByTestId(`basis-cell-${id}`);
      expect(cell).toHaveTextContent(value == null ? "—" : "0.00");
      if (value == null) expect(cell).not.toHaveTextContent("0.00");
    }
  },
);

it("falls back to unrealized P&L with a focusable Est. button in Total Return and sorts by the displayed value", () => {
  const smallerDisplayed: Holding = {
    id: "smaller-displayed",
    accountId: "a",
    holdingType: HoldingType.SECURITY,
    quantity: 7,
    localCurrency: "USD",
    baseCurrency: "USD",
    marketValue: { local: 310, base: 310 },
    costBasis: { local: 300, base: 300 },
    unrealizedGain: { local: 10, base: 10 },
    unrealizedGainPct: 0.0333,
    totalGain: { local: 100, base: 100 },
    totalReturn: null,
    totalReturnPct: null,
    weight: 0.5,
    asOfDate: "2026-11-27",
  };
  const largerDisplayed: Holding = {
    id: "larger-displayed",
    accountId: "a",
    holdingType: HoldingType.SECURITY,
    quantity: 7,
    localCurrency: "USD",
    baseCurrency: "USD",
    marketValue: { local: 320, base: 320 },
    costBasis: { local: 300, base: 300 },
    unrealizedGain: { local: 20, base: 20 },
    unrealizedGainPct: 0.0667,
    totalGain: { local: 20, base: 20 },
    totalReturn: { local: 20, base: 20 },
    totalReturnPct: 0.0667,
    weight: 0.5,
    asOfDate: "2026-11-27",
  };

  render(
    <HoldingsTable
      holdings={[smallerDisplayed, largerDisplayed]}
      isLoading={false}
      visibilityFilters={["open"]}
    />,
  );

  const cell = screen.getByTestId("basis-cell-totalReturn");
  expect(cell).toHaveTextContent("10.00");
  const estButton = screen.getByRole("button", { name: "Est." });
  expect(cell).toContainElement(estButton);
  const disclosureText =
    "Estimated from unrealized P&L (market value minus cost basis); excludes realized gains and dividend history.";
  expect(screen.queryByText(disclosureText)).not.toBeInTheDocument();
  fireEvent.click(estButton);
  expect(screen.getByText(disclosureText)).toBeInTheDocument();
  expect(screen.getByTestId("total-return-sorted-ids")).toHaveTextContent(
    "smaller-displayed,larger-displayed",
  );
});

it("sorts Total Return by base currency across mixed currencies even when showing local values", () => {
  const jpyEstimated: Holding = {
    id: "jpy-estimated",
    accountId: "a",
    holdingType: HoldingType.SECURITY,
    quantity: 10,
    localCurrency: "JPY",
    baseCurrency: "USD",
    marketValue: { local: 15000, base: 100 },
    costBasis: { local: 13500, base: 90 },
    unrealizedGain: { local: 1500, base: 10 },
    unrealizedGainPct: 0.1111,
    totalReturn: null,
    totalReturnPct: null,
    weight: 0.5,
    asOfDate: "2026-11-27",
  };
  const usdExact: Holding = {
    id: "usd-exact",
    accountId: "a",
    holdingType: HoldingType.SECURITY,
    quantity: 1,
    localCurrency: "USD",
    baseCurrency: "USD",
    marketValue: { local: 120, base: 120 },
    costBasis: { local: 100, base: 100 },
    unrealizedGain: { local: 20, base: 20 },
    unrealizedGainPct: 0.2,
    totalReturn: { local: 20, base: 20 },
    totalReturnPct: 0.2,
    weight: 0.5,
    asOfDate: "2026-11-27",
  };

  render(
    <HoldingsTable
      holdings={[usdExact, jpyEstimated]}
      isLoading={false}
      visibilityFilters={["open"]}
    />,
  );

  // Ascending by base currency: 10 USD < 20 USD (not 1500 JPY > 20 USD)
  expect(screen.getByTestId("total-return-sorted-ids")).toHaveTextContent(
    "jpy-estimated,usd-exact",
  );
});

it("does not fall back to unrealized P&L for closed positions in Total Return", () => {
  const closedHolding: Holding = {
    id: "closed-no-total-return",
    accountId: "a",
    holdingType: HoldingType.SECURITY,
    isClosed: true,
    quantity: 0,
    localCurrency: "USD",
    baseCurrency: "USD",
    marketValue: { local: 0, base: 0 },
    costBasis: { local: 0, base: 0 },
    unrealizedGain: { local: 15, base: 15 },
    unrealizedGainPct: 0.15,
    totalReturn: null,
    totalReturnPct: null,
    weight: 0,
    asOfDate: "2026-11-27",
  };

  render(
    <HoldingsTable
      holdings={[closedHolding]}
      isLoading={false}
      visibilityFilters={["open", "closed"]}
    />,
  );

  const cell = screen.getByTestId("basis-cell-totalReturn");
  expect(cell).toHaveTextContent("—");
  expect(screen.queryByRole("button", { name: "Est." })).not.toBeInTheDocument();
});
