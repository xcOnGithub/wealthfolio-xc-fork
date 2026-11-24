import { it, expect, vi } from "vitest";
import { extractMerrillHistory, extractMerrillHistoryStatements } from "./merrill-history";
import { monthlyFixture } from "./test-fixtures/monthly-statement";
import {
  cashActivityPage,
  activityHeader,
  merrillActivityFixture,
} from "./test-fixtures/merrill-activity";
import { pdfItem, atRight } from "./test-fixtures/merrill-pdf";
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
  expect(draft.evidence.activities.map((row) => draft.activityHoldingLabels[row.id])).toEqual([
    "Synthetic equity DEMO",
    "Synthetic equity DEMO",
  ]);
});
it("leaves identical issuer descriptions ambiguous when the printed tickers differ", () => {
  const pages = historyFixture();
  pages[1].push(
    pdfItem("Synthetic equity", 36, 550),
    pdfItem("OTHER", 180, 550),
    atRight("1", 300, 550),
    atRight("11", 400, 550),
    atRight("100", 600, 550),
  );
  const { draft } = extractMerrillHistory(pages);
  expect(draft.activityHoldingLabels[draft.evidence.activities[0].id]).toBeNull();
});
it("matches wrapped activity descriptions to holding labels and leaves unrecognized issuers unmatched", () => {
  const pages = historyFixture();
  const sale = pages[2].find((c) => c.str === "Synthetic equity" && c.transform[5] === 630)!;
  sale.str = "Synthetic";
  pages[2].push(pdfItem("equity", 79, 620));
  const transfer = pages[2].find((c) => c.str === "Synthetic equity" && c.transform[5] === 440)!;
  transfer.str = "Unrecognized issuer";
  const { draft } = extractMerrillHistory(pages);
  expect(draft.activityHoldingLabels[draft.evidence.activities[0].id]).toBe(
    "Synthetic equity DEMO",
  );
  expect(draft.activityHoldingLabels[draft.evidence.activities[1].id]).toBeUndefined();
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

it.each([
  { balanceCase: "equal", openingCash: "10" },
  { balanceCase: "different", openingCash: "(5)" },
])(
  "keeps reconciliation unknown without activity pages and $balanceCase balances",
  ({ openingCash }) => {
    const pages = historyFixture();
    pages.pop();
    pages[0].find((c) => c.str === "(5)")!.str = openingCash;
    const { draft } = extractMerrillHistory(pages);
    expect(draft.evidence.sections).toEqual({});
    expect(draft.evidence.activities).toEqual([]);
    expect(draft.issues).toContainEqual({ code: "missingActivitySections" });
    expect(draft.cashReconciliation).toEqual({
      knownMovementTotal: "0",
      difference: null,
      matches: null,
    });
    expect(draft.issues).not.toContainEqual({ code: "cashMismatch" });
    expect(draft.closing.totalValue).toBe("510");
  },
);
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

it("extracts multiple reconciled monthly drafts from a bundled multi-month PDF", () => {
  const april = historyFixture();
  for (const page of april) {
    for (const item of page) {
      item.str = item.str
        .replaceAll("April 30", "March 31")
        .replaceAll("May 01, 2025 - May 12, 2025", "April 01, 2025 - April 30, 2025")
        .replaceAll("May 12", "April 30")
        .replace(/^05\//, "04/");
    }
  }
  const may = historyFixture();
  // Bundle in reverse order (May first: pages 1-3, April second: pages 4-6)
  const results = extractMerrillHistoryStatements([...may, ...april]);
  expect(results).toHaveLength(2);
  expect(results[0].draft).toMatchObject({
    from: "2025-04-01",
    through: "2025-04-30",
    issues: [],
    openingCash: { value: "-5", source: { page: 4 } },
    closing: { date: "2025-04-30", sourcePage: 4 },
  });
  expect(results[0].draft.evidence.activities.every((a) => a.page === 6)).toBe(true);
  expect(results[1].draft).toMatchObject({
    from: "2025-05-01",
    through: "2025-05-12",
    issues: [],
    openingCash: { value: "-5", source: { page: 1 } },
    closing: { date: "2025-05-12", sourcePage: 1 },
  });
  expect(results[1].draft.evidence.activities.every((a) => a.page === 3)).toBe(true);
});

it("reconciles CASH/OTHER TRANSACTIONS NET TOTAL against net cash when both debits and credits occur", () => {
  const pages = historyFixture();
  // Opening cash -5, existing sale +15, plus cash debit 10 and credit 6 (net -4) -> closing cash 6.
  pages[0].find((c) => c.str === "10.00" && c.transform[5] === 580)!.str = "6.00";
  pages[0].find((c) => c.str === "510.00" && c.transform[5] === 520)!.str = "506.00";
  const cashPage = [
    ...activityHeader(),
    pdfItem("CASH/OTHER TRANSACTIONS", 37, 680),
    pdfItem("Date", 37, 650),
    pdfItem("Description", 75, 650),
    pdfItem("Transaction Type", 247, 650),
    atRight("Quantity", 509, 650),
    atRight("Debit", 626, 650),
    atRight("Credit", 749, 650),
    pdfItem("05/04", 37, 630),
    pdfItem("Electronic Transfer Out", 75, 630),
    pdfItem("Withdrawal", 247, 630),
    atRight("10.00", 626, 630),
    pdfItem("05/05", 37, 610),
    pdfItem("Electronic Transfer In", 75, 610),
    pdfItem("Deposit", 247, 610),
    atRight("6.00", 749, 610),
    pdfItem("NET TOTAL", 75, 590),
    atRight("4.00", 626, 590),
  ];
  pages.push(cashPage);
  const { draft } = extractMerrillHistory(pages);
  expect(draft.cashReconciliation).toEqual({
    knownMovementTotal: "11",
    difference: "0",
    matches: true,
  });
  expect(draft.issues).toEqual([]);

  // A mismatched NET TOTAL must still fail reconciliation.
  cashPage.find((c) => c.str === "4.00")!.str = "5.00";
  expect(() => extractMerrillHistory(pages)).toThrow("brokerageReconcile");
});

it("rejects a nonzero CASH/OTHER TRANSACTIONS NET TOTAL when zero cash rows were parsed", () => {
  const pages = historyFixture();
  pages.push([
    ...activityHeader(),
    pdfItem("CASH/OTHER TRANSACTIONS", 37, 680),
    pdfItem("Date", 37, 650),
    pdfItem("Description", 75, 650),
    pdfItem("Transaction Type", 247, 650),
    atRight("Quantity", 509, 650),
    atRight("Debit", 626, 650),
    atRight("Credit", 749, 650),
    pdfItem("NET TOTAL", 75, 590),
    atRight("4.00", 626, 590),
  ]);
  expect(() => extractMerrillHistory(pages)).toThrow("brokerageReconcile");
});

it("keeps Merrill CMA unsettled trades out of settled cash without blocking daily reconstruction", () => {
  const pages = historyFixture();
  pages.push([
    ...activityHeader(),
    pdfItem("UNSETTLED TRADES", 37, 680),
    pdfItem("Trade", 37, 660),
    pdfItem("Settlement", 104, 660),
    pdfItem("Date", 37, 650),
    pdfItem("Date", 104, 650),
    pdfItem("Description", 144, 650),
    pdfItem("Type", 450, 650),
    atRight("Quantity", 550, 650),
    atRight("Amount", 750, 650),
    pdfItem("05/12", 37, 630),
    pdfItem("05/14", 104, 630),
    pdfItem("Synthetic equity", 144, 630),
    pdfItem("Purchase", 450, 630),
    atRight("1", 550, 630),
    atRight("(10)", 750, 630),
    pdfItem("NET TOTAL", 144, 610),
    atRight("(10)", 750, 610),
  ]);
  const { draft } = extractMerrillHistory(pages);
  expect(draft.issues).toEqual([]);
  expect(draft.cashReconciliation).toEqual({
    knownMovementTotal: "15",
    difference: "0",
    matches: true,
  });
  expect(draft.evidence.activities.filter((r) => r.section === "pending")).toHaveLength(1);

  // If closing cash unexpectedly reflected the unsettled trade, cash reconciliation still catches it.
  pages[0].find((c) => c.str === "10.00" && c.transform[5] === 580)!.str = "0.00";
  pages[0].find((c) => c.str === "510.00" && c.transform[5] === 520)!.str = "500.00";
  const mismatched = extractMerrillHistory(pages).draft;
  expect(mismatched.cashReconciliation).toMatchObject({ difference: "10", matches: false });
  expect(mismatched.issues).toContainEqual({ code: "cashMismatch" });
});

it("blocks daily reconstruction when only informational sections such as UNSETTLED TRADES are present", () => {
  const pages = historyFixture();
  pages.pop(); // Drop settled activity page while keeping summary, holdings, and matching cash.
  pages[0].find((c) => c.str === "(5)")!.str = "10";
  pages.push([
    ...activityHeader(),
    pdfItem("UNSETTLED TRADES", 37, 680),
    pdfItem("Trade", 37, 660),
    pdfItem("Settlement", 104, 660),
    pdfItem("Date", 37, 650),
    pdfItem("Date", 104, 650),
    pdfItem("Description", 144, 650),
    pdfItem("Type", 450, 650),
    atRight("Quantity", 550, 650),
    atRight("Amount", 750, 650),
    pdfItem("05/12", 37, 630),
    pdfItem("05/14", 104, 630),
    pdfItem("Synthetic equity", 144, 630),
    pdfItem("Purchase", 450, 630),
    atRight("1", 550, 630),
    atRight("(10)", 750, 630),
  ]);
  const { draft } = extractMerrillHistory(pages);
  expect(draft.evidence.sections).toEqual({ pending: 1 });
  expect(draft.issues).toContainEqual({ code: "missingActivitySections" });
  expect(draft.cashReconciliation).toEqual({
    knownMovementTotal: "0",
    difference: null,
    matches: null,
  });
});

it("flags missing pages in numbered statements even when a settled cash section and unsettled trades remain", () => {
  const base = historyFixture();
  base.pop(); // Replace default activity page with separate dividend, transfer, and pending pages.
  base[0].find((c) => c.str === "(5)")!.str = "6"; // Opening cash 6 + dividend 4 = closing cash 10.
  const dividendPage = [
    ...activityHeader(),
    pdfItem("DIVIDENDS/INTEREST INCOME TRANSACTIONS", 37, 680),
    pdfItem("Date", 37, 650),
    pdfItem("Description", 70, 650),
    pdfItem("Transaction Type", 250, 650),
    atRight("Quantity", 430, 650),
    atRight("Income", 673, 650),
    atRight("Year To Date", 750, 650),
    pdfItem("05/07", 37, 630),
    pdfItem("Synthetic dividend", 70, 630),
    pdfItem("Dividend", 250, 630),
    atRight("4.00", 673, 630),
  ];
  const transferPage = [
    ...activityHeader(),
    pdfItem("SECURITIES YOU TRANSFERRED IN/OUT", 37, 680),
    pdfItem("Date", 37, 650),
    pdfItem("Description", 91, 650),
    pdfItem("Transaction Type", 293, 650),
    atRight("Quantity", 506, 650),
    atRight("Value of Securities", 672, 650),
    pdfItem("05/02", 37, 630),
    pdfItem("Synthetic equity", 91, 630),
    pdfItem("Security Transfer In", 293, 630),
    atRight("2", 506, 630),
    atRight("200", 672, 630),
  ];
  const pendingPage = [
    ...activityHeader(),
    pdfItem("UNSETTLED TRADES", 37, 680),
    pdfItem("Trade", 37, 660),
    pdfItem("Settlement", 104, 660),
    pdfItem("Date", 37, 650),
    pdfItem("Date", 104, 650),
    pdfItem("Description", 144, 650),
    pdfItem("Type", 450, 650),
    atRight("Quantity", 550, 650),
    atRight("Amount", 750, 650),
    pdfItem("05/12", 37, 630),
    pdfItem("05/14", 104, 630),
    pdfItem("Synthetic equity", 144, 630),
    pdfItem("Purchase", 450, 630),
    atRight("1", 550, 630),
    atRight("(10)", 750, 630),
    pdfItem("NET TOTAL", 144, 610),
    atRight("(10)", 750, 610),
  ];
  const fullPages = [...base, dividendPage, transferPage, pendingPage].map((page, index, all) => [
    ...page,
    atRight(`${index + 1} of ${all.length}`, 780, 20),
  ]);

  const complete = extractMerrillHistory(fullPages).draft;
  expect(complete.issues).toEqual([]);
  expect(complete.cashReconciliation).toEqual({
    knownMovementTotal: "4",
    difference: "0",
    matches: true,
  });

  // Omit page 4 of 5 (the in-kind transfer page) while retaining dividend + unsettled trades.
  const truncatedPages = [fullPages[0], fullPages[1], fullPages[2], fullPages[4]];
  const truncated = extractMerrillHistory(truncatedPages).draft;
  expect(truncated.evidence.sections).toEqual({ income: 1, pending: 1 });
  expect(truncated.issues).toContainEqual({ code: "missingActivitySections" });
  expect(truncated.cashReconciliation).toEqual({
    knownMovementTotal: "4",
    difference: null,
    matches: null,
  });

  // Split footer items ("Page", "4", "of", "5" and bare "4", "of", "5") with a trailing label
  // and a statement form reference row below the page number must also be recognized.
  for (const includePageWord of [true, false]) {
    const splitFooterPages = [...base, dividendPage, transferPage, pendingPage].map(
      (page, index, all) => [
        ...page,
        ...(includePageWord ? [pdfItem("Page", 760, 30)] : []),
        pdfItem(String(index + 1), 790, 30),
        pdfItem("of", 805, 30),
        pdfItem(String(all.length), 820, 30),
        pdfItem("Merrill CMA", 840, 30),
        pdfItem("Form 018-CMA", 36, 10),
      ],
    );
    const splitTruncated = extractMerrillHistory([
      splitFooterPages[0],
      splitFooterPages[1],
      splitFooterPages[2],
      splitFooterPages[4],
    ]).draft;
    expect(splitTruncated.issues).toContainEqual({ code: "missingActivitySections" });
    expect(splitTruncated.cashReconciliation).toEqual({
      knownMovementTotal: "4",
      difference: null,
      matches: null,
    });
  }

  // Unnumbered cover sheets are allowed both before a single statement and between
  // complete numbered statements in a multi-month bundle.
  const coverSheet = [pdfItem("Merrill", 36, 780), pdfItem("Account Summary Cover", 36, 740)];
  const withCover = extractMerrillHistoryStatements([coverSheet, ...fullPages]);
  expect(withCover).toHaveLength(1);
  expect(withCover[0].draft.issues).toEqual([]);
  expect(withCover[0].draft.cashReconciliation).toEqual({
    knownMovementTotal: "4",
    difference: "0",
    matches: true,
  });

  const aprilPages = structuredClone(fullPages);
  for (const page of aprilPages) {
    for (const item of page) {
      item.str = item.str
        .replaceAll("April 30", "March 31")
        .replaceAll("May 01, 2025 - May 12, 2025", "April 01, 2025 - April 30, 2025")
        .replaceAll("May 12", "April 30")
        .replace(/^05\//, "04/");
    }
  }
  const bundledWithCovers = extractMerrillHistoryStatements([
    structuredClone(coverSheet),
    ...aprilPages,
    structuredClone(coverSheet),
    ...fullPages,
  ]);
  expect(bundledWithCovers).toHaveLength(2);
  expect(bundledWithCovers[0].draft).toMatchObject({
    from: "2025-04-01",
    through: "2025-04-30",
    issues: [],
    cashReconciliation: { knownMovementTotal: "4", difference: "0", matches: true },
  });
  expect(bundledWithCovers[1].draft).toMatchObject({
    from: "2025-05-01",
    through: "2025-05-12",
    issues: [],
    cashReconciliation: { knownMovementTotal: "4", difference: "0", matches: true },
  });

  // Unnumbered boundary pages carrying statement activity or holdings must still fail completeness.
  const undatedAssetsBoundary = [pdfItem("Merrill", 36, 780), pdfItem("CMA® ASSETS", 36, 720)];
  for (const boundaryPage of [transferPage, undatedAssetsBoundary]) {
    expect(extractMerrillHistory([...fullPages, boundaryPage]).draft.issues).toContainEqual({
      code: "missingActivitySections",
    });
  }
});
