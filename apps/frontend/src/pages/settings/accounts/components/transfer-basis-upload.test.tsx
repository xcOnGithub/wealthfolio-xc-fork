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
async function propose() {
  const view = within(panel());
  fireEvent.change(await view.findByLabelText("Incoming transfer to resolve"), {
    target: { value: "incoming" },
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
