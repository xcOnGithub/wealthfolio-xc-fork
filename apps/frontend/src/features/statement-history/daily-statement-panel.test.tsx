import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useBalancePrivacy } from "@wealthfolio/ui";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@/test/render";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  getAssets,
  listStatements,
  listDailyStatements,
  previewDailyStatements,
  saveDailyStatements,
  refreshStatementQuotes,
  registerStatementSecurities,
} from "@/adapters";
import { readBasisFile } from "./statement-pdf";
import { extractStatementHistories } from "./statement-parser";
import { DailyStatementPanel } from "./daily-statement-panel";
import { dailyStatementWrite } from "./daily-review";
import { dailyDraft } from "./test-fixtures/daily-draft";
import type { DailyStatementPreview } from "@/adapters/shared/statements";
vi.mock("@/adapters", () => ({
  getAssets: vi.fn(),
  listStatements: vi.fn(),
  listDailyStatements: vi.fn(),
  previewDailyStatements: vi.fn(),
  saveDailyStatements: vi.fn(),
  refreshStatementQuotes: vi.fn(),
  registerStatementSecurities: vi.fn(),
}));
vi.mock("./statement-parser", () => ({ extractStatementHistories: vi.fn() }));
vi.mock("./statement-pdf", async (original) => ({
  ...(await original<typeof import("./statement-pdf")>()),
  readBasisFile: vi.fn(),
}));
const preview: DailyStatementPreview = {
  liveHandoff: "unverified",
  reconstructed: {
    from: "2025-05-02",
    through: "2025-05-05",
    currency: "USD",
    openingCash: "100",
    openingPositions: { demo: "1" },
    days: [],
  },
  priced: {
    from: "2025-05-02",
    through: "2025-05-05",
    currency: "USD",
    closingPricesMatch: true,
    returnsEligible: false,
    days: [{ date: "2025-05-02", cash: "90", value: "110", quality: "COMPLETE", staleAssets: [] }],
  },
};
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getAssets).mockResolvedValue([
    {
      id: "demo",
      name: "Synthetic",
      displayCode: "DEMO",
      kind: "INVESTMENT",
      instrumentType: "EQUITY",
      quoteMode: "MANUAL",
      quoteCcy: "USD",
      createdAt: "",
      updatedAt: "",
    },
  ]);
  vi.mocked(listStatements).mockResolvedValue([]);
  vi.mocked(listDailyStatements).mockResolvedValue([]);
  vi.mocked(readBasisFile).mockImplementation(async (_, extract) => extract!([]));
  vi.mocked(extractStatementHistories).mockImplementation(() => [
    {
      identity: "TRANSIENT-ACCOUNT",
      draft: dailyDraft(),
    },
  ]);
  vi.mocked(previewDailyStatements).mockResolvedValue([preview]);
  vi.mocked(saveDailyStatements).mockResolvedValue();
  vi.mocked(refreshStatementQuotes).mockResolvedValue(0);
  vi.mocked(registerStatementSecurities).mockResolvedValue({});
});
afterEach(() => {
  cleanup();
  document.documentElement.dir = "ltr";
  localStorage.removeItem("privacy-settings");
});
function BalancePrivacyToggle() {
  const { toggleBalanceVisibility } = useBalancePrivacy();
  return <button onClick={toggleBalanceVisibility}>Toggle balance privacy</button>;
}
async function setup(multiple = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <BalancePrivacyToggle />
      <DailyStatementPanel accountId="a" currency="USD" />
    </QueryClientProvider>,
  );
  const input = screen.getByLabelText(/Monthly brokerage statements/);
  await waitFor(() => expect(input).toBeEnabled());
  fireEvent.change(input, {
    target: { files: [new File(["synthetic"], "synthetic.pdf", { type: "application/pdf" })] },
  });
  if (multiple) await screen.findAllByLabelText("DEMO · 2");
  else await screen.findByLabelText("DEMO · 2");
}
function map() {
  fireEvent.change(screen.getByLabelText("DEMO · 2"), { target: { value: "demo" } });
  fireEvent.change(screen.getByLabelText("DEMO", { exact: true }), { target: { value: "demo" } });
}
it("registers distinct printed tickers and prefills holdings and events without fetching quotes", async () => {
  const draft = dailyDraft();
  draft.holdingTickers = new Map([["demo", "MSFT"]]);
  draft.activityHoldingLabels = { "row-1": "DEMO" };
  const second = { ...draft, from: "2025-06-01", through: "2025-06-30" };
  vi.mocked(extractStatementHistories).mockReturnValue(
    [draft, second].map((draft) => ({ identity: "TRANSIENT-ACCOUNT", draft })),
  );
  const [other] = await getAssets();
  const registered = {
    ...other,
    id: "registered-msft",
    displayCode: "MSFT",
    instrumentSymbol: "MSFT",
    quoteMode: "MARKET" as const,
    isActive: false,
  };
  vi.mocked(registerStatementSecurities).mockImplementation(() => {
    vi.mocked(getAssets).mockResolvedValue([other, registered]);
    return Promise.resolve({ MSFT: registered });
  });
  await setup(true);
  expect(registerStatementSecurities).toHaveBeenCalledExactlyOnceWith("a", ["MSFT"]);
  const holdings = screen.getAllByLabelText("DEMO · 2");
  const events = screen.getAllByLabelText("DEMO", { exact: true });
  for (const row of [...holdings, ...events]) expect(row).toHaveValue("registered-msft");
  fireEvent.change(holdings[0], { target: { value: "demo" } });
  for (const row of [...holdings, ...events]) expect(row).toHaveValue("demo");
  expect(previewDailyStatements).not.toHaveBeenCalled();
  expect(saveDailyStatements).not.toHaveBeenCalled();
  expect(refreshStatementQuotes).not.toHaveBeenCalled();
});

it("keeps manual mapping available after registration fails", async () => {
  const draft = dailyDraft();
  draft.holdingTickers = new Map([["demo", "MSFT"]]);
  vi.mocked(extractStatementHistories).mockReturnValue([{ identity: "TRANSIENT-ACCOUNT", draft }]);
  vi.mocked(registerStatementSecurities).mockRejectedValueOnce(new Error("server unavailable"));
  await setup();
  expect(screen.getByRole("alert")).toHaveTextContent("Could not register securities");
  const holding = screen.getByLabelText("DEMO · 2");
  expect(holding).toHaveValue("");
  fireEvent.change(holding, { target: { value: "demo" } });
  expect(holding).toHaveValue("demo");
});

it.each(["ltr", "rtl"])(
  "prefills reviewed matches and lets users approve a correction in %s",
  async (dir) => {
    document.documentElement.dir = dir;
    const uploaded = dailyDraft();
    uploaded.holdingTickers = new Map([["demo", "MSFT"]]);
    uploaded.activityHoldingLabels = { "row-1": "DEMO" };
    vi.mocked(extractStatementHistories).mockReturnValue([
      { identity: "TRANSIENT-ACCOUNT", draft: uploaded },
    ]);
    const prior = dailyDraft();
    prior.through = prior.closing.date = "2025-04-30";
    const { snapshot, interval, activitySecurityKeys, closingAssetIds } = dailyStatementWrite(
      prior,
      { "holding:0": "demo", "activity:row-1": "demo" },
      null,
      null,
    );
    vi.mocked(listStatements).mockResolvedValue([{ snapshot, revision: "monthly" }]);
    vi.mocked(listDailyStatements).mockResolvedValue([
      { version: 1, revision: "daily", interval, activitySecurityKeys, closingAssetIds },
    ]);
    const [asset] = await getAssets();
    vi.mocked(getAssets).mockResolvedValue([
      asset,
      { ...asset, id: "corrected", displayCode: "CORRECTED" },
    ]);
    await setup();
    const holding = screen.getByLabelText("DEMO · 2");
    const activity = screen.getByLabelText("DEMO", { exact: true });
    expect(holding).toHaveValue("demo");
    expect(activity).toHaveValue("demo");
    expect(registerStatementSecurities).not.toHaveBeenCalled();

    const previewButton = screen.getByRole("button", { name: "Preview daily history" });
    fireEvent.click(previewButton);
    fireEvent.click(await screen.findByRole("checkbox"));
    fireEvent.change(holding, { target: { value: "corrected" } });
    expect(activity).toHaveValue("corrected");
    expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
    expect(saveDailyStatements).not.toHaveBeenCalled();

    fireEvent.click(previewButton);
    const save = await screen.findByRole("button", { name: "Save" });
    expect(save).toBeDisabled();
    fireEvent.click(screen.getByRole("checkbox"));
    fireEvent.click(save);
    await waitFor(() => expect(saveDailyStatements).toHaveBeenCalledOnce());
    const [, [write], confirmed] = vi.mocked(saveDailyStatements).mock.calls[0];
    expect(write.closingAssetIds).toEqual(["corrected"]);
    expect(write.interval.events[0].assetId).toBe("corrected");
    expect(confirmed).toBe(true);
  },
);
it("fills repeated dated-event dropdowns from a single selection on the current upload", async () => {
  const draft = dailyDraft();
  const row = {
    ...draft.evidence.activities[0],
    security: undefined,
    description: "Synthetic equity",
  };
  draft.evidence.activities = [row, { ...row, id: "second-event" }];
  draft.activityHoldingLabels = { [row.id]: "DEMO", "second-event": "DEMO" };
  vi.mocked(extractStatementHistories).mockReturnValue([{ identity: "TRANSIENT-ACCOUNT", draft }]);
  await setup();
  const events = screen.getAllByLabelText("Synthetic equity", { exact: true });
  expect(events).toHaveLength(2);
  expect(events[0]).toHaveValue("");
  expect(events[1]).toHaveValue("");
  fireEvent.change(events[0], { target: { value: "demo" } });
  expect(events[1]).toHaveValue("demo");
  expect(screen.getByLabelText("DEMO · 2")).toHaveValue("demo");
  expect(saveDailyStatements).not.toHaveBeenCalled();
});
it("warns before replacing a monthly snapshot when adding daily history", async () => {
  vi.mocked(listStatements).mockResolvedValue([
    { snapshot: dailyDraft().closing, revision: "monthly-revision" },
  ]);
  await setup();
  expect(screen.getByRole("alert")).toHaveTextContent(/existing snapshot for this date/);
  map();
  fireEvent.click(screen.getByRole("button", { name: "Preview daily history" }));
  await screen.findByRole("checkbox");
  expect(previewDailyStatements).toHaveBeenCalledWith("a", [
    expect.objectContaining({
      expectedRevision: null,
      expectedSnapshotRevision: "monthly-revision",
    }),
  ]);
});
it("masks opening cash, movements, and closing cash when Balance Privacy is on", async () => {
  await setup();
  const summary = screen.getByText(/Cash reconciliation:/);
  expect(summary).toHaveTextContent(/\b100\b.*-.*\b10\b.*\b90\b/);
  fireEvent.click(screen.getByRole("button", { name: "Toggle balance privacy" }));
  expect(summary).toHaveTextContent("Cash reconciliation: •••• + •••• = ••••");
  expect(summary).not.toHaveTextContent(/\d/);
});
it.each(["ltr", "rtl"])("saves only after mapped preview and confirmation in %s", async (dir) => {
  document.documentElement.dir = dir;
  await setup();
  map();
  fireEvent.click(screen.getByRole("button", { name: "Preview daily history" }));
  const save = await screen.findByRole("button", { name: "Save" });
  expect(save).toBeDisabled();
  const headers = screen.getAllByRole("columnheader");
  expect(headers).toHaveLength(3);
  for (const header of headers) {
    expect(header).toHaveClass("text-start");
  }
  expect(refreshStatementQuotes).not.toHaveBeenCalled();
  expect(JSON.stringify(vi.mocked(previewDailyStatements).mock.calls[0])).not.toContain(
    "TRANSIENT-ACCOUNT",
  );
  expect(JSON.stringify(vi.mocked(previewDailyStatements).mock.calls[0])).not.toContain(
    "Private transaction memo",
  );
  fireEvent.click(screen.getByRole("checkbox"));
  fireEvent.click(save);
  await waitFor(() => expect(saveDailyStatements).toHaveBeenCalledOnce());
  expect(vi.mocked(saveDailyStatements).mock.calls[0][2]).toBe(true);
});
it("invalidates preview and approval when an asset mapping changes", async () => {
  await setup();
  map();
  fireEvent.click(screen.getByRole("button", { name: "Preview daily history" }));
  await screen.findByRole("checkbox");
  fireEvent.click(screen.getByRole("checkbox"));
  fireEvent.change(screen.getByLabelText("DEMO · 2"), { target: { value: "" } });
  expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
  expect(saveDailyStatements).not.toHaveBeenCalled();
});
it("never requests a preview or provider for an unmapped security", async () => {
  await setup();
  fireEvent.click(screen.getByRole("button", { name: "Preview daily history" }));
  await screen.findByRole("alert");
  expect(previewDailyStatements).not.toHaveBeenCalled();
  expect(refreshStatementQuotes).not.toHaveBeenCalled();
});
it("fetches prices only on explicit refresh and shows unavailable days without invented values", async () => {
  await setup();
  map();
  vi.mocked(previewDailyStatements).mockResolvedValue([
    {
      ...preview,
      priced: {
        ...preview.priced,
        closingPricesMatch: null,
        days: [{ ...preview.priced.days[0], value: null, quality: "UNAVAILABLE" }],
      },
    },
  ]);
  fireEvent.click(screen.getByRole("button", { name: "Refresh historical prices" }));
  await waitFor(() => expect(refreshStatementQuotes).toHaveBeenCalledOnce());
  await screen.findByText("Prices unavailable");
  await screen.findByText(/Some daily values cannot be published/);
});
it("ignores a preview resolved after unmount", async () => {
  await setup();
  map();
  let resolve!: (r: DailyStatementPreview[]) => void;
  vi.mocked(previewDailyStatements).mockReturnValue(
    new Promise((r) => {
      resolve = r;
    }),
  );
  fireEvent.click(screen.getByRole("button", { name: "Preview daily history" }));
  await waitFor(() => expect(previewDailyStatements).toHaveBeenCalled());
  cleanup();
  await act(async () => resolve([preview]));
  expect(saveDailyStatements).not.toHaveBeenCalled();
});
it("blocks incomplete statement sections and retains the monthly alternative", async () => {
  vi.mocked(extractStatementHistories).mockImplementation(() => [
    {
      identity: "TRANSIENT-ACCOUNT",
      draft: { ...dailyDraft(), issues: [{ code: "unsettledTrades" }] },
    },
  ]);
  await setup();
  expect(screen.getByRole("button", { name: "Preview daily history" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Refresh historical prices" })).toBeDisabled();
  await screen.findByText(/Unsettled trades need a verified/);
});
it("stages, links across months, previews, and saves a bundled multi-month PDF", async () => {
  const april = dailyDraft();
  april.from = "2025-04-01";
  april.through = april.closing.date = "2025-04-30";
  april.evidence.from = "2025-04-01";
  april.evidence.through = "2025-04-30";
  april.evidence.activities = [
    { ...april.evidence.activities[0], id: "april-row-1", date: "2025-04-03" },
  ];
  april.activityHoldingLabels = { "april-row-1": "DEMO" };
  const may = dailyDraft();
  vi.mocked(extractStatementHistories).mockReturnValue([
    { identity: "TRANSIENT-ACCOUNT", draft: april },
    { identity: "TRANSIENT-ACCOUNT", draft: may },
  ]);
  vi.mocked(previewDailyStatements).mockResolvedValue([
    {
      ...preview,
      reconstructed: { ...preview.reconstructed, from: "2025-04-01", through: "2025-04-30" },
      priced: { ...preview.priced, from: "2025-04-01", through: "2025-04-30" },
    },
    preview,
  ]);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <DailyStatementPanel accountId="a" currency="USD" />
    </QueryClientProvider>,
  );
  const input = screen.getByLabelText(/Monthly brokerage statements/);
  await waitFor(() => expect(input).toBeEnabled());
  fireEvent.change(input, {
    target: { files: [new File(["synthetic"], "bundle.pdf", { type: "application/pdf" })] },
  });
  const holdings = await screen.findAllByLabelText("DEMO · 2");
  expect(holdings).toHaveLength(2);
  // Selecting once propagates to both months' holdings and activities
  fireEvent.change(holdings[0], { target: { value: "demo" } });
  expect(holdings[1]).toHaveValue("demo");
  fireEvent.click(screen.getByRole("button", { name: "Preview daily history" }));
  const save = await screen.findByRole("button", { name: "Save" });
  fireEvent.click(screen.getByRole("checkbox"));
  fireEvent.click(save);
  await waitFor(() => expect(saveDailyStatements).toHaveBeenCalledOnce());
  const [, writes, confirmed] = vi.mocked(saveDailyStatements).mock.calls[0];
  expect(confirmed).toBe(true);
  expect(writes.map((w) => w.interval.through)).toEqual(["2025-04-30", "2025-05-05"]);
});
