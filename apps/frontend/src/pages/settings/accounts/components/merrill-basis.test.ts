import { describe, expect, it } from "vitest";
import { normalizeMerrillActivity } from "./merrill-basis-csv";
import { extractMerrillStatement } from "./merrill-basis-pdf";
import { calculateBasis } from "./opening-basis";
import { BROKERAGE_BASIS_FORMATS } from "./brokerage-basis-formats";
import {
  merrillCsvFixture,
  merrillPdfFixture,
  pdfItem,
  positionHeader,
  statementHeader,
} from "./test-fixtures/merrill-basis";

describe("versioned Merrill opening-basis formats", () => {
  it("recognizes settled cash withdrawals without changing security acquisition basis", () => {
    const input = merrillCsvFixture();
    const withdrawal = [...input.rows[0]];
    withdrawal[6] = "FundTransfers";
    withdrawal[7] = "Withdrawal";
    withdrawal[9] = "--";
    withdrawal[12] = "(15.00)";
    input.rows.push(withdrawal);
    const table = normalizeMerrillActivity(input, "USD");
    expect(table.cashRows).toEqual([1, 3]);
    expect(table.rows).toEqual([["DEMO", "2", "21", "2025-05-02", "BUY", "USD"]]);
    const result = calculateBasis({
      ...table,
      mode: "transactions",
      asOf: "2025-05-12",
      startDate: "2025-05-13",
      currency: "USD",
      completeHistory: true,
    });
    expect(result.candidates[0]).toMatchObject({
      quantity: "2",
      totalCost: "21",
      unitCost: "10.5",
    });
  });
  it.each(["-15.00", "(15.00)"])("supports an explicit cash withdrawal debit %s", (cash) => {
    const input = merrillCsvFixture();
    const row = [...input.rows[0]];
    row[6] = "FundTransfers";
    row[7] = "Withdrawal";
    row[9] = "--";
    row[12] = cash;
    input.rows = [row];
    expect(normalizeMerrillActivity(input, "USD", { allowCashOnly: true }).cashRows).toEqual([1]);
    expect(() => normalizeMerrillActivity(input, "USD")).toThrow("noBasisTrades");
  });
  it.each([
    [9, "DEMO"],
    [10, "1"],
    [10, "0"],
    [11, "5"],
    [11, "0"],
    [12, "15.00"],
    [12, "0"],
    [7, "Unknown transfer"],
  ] as const)("rejects ambiguous withdrawal field %s=%s", (field, value) => {
    const input = merrillCsvFixture();
    const row = [...input.rows[0]];
    row[6] = "FundTransfers";
    row[7] = "Withdrawal";
    row[9] = "--";
    row[12] = "(15.00)";
    row[field] = value;
    input.rows = [row];
    expect(() => normalizeMerrillActivity(input, "USD", { allowCashOnly: true })).toThrow(
      "brokerageRecord",
    );
  });
  it("registers a separately identified CSV and PDF adapter", () => {
    expect(BROKERAGE_BASIS_FORMATS.map((f) => [f.brokerage, f.id, f.kind])).toEqual([
      ["merrill_edge", "merrill-settled-activity-v1", "csv"],
      ["merrill_edge", "merrill-cma-statement-v1", "pdf"],
    ]);
  });
  it("normalizes net acquisition debit including fees, identifies cash income, and preserves source rows", () => {
    const input = merrillCsvFixture();
    input.headers[7] += " ";
    const table = normalizeMerrillActivity(input, "USD");
    expect(table.rows).toEqual([["DEMO", "2", "21", "2025-05-02", "BUY", "USD"]]);
    expect(table.sourceRows).toEqual([2]);
    expect(table.cashRows).toEqual([1]);
    const result = calculateBasis({
      ...table,
      mode: "transactions",
      asOf: "2025-05-12",
      startDate: "2025-05-13",
      currency: "USD",
      completeHistory: true,
    });
    expect(result.candidates).toEqual([
      { security: "DEMO", quantity: "2", totalCost: "21", unitCost: "10.5", rows: [2] },
    ]);
    expect(JSON.stringify(table)).not.toContain("Synthetic owner");
    expect(JSON.stringify(table)).not.toContain("SYNTH-01");
  });
  it("rejects unrecognized headers, currencies, account mixtures and pending records", () => {
    const input = merrillCsvFixture();
    expect(() => normalizeMerrillActivity(input, "EUR")).toThrow("currency");
    input.headers[12] = "Proceeds";
    expect(() => normalizeMerrillActivity(input, "USD")).toThrow("brokerageFormat");
    const multi = merrillCsvFixture();
    multi.rows[1][5] = "OTHER-02";
    expect(() => normalizeMerrillActivity(multi, "USD")).toThrow("brokerageAccount");
    const pending = merrillCsvFixture();
    pending.rows[1][2] = "Pending";
    expect(() => normalizeMerrillActivity(pending, "USD")).toThrow("brokeragePending");
  });
  it.each([
    "REINVESTMENT PURCHASE",
    "RETURN OF CAPITAL DIVIDEND",
    "STOCK SPLIT",
    "MERGER",
    "TRANSFER",
    "OPTION PURCHASE",
    "TREASURY PURCHASE",
  ])("blocks unsupported economics %s", (description) => {
    const input = merrillCsvFixture();
    input.rows[1][7] = description;
    expect(() => normalizeMerrillActivity(input, "USD")).toThrow("brokerageRecord");
  });
  it("never treats a sale's proceeds as disposed cost basis", () => {
    const input = merrillCsvFixture();
    input.rows[1][7] = "SALE";
    input.rows[1][12] = "20";
    expect(() => normalizeMerrillActivity(input, "USD")).toThrow("saleBasis");
  });
  it("does not mistake a fund's name for short selling, a bond trade or a basis adjustment", () => {
    const input = merrillCsvFixture();
    input.rows[1][8] = "Synthetic short treasury bond exchange fund";
    expect(normalizeMerrillActivity(input, "USD").rows[0][2]).toBe("21");
  });
  it.each([
    ["10", "0"],
    ["-21", "-2"],
    ["-19", "2"],
  ])("rejects inconsistent purchase amount/quantity %s/%s", (cash, qty) => {
    const input = merrillCsvFixture();
    input.rows[1][12] = cash;
    input.rows[1][10] = qty;
    expect(() => normalizeMerrillActivity(input, "USD")).toThrow("brokerageRecord");
  });
  it("blocks unknown categories and ambiguous income instead of dropping them", () => {
    const input = merrillCsvFixture();
    input.rows[0][6] = "Other";
    expect(() => normalizeMerrillActivity(input, "USD")).toThrow("brokerageRecord");
    const income = merrillCsvFixture();
    income.rows[0][10] = "1";
    expect(() => normalizeMerrillActivity(income, "USD")).toThrow("brokerageRecord");
    const only = merrillCsvFixture();
    only.rows.pop();
    expect(() => normalizeMerrillActivity(only, "USD")).toThrow("noBasisTrades");
  });
  it("extracts labeled PDF basis instead of market value and retains page provenance", () => {
    const text = extractMerrillStatement(merrillPdfFixture());
    expect(
      text
        .split("\n")
        .slice(1)
        .map((line) => line.split(",").slice(0, 6)),
    ).toEqual([
      ["DEMO", "2", "21", "2025-05-12", "USD", "1"],
      ["FUNDX", "3", "45", "2025-05-12", "USD", "1"],
    ]);
    expect(text).not.toContain("SYNTH-01");
    expect(text).not.toContain("Synthetic equity");
  });
  it("requires section totals to match every extracted position", () => {
    const pages = merrillPdfFixture();
    pages[0].find((item) => item.str === "21.00" && item.transform[5] === 540)!.str = "22.00";
    expect(() => extractMerrillStatement(pages)).toThrow("brokerageReconcile");
  });
  it("handles repeated headers and funds totals on continuation pages", () => {
    const pages = merrillPdfFixture();
    const first = pages[0];
    const total = first.filter((item) => item.transform[5] === 440);
    pages[0] = first.filter((item) => item.transform[5] !== 440);
    pages.push([
      ...statementHeader(),
      pdfItem("MUTUAL FUNDS/CLOSED END FUNDS/UITs/ETPs", 36, 600),
      ...positionHeader(590, false),
      ...total,
    ]);
    expect(extractMerrillStatement(pages).split("\n")).toHaveLength(3);
  });
  it("fails closed on missing basis, unknown layouts and unsupported sections", () => {
    const missing = merrillPdfFixture();
    missing[0].find((x) => x.str === "21.00" && x.transform[5] === 570)!.str = "N/A";
    expect(() => extractMerrillStatement(missing)).toThrow("brokerageBasis");
    const unknown = merrillPdfFixture();
    unknown[0].find((x) => x.str === "EQUITIES")!.str = "FIXED INCOME";
    expect(() => extractMerrillStatement(unknown)).toThrow("brokerageFormat");
    const headers = merrillPdfFixture();
    headers[0].find((x) => x.str === "Cost Basis")!.str = "Adjusted Amount";
    expect(() => extractMerrillStatement(headers)).toThrow("brokerageFormat");
    const unit = merrillPdfFixture();
    unit[0].find((x) => x.str === "Total")!.str = "Unit";
    expect(() => extractMerrillStatement(unit)).toThrow("brokerageFormat");
  });
  it("rejects cross-account or inconsistent statement periods", () => {
    const pages = merrillPdfFixture();
    pages.push([
      ...statementHeader("OTHER-02"),
      pdfItem("EQUITIES", 36, 600),
      ...positionHeader(590),
    ]);
    expect(() => extractMerrillStatement(pages)).toThrow("brokerageAccount");
    const periods = merrillPdfFixture();
    periods.push(statementHeader("SYNTH-01", "May 01, 2025 - May 11, 2025"));
    expect(() => extractMerrillStatement(periods)).toThrow("brokerageAccount");
  });
  it("requires a fund symbol and never substitutes a description", () => {
    const pages = merrillPdfFixture();
    pages[0] = pages[0].filter((x) => !x.str.includes("SYMBOL:"));
    expect(() => extractMerrillStatement(pages)).toThrow("brokerageReconcile");
  });
});
