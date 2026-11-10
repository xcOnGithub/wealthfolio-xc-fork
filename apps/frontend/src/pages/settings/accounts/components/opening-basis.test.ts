import { describe, expect, it } from "vitest";
import {
  basisDate,
  calculateBasis,
  matchingBasis,
  openingDate,
  suggestBasisColumns,
} from "./opening-basis";

const positions = () => ({
  headers: ["Symbol", "Quantity", "Cost basis", "Currency"],
  rows: [["ABC", "3", "100", "USD"]],
  columns: { security: 0, quantity: 1, totalCost: 2, currency: 3 },
  mode: "positions" as const,
  asOf: "2025-05-12",
  startDate: "2025-05-13",
  currency: "USD",
  completeHistory: false,
});
const transactions = () => ({
  ...positions(),
  mode: "transactions" as const,
  completeHistory: true,
  headers: ["Symbol", "Quantity", "Cost basis", "Date", "Action", "Price", "Fee"],
  columns: { security: 0, quantity: 1, totalCost: 2, date: 3, action: 4, price: 5, fee: 6 },
  rows: [
    ["ABC", "2", "", "2025-01-01", "BUY", "10", "1"],
    ["ABC", "2", "40", "05/02/2025", "BUY", "", ""],
    ["ABC", "1", "10.5", "2025-05-12", "SELL", "999", "0"],
    ["ABC", "1", "999", "2025-05-13", "BUY", "", ""],
  ],
});
function fails(input: Parameters<typeof calculateBasis>[0], code: string) {
  expect(() => calculateBasis(input)).toThrowError(code);
}
describe("opening cost basis evidence", () => {
  it("bridges an earlier monthly statement with CSV adjustments without counting overlap twice", () => {
    const statement = calculateBasis({ ...positions(), asOf: "2025-04-30" });
    const result = calculateBasis({
      ...transactions(),
      checkpoint: {
        asOf: "2025-04-30",
        currency: "USD",
        candidates: statement.candidates,
      },
    });
    expect(result.candidates).toEqual([
      { security: "ABC", quantity: "4", totalCost: "129.5", unitCost: "32.375", rows: [2, 3] },
    ]);
    expect(result.excluded).toBe(2);
  });
  it("rejects a starting statement in another currency or after the opening cutoff", () => {
    const checkpoint = {
      asOf: "2025-04-30",
      currency: "EUR",
      candidates: calculateBasis(positions()).candidates,
    };
    fails({ ...transactions(), checkpoint }, "cutoff");
    fails(
      { ...transactions(), checkpoint: { ...checkpoint, currency: "USD", asOf: "2025-05-13" } },
      "cutoff",
    );
  });
  it("calculates aggregate basis with decimal precision and keeps source row provenance", () => {
    const { candidates } = calculateBasis(positions());
    expect(candidates).toEqual([
      {
        security: "ABC",
        quantity: "3",
        totalCost: "100",
        unitCost: "33.333333333333333333",
        rows: [1],
      },
    ]);
    expect(matchingBasis(candidates[0], "3.00000000")).toBe(true);
    expect(matchingBasis(candidates[0], "3.00000001")).toBe(false);
    expect(matchingBasis(candidates[0], "bad")).toBe(false);
  });
  it("uses actual sale basis, not proceeds, and excludes Plaid overlap", () => {
    const result = calculateBasis(transactions());
    expect(result).toEqual({
      candidates: [
        {
          security: "ABC",
          quantity: "3",
          totalCost: "50.5",
          unitCost: "16.833333333333333333",
          rows: [1, 2, 3],
        },
      ],
      excluded: 1,
    });
  });
  it("sorts historical days without changing same-day source order", () => {
    const input = transactions();
    input.rows = [input.rows[2], input.rows[1], input.rows[0]];
    expect(calculateBasis(input).candidates[0].totalCost).toBe("50.5");
    input.rows = [
      ["ABC", "1", "1", "2025-01-01", "SELL", "", ""],
      ["ABC", "1", "1", "2025-01-01", "BUY", "", ""],
    ];
    fails(input, "incomplete");
  });
  it("supports explicit zero basis, US thousands, and unit-cost position statements", () => {
    const input = positions();
    input.rows[0][2] = "$1,234.56";
    expect(calculateBasis(input).candidates[0].totalCost).toBe("1234.56");
    input.rows[0][2] = "0";
    expect(calculateBasis(input).candidates[0].unitCost).toBe("0");
    expect(
      calculateBasis({ ...input, columns: { security: 0, quantity: 1, unitCost: 2 } }).candidates[0]
        .totalCost,
    ).toBe("0");
  });
  it.each([
    "-2",
    "NaN",
    "Infinity",
    "1.234,56",
    "1,23",
    "1e3",
    "estimated 4",
    "",
    "1e100",
    "1.0000000000000000001",
  ])("rejects ambiguous or invalid money %s", (number) => {
    const input = positions();
    input.rows[0][2] = number;
    fails(input, "number");
  });
  it.each(["0", "-1"])("rejects nonpositive statement quantity %s", (quantity) => {
    const input = positions();
    input.rows[0][1] = quantity;
    fails(input, "number");
  });
  it("never maps market value or sale proceeds as cost basis automatically", () => {
    const headers = ["Symbol", "Quantity", "Market Value", "Proceeds"];
    expect(suggestBasisColumns(headers)).toEqual({ security: 0, quantity: 1 });
    fails({ ...positions(), headers, columns: suggestBasisColumns(headers) }, "columns");
    expect(suggestBasisColumns(["Symbol", "Ticker", "Quantity", "Cost basis"])).toEqual({
      quantity: 2,
      totalCost: 3,
    });
  });
  it("rejects duplicate columns, missing fields, malformed rows, empty data and large tables", () => {
    fails({ ...positions(), columns: { security: 0, quantity: 0, totalCost: 2 } }, "columns");
    fails({ ...positions(), columns: { security: 9, quantity: 1, totalCost: 2 } }, "columns");
    fails({ ...positions(), rows: [["ABC"]] }, "structure");
    fails({ ...positions(), rows: [] }, "size");
    fails(
      { ...positions(), rows: Array.from({ length: 10001 }, () => positions().rows[0]) },
      "size",
    );
  });
  it("blocks duplicate statement rows and transactions rather than double counting", () => {
    const input = positions();
    input.rows.push([...input.rows[0]]);
    fails(input, "duplicate");
    const tx = transactions();
    tx.rows.push([...tx.rows[0]]);
    fails(tx, "duplicate");
  });
  it("requires matching source date, currency and nonempty identity", () => {
    fails({ ...positions(), asOf: "2025-05-14" }, "cutoff");
    const input = positions();
    input.rows[0][3] = "EUR";
    fails(input, "currency");
    input.rows[0][3] = "USD";
    input.rows[0][0] = "";
    fails(input, "security");
    fails(
      {
        ...positions(),
        headers: [...positions().headers, "Date"],
        rows: [[...positions().rows[0], "2025-05-11"]],
        columns: { ...positions().columns, date: 4 },
      },
      "cutoff",
    );
  });
  it("blocks conflicting position total and unit costs", () => {
    fails(
      {
        ...positions(),
        headers: [...positions().headers, "Unit cost"],
        rows: [[...positions().rows[0], "10"]],
        columns: { ...positions().columns, unitCost: 4 },
      },
      "conflict",
    );
  });
  it("requires full-history attestation and actual removed basis on sales", () => {
    fails({ ...transactions(), completeHistory: false }, "history");
    const input = transactions();
    input.rows[2][2] = "";
    fails(input, "saleBasis");
  });
  it.each(["SPLIT", "TRANSFER", "REINVEST", "DIVIDEND", "UNKNOWN"])(
    "blocks unsupported action %s",
    (action) => {
      const input = transactions();
      input.rows[1][4] = action;
      fails(input, "action");
    },
  );
  it("blocks missing opening shares and inconsistent residual basis", () => {
    const input = transactions();
    input.rows[2][1] = "5";
    fails(input, "incomplete");
    input.rows[2][1] = "4";
    fails(input, "incomplete");
    input.rows[2][2] = "61";
    fails(input, "empty");
    input.rows[2][1] = "1";
    input.rows[2][2] = "100";
    fails(input, "incomplete");
  });
  it("validates calendar dates and handles leap-year opening cutoffs", () => {
    expect(openingDate("2024-03-01")).toBe("2024-02-29");
    for (const date of ["2025-02-29", "2025-13-01", "05/12/2025", "invalid"])
      expect(() => basisDate(date)).toThrow();
    const input = transactions();
    input.rows[0][3] = "31/01/2025";
    fails(input, "date");
  });
});
