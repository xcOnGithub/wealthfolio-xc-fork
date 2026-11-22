import { describe, it, expect } from "vitest";
import { extractMonthlyMerrill, extractMonthlyMerrillStatements } from "./merrill-statement";
import { monthlyFixture } from "./test-fixtures/monthly-statement";

describe("Merrill monthly statement observations", () => {
  it("extracts the closing dated value, not prior value or security cost basis", () => {
    const { snapshot, holdingTickers } = extractMonthlyMerrill(monthlyFixture());
    expect(snapshot).toMatchObject({
      date: "2025-05-12",
      currency: "USD",
      totalValue: "510",
      cashValue: "10",
      positionsComplete: true,
      sourcePage: 1,
    });
    expect(snapshot.positions.map((p) => p.costBasis)).toEqual(["21", "45"]);
    expect(snapshot.positions.map((p) => p.marketValue)).toEqual(["200", "300"]);
    expect(holdingTickers).toEqual(
      new Map([
        ["synthetic equity demo", "DEMO"],
        ["synthetic fund", "FUNDX"],
      ]),
    );
    expect(JSON.stringify(snapshot)).not.toContain("SYNTH-01");
  });
  it("keeps account value when basis is explicitly unavailable", () => {
    const pages = monthlyFixture();
    pages[1].find((i) => i.str === "21.00" && i.transform[5] === 570)!.str = "--";
    const { snapshot } = extractMonthlyMerrill(pages);
    expect(snapshot.totalValue).toBe("510");
    expect(snapshot.positions[0].costBasis).toBeNull();
    expect(snapshot.positionsComplete).toBe(true);
  });
  it("keeps a verified total when position parsing is incomplete", () => {
    const pages = monthlyFixture();
    pages[1].find((i) => i.str === "21.00" && i.transform[5] === 570)!.str = "malformed";
    const { snapshot } = extractMonthlyMerrill(pages);
    expect(snapshot.totalValue).toBe("510");
    expect(snapshot.positionsComplete).toBe(false);
    expect(snapshot.positions).toHaveLength(1);
  });
  it("discloses incomplete reconciliation without substituting partial position totals", () => {
    const pages = monthlyFixture();
    pages[0].find((i) => i.str === "510.00" && i.transform[5] === 520)!.str = "600.00";
    const { snapshot } = extractMonthlyMerrill(pages);
    expect(snapshot.totalValue).toBe("600");
    expect(snapshot.positionsComplete).toBe(false);
  });
  it.each([
    "multiAccount",
    "multiPeriod",
    "missingHeader",
    "multipleTotals",
    "malformedTotal",
    "wrongColumn",
  ])("rejects ambiguous or unsupported summary: %s", (kind) => {
    const pages = monthlyFixture();
    if (kind === "multiAccount")
      pages[1].find((i) => i.str.startsWith("Account Number:"))!.str = "Account Number: OTHER-02";
    if (kind === "multiPeriod")
      pages[1].find((i) => i.str.startsWith("May 01"))!.str = "April 01, 2025 - April 30, 2025";
    if (kind === "missingHeader") pages[0] = pages[0].filter((i) => i.str !== "May 12");
    if (kind === "multipleTotals") pages.push([...pages[0]]);
    if (kind === "malformedTotal")
      pages[0].find((i) => i.str === "510.00" && i.transform[5] === 520)!.str = "5O0.00";
    if (kind === "wrongColumn")
      pages[0].find((i) => i.str === "510.00" && i.transform[5] === 520)!.transform[4] += 30;
    expect(() => extractMonthlyMerrill(pages)).toThrow();
  });
});

it("rejects reversed or inconsistent reporting periods", () => {
  for (const replacement of ["May 13, 2025 - May 12, 2025", "April 01, 2025 - May 12, 2025"]) {
    const pages = monthlyFixture();
    pages[1].find((i) => i.str.startsWith("May 01"))!.str = replacement;
    expect(() => extractMonthlyMerrill(pages)).toThrow();
  }
});

it.each(["negativeQuantity", "negativeBasis", "longLabel"])(
  "keeps a verified total when an optional position violates the storage contract: %s",
  (kind) => {
    const pages = monthlyFixture();
    if (kind === "negativeQuantity")
      pages[1].find((i) => i.str === "2" && i.transform[5] === 570)!.str = "-2";
    if (kind === "negativeBasis")
      pages[1].find((i) => i.str === "21.00" && i.transform[5] === 570)!.str = "-21.00";
    if (kind === "longLabel")
      pages[1].find((i) => i.str === "Synthetic equity")!.str = "x".repeat(257);
    const { snapshot } = extractMonthlyMerrill(pages);
    expect(snapshot.totalValue).toBe("510");
    expect(snapshot.positionsComplete).toBe(false);
    expect(snapshot.positions).toHaveLength(1);
  },
);
it("retains reported basis when market value is explicitly unavailable", () => {
  const pages = monthlyFixture();
  pages[1].find((i) => i.str === "200.00" && i.transform[5] === 570)!.str = "--";
  const { snapshot } = extractMonthlyMerrill(pages);
  expect(snapshot.positionsComplete).toBe(false);
  expect(snapshot.positions[0]).toMatchObject({ costBasis: "21", marketValue: null });
  expect(snapshot.totalValue).toBe("510");
});

it("keeps a leading-decimal fractional holding with unknown basis in closing value", () => {
  const pages = monthlyFixture();
  pages[1].find((i) => i.str === "2" && i.transform[5] === 570)!.str = ".0002";
  pages[1].find((i) => i.str === "21.00" && i.transform[5] === 570)!.str = "N/A";
  const { snapshot } = extractMonthlyMerrill(pages);
  expect(snapshot.positionsComplete).toBe(true);
  expect(snapshot.positions[0]).toMatchObject({
    quantity: "0.0002",
    costBasis: null,
    marketValue: "200",
  });
});

it("extracts multiple monthly snapshots from a bundled multi-month PDF with original page numbers", () => {
  const march = monthlyFixture();
  for (const page of march) {
    for (const item of page) {
      item.str = item.str
        .replaceAll("May 01, 2025 - May 12, 2025", "March 01, 2025 - March 31, 2025")
        .replaceAll("May 12", "March 31")
        .replaceAll("April 30", "February 28");
    }
  }
  const april = monthlyFixture();
  for (const page of april) {
    for (const item of page) {
      item.str = item.str
        .replaceAll("April 30", "March 31")
        .replaceAll("May 01, 2025 - May 12, 2025", "April 01, 2025 - April 30, 2025")
        .replaceAll("May 12", "April 30");
    }
  }

  const results = extractMonthlyMerrillStatements([...march, ...april]);
  expect(results).toHaveLength(2);
  expect(results[0].snapshot).toMatchObject({
    date: "2025-03-31",
    totalValue: "510",
    sourcePage: 1,
  });
  expect(results[1].snapshot).toMatchObject({
    date: "2025-04-30",
    totalValue: "510",
    sourcePage: 3,
  });
});

it("rejects a bundled PDF if any month segment is missing a summary", () => {
  const march = monthlyFixture();
  const brokenApril = monthlyFixture();
  for (const page of brokenApril) {
    for (const item of page) {
      item.str = item.str
        .replaceAll("May 01, 2025 - May 12, 2025", "April 01, 2025 - April 30, 2025")
        .replaceAll("May 12", "April 30");
    }
  }
  // Drop summary page from April segment
  expect(() => extractMonthlyMerrillStatements([...march, brokenApril[1]])).toThrow();
});

it("does not guess a ticker from the holding name or register an identifier from a non-equity section", () => {
  const pages = monthlyFixture();
  pages[1].find((item) => item.str === "EQUITIES")!.str = "FIXED INCOME";
  pages[1].find((item) => item.str === "Synthetic fund")!.str = "Synthetic fund MSFT";
  pages[1] = pages[1].filter((item) => !item.str.includes("SYMBOL:"));
  const { snapshot, holdingTickers } = extractMonthlyMerrill(pages);
  expect(snapshot.positionsComplete).toBe(true);
  expect(holdingTickers.size).toBe(0);
});
