import { beforeEach, describe, expect, it, vi } from "vitest";
import { calculateTransferBasis, readTransferStatement } from "./transfer-basis";
import { BasisError, calculateBasis } from "./opening-basis";
import { parseCsv } from "@/adapters";
import { readBasisFile } from "./basis-pdf";
import { createDefaultParseConfig } from "@/pages/activity/import/utils/default-activity-template";
import { BROKERAGE_BASIS_FORMATS } from "./brokerage-basis-formats";
import { merrillPdfFixture } from "./test-fixtures/merrill-basis";

vi.mock("@/adapters", () => ({ parseCsv: vi.fn() }));
vi.mock("./basis-pdf", async (original) => ({
  ...(await original<typeof import("./basis-pdf")>()),
  readBasisFile: vi.fn(),
}));

import { transferPreview, transferStatement } from "./test-fixtures/transfer-basis";

function setup() {
  const statement = transferStatement();
  return {
    preview: transferPreview(),
    statement,
    candidate: statement.candidates[0],
    transferId: "incoming",
    openingCosts: {} as Record<string, string>,
    transferCosts: {} as Record<string, string>,
    disposedCosts: {} as Record<string, string>,
  };
}
function rejects(input: ReturnType<typeof setup>, code: string) {
  expect(() => calculateTransferBasis(input)).toThrowError(new BasisError(code));
}
describe("statement reconciliation for one incoming transfer", () => {
  it("accepts an October checkpoint without treating transfer market value or fees as basis", () => {
    expect(calculateTransferBasis(setup())).toMatchObject({
      totalCost: "100",
      unitCost: "25",
      prior: "0",
      disposed: "0",
    });
  });
  it("subtracts an existing position, net purchase cost including fees, and reviewed incoming basis", () => {
    const input = setup();
    input.preview.openingPositions = [
      { securityId: "demo", name: "Synthetic equity", quantity: "2", unitCost: null },
    ];
    input.openingCosts.demo = "10";
    input.preview.records.push({
      ...input.preview.records[0],
      id: "buy",
      date: "2025-06-01",
      type: "BUY",
      quantity: "1",
      cash: "-13",
      fee: "1",
      price: "12",
      requiresTransferCost: false,
    });
    input.preview.records.push({
      ...input.preview.records[0],
      id: "reviewed",
      quantity: "2",
      transferUnitCost: "5",
    });
    input.candidate.quantity = "9";
    input.candidate.totalCost = "143";
    expect(calculateTransferBasis(input)).toMatchObject({
      prior: "20",
      purchases: "13",
      otherTransfers: "10",
      totalCost: "100",
      unitCost: "25",
    });
  });
  it.each(["SELL", "TRANSFER_OUT"])(
    "requires actual disposed basis for %s; never substitutes proceeds",
    (type) => {
      const input = setup();
      input.preview.records.push({
        ...input.preview.records[0],
        id: "disposed",
        date: "2025-10-20",
        type,
        quantity: "1",
        cash: "999",
        requiresTransferCost: false,
      });
      input.candidate.quantity = "3";
      input.candidate.totalCost = "75";
      rejects(input, "missingCost");
      input.disposedCosts.disposed = "25";
      expect(calculateTransferBasis(input).unitCost).toBe("25");
    },
  );
  it("does not infer a missing opening basis", () => {
    const input = setup();
    input.preview.openingPositions = [
      { securityId: "demo", name: "Synthetic", quantity: "2", unitCost: null },
    ];
    rejects(input, "missingCost");
  });
  it("cannot split a residual across multiple unreviewed transfers", () => {
    const input = setup();
    input.preview.records.push({ ...input.preview.records[0], id: "other", quantity: "1" });
    input.candidate.quantity = "5";
    rejects(input, "ambiguousTransfers");
    input.transferCosts.other = "0";
    expect(calculateTransferBasis(input).unitCost).toBe("25");
  });
  it("ignores December activity for an October statement and cannot resolve a December transfer", () => {
    const input = setup();
    input.preview.records.push({
      ...input.preview.records[0],
      id: "december",
      date: "2025-12-11",
      quantity: "120",
    });
    expect(calculateTransferBasis(input).unitCost).toBe("25");
    input.transferId = "december";
    rejects(input, "transferDate");
  });
  it("rejects quantity mismatches, unsupported records, and a blocked preview", () => {
    const input = setup();
    input.candidate.quantity = "5";
    rejects(input, "transferQuantity");
    input.candidate.quantity = "4";
    input.preview.records.push({ ...input.preview.records[0], id: "split", type: "SPLIT" });
    rejects(input, "action");
    input.preview.issues = [{ recordId: "split", reason: "unsupportedTransaction" }];
    rejects(input, "previewIssues");
  });
  it("retains the existing bound on explicitly excluded fractional shares and preserves total basis", () => {
    const input = setup();
    input.candidate.quantity = "3.9999";
    rejects(input, "transferQuantity");
    input.statement.exclusions = [{ label: "DEMO", quantity: "0.0001", page: 1, line: 4 }];
    expect(calculateTransferBasis(input)).toMatchObject({
      totalCost: "100",
      unitCost: "25",
      excludedQuantity: "0.0001",
    });
    input.statement.exclusions[0].label = "OTHER";
    rejects(input, "transferQuantity");
  });
  it("rejects negative residual basis, duplicate source records and invalid dates", () => {
    const input = setup();
    input.preview.openingPositions = [
      { securityId: "demo", name: "Synthetic", quantity: "1", unitCost: "200" },
    ];
    input.candidate.quantity = "5";
    rejects(input, "incomplete");
    input.preview.openingPositions = [];
    input.preview.records.push({ ...input.preview.records[0] });
    rejects(input, "duplicate");
    input.statement.asOf = "2025-02-30";
    rejects(input, "date");
  });
  it("does not let a later transfer fund basis removed by an earlier sale", () => {
    const input = setup();
    input.preview.openingPositions = [
      { securityId: "demo", name: "Synthetic", quantity: "1", unitCost: "20" },
    ];
    input.preview.records.push({
      ...input.preview.records[0],
      id: "sale",
      date: "2025-07-01",
      type: "SELL",
      quantity: "1",
      cash: "200",
      requiresTransferCost: false,
    });
    input.disposedCosts.sale = "30";
    rejects(input, "incomplete");
    input.disposedCosts.sale = "10";
    rejects(input, "incomplete");
    input.disposedCosts.sale = "20";
    expect(calculateTransferBasis(input).unitCost).toBe("25");
  });
  it("bounds a fully disposed position's cost rounding by its 18-place representation", () => {
    const input = setup();
    input.preview.openingPositions = [
      { securityId: "demo", name: "Synthetic", quantity: "3", unitCost: "0.333333333333333333" },
    ];
    input.preview.records.push({
      ...input.preview.records[0],
      id: "sale",
      date: "2025-07-01",
      type: "SELL",
      quantity: "3",
      requiresTransferCost: false,
    });
    input.disposedCosts.sale = "1";
    expect(calculateTransferBasis(input).unitCost).toBe("25");
    input.disposedCosts.sale = "1.01";
    rejects(input, "incomplete");
  });
  it("cannot overwrite reviewed transfer basis and uses the committed opening cost", () => {
    const input = setup();
    input.preview.initial = false;
    input.preview.openingPositions = [
      { securityId: "demo", name: "Synthetic", quantity: "1", unitCost: "20" },
    ];
    input.openingCosts.demo = "999";
    input.candidate.quantity = "5";
    expect(calculateTransferBasis(input).unitCost).toBe("20");
    input.preview.records[0].transferUnitCost = "20";
    rejects(input, "transferTarget");
  });
  it.each(["-1", "Infinity", "1e3", " ", "0.0000000000000000001"])(
    "rejects invalid verified basis %s",
    (value) => {
      const input = setup();
      input.preview.openingPositions = [
        { securityId: "demo", name: "Synthetic", quantity: "1", unitCost: null },
      ];
      input.openingCosts.demo = value;
      expect(() => calculateTransferBasis(input)).toThrow(BasisError);
    },
  );
  it("accepts explicitly verified zero transferred basis and maintains decimal precision", () => {
    const input = setup();
    input.candidate.totalCost = "0";
    expect(calculateTransferBasis(input).unitCost).toBe("0");
    input.candidate.totalCost = "1";
    input.preview.records[0].quantity = "3";
    input.candidate.quantity = "3";
    expect(calculateTransferBasis(input).unitCost).toBe("0.333333333333333333");
  });
  it("does not borrow another security's opening costs and ignores cash income", () => {
    const input = setup();
    input.preview.openingPositions = [
      { securityId: "other", name: "Other", quantity: "999", unitCost: null },
    ];
    input.preview.records.push({
      ...input.preview.records[0],
      id: "income",
      type: "DIVIDEND",
      quantity: "0",
      cash: "999",
    });
    expect(calculateTransferBasis(input).unitCost).toBe("25");
  });
});

describe("later statement admission", () => {
  beforeEach(() => {
    vi.resetAllMocks();
  });
  it("reuses the Merrill adapter while leaving the opening-cutoff check intact", async () => {
    vi.mocked(readBasisFile).mockImplementation((_file, extract) => {
      const pages = merrillPdfFixture();
      for (const item of pages[0])
        if (item.str === "May 01, 2025 - May 12, 2025")
          item.str = "October 01, 2025 - October 31, 2025";
      return Promise.resolve(extract!(pages));
    });
    vi.mocked(parseCsv).mockImplementation(async (file) => {
      const lines = (await file.text())
        .trim()
        .split("\n")
        .map((line) => line.split(","));
      return {
        headers: lines[0],
        rows: lines.slice(1),
        rowCount: lines.length - 1,
        errors: [],
        detectedConfig: createDefaultParseConfig(),
      };
    });
    const result = await readTransferStatement(
      new File(["synthetic"], "october.pdf"),
      "merrill-cma-statement-v1",
      "USD",
      false,
      vi.fn(),
    );
    expect(result.asOf).toBe("2025-10-31");
    expect(result.candidates[0]).toMatchObject({
      security: "DEMO",
      quantity: "2",
      totalCost: "21",
    });
    expect(result.sources[1].page).toBe("1");
    const parsed = await parseCsv(
      new File(
        [
          await readBasisFile(
            new File(["x"], "october.pdf"),
            BROKERAGE_BASIS_FORMATS[1].extractPdf,
          ),
        ],
        "x.csv",
      ),
      createDefaultParseConfig(),
    );
    const table = BROKERAGE_BASIS_FORMATS[1].normalize(parsed, "USD");
    expect(() =>
      calculateBasis({
        ...table,
        mode: "positions",
        asOf: result.asOf,
        startDate: "2025-05-13",
        currency: "USD",
        completeHistory: false,
      }),
    ).toThrow("cutoff");
  });
  it.each(["manual", "merrill-settled-activity-v1", "unknown"])(
    "rejects unsupported format %s before reading",
    async (format) => {
      await expect(
        readTransferStatement(new File(["x"], "a.pdf"), format, "USD", false, vi.fn()),
      ).rejects.toThrow("brokerageFormat");
      expect(readBasisFile).not.toHaveBeenCalled();
      expect(parseCsv).not.toHaveBeenCalled();
    },
  );
  it("does not parse a superseded upload", async () => {
    vi.mocked(readBasisFile).mockResolvedValue("synthetic");
    await expect(
      readTransferStatement(
        new File(["x"], "a.pdf"),
        "merrill-cma-statement-v1",
        "USD",
        false,
        () => {
          throw new Error("stale");
        },
      ),
    ).rejects.toThrow("stale");
    expect(parseCsv).not.toHaveBeenCalled();
  });
  it("rejects parser warnings and incomplete rows", async () => {
    vi.mocked(readBasisFile).mockResolvedValue("synthetic");
    vi.mocked(parseCsv).mockResolvedValue({
      headers: [],
      rows: [],
      rowCount: 1,
      errors: [],
      detectedConfig: createDefaultParseConfig(),
    });
    await expect(
      readTransferStatement(
        new File(["x"], "a.pdf"),
        "merrill-cma-statement-v1",
        "USD",
        false,
        vi.fn(),
      ),
    ).rejects.toThrow("structure");
  });
});
