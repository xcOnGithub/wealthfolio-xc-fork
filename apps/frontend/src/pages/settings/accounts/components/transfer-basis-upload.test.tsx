import { act, cleanup, fireEvent, render, screen, within } from "@/test/render";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { readTransferStatement } from "./transfer-basis";
import { PlaidInvestmentReview } from "./plaid-investment-review";
import { transferPreview, transferStatement } from "./test-fixtures/transfer-basis";
import { BasisError } from "./opening-basis";
import type { PlaidInvestmentPreview } from "@/adapters/shared/plaid";

vi.mock("./transfer-basis", async (original) => ({
  ...(await original<typeof import("./transfer-basis")>()),
  readTransferStatement: vi.fn(),
}));
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(readTransferStatement).mockResolvedValue(transferStatement());
});
afterEach(() => {
  cleanup();
  document.documentElement.dir = "ltr";
});
function panel() {
  return screen.getByText("Fill transfer costs from a statement").parentElement!;
}
function upload() {
  const view = within(panel());
  fireEvent.click(view.getByText("Fill transfer costs from a statement"));
  fireEvent.change(view.getByLabelText("Brokerage file format"), {
    target: { value: "merrill-cma-statement-v1" },
  });
  fireEvent.change(view.getByLabelText("Transfer-cost statement PDF"), {
    target: { files: [new File(["synthetic"], "october.pdf")] },
  });
}
async function propose(transferId = "incoming") {
  const view = within(panel());
  fireEvent.change(await view.findByLabelText("Incoming transfer to resolve"), {
    target: { value: transferId },
  });
  fireEvent.change(view.getByLabelText("Statement holding for the same security"), {
    target: { value: "DEMO" },
  });
  fireEvent.click(view.getByRole("button", { name: "Calculate proposals" }));
}
function renderReview(p: PlaidInvestmentPreview = transferPreview()) {
  const approve = vi.fn();
  const view = render(
    <PlaidInvestmentReview preview={p} busy={false} onApprove={approve} onClose={vi.fn()} />,
  );
  return { approve, ...view };
}
it.each(["ltr", "rtl"])(
  "fills a reviewed October transfer, then requires separate ledger approval in %s",
  async (direction) => {
    document.documentElement.dir = direction;
    const { approve } = renderReview();
    upload();
    await propose();
    const view = within(panel());
    expect(view.getByText(/Proposed transfer basis: USD 100; cost per unit: 25/)).toBeVisible();
    expect(view.getByText("Statement page 2, line 3")).toBeVisible();
    const apply = view.getByRole("button", { name: "Fill reviewed transfer cost" });
    expect(apply).toBeDisabled();
    fireEvent.click(view.getByRole("checkbox", { name: /I verified the same source account/ }));
    fireEvent.click(apply);
    expect(screen.getByRole("textbox", { name: /Verified incoming-transfer cost/ })).toHaveValue(
      "25",
    );
    expect(approve).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Confirm" })).toBeDisabled();
    fireEvent.click(screen.getByRole("checkbox", { name: /I reviewed/ }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));
    expect(approve).toHaveBeenCalledExactlyOnceWith({}, { incoming: "25" });
  },
);
it("preserves a manually entered cost and invalidates evidence confirmation when costs change", async () => {
  renderReview();
  upload();
  await propose();
  const view = within(panel());
  fireEvent.click(view.getByRole("checkbox", { name: /I verified the same source account/ }));
  const field = screen.getByRole("textbox", { name: /Verified incoming-transfer cost/ });
  fireEvent.change(field, { target: { value: "30" } });
  expect(
    view.queryByRole("button", { name: "Fill reviewed transfer cost" }),
  ).not.toBeInTheDocument();
  fireEvent.click(view.getByRole("button", { name: "Calculate proposals" }));
  fireEvent.click(view.getByRole("checkbox", { name: /I verified the same source account/ }));
  expect(view.getByRole("button", { name: "Fill reviewed transfer cost" })).toBeDisabled();
  expect(field).toHaveValue("30");
});
it("cannot use an October statement for a December transfer", async () => {
  const p = transferPreview();
  p.records[0].date = "2025-12-11";
  renderReview(p);
  upload();
  await propose();
  expect(within(panel()).getByRole("alert")).toHaveTextContent(
    "statement must be on or after this transfer",
  );
  expect(
    screen.queryByRole("button", { name: "Fill reviewed transfer cost" }),
  ).not.toBeInTheDocument();
});
it("requires actual disposed basis and recalculation after editing it", async () => {
  const p = transferPreview();
  p.records.push({
    ...p.records[0],
    id: "sale",
    date: "2025-10-20",
    type: "SELL",
    quantity: "1",
    cash: "999",
    requiresTransferCost: false,
  });
  const statement = transferStatement();
  statement.candidates[0].quantity = "3";
  statement.candidates[0].totalCost = "75";
  vi.mocked(readTransferStatement).mockResolvedValue(statement);
  renderReview(p);
  upload();
  await propose();
  const view = within(panel());
  expect(view.getByRole("alert")).toHaveTextContent("basis removed by each disposition");
  fireEvent.change(view.getByRole("textbox", { name: /Verified total basis removed/ }), {
    target: { value: "25" },
  });
  fireEvent.click(view.getByRole("button", { name: "Calculate proposals" }));
  fireEvent.click(view.getByRole("checkbox", { name: /I verified the same source account/ }));
  expect(view.getByRole("button", { name: "Fill reviewed transfer cost" })).toBeEnabled();
  fireEvent.change(view.getByRole("textbox", { name: /Verified total basis removed/ }), {
    target: { value: "20" },
  });
  expect(
    view.queryByRole("button", { name: "Fill reviewed transfer cost" }),
  ).not.toBeInTheDocument();
});
it("clears evidence when the exclusion policy changes and reports safe parser errors", async () => {
  renderReview();
  upload();
  await propose();
  const view = within(panel());
  fireEvent.click(view.getByRole("checkbox", { name: /Exclude statement entries/ }));
  expect(view.queryByLabelText("Incoming transfer to resolve")).not.toBeInTheDocument();
  vi.mocked(readTransferStatement).mockRejectedValueOnce(new Error("sensitive parser contents"));
  fireEvent.change(view.getByLabelText("Transfer-cost statement PDF"), {
    target: { files: [new File(["x"], "bad.pdf")] },
  });
  expect(await view.findByRole("alert")).not.toHaveTextContent("sensitive");
});
it("discards an in-flight document after the preview digest changes", async () => {
  let finish!: (value: ReturnType<typeof transferStatement>) => void;
  vi.mocked(readTransferStatement).mockReturnValueOnce(
    new Promise((resolve) => {
      finish = resolve;
    }),
  );
  const p = transferPreview();
  const { rerender } = renderReview(p);
  upload();
  rerender(
    <PlaidInvestmentReview
      preview={{ ...p, digest: "new" }}
      busy={false}
      onApprove={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  await act(async () => {
    finish(transferStatement());
    await Promise.resolve();
  });
  expect(within(panel()).queryByLabelText("Incoming transfer to resolve")).not.toBeInTheDocument();
});
it("rejects an unsupported PDF without leaking the original exception", async () => {
  vi.mocked(readTransferStatement).mockRejectedValueOnce(new BasisError("brokerageFormat"));
  renderReview();
  upload();
  expect(await within(panel()).findByRole("alert")).toHaveTextContent(
    "does not match the selected brokerage",
  );
});

function fillTransferProposal() {
  const view = within(panel());
  fireEvent.click(view.getByRole("checkbox", { name: /I verified the same source account/ }));
  fireEvent.click(view.getByRole("button", { name: "Fill reviewed transfer cost" }));
}
it.each(["ltr", "rtl"])(
  "invalidates a filled transfer after its opening cost changes in %s",
  async (direction) => {
    document.documentElement.dir = direction;
    const p = transferPreview();
    p.openingPositions = [
      { securityId: "demo", name: "Synthetic equity", quantity: "2", unitCost: null },
    ];
    const statement = transferStatement();
    statement.candidates[0].quantity = "6";
    statement.candidates[0].totalCost = "120";
    vi.mocked(readTransferStatement).mockResolvedValue(statement);
    const { approve } = renderReview(p);
    const opening = screen.getByRole("textbox", { name: "Opening cost per unit Synthetic equity" });
    fireEvent.change(opening, { target: { value: "10" } });
    upload();
    await propose();
    fillTransferProposal();
    const transfer = screen.getByRole("textbox", { name: /Verified incoming-transfer cost/ });
    expect(transfer).toHaveValue("25");
    fireEvent.click(screen.getByRole("checkbox", { name: /I reviewed/ }));
    fireEvent.change(opening, { target: { value: "20" } });
    expect(transfer).toHaveValue("");
    const confirm = screen.getByRole("button", { name: "Confirm" });
    fireEvent.click(screen.getByRole("checkbox", { name: /I reviewed/ }));
    expect(confirm).toBeDisabled();
    fireEvent.click(confirm);
    expect(approve).not.toHaveBeenCalled();
    await propose();
    fillTransferProposal();
    expect(transfer).toHaveValue("20");
    fireEvent.click(screen.getByRole("checkbox", { name: /I reviewed/ }));
    fireEvent.click(confirm);
    expect(approve).toHaveBeenCalledExactlyOnceWith({ demo: "20" }, { incoming: "20" });
  },
);
it("invalidates a fill after another incoming transfer cost changes, preserving manual and committed costs", async () => {
  const p = transferPreview();
  p.records.push(
    { ...p.records[0], id: "other", quantity: "2" },
    {
      ...p.records[0],
      id: "committed",
      securityId: "unrelated",
      quantity: "1",
      transferUnitCost: "9",
    },
  );
  const statement = transferStatement();
  statement.candidates[0].quantity = "6";
  statement.candidates[0].totalCost = "120";
  vi.mocked(readTransferStatement).mockResolvedValue(statement);
  const { approve } = renderReview(p);
  const other = screen.getByRole("textbox", { name: / other$/ });
  fireEvent.change(other, { target: { value: "10" } });
  upload();
  await propose();
  fillTransferProposal();
  const target = screen.getByRole("textbox", { name: / incoming$/ });
  expect(target).toHaveValue("25");
  fireEvent.change(other, { target: { value: "20" } });
  expect(target).toHaveValue("");
  expect(other).toHaveValue("20");
  expect(screen.getByRole("textbox", { name: / committed$/ })).toHaveValue("9");
  expect(screen.getByRole("textbox", { name: / committed$/ })).toBeDisabled();
  fireEvent.click(screen.getByRole("checkbox", { name: /I reviewed/ }));
  expect(screen.getByRole("button", { name: "Confirm" })).toBeDisabled();
  expect(approve).not.toHaveBeenCalled();
  await propose();
  fillTransferProposal();
  expect(target).toHaveValue("20");
});
it("preserves an explicit manual replacement of a helper-filled cost", async () => {
  const p = transferPreview();
  p.openingPositions = [
    { securityId: "demo", name: "Synthetic equity", quantity: "2", unitCost: null },
  ];
  const statement = transferStatement();
  statement.candidates[0].quantity = "6";
  statement.candidates[0].totalCost = "120";
  vi.mocked(readTransferStatement).mockResolvedValue(statement);
  renderReview(p);
  const opening = screen.getByRole("textbox", { name: "Opening cost per unit Synthetic equity" });
  fireEvent.change(opening, { target: { value: "10" } });
  upload();
  await propose();
  fillTransferProposal();
  const target = screen.getByRole("textbox", { name: / incoming$/ });
  fireEvent.change(target, { target: { value: "30" } });
  fireEvent.change(opening, { target: { value: "20" } });
  expect(target).toHaveValue("30");
});
it("clears dependent fills from successive statements when their opening source changes", async () => {
  const p = transferPreview();
  p.openingPositions = [
    { securityId: "demo", name: "Synthetic equity", quantity: "2", unitCost: null },
  ];
  p.records.push({ ...p.records[0], id: "later", date: "2025-11-10", quantity: "2" });
  const october = transferStatement();
  october.candidates[0].quantity = "6";
  october.candidates[0].totalCost = "120";
  vi.mocked(readTransferStatement).mockResolvedValueOnce(october);
  renderReview(p);
  const opening = screen.getByRole("textbox", { name: "Opening cost per unit Synthetic equity" });
  fireEvent.change(opening, { target: { value: "10" } });
  upload();
  await propose();
  fillTransferProposal();
  expect(screen.getByRole("textbox", { name: / incoming$/ })).toHaveValue("25");
  const november = transferStatement();
  november.asOf = "2025-11-30";
  november.candidates[0].quantity = "8";
  november.candidates[0].totalCost = "160";
  vi.mocked(readTransferStatement).mockResolvedValueOnce(november);
  fireEvent.change(within(panel()).getByLabelText("Transfer-cost statement PDF"), {
    target: { files: [new File(["synthetic"], "november.pdf")] },
  });
  await propose("later");
  fillTransferProposal();
  expect(screen.getByRole("textbox", { name: / later$/ })).toHaveValue("20");
  fireEvent.change(opening, { target: { value: "20" } });
  expect(screen.getByRole("textbox", { name: / incoming$/ })).toHaveValue("");
  expect(screen.getByRole("textbox", { name: / later$/ })).toHaveValue("");
});
it.each(["ltr", "rtl"])(
  "invalidates filled costs after disposed-basis evidence changes in %s",
  async (direction) => {
    document.documentElement.dir = direction;
    const p = transferPreview();
    p.records.push({
      ...p.records[0],
      id: "sale",
      date: "2025-10-20",
      type: "SELL",
      quantity: "1",
      cash: "999",
      requiresTransferCost: false,
    });
    const statement = transferStatement();
    statement.candidates[0].quantity = "3";
    statement.candidates[0].totalCost = "75";
    vi.mocked(readTransferStatement).mockResolvedValue(statement);
    const { approve } = renderReview(p);
    upload();
    await propose();
    const view = within(panel());
    const disposed = view.getByRole("textbox", { name: /Verified total basis removed/ });
    fireEvent.change(disposed, { target: { value: "25" } });
    fireEvent.click(view.getByRole("button", { name: "Calculate proposals" }));
    fillTransferProposal();
    const target = screen.getByRole("textbox", { name: / incoming$/ });
    expect(target).toHaveValue("25");
    fireEvent.click(screen.getByRole("checkbox", { name: /I reviewed/ }));
    expect(screen.getByRole("button", { name: "Confirm" })).toBeEnabled();
    fireEvent.change(disposed, { target: { value: "20" } });
    expect(target).toHaveValue("");
    expect(screen.getByRole("checkbox", { name: /I reviewed/ })).not.toBeChecked();
    fireEvent.click(screen.getByRole("checkbox", { name: /I reviewed/ }));
    const confirm = screen.getByRole("button", { name: "Confirm" });
    expect(confirm).toBeDisabled();
    fireEvent.click(confirm);
    expect(approve).not.toHaveBeenCalled();
    fireEvent.click(view.getByRole("button", { name: "Calculate proposals" }));
    fillTransferProposal();
    expect(target).toHaveValue("23.75");
    fireEvent.click(screen.getByRole("checkbox", { name: /I reviewed/ }));
    fireEvent.click(confirm);
    expect(approve).toHaveBeenCalledExactlyOnceWith({}, { incoming: "23.75" });
  },
);
it("preserves manual replacements and committed costs after disposed-basis edits", async () => {
  const p = transferPreview();
  p.records.push(
    {
      ...p.records[0],
      id: "sale",
      date: "2025-10-20",
      type: "SELL",
      quantity: "1",
      requiresTransferCost: false,
    },
    { ...p.records[0], id: "committed", securityId: "other", transferUnitCost: "9" },
  );
  const statement = transferStatement();
  statement.candidates[0].quantity = "3";
  statement.candidates[0].totalCost = "75";
  vi.mocked(readTransferStatement).mockResolvedValue(statement);
  renderReview(p);
  upload();
  await propose();
  const view = within(panel());
  const disposed = view.getByRole("textbox", { name: /Verified total basis removed/ });
  fireEvent.change(disposed, { target: { value: "25" } });
  fireEvent.click(view.getByRole("button", { name: "Calculate proposals" }));
  fillTransferProposal();
  const target = screen.getByRole("textbox", { name: / incoming$/ });
  expect(target).toHaveValue("25");
  fireEvent.change(target, { target: { value: "24" } });
  fireEvent.click(screen.getByRole("checkbox", { name: /I reviewed/ }));
  fireEvent.change(disposed, { target: { value: "20" } });
  expect(target).toHaveValue("24");
  expect(screen.getByRole("textbox", { name: / committed$/ })).toHaveValue("9");
  expect(screen.getByRole("textbox", { name: / committed$/ })).toBeDisabled();
  expect(screen.getByRole("checkbox", { name: /I reviewed/ })).not.toBeChecked();
});
it("shares disposed-basis evidence across statements and invalidates dependent fills", async () => {
  const p = transferPreview();
  p.records.push(
    {
      ...p.records[0],
      id: "sale",
      date: "2025-10-20",
      type: "SELL",
      quantity: "1",
      requiresTransferCost: false,
    },
    { ...p.records[0], id: "later", date: "2025-11-10", quantity: "2" },
  );
  const october = transferStatement();
  october.candidates[0].quantity = "3";
  october.candidates[0].totalCost = "75";
  vi.mocked(readTransferStatement).mockResolvedValueOnce(october);
  renderReview(p);
  upload();
  await propose();
  const view = within(panel());
  fireEvent.change(view.getByRole("textbox", { name: /Verified total basis removed/ }), {
    target: { value: "25" },
  });
  fireEvent.click(view.getByRole("button", { name: "Calculate proposals" }));
  fillTransferProposal();
  expect(screen.getByRole("textbox", { name: / incoming$/ })).toHaveValue("25");
  const november = transferStatement();
  november.asOf = "2025-11-30";
  november.candidates[0].quantity = "5";
  november.candidates[0].totalCost = "115";
  vi.mocked(readTransferStatement).mockResolvedValueOnce(november);
  fireEvent.change(view.getByLabelText("Transfer-cost statement PDF"), {
    target: { files: [new File(["synthetic"], "november.pdf")] },
  });
  await propose("later");
  const disposed = view.getByRole("textbox", { name: /Verified total basis removed/ });
  expect(disposed).toHaveValue("25");
  fillTransferProposal();
  expect(screen.getByRole("textbox", { name: / later$/ })).toHaveValue("20");
  fireEvent.click(screen.getByRole("checkbox", { name: /I reviewed/ }));
  fireEvent.change(disposed, { target: { value: "20" } });
  expect(screen.getByRole("textbox", { name: / incoming$/ })).toHaveValue("");
  expect(screen.getByRole("textbox", { name: / later$/ })).toHaveValue("");
  expect(screen.getByRole("checkbox", { name: /I reviewed/ })).not.toBeChecked();
  fireEvent.click(screen.getByRole("checkbox", { name: /I reviewed/ }));
  expect(screen.getByRole("button", { name: "Confirm" })).toBeDisabled();
});
it.each(["ltr", "rtl"])(
  "shows the actual blocking preview issues beside the uploader error in %s",
  async (direction) => {
    document.documentElement.dir = direction;
    const p = transferPreview();
    p.issues = [
      { recordId: "bad-trade", name: "Synthetic record", reason: "tradeValues" },
      { recordId: "account", reason: "reconciliation" },
      { recordId: "future", name: "Synthetic future issue", reason: "futureReason" },
    ];
    const { approve } = renderReview(p);
    upload();
    await propose();
    const alert = within(panel()).getByRole("alert");
    expect(alert).toHaveTextContent(
      "Resolve the investment-preview issues before calculating transfer costs",
    );
    expect(alert).toHaveTextContent("Synthetic record");
    expect(alert).toHaveTextContent("Trade quantity is included in the opening-position preview");
    expect(alert).toHaveTextContent("Account: The ledger does not match current holdings or cash");
    expect(alert).toHaveTextContent("futureReason");
    expect(within(alert).getAllByRole("listitem")).toHaveLength(3);
    expect(
      within(panel()).queryByRole("button", { name: "Fill reviewed transfer cost" }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("checkbox", { name: /I reviewed/ }));
    const confirm = screen.getByRole("button", { name: "Confirm" });
    expect(confirm).toBeDisabled();
    fireEvent.click(confirm);
    expect(approve).not.toHaveBeenCalled();
  },
);
it.each(["ltr", "rtl"])(
  "proposes a transfer despite explicitly scoped issues on other securities, while keeping import blocked in %s",
  async (direction) => {
    document.documentElement.dir = direction;
    const p = transferPreview();
    p.issues = [
      {
        recordId: "cash-holding",
        securityId: "cash",
        name: "Synthetic sweep",
        reason: "unsupportedInstrument",
      },
      {
        recordId: "other-trade",
        securityId: "treasury-etf",
        name: "Synthetic ETF",
        reason: "tradeValues",
      },
    ];
    const { approve } = renderReview(p);
    upload();
    await propose();
    const view = within(panel());
    expect(view.getByText(/Proposed transfer basis: USD 100; cost per unit: 25/)).toBeVisible();
    fillTransferProposal();
    expect(screen.getByRole("textbox", { name: / incoming$/ })).toHaveValue("25");
    fireEvent.click(screen.getByRole("checkbox", { name: /I reviewed/ }));
    const confirm = screen.getByRole("button", { name: "Confirm" });
    expect(confirm).toBeDisabled();
    fireEvent.click(confirm);
    expect(approve).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent("Synthetic sweep");
  },
);
