import { listStatementActivities } from "@/adapters";
import { cleanup, fireEvent, render, screen, within } from "@/test/render";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useBalancePrivacy } from "@wealthfolio/ui";
import { afterEach, expect, it, vi } from "vitest";
import { StatementActivitiesPanel, useStatementActivities } from "./statement-activities-panel";

vi.mock("@/adapters", () => ({ listStatementActivities: vi.fn() }));

const row = {
  sourceId: "trade-1",
  sourcePage: 2,
  sourceLine: 1,
  date: "2025-05-02",
  statementDate: "2025-05-05",
  kind: "BUY" as const,
  assetId: "demo",
  assetLabel: "DEMO",
  quantity: "0.125",
  cash: "-10",
  currency: "USD",
};

function View({ enabled = true }: { enabled?: boolean }) {
  const query = useStatementActivities("securities-account", enabled);
  const { toggleBalanceVisibility } = useBalancePrivacy();
  return (
    <>
      <button onClick={toggleBalanceVisibility}>Toggle privacy</button>
      <StatementActivitiesPanel query={query} />
    </>
  );
}

function setup(enabled?: boolean) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <View enabled={enabled} />
    </QueryClientProvider>,
  );
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  document.documentElement.dir = "ltr";
  localStorage.removeItem("privacy-settings");
});

it.each(["ltr", "rtl"])("shows and masks read-only activity in %s", async (dir) => {
  document.documentElement.dir = dir;
  vi.mocked(listStatementActivities)
    .mockResolvedValueOnce({ rows: [row], nextOffset: 1 })
    .mockResolvedValueOnce({
      rows: [
        { ...row, sourceId: "cash-1", kind: "CASH", quantity: null, assetId: null, cash: "5" },
      ],
      nextOffset: null,
    });
  setup();
  const section = await screen.findByRole("region", { name: "Merrill statement activity" });
  const buy = await within(section).findByRole("row", { name: /Buy DEMO/ });
  expect(buy).toHaveTextContent("0.125");
  expect(buy).toHaveTextContent("$10.00");
  expect(within(buy).queryByRole("button")).not.toBeInTheDocument();
  expect(listStatementActivities).toHaveBeenCalledWith("securities-account", 0);
  fireEvent.click(screen.getByRole("button", { name: /Load more/ }));
  await within(section).findByText("Cash movement");
  expect(listStatementActivities).toHaveBeenLastCalledWith("securities-account", 1);
  expect(screen.queryByRole("button", { name: /Load more/ })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Toggle privacy" }));
  expect(buy).not.toHaveTextContent("0.125");
  expect(buy).not.toHaveTextContent("$10.00");
  expect(within(buy).getAllByText("••••")).toHaveLength(2);
});

it("waits for the account activities tab before requesting statement rows", () => {
  setup(false);
  expect(listStatementActivities).not.toHaveBeenCalled();
  expect(
    screen.queryByRole("region", { name: "Merrill statement activity" }),
  ).not.toBeInTheDocument();
});
