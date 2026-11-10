import { cleanup, fireEvent, render, screen, waitFor, within } from "@/test/render";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { parseCsv } from "@/adapters";
import { readBasisFile } from "./basis-pdf";
import { PlaidInvestmentReview } from "./plaid-investment-review";
import type { PlaidInvestmentPreview } from "@/adapters/shared/plaid";
import { createDefaultParseConfig } from "@/pages/activity/import/utils/default-activity-template";
import { merrillCsvFixture } from "./test-fixtures/merrill-basis";

vi.mock("@/adapters", () => ({ parseCsv: vi.fn() }));
vi.mock("./basis-pdf", () => ({ readBasisFile: vi.fn(), MAX_BASIS_TEXT: 1_000_000 }));
const preview = (): PlaidInvestmentPreview => ({
  mappingId: "mapping",
  digest: "digest",
  initial: true,
  startDate: "2025-05-13",
  currency: "USD",
  openingCash: "100",
  currentCash: "100",
  openingPositions: [{ securityId: "a", name: "Synthetic equity", quantity: "2", unitCost: null }],
  records: [],
  issues: [],
  sourceTransactionCount: 0,
});
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(readBasisFile).mockResolvedValue("Symbol,Quantity,Cost basis\nABC,2,25");
  vi.mocked(parseCsv).mockResolvedValue({
    headers: ["Symbol", "Quantity", "Cost basis"],
    rows: [["ABC", "2", "25"]],
    rowCount: 1,
    errors: [],
    detectedConfig: createDefaultParseConfig(),
  });
});
afterEach(() => {
  cleanup();
  document.documentElement.dir = "ltr";
});
function renderReview(p = preview(), approve = vi.fn()) {
  render(<PlaidInvestmentReview preview={p} busy={false} onApprove={approve} onClose={vi.fn()} />);
  fireEvent.click(screen.getByText("Fill opening costs from a PDF or CSV"));
  fireEvent.change(screen.getByLabelText("Brokerage file format"), { target: { value: "manual" } });
  return approve;
}
async function upload() {
  fireEvent.change(screen.getByLabelText(/Statement PDF or transaction/), {
    target: { files: [new File(["synthetic"], "statement.pdf", { type: "application/pdf" })] },
  });
  await screen.findByLabelText("Review extracted table");
  fireEvent.click(screen.getByRole("button", { name: "Read table" }));
  await screen.findByLabelText("Source data");
}
async function calculate() {
  fireEvent.change(screen.getByLabelText("Verified end-of-day date"), {
    target: { value: "2025-05-12" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Calculate proposals" }));
  await screen.findByRole("combobox", { name: "Match ABC to an opening position" });
}
describe("document evidence in the existing Plaid review", () => {
  it("requires an explicit format selection before accepting a file", () => {
    render(
      <PlaidInvestmentReview
        preview={preview()}
        busy={false}
        onApprove={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByText("Fill opening costs from a PDF or CSV"));
    expect(screen.getByLabelText(/Statement PDF or transaction/)).toBeDisabled();
    fireEvent.change(screen.getByLabelText("Brokerage file format"), {
      target: { value: "merrill-cma-statement-v1" },
    });
    expect(screen.getByLabelText(/Statement PDF or transaction/)).toBeEnabled();
  });
  it.each(["ltr", "rtl"])(
    "uses the Merrill CSV mapping without guessing columns in %s",
    async (direction) => {
      document.documentElement.dir = direction;
      const approve = renderReview();
      const fixture = merrillCsvFixture();
      vi.mocked(parseCsv).mockResolvedValueOnce({
        ...fixture,
        errors: [],
        rowCount: 2,
        detectedConfig: createDefaultParseConfig(),
      });
      fireEvent.change(screen.getByLabelText("Brokerage file format"), {
        target: { value: "merrill-settled-activity-v1" },
      });
      fireEvent.change(screen.getByLabelText(/Statement PDF or transaction/), {
        target: { files: [new File(["synthetic"], "activity.csv")] },
      });
      await screen.findByLabelText("Review extracted table");
      expect(screen.getByLabelText("Review extracted table")).toBeDisabled();
      fireEvent.click(screen.getByRole("button", { name: "Read table" }));
      await screen.findByLabelText("Source data");
      expect(screen.getByLabelText("Source data")).toHaveValue("transactions");
      expect(screen.getByLabelText("Source data")).toBeDisabled();
      expect(screen.getByText(/Cash-only income excluded from cost basis: 1/)).toBeVisible();
      fireEvent.change(screen.getByLabelText("Verified end-of-day date"), {
        target: { value: "2025-05-12" },
      });
      fireEvent.click(screen.getByLabelText(/I verified complete security history/));
      fireEvent.click(screen.getByRole("button", { name: "Calculate proposals" }));
      fireEvent.change(screen.getByLabelText("Match DEMO to an opening position"), {
        target: { value: "a" },
      });
      fireEvent.click(screen.getByLabelText(/I verified the source account/));
      fireEvent.click(screen.getByRole("button", { name: "Fill reviewed opening costs" }));
      expect(screen.getByLabelText("Opening cost per unit Synthetic equity")).toHaveValue("10.5");
      expect(approve).not.toHaveBeenCalled();
    },
  );
  it("uses the PDF's statement date and fixed position mapping", async () => {
    renderReview();
    vi.mocked(parseCsv).mockResolvedValueOnce({
      headers: [
        "Security",
        "Quantity",
        "Total cost basis",
        "Date",
        "Currency",
        "Source page",
        "Source line",
      ],
      rows: [["DEMO", "2", "21", "2025-05-12", "USD", "3", "12"]],
      errors: [],
      rowCount: 1,
      detectedConfig: createDefaultParseConfig(),
    });
    fireEvent.change(screen.getByLabelText("Brokerage file format"), {
      target: { value: "merrill-cma-statement-v1" },
    });
    await upload();
    expect(readBasisFile).toHaveBeenCalledWith(expect.any(File), expect.any(Function));
    expect(screen.getByLabelText("Verified end-of-day date")).toHaveValue("2025-05-12");
    expect(screen.getByLabelText("Verified end-of-day date")).toBeDisabled();
    expect(screen.getByLabelText("Quantity")).toBeDisabled();
  });
  it("blocks the wrong brokerage schema without silently falling back to manual mapping", async () => {
    renderReview();
    fireEvent.change(screen.getByLabelText("Brokerage file format"), {
      target: { value: "merrill-settled-activity-v1" },
    });
    fireEvent.change(screen.getByLabelText(/Statement PDF or transaction/), {
      target: { files: [new File(["synthetic"], "unknown.csv")] },
    });
    await screen.findByLabelText("Review extracted table");
    fireEvent.click(screen.getByRole("button", { name: "Read table" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "does not match the selected brokerage format",
    );
    expect(screen.queryByRole("button", { name: "Calculate proposals" })).not.toBeInTheDocument();
    expect(screen.getByLabelText("Brokerage file format")).toHaveValue(
      "merrill-settled-activity-v1",
    );
  });
  it("keeps a monthly statement in memory and bridges it with a separate CSV before applying", async () => {
    renderReview();
    await upload();
    fireEvent.change(screen.getByLabelText("Verified end-of-day date"), {
      target: { value: "2025-04-30" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Calculate proposals" }));
    expect(screen.getByRole("button", { name: "Fill reviewed opening costs" })).toBeDisabled();
    fireEvent.click(
      screen.getByRole("button", { name: "Use statement, then upload intervening CSV" }),
    );
    expect(screen.getByText(/Starting statement: Apr 30, 2025/)).toBeVisible();
    vi.mocked(parseCsv).mockResolvedValueOnce({
      headers: ["Symbol", "Quantity", "Date", "Action", "Cost basis"],
      rows: [
        ["ABC", "1", "2025-05-01", "BUY", "20"],
        ["ABC", "1", "2025-05-02", "SELL", "12.5"],
      ],
      errors: [],
      rowCount: 2,
      detectedConfig: createDefaultParseConfig(),
    });
    await upload();
    expect(screen.getByLabelText("Source data")).toHaveValue("transactions");
    fireEvent.change(screen.getByLabelText("Verified end-of-day date"), {
      target: { value: "2025-05-12" },
    });
    fireEvent.click(screen.getByLabelText(/I verified complete security history/));
    fireEvent.click(screen.getByRole("button", { name: "Calculate proposals" }));
    fireEvent.change(screen.getByLabelText("Match ABC to an opening position"), {
      target: { value: "a" },
    });
    fireEvent.click(screen.getByLabelText(/I verified the source account/));
    fireEvent.click(screen.getByRole("button", { name: "Fill reviewed opening costs" }));
    expect(
      screen.getByRole("textbox", { name: "Opening cost per unit Synthetic equity" }),
    ).toHaveValue("16.25");
  });
  it.each(["ltr", "rtl"])(
    "fills reviewed costs but still requires final import approval in %s",
    async (direction) => {
      document.documentElement.dir = direction;
      const approve = renderReview();
      await upload();
      await calculate();
      const apply = screen.getByRole("button", { name: "Fill reviewed opening costs" });
      expect(apply).toBeDisabled();
      fireEvent.change(screen.getByLabelText("Match ABC to an opening position"), {
        target: { value: "a" },
      });
      expect(apply).toBeDisabled();
      fireEvent.click(screen.getByLabelText(/I verified the source account/));
      expect(apply).toBeEnabled();
      fireEvent.click(apply);
      expect(
        screen.getByRole("textbox", { name: "Opening cost per unit Synthetic equity" }),
      ).toHaveValue("12.5");
      expect(approve).not.toHaveBeenCalled();
      expect(screen.getByRole("button", { name: "Confirm" })).toBeDisabled();
      fireEvent.click(screen.getByLabelText(/I reviewed the opening balances/));
      fireEvent.click(screen.getByRole("button", { name: "Confirm" }));
      expect(approve).toHaveBeenCalledExactlyOnceWith({ a: "12.5" });
      expect(parseCsv).toHaveBeenCalledOnce();
    },
  );
  it("requires exact quantity reconciliation and explicit identity mapping", async () => {
    const p = preview();
    p.openingPositions[0].quantity = "3";
    renderReview(p);
    await upload();
    await calculate();
    expect(
      within(screen.getByLabelText("Match ABC to an opening position")).getByRole("option", {
        name: /Synthetic equity/,
      }),
    ).toBeDisabled();
    fireEvent.click(screen.getByLabelText(/I verified the source account/));
    expect(screen.getByRole("button", { name: "Fill reviewed opening costs" })).toBeDisabled();
  });
  it("invalidates proposals and attestation when the table, columns or date changes", async () => {
    renderReview();
    await upload();
    await calculate();
    fireEvent.change(screen.getByLabelText("Verified end-of-day date"), {
      target: { value: "2025-05-14" },
    });
    expect(
      screen.queryByRole("button", { name: "Fill reviewed opening costs" }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Calculate proposals" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Use a statement on or before");
    fireEvent.change(screen.getByLabelText("Review extracted table"), {
      target: { value: "changed" },
    });
    expect(screen.queryByLabelText("Source data")).not.toBeInTheDocument();
  });
  it("does not replace unrelated manual costs and resets an earlier final approval", async () => {
    const p = preview();
    p.openingPositions.push({
      securityId: "b",
      name: "Other equity",
      quantity: "4",
      unitCost: null,
    });
    renderReview(p);
    fireEvent.change(screen.getByRole("textbox", { name: "Opening cost per unit Other equity" }), {
      target: { value: "9" },
    });
    fireEvent.click(screen.getByLabelText(/I reviewed the opening balances/));
    await upload();
    await calculate();
    fireEvent.change(screen.getByLabelText("Match ABC to an opening position"), {
      target: { value: "a" },
    });
    fireEvent.click(screen.getByLabelText(/I verified the source account/));
    fireEvent.click(screen.getByRole("button", { name: "Fill reviewed opening costs" }));
    expect(screen.getByRole("textbox", { name: "Opening cost per unit Other equity" })).toHaveValue(
      "9",
    );
    expect(screen.getByLabelText(/I reviewed the opening balances/)).not.toBeChecked();
  });
  it("blocks duplicate assignments even with identical quantities", async () => {
    vi.mocked(parseCsv).mockResolvedValueOnce({
      headers: ["Symbol", "Quantity", "Cost basis"],
      rows: [
        ["ABC", "2", "25"],
        ["XYZ", "2", "30"],
      ],
      rowCount: 2,
      errors: [],
      detectedConfig: createDefaultParseConfig(),
    });
    renderReview();
    await upload();
    await calculate();
    for (const symbol of ["ABC", "XYZ"])
      fireEvent.change(screen.getByLabelText(`Match ${symbol} to an opening position`), {
        target: { value: "a" },
      });
    fireEvent.click(screen.getByLabelText(/I verified the source account/));
    expect(screen.getByRole("button", { name: "Fill reviewed opening costs" })).toBeDisabled();
  });
  it("does not expose raw parsing errors or partially accept parser warnings", async () => {
    vi.mocked(parseCsv).mockRejectedValueOnce(new Error("private document content"));
    renderReview();
    fireEvent.change(screen.getByLabelText(/Statement PDF or transaction/), {
      target: { files: [new File(["text"], "input.csv")] },
    });
    await screen.findByLabelText("Review extracted table");
    fireEvent.click(screen.getByRole("button", { name: "Read table" }));
    await screen.findByRole("alert");
    expect(screen.getByRole("alert")).toHaveTextContent("table could not be parsed");
    expect(screen.queryByText(/private document content/)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Calculate proposals" })).not.toBeInTheDocument();
  });
  it("uses skipTopRows rather than retaining statement prose as data", async () => {
    renderReview();
    fireEvent.change(screen.getByLabelText(/Statement PDF or transaction/), {
      target: { files: [new File(["text"], "input.csv")] },
    });
    await screen.findByLabelText("Review extracted table");
    fireEvent.change(screen.getByLabelText("Header row (starting at 1)"), {
      target: { value: "3" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Read table" }));
    await waitFor(() =>
      expect(parseCsv).toHaveBeenCalledWith(
        expect.any(File),
        expect.objectContaining({ skipTopRows: 2, headerRowIndex: 0 }),
      ),
    );
  });
  it("does not offer evidence uploads for an already committed baseline", () => {
    const p = preview();
    p.initial = false;
    p.openingPositions[0].unitCost = "10";
    render(
      <PlaidInvestmentReview preview={p} busy={false} onApprove={vi.fn()} onClose={vi.fn()} />,
    );
    expect(screen.queryByText("Fill opening costs from a PDF or CSV")).not.toBeInTheDocument();
  });
});
