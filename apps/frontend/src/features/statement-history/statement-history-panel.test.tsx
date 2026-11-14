import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@/test/render";
import { afterEach, beforeEach, it, expect, vi } from "vitest";
import { listStatements, saveStatements, deleteStatement } from "@/adapters";
import { readBasisFile } from "@/pages/settings/accounts/components/basis-pdf";
import { StatementHistoryPanel } from "./statement-history-panel";
import { extractMonthlyMerrill } from "./merrill-statement";
import { monthlyFixture } from "./test-fixtures/monthly-statement";
vi.mock("@/adapters", () => ({
  listStatements: vi.fn(),
  saveStatements: vi.fn(),
  deleteStatement: vi.fn(),
}));
vi.mock("@/pages/settings/accounts/components/basis-pdf", async (original) => ({
  ...(await original<typeof import("@/pages/settings/accounts/components/basis-pdf")>()),
  readBasisFile: vi.fn(),
}));
let client: QueryClient;
beforeEach(() => {
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  vi.clearAllMocks();
  vi.mocked(listStatements).mockResolvedValue([]);
  vi.mocked(saveStatements).mockResolvedValue();
  vi.mocked(deleteStatement).mockResolvedValue();
  vi.mocked(readBasisFile).mockImplementation(async (_file, extract) => extract!(monthlyFixture()));
});
afterEach(() => {
  cleanup();
  localStorage.removeItem("privacy-settings");
  document.documentElement.dir = "ltr";
});
async function open() {
  render(
    <QueryClientProvider client={client}>
      <StatementHistoryPanel accountId="a" currency="USD" />
    </QueryClientProvider>,
  );
  fireEvent.click(screen.getByText("Import historical statements"));
  const input = screen.getByLabelText(/Merrill monthly statements/);
  await waitFor(() => expect(input).toBeEnabled());
  return input;
}
function upload(input: HTMLElement, names = ["statement.pdf"]) {
  fireEvent.change(input, {
    target: {
      files: names.map((name) => new File(["synthetic"], name, { type: "application/pdf" })),
    },
  });
}
it.each(["ltr", "rtl"])(
  "requires explicit review and saves only normalized observations in %s",
  async (direction) => {
    document.documentElement.dir = direction;
    const input = await open();
    upload(input);
    const save = await screen.findByRole("button", { name: "Save" });
    expect(save).toBeDisabled();
    expect(saveStatements).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("checkbox"));
    fireEvent.click(save);
    await waitFor(() => expect(saveStatements).toHaveBeenCalledOnce());
    expect(vi.mocked(saveStatements).mock.calls[0]).toEqual([
      "a",
      [{ snapshot: extractMonthlyMerrill(monthlyFixture()).snapshot, expectedRevision: null }],
      true,
    ]);
  },
);
it("rejects six documents before reading anything", async () => {
  const input = await open();
  upload(
    input,
    Array.from({ length: 6 }, (_, i) => `${i}.pdf`),
  );
  await screen.findByRole("alert");
  expect(readBasisFile).not.toHaveBeenCalled();
  expect(saveStatements).not.toHaveBeenCalled();
});
it("does not leave a partial batch after a later file fails", async () => {
  vi.mocked(readBasisFile)
    .mockImplementationOnce(async (_file, extract) => extract!(monthlyFixture()))
    .mockRejectedValueOnce(new Error("private parser error"));
  const input = await open();
  upload(input, ["a.pdf", "b.pdf"]);
  await screen.findByRole("alert");
  expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
  expect(screen.queryByText("private parser error")).not.toBeInTheDocument();
});
it("discloses replacement and submits the reviewed existing revision", async () => {
  const old = extractMonthlyMerrill(monthlyFixture()).snapshot;
  old.totalValue = "500";
  old.positionsComplete = false;
  vi.mocked(listStatements).mockResolvedValue([{ snapshot: old, revision: "old-revision" }]);
  const input = await open();
  upload(input);
  await screen.findByText(/An existing snapshot/);
  fireEvent.click(screen.getByRole("checkbox"));
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(saveStatements).toHaveBeenCalledOnce());
  expect(vi.mocked(saveStatements).mock.calls[0][1][0].expectedRevision).toBe("old-revision");
});
it("requires a separate confirmation before deleting saved observations", async () => {
  const snapshot = extractMonthlyMerrill(monthlyFixture()).snapshot;
  vi.mocked(listStatements).mockResolvedValue([{ snapshot, revision: "saved" }]);
  await open();
  fireEvent.click(screen.getByRole("button", { name: "Delete" }));
  expect(deleteStatement).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Confirm" }));
  await waitFor(() => expect(deleteStatement).toHaveBeenCalledWith("a", snapshot.date, "saved"));
});
it("stages five different monthly statements before saving the batch", async () => {
  const months = ["January", "February", "March", "April", "May"];
  vi.mocked(readBasisFile).mockImplementation(async (file, extract) => {
    const index = Number(file.name[0]);
    const pages = monthlyFixture();
    for (const page of pages)
      for (const item of page) {
        item.str = item.str
          .replaceAll("May", months[index])
          .replace("April 30", `${index === 0 ? "December" : months[index - 1]} 30`);
      }
    return extract!(pages);
  });
  const input = await open();
  upload(
    input,
    months.map((_, i) => `${i}.pdf`),
  );
  const save = await screen.findByRole("button", { name: "Save" });
  expect(readBasisFile).toHaveBeenCalledTimes(5);
  expect(saveStatements).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("checkbox"));
  fireEvent.click(save);
  await waitFor(() => expect(saveStatements).toHaveBeenCalledOnce());
  expect(vi.mocked(saveStatements).mock.calls[0][1]).toHaveLength(5);
});
it("rejects mixed institution accounts before retaining a batch", async () => {
  vi.mocked(readBasisFile)
    .mockImplementationOnce(async (_file, extract) => extract!(monthlyFixture()))
    .mockImplementationOnce(async (_file, extract) => {
      const pages = monthlyFixture();
      for (const page of pages)
        for (const item of page) item.str = item.str.replace("SYNTH-01", "OTHER-02");
      return extract!(pages);
    });
  const input = await open();
  upload(input, ["a.pdf", "b.pdf"]);
  await screen.findByRole("alert");
  expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
  expect(saveStatements).not.toHaveBeenCalled();
});
it("enforces cumulative PDF text limits across documents", async () => {
  vi.mocked(readBasisFile).mockImplementation(async (_file, extract) => {
    const pages = monthlyFixture();
    pages[0].push({ str: "x".repeat(600000), transform: [1, 0, 0, 1, 36, 100], width: 30 });
    return extract!(pages);
  });
  const input = await open();
  upload(input, ["a.pdf", "b.pdf"]);
  await screen.findByRole("alert");
  expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
  expect(saveStatements).not.toHaveBeenCalled();
});

it("keeps the reviewed revision when cached statements change during review", async () => {
  const old = extractMonthlyMerrill(monthlyFixture()).snapshot;
  old.totalValue = "500";
  old.positionsComplete = false;
  vi.mocked(listStatements).mockResolvedValue([{ snapshot: old, revision: "reviewed" }]);
  const input = await open();
  upload(input);
  await screen.findByText(/An existing snapshot/);
  await act(async () => {
    client.setQueryData(
      ["statementHistory", "a"],
      [{ snapshot: { ...old, totalValue: "490" }, revision: "concurrent" }],
    );
  });
  fireEvent.click(screen.getByRole("checkbox"));
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(saveStatements).toHaveBeenCalledOnce());
  expect(vi.mocked(saveStatements).mock.calls[0][1][0].expectedRevision).toBe("reviewed");
});
it("respects balance privacy for stored statement amounts", async () => {
  localStorage.setItem("privacy-settings", "true");
  vi.mocked(listStatements).mockResolvedValue([
    { snapshot: extractMonthlyMerrill(monthlyFixture()).snapshot, revision: "saved" },
  ]);
  await open();
  expect(screen.getAllByText("••••").length).toBeGreaterThan(0);
  expect(screen.queryByText("$510.00")).not.toBeInTheDocument();
});
it("shows reported position gains only when market value and basis are known", async () => {
  const snapshot = extractMonthlyMerrill(monthlyFixture()).snapshot;
  snapshot.positions[1].costBasis = null;
  vi.mocked(listStatements).mockResolvedValue([{ snapshot, revision: "saved" }]);
  await open();
  fireEvent.click(screen.getByText("Reported positions", { selector: "summary" }));
  expect(screen.getByText("$179.00")).toBeInTheDocument();
  const rows = screen.getAllByRole("row");
  expect(rows[2].textContent?.match(/—/g)).toHaveLength(2);
});
