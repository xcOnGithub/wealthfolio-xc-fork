import { beforeEach, describe, expect, it, vi } from "vitest";
import { parseCsv } from "@/adapters";
import { readBasisFile } from "./basis-pdf";
import { calculateBasis } from "./opening-basis";
import {
  combineBasisDocuments,
  prepareBasisBatch,
  validateBasisFiles,
  MAX_BASIS_BATCH_BYTES,
  type PreparedBasisDocument,
} from "./opening-basis-batch";
import { merrillCsvFixture, merrillPdfFixture } from "./test-fixtures/merrill-basis";
import { createDefaultParseConfig } from "@/pages/activity/import/utils/default-activity-template";

vi.mock("@/adapters", () => ({ parseCsv: vi.fn() }));
vi.mock("./basis-pdf", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./basis-pdf")>()),
  readBasisFile: vi.fn(),
}));
const statement = (
  fileIndex: number,
  asOf: string,
  quantity = "2",
  cost = "21",
): PreparedBasisDocument => ({
  fileIndex,
  name: `statement-${fileIndex}.pdf`,
  kind: "pdf",
  formatId: "merrill-cma-statement-v1",
  table: {
    headers: [
      "Security",
      "Quantity",
      "Total cost basis",
      "Date",
      "Currency",
      "Source page",
      "Source line",
    ],
    rows: [["DEMO", quantity, cost, asOf, "USD", "3", "12"]],
    columns: { security: 0, quantity: 1, totalCost: 2, date: 3, currency: 4 },
  },
});
const trades = (
  fileIndex: number,
  date: string,
  quantity = "2",
  cost = "21",
): PreparedBasisDocument => ({
  fileIndex,
  name: `activity-${fileIndex}.csv`,
  kind: "csv",
  formatId: "merrill-settled-activity-v1",
  table: {
    headers: ["Security", "Quantity", "Total cost basis", "Date", "Action", "Currency"],
    rows: [["DEMO", quantity, cost, date, "BUY", "USD"]],
    columns: { security: 0, quantity: 1, totalCost: 2, date: 3, action: 4, currency: 5 },
    sourceRows: [9],
    cashRows: [],
  },
});
const apply = (batch: ReturnType<typeof combineBasisDocuments>, completeHistory = true) =>
  calculateBasis({
    ...batch,
    asOf: batch.asOf || "2025-05-12",
    startDate: "2025-05-13",
    currency: "USD",
    completeHistory,
  });
const parsed = (data: ReturnType<typeof merrillCsvFixture>) => ({
  ...data,
  errors: [],
  rowCount: data.rows.length,
  detectedConfig: createDefaultParseConfig(),
});
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(readBasisFile).mockResolvedValue("synthetic table");
});

describe("in-memory opening-basis document batches", () => {
  it("combines five disjoint CSV exports and preserves the original file and row", () => {
    const batch = combineBasisDocuments(
      [5, 3, 1, 4, 2].map((i) => trades(i, `2025-05-0${i}`, "1", String(i * 10))),
      "USD",
      "2025-05-13",
    );
    expect(apply(batch).candidates[0]).toMatchObject({
      quantity: "5",
      totalCost: "150",
      unitCost: "30",
    });
    expect(batch.origins[1]).toEqual({ file: 5, name: "activity-5.csv", row: 9 });
    expect(() => apply(batch, false)).toThrow("history");
  });
  it("uses the latest statement instead of adding monthly snapshots", () => {
    const batch = combineBasisDocuments(
      [statement(1, "2025-04-30", "1", "10"), statement(2, "2025-05-12", "2", "50")],
      "USD",
      "2025-05-13",
    );
    expect(apply(batch).candidates[0].totalCost).toBe("50");
    expect(batch.documents.map((doc) => doc.role)).toEqual(["superseded", "baseline"]);
    expect(batch.origins[1]).toMatchObject({ file: 2, page: "3", line: "12" });
  });
  it("bridges the latest baseline and excludes trades already covered by it", () => {
    const batch = combineBasisDocuments(
      [
        statement(1, "2025-04-30", "1", "10"),
        trades(2, "2025-05-05", "2", "20"),
        statement(3, "2025-05-10", "3", "30"),
        trades(4, "2025-05-11", "1", "15"),
      ],
      "USD",
      "2025-05-13",
    );
    expect(batch.checkpoint?.asOf).toBe("2025-05-10");
    expect(apply(batch)).toMatchObject({
      excluded: 1,
      candidates: [{ quantity: "4", totalCost: "45", unitCost: "11.25" }],
    });
  });
  it("marks CSVs covered when a statement establishes the cutoff itself", () => {
    const batch = combineBasisDocuments(
      [statement(1, "2025-05-12"), trades(2, "2025-05-02")],
      "USD",
      "2025-05-13",
    );
    expect(batch.mode).toBe("positions");
    expect(batch.documents[1].role).toBe("covered");
    expect(apply(batch).candidates[0].quantity).toBe("2");
  });
  it("rejects conflicting same-day statements, but treats identical snapshots as redundant", () => {
    expect(() =>
      combineBasisDocuments(
        [statement(1, "2025-05-12"), statement(2, "2025-05-12", "2", "50")],
        "USD",
        "2025-05-13",
      ),
    ).toThrow("batchConflict");
    const batch = combineBasisDocuments(
      [statement(1, "2025-05-12"), statement(2, "2025-05-12")],
      "USD",
      "2025-05-13",
    );
    expect(apply(batch).candidates).toHaveLength(1);
  });
  it("does not silently backdate newer statements or assume no intervening trades", () => {
    expect(() => combineBasisDocuments([statement(1, "2025-05-14")], "USD", "2025-05-13")).toThrow(
      "cutoff",
    );
    const batch = combineBasisDocuments(
      [statement(1, "2025-04-30"), statement(2, "2025-05-10")],
      "USD",
      "2025-05-13",
    );
    expect(batch.asOf).toBe("2025-05-10");
    expect(batch.mode).toBe("positions");
  });
  it("blocks duplicate transactions across files instead of guessing which to drop", () => {
    const batch = combineBasisDocuments(
      [trades(1, "2025-05-01"), trades(2, "2025-05-01")],
      "USD",
      "2025-05-13",
    );
    expect(() => apply(batch)).toThrow("duplicate");
  });
  it("supports cash-only CSVs after a verified baseline with explicit history confirmation", () => {
    const cash = trades(2, "2025-05-02");
    cash.table.rows = [];
    cash.table.sourceRows = [];
    cash.table.cashRows = [1];
    const batch = combineBasisDocuments([statement(1, "2025-04-30"), cash], "USD", "2025-05-13");
    expect(apply(batch).candidates[0].totalCost).toBe("21");
    expect(() => apply(batch, false)).toThrow("history");
    expect(() => combineBasisDocuments([cash], "USD", "2025-05-13")).toThrow("noBasisTrades");
  });
  it("enforces five-file and aggregate byte limits before reading anything", async () => {
    const files = Array.from({ length: 6 }, () => new File(["x"], "activity.csv"));
    await expect(
      prepareBasisBatch(files, "merrill-settled-activity-v1", "USD", "2025-05-13"),
    ).rejects.toThrow("batchCount");
    expect(readBasisFile).not.toHaveBeenCalled();
    expect(() =>
      validateBasisFiles(Array.from({ length: 3 }, () => ({ size: 10 * 1024 * 1024 }) as File)),
    ).toThrow("batchSize");
    expect(MAX_BASIS_BATCH_BYTES).toBe(25 * 1024 * 1024);
    expect(() => validateBasisFiles(files.slice(0, 5))).not.toThrow();
  });
  it("requires supported brokerage formats for mixed batches", async () => {
    await expect(
      prepareBasisBatch(
        [new File(["x"], "a.csv"), new File(["x"], "b.pdf")],
        "manual",
        "USD",
        "2025-05-13",
      ),
    ).rejects.toThrow("batchFormat");
    expect(readBasisFile).not.toHaveBeenCalled();
  });
  it("checks account identity across exports without returning it", async () => {
    const a = merrillCsvFixture(),
      b = merrillCsvFixture();
    b.rows.forEach((row) => {
      row[5] = "OTHER-02";
    });
    vi.mocked(parseCsv).mockResolvedValueOnce(parsed(a)).mockResolvedValueOnce(parsed(b));
    await expect(
      prepareBasisBatch(
        [new File(["x"], "a.csv"), new File(["x"], "b.csv")],
        "merrill-settled-activity-v1",
        "USD",
        "2025-05-13",
      ),
    ).rejects.toMatchObject({ code: "brokerageAccount", fileIndex: 2 });
  });
  it("processes mixed files sequentially and returns only normalized evidence", async () => {
    const page = merrillPdfFixture().map((items) =>
      items.filter((item) => item.transform[5] >= 540),
    );
    page[0].find((item) => item.str.startsWith("May 01"))!.str = "April 01, 2025 - April 30, 2025";
    vi.mocked(readBasisFile).mockImplementation(async (file, extract) =>
      file.name.endsWith(".pdf") ? extract!(page) : "synthetic CSV",
    );
    vi.mocked(parseCsv)
      .mockResolvedValueOnce({
        headers: statement(1, "2025-04-30").table.headers,
        rows: statement(1, "2025-04-30").table.rows,
        errors: [],
        rowCount: 1,
        detectedConfig: createDefaultParseConfig(),
      })
      .mockResolvedValueOnce(parsed(merrillCsvFixture()));
    const batch = await prepareBasisBatch(
      [new File(["x"], "a.pdf"), new File(["x"], "b.csv")],
      "merrill-settled-activity-v1",
      "USD",
      "2025-05-13",
    );
    expect(apply(batch).candidates[0].quantity).toBe("4");
    expect(JSON.stringify(batch)).not.toContain("SYNTH01");
    expect(vi.mocked(readBasisFile).mock.invocationCallOrder[1]).toBeGreaterThan(
      vi.mocked(parseCsv).mock.invocationCallOrder[0],
    );
  });
  it("returns no partial result when a later file fails and never reads the remaining file", async () => {
    vi.mocked(parseCsv)
      .mockResolvedValueOnce(parsed(merrillCsvFixture()))
      .mockRejectedValueOnce(new Error("private source error"));
    await expect(
      prepareBasisBatch(
        ["a", "b", "c"].map((name) => new File(["x"], `${name}.csv`)),
        "merrill-settled-activity-v1",
        "USD",
        "2025-05-13",
      ),
    ).rejects.toMatchObject({ code: "parse", fileIndex: 2 });
    expect(readBasisFile).toHaveBeenCalledTimes(2);
  });
  it("stops after unmount/admission loss and enforces cumulative extracted-text limits", async () => {
    const files = [new File(["x"], "a.csv"), new File(["x"], "b.csv")];
    const admit = vi
      .fn()
      .mockImplementationOnce(() => {})
      .mockImplementation(() => {
        throw new Error("closed");
      });
    await expect(
      prepareBasisBatch(files, "merrill-settled-activity-v1", "USD", "2025-05-13", admit),
    ).rejects.toMatchObject({ fileIndex: 1 });
    expect(parseCsv).not.toHaveBeenCalled();
    vi.clearAllMocks();
    vi.mocked(readBasisFile).mockResolvedValue("x".repeat(600001));
    vi.mocked(parseCsv).mockResolvedValue(parsed(merrillCsvFixture()));
    await expect(
      prepareBasisBatch(files, "merrill-settled-activity-v1", "USD", "2025-05-13"),
    ).rejects.toMatchObject({ code: "batchSize", fileIndex: 2 });
    expect(parseCsv).toHaveBeenCalledOnce();
  });
});
