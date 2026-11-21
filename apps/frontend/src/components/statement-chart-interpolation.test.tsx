import { render, screen } from "@/test/render";
import { PrivacyProvider } from "@/context/privacy-context";
import type { NetWorthHistoryPoint } from "@/lib/types";
import { NetWorthChart } from "@/pages/net-worth/net-worth-chart";
import { HistoryChart } from "./history-chart";
import type { ReactNode } from "react";
import { expect, it, vi } from "vitest";

vi.mock("@/hooks", () => ({ useHapticFeedback: () => ({ triggerHaptic: vi.fn() }) }));
vi.mock("@wealthfolio/ui/components/ui/chart", () => ({
  ChartContainer: ({ children }: { children: ReactNode }) => <div>{children}</div>,
}));
vi.mock("recharts", () => ({
  Area: ({ dataKey, type }: { dataKey: string; type: string }) => (
    <span data-testid={dataKey}>{type}</span>
  ),
  AreaChart: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  ReferenceDot: () => null,
  ReferenceLine: () => null,
  Tooltip: () => null,
  XAxis: () => null,
  YAxis: () => null,
}));

it.each([
  { history: "daily", carryForward: false, curve: "monotone" },
  { history: "mixed", carryForward: true, curve: "stepAfter" },
])("uses $curve in both charts for $history history", ({ carryForward, curve }) => {
  const statementDate = "2025-05-02";
  const observations = [
    { date: "2025-05-01", value: 100, source: "statementDaily" },
    { date: statementDate, value: 120, source: "statementClosing" },
  ];
  if (carryForward) {
    observations.push({ date: "2025-05-03", value: 120, source: "monthlyCarryForward" });
  }
  render(
    <PrivacyProvider>
      <HistoryChart
        data={observations.map(({ date, value, source }) => ({
          date,
          currency: "USD",
          totalValue: value,
          netContribution: 0,
          statementDate,
          statementSource: source,
        }))}
      />
      <NetWorthChart
        data={observations.map(({ date, value, source }): NetWorthHistoryPoint => ({
          date,
          currency: "USD",
          netWorth: value,
          portfolioValue: value,
          totalAssets: value,
          totalLiabilities: 0,
          alternativeAssetsValue: 0,
          netContribution: 0,
          breakdown: {},
          statementSources: [{ accountId: "a", source, statementDate }],
        }))}
      />
    </PrivacyProvider>,
  );
  expect(screen.getByTestId("totalValue")).toHaveTextContent(curve);
  expect(screen.getByTestId("netWorth")).toHaveTextContent(curve);
});
