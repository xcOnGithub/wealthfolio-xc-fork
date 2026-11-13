import { describe, it, expect } from "vitest";
import { extractMonthlyMerrill } from "./merrill-statement";
import { monthlyFixture } from "./test-fixtures/monthly-statement";

describe("Merrill monthly statement observations", () => {
  it("extracts the closing dated value, not prior value or security cost basis", () => {
    const { snapshot } = extractMonthlyMerrill(monthlyFixture());
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
