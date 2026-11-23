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
  default: ({ onSelectResult }: { onSelectResult: (symbol: string) => void }) => (
    <button type="button" data-testid="mock-add-ticker" onClick={() => onSelectResult("QQQ")}>
      Add QQQ
    </button>
  ),
}));

vi.mock("./components/backtest-charts", () => ({
  BacktestCharts: ({ result }: { result: BacktestResponse }) => (
    <div data-testid="backtest-charts">{result.strategy.label}</div>
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
    label: "Quarterly Rebalance",
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
      { symbol: "VTI", weight: 0.61, value: 10016.2 },
      { symbol: "BND", weight: 0.39, value: 6403.8 },
    ],
  },
  buyAndHold: {
    label: "Buy & Hold",
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
      { symbol: "VTI", weight: 0.68, value: 10948 },
      { symbol: "BND", weight: 0.32, value: 5152 },
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
  warnings: [],
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

  it("submits default 60/40 periodic rebalance backtest and renders results", async () => {
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole("button", { name: /actions\.run/i }));

    await waitFor(() => {
      expect(mocks.runBacktest).toHaveBeenCalledTimes(1);
    });

    const submitted = mocks.runBacktest.mock.calls[0][0];
    expect(submitted.assets).toEqual([
      { symbol: "VTI", weight: 60 },
      { symbol: "BND", weight: 40 },
    ]);
    expect(submitted.strategy).toEqual({
      type: "periodicRebalance",
      frequency: "quarterly",
    });

    expect(await screen.findByTestId("backtest-charts")).toHaveTextContent("Quarterly Rebalance");
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
    });
  });
});
