import type { SymbolSearchResult } from "@/lib/types";
import type { BacktestResponse } from "@/lib/types/backtest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { BacktestPage } from "./backtest-page";

const mocks = vi.hoisted(() => ({
  runBacktest: vi.fn(),
  useHoldings: vi.fn(),
}));

vi.mock("@/adapters", () => ({
  runBacktest: mocks.runBacktest,
}));

vi.mock("@/hooks/use-holdings", () => ({
  useHoldings: mocks.useHoldings,
}));

vi.mock("@/components/ticker-search", () => ({
  default: ({
    onSelectResult,
  }: {
    onSelectResult: (symbol: string, searchResult?: SymbolSearchResult) => void;
  }) => (
    <div>
      <button
        type="button"
        data-testid="mock-add-us-etf"
        onClick={() =>
          onSelectResult("QQQ", {
            symbol: "QQQ",
            exchange: "NMS",
            exchangeMic: "XNAS",
            canonicalExchangeMic: "XNAS",
            quoteType: "ETF",
            currency: "USD",
            shortName: "Invesco QQQ",
            longName: "Invesco QQQ Trust",
            index: "quotes",
            score: 1,
            typeDisplay: "ETF",
          })
        }
      >
        Add QQQ
      </button>
      <button
        type="button"
        data-testid="mock-add-non-us"
        onClick={() =>
          onSelectResult("VWRA.L", {
            symbol: "VWRA.L",
            exchange: "LSE",
            exchangeMic: "XLON",
            canonicalExchangeMic: "XLON",
            quoteType: "ETF",
            currency: "USD",
            shortName: "Vanguard FTSE All-World",
            longName: "Vanguard FTSE All-World UCITS ETF",
            index: "quotes",
            score: 1,
            typeDisplay: "ETF",
          })
        }
      >
        Add VWRA.L
      </button>
    </div>
  ),
}));

vi.mock("./components/backtest-charts", () => ({
  BacktestCharts: ({ result }: { result: BacktestResponse }) => (
    <div data-testid="backtest-charts">{result.strategy.descriptor.kind}</div>
  ),
}));

const sampleResponse: BacktestResponse = {
  currency: "USD",
  requestedStartDate: "2021-01-01",
  requestedEndDate: "2025-12-31",
  effectiveStartDate: "2021-01-04",
  effectiveEndDate: "2025-12-31",
  tradingDays: 1256,
  dividendMode: "reinvest",
  strategy: {
    descriptor: { kind: "periodicRebalance", frequency: "quarterly" },
    metrics: {
      finalValue: 16420,
      totalInvested: 10000,
      totalProfit: 6420,
      totalReturn: 0.642,
      twr: 0.642,
      cagr: 0.104,
      annualizedVolatility: 0.115,
      sharpeRatio: 0.91,
      sortinoRatio: 1.28,
      maxDrawdown: -0.182,
      maxDrawdownPeakDate: "2022-01-03",
      maxDrawdownTroughDate: "2022-10-12",
      maxDrawdownRecoveryDate: "2023-12-14",
      bestYear: { year: 2023, returnPct: 0.19 },
      worstYear: { year: 2022, returnPct: -0.15 },
      totalDividends: 1250,
      totalFees: 0,
      rebalanceCount: 19,
      annualizedTurnover: 0.08,
    },
    series: [
      { date: "2021-01-04", value: 10000, invested: 10000, cumulativeReturn: 0, drawdown: 0 },
      { date: "2025-12-31", value: 16420, invested: 10000, cumulativeReturn: 0.642, drawdown: 0 },
    ],
    annualReturns: [
      { year: 2022, returnPct: -0.15 },
      { year: 2023, returnPct: 0.19 },
    ],
    endingHoldings: [
      { symbol: "VTI", weight: 0.61, value: 10016.2, isCash: false },
      { symbol: "BND", weight: 0.39, value: 6403.8, isCash: false },
    ],
  },
  buyAndHold: {
    descriptor: { kind: "buyAndHold" },
    metrics: {
      finalValue: 16100,
      totalInvested: 10000,
      totalProfit: 6100,
      totalReturn: 0.61,
      twr: 0.61,
      cagr: 0.1,
      annualizedVolatility: 0.12,
      sharpeRatio: 0.85,
      sortinoRatio: 1.18,
      maxDrawdown: -0.195,
      maxDrawdownPeakDate: "2022-01-03",
      maxDrawdownTroughDate: "2022-10-12",
      maxDrawdownRecoveryDate: "2024-01-19",
      bestYear: { year: 2023, returnPct: 0.18 },
      worstYear: { year: 2022, returnPct: -0.16 },
      totalDividends: 1210,
      totalFees: 0,
      rebalanceCount: 0,
      annualizedTurnover: 0,
    },
    series: [
      { date: "2021-01-04", value: 10000, invested: 10000, cumulativeReturn: 0, drawdown: 0 },
      { date: "2025-12-31", value: 16100, invested: 10000, cumulativeReturn: 0.61, drawdown: 0 },
    ],
    annualReturns: [
      { year: 2022, returnPct: -0.16 },
      { year: 2023, returnPct: 0.18 },
    ],
    endingHoldings: [
      { symbol: "VTI", weight: 0.68, value: 10948, isCash: false },
      { symbol: "BND", weight: 0.32, value: 5152, isCash: false },
    ],
  },
  benchmark: null,
  symbolCoverage: [
    {
      symbol: "VTI",
      currency: "USD",
      firstDate: "2021-01-04",
      lastDate: "2025-12-31",
      quoteCount: 1256,
      dividendEventsCount: 20,
      dividendsAvailable: true,
    },
  ],
  warnings: [{ code: "dividendsUnavailable", symbol: "BND" }],
};

function renderPage() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <BacktestPage />
    </QueryClientProvider>,
  );
}

describe("BacktestPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.useHoldings.mockReturnValue({ holdings: [] });
    mocks.runBacktest.mockResolvedValue(sampleResponse);
  });

  it("submits default 60/40 periodic rebalance backtest with US listing metadata and localized warnings", async () => {
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole("button", { name: /actions\.run/i }));

    await waitFor(() => {
      expect(mocks.runBacktest).toHaveBeenCalledTimes(1);
    });

    const submitted = mocks.runBacktest.mock.calls[0][0];
    expect(submitted.assets).toEqual([
      { symbol: "VTI", weight: 60, exchangeMic: "ARCX", instrumentType: "ETF" },
      { symbol: "BND", weight: 40, exchangeMic: "XNAS", instrumentType: "ETF" },
    ]);
    expect(submitted.benchmarkSymbol).toBe("SPY");
    expect(submitted.benchmarkExchangeMic).toBe("ARCX");
    expect(submitted.benchmarkInstrumentType).toBe("ETF");
    expect(submitted.strategy).toEqual({
      type: "periodicRebalance",
      frequency: "quarterly",
    });

    expect(await screen.findByTestId("backtest-charts")).toHaveTextContent("periodicRebalance");
    expect(screen.getByText(/warnings\.dividends_unavailable/i)).toBeInTheDocument();
  });

  it("blocks submission when asset weights do not sum to 100%", async () => {
    const user = userEvent.setup();
    renderPage();

    const vtiWeightInput = screen.getByLabelText(/VTI/i);
    await user.clear(vtiWeightInput);
    await user.type(vtiWeightInput, "20");

    await user.click(screen.getByRole("button", { name: /actions\.run/i }));

    expect(mocks.runBacktest).not.toHaveBeenCalled();
  });

  it("preserves US listing metadata from search results and rejects non-US USD listings", async () => {
    const user = userEvent.setup();
    renderPage();

    // Non-US London USD ETF is rejected client-side
    await user.click(screen.getByTestId("mock-add-non-us"));
    expect(screen.getByText(/validation\.non_us_listing/i)).toBeInTheDocument();
    expect(screen.queryByText("VWRA.L")).not.toBeInTheDocument();

    // US NASDAQ ETF is added with exchangeMic and instrumentType preserved
    await user.click(screen.getByTestId("mock-add-us-etf"));
    await user.click(screen.getByRole("button", { name: /actions\.equalize/i }));
    await user.click(screen.getByRole("button", { name: /actions\.run/i }));

    await waitFor(() => {
      expect(mocks.runBacktest).toHaveBeenCalledTimes(1);
    });

    const qqq = mocks.runBacktest.mock.calls[0][0].assets.find(
      (a: { symbol: string }) => a.symbol === "QQQ",
    );
    expect(qqq).toMatchObject({
      symbol: "QQQ",
      exchangeMic: "XNAS",
      instrumentType: "ETF",
    });
  });

  it("switches to Dual Momentum preset and runs momentumRotation strategy", async () => {
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole("button", { name: /presets\.dual_momentum\.name/i }));
    await user.click(screen.getByRole("button", { name: /actions\.run/i }));

    await waitFor(() => {
      expect(mocks.runBacktest).toHaveBeenCalledTimes(1);
    });

    expect(mocks.runBacktest.mock.calls[0][0].strategy).toEqual({
      type: "momentumRotation",
      lookbackDays: 126,
      topN: 2,
      rebalanceFrequency: "monthly",
      weighting: "equal",
      absoluteMomentum: true,
      defensiveSymbol: "SHY",
      defensiveExchangeMic: "XNAS",
      defensiveInstrumentType: "ETF",
    });
  });
});
