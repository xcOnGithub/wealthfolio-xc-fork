import { it, expect, vi } from "vitest";
import { extractMerrillHistory } from "./merrill-history";
import { monthlyFixture } from "./test-fixtures/monthly-statement";
import {
  cashActivityPage,
  activityHeader,
} from "@/pages/settings/accounts/components/test-fixtures/merrill-statement-v2";
import { merrillActivityFixture } from "@/pages/settings/accounts/components/test-fixtures/merrill-statement-v2";
import { pdfItem, atRight } from "@/pages/settings/accounts/components/test-fixtures/merrill-basis";
vi.mock("@/adapters", () => ({}));

function historyFixture() {
  const pages = monthlyFixture();
  pages[0].push(
    pdfItem("This Statement", 240, 480),
    pdfItem("Year to Date", 400, 480),
    pdfItem("Opening Cash/Money Accounts", 36, 460),
    atRight("(5)", 270, 460),
    atRight("999", 430, 460),
  );
  const activity = merrillActivityFixture()[1];
  // Opening cash -5 plus net sale 15 equals closing cash 10; transfer value 200 is not cash.
  pages.push(activity);
  return pages;
}
it("builds reconciled statement facts without Plaid data, calls or acquisition costs", () => {
  const pages = historyFixture();
  pages[1]
    .filter((c) => ["21.00", "45.00"].includes(c.str) && [570, 470].includes(c.transform[5]))
    .forEach((c) => {
      c.str = "N/A";
    });
  const { identity, draft } = extractMerrillHistory(pages);
  expect(identity).toBe("SYNTH01");
  expect(JSON.stringify(draft)).not.toContain("SYNTH-01");
  expect(JSON.stringify(draft)).not.toContain("SYNTH01");
  expect(draft.closing.positionsComplete).toBe(true);
  expect(draft.closing.positions.every((p) => p.costBasis === null)).toBe(true);
  expect(draft.cashReconciliation).toEqual({
    knownMovementTotal: "15",
    difference: "0",
    matches: true,
  });
  expect(draft.issues).toEqual([]);
  expect(draft.evidence.activities).toHaveLength(2);
});
it("preserves a verified snapshot but reports missing opening cash", () => {
  const pages = historyFixture();
  pages[0] = pages[0].filter((c) => c.str !== "Opening Cash/Money Accounts");
  const { draft } = extractMerrillHistory(pages);
  expect(draft.closing.totalValue).toBe("510");
  expect(draft.cashReconciliation.matches).toBeNull();
  expect(draft.issues).toContainEqual({ code: "missingOpeningCash" });
});
it("flags a cash discrepancy rather than inventing an adjustment", () => {
  const pages = historyFixture();
  pages[0].find((c) => c.str === "(5)")!.str = "(4)";
  const { draft } = extractMerrillHistory(pages);
  expect(draft.cashReconciliation).toMatchObject({ difference: "1", matches: false });
  expect(draft.issues).toContainEqual({ code: "cashMismatch" });
  expect(draft.evidence.activities).toHaveLength(2);
});
it("does not silently accept an ambiguous opening-cash summary", () => {
  const pages = historyFixture();
  pages[0].push(atRight("1", 270, 460));
  expect(() => extractMerrillHistory(pages)).toThrow("brokerageFormat");
});

it("does not assume no activity when the activity pages are missing", () => {
  const pages = historyFixture();
  pages.pop();
  const { draft } = extractMerrillHistory(pages);
  expect(draft.issues).toContainEqual({ code: "missingActivitySections" });
});
it("does not treat a missing trade cash amount as zero", () => {
  const pages = historyFixture();
  pages[2].find((c) => c.str === "15.00" && c.transform[5] === 630)!.str = "N/A";
  const { draft } = extractMerrillHistory(pages);
  expect(draft.cashReconciliation.matches).toBeNull();
  expect(draft.issues).toEqual(
    expect.arrayContaining([expect.objectContaining({ code: "unknownCashMovement" })]),
  );
});

it("keeps an explicitly unavailable cash side unknown instead of using the known side alone", () => {
  const pages = historyFixture(),
    cash = cashActivityPage("cash");
  cash.push(atRight("N/A", 750, 630));
  pages.push(cash);
  const { draft } = extractMerrillHistory(pages);
  expect(draft.evidence.activities.at(-1)?.cash).toBeNull();
  expect(draft.cashReconciliation.matches).toBeNull();
  expect(draft.issues).toContainEqual(expect.objectContaining({ code: "unknownCashMovement" }));
});
it("keeps internal cash sweeps out of reconciled external and trade cash", () => {
  const pages = historyFixture();
  const page = [...activityHeader(), pdfItem("YOUR CMA MONEY ACCOUNT TRANSACTIONS", 37, 680)];
  for (const x of [0, 360])
    page.push(
      pdfItem("Date", 40 + x, 650),
      pdfItem("Description", 84 + x, 650),
      atRight("Withdrawals", 260 + x, 650),
      atRight("Deposits", 340 + x, 650),
      pdfItem("05/02", 40 + x, 630),
      pdfItem("Sweep", 84 + x, 630),
      atRight("99", 340 + x, 630),
    );
  pages.push(page);
  const { draft } = extractMerrillHistory(pages);
  expect(draft.cashReconciliation).toMatchObject({ knownMovementTotal: "15", matches: true });
  expect(draft.evidence.activities.filter((r) => r.section === "sweep")).toHaveLength(2);
});

it("preserves an account total when closing cash is explicitly unavailable", () => {
  const pages = historyFixture();
  pages[0].find((c) => c.str === "10.00" && c.transform[5] === 580)!.str = "n/a";
  const { draft } = extractMerrillHistory(pages);
  expect(draft.closing.totalValue).toBe("510");
  expect(draft.closing.cashValue).toBeNull();
  expect(draft.cashReconciliation.matches).toBeNull();
  expect(draft.issues).toContainEqual({ code: "missingClosingCash" });
});
it("preserves unknown basis markers regardless of case", () => {
  const pages = historyFixture();
  pages[1].find((c) => c.str === "21.00" && c.transform[5] === 570)!.str = "n/a";
  expect(extractMerrillHistory(pages).draft.closing.positions[0].costBasis).toBeNull();
});
