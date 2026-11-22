import { describe, it, expect } from "vitest";
import type { PdfTextItem } from "@/pages/settings/accounts/components/basis-pdf";
import { atRight, pdfItem } from "@/pages/settings/accounts/components/test-fixtures/merrill-basis";
import { monthlyFixture } from "./test-fixtures/monthly-statement";
import { extractMonthlySchwab } from "./schwab-statement";
import { extractSchwabHistory } from "./schwab-history";
import { schwabNumber, schwabPeriod } from "./schwab-format";
import { extractMonthlyStatement, extractStatementHistory } from "./statement-parser";

function schwabFixture(): PdfTextItem[][] {
  const page1: PdfTextItem[] = [
    pdfItem("Schwab One® Account", 78, 566),
    pdfItem("Account Number", 336, 541),
    pdfItem("Statement Period", 408, 541),
    pdfItem("1688-6083", 336, 528),
    pdfItem("October 1-31, 2025", 408, 528),
    pdfItem("Account Summary", 408, 501),
    atRight("This Statement", 678, 267),
    atRight("YTD", 770, 267),
    pdfItem("Beginning Account Value", 413, 249),
    atRight("$1,000.00", 678, 249),
    atRight("$900.00", 770, 249),
    pdfItem("Ending Account Value", 413, 126),
    atRight("$1,250.00", 678, 126),
    atRight("$1,250.00", 770, 126),
  ];

  const page2: PdfTextItem[] = [
    pdfItem("Schwab One® Account", 78, 566),
    pdfItem("Statement Period", 408, 541),
    pdfItem("October 1-31, 2025", 408, 528),
    pdfItem("Cash and Cash Investments", 18, 216),
    atRight("Beginning", 453, 204),
    atRight("Ending", 535, 204),
    pdfItem("Total Cash and Cash Investments", 23, 130),
    atRight("$200.00", 453, 130),
    atRight("$150.00", 535, 130),
  ];

  const page3: PdfTextItem[] = [
    pdfItem("Schwab One® Account", 78, 566),
    pdfItem("Statement Period", 408, 541),
    pdfItem("October 1-31, 2025", 408, 528),
    pdfItem("Positions - Equities", 18, 505),
    pdfItem("Symbol", 18, 486),
    pdfItem("Description", 60, 486),
    atRight("Quantity", 338, 486),
    atRight("Price($)", 396, 486),
    atRight("Market Value($)", 475, 486),
    atRight("Cost Basis($)", 554, 486),
    pdfItem("AAPL", 18, 469),
    pdfItem("APPLE INC", 60, 469),
    atRight("4.0000", 338, 469),
    atRight("200.00000", 396, 469),
    atRight("800.00", 475, 469),
    atRight("600.00", 554, 469),
    pdfItem("Total Equities", 22, 433),
    atRight("$800.00", 475, 433),
    pdfItem("Positions - Exchange Traded Funds", 18, 225),
    pdfItem("Symbol", 18, 205),
    pdfItem("Description", 61, 205),
    atRight("Quantity", 339, 205),
    atRight("Price($)", 398, 205),
    atRight("Market Value($)", 474, 205),
    atRight("Cost Basis($)", 550, 205),
    pdfItem("VCEB", 18, 153),
    pdfItem("VANGUARD ESG U.S.", 61, 153),
    atRight("5.0000", 339, 153),
    atRight("60.00000", 398, 153),
    atRight("300.00", 474, 153),
    atRight("N/A", 546, 153),
    pdfItem("Total Exchange Traded Funds", 22, 130),
    atRight("$300.00", 474, 130),
  ];

  // Opening cash 200 - purchase 100 + dividend 50 = closing cash 150.
  // In-kind Account Transfer (VCEB 5 shares, value 300) does not affect cash.
  const page4: PdfTextItem[] = [
    pdfItem("Schwab One® Account", 78, 566),
    pdfItem("Statement Period", 408, 541),
    pdfItem("October 1-31, 2025", 408, 528),
    pdfItem("Transaction Details", 18, 505),
    pdfItem("Date", 18, 482),
    pdfItem("Category", 45, 482),
    pdfItem("Action", 96, 482),
    pdfItem("CUSIP", 175, 482),
    pdfItem("Description", 248, 482),
    atRight("Quantity", 491, 482),
    atRight("Interest($)", 611, 482),
    atRight("Amount($)", 688, 482),
    pdfItem("10/02 Purchase", 18, 465),
    pdfItem("AAPL", 175, 465),
    pdfItem("APPLE INC", 248, 465),
    atRight("0.5000", 491, 465),
    atRight("(100.00)", 692, 465),
    pdfItem("Dividend", 45, 448),
    pdfItem("Qual. Dividend", 96, 448),
    pdfItem("AAPL", 175, 448),
    pdfItem("APPLE INC", 248, 448),
    atRight("50.00", 692, 448),
    pdfItem("10/15 Other", 18, 430),
    pdfItem("Account Transfer", 96, 430),
    pdfItem("VCEB", 175, 430),
    pdfItem("VANGUARD ESG U.S.", 248, 430),
    atRight("5.0000", 491, 430),
    atRight("300.00", 692, 430),
    pdfItem("Activity", 45, 420),
    pdfItem("Total Transactions", 23, 390),
    atRight("$250.00", 691, 390),
    pdfItem("Bank Sweep Activity", 18, 350),
    pdfItem("Pending / Open Activity", 18, 250),
    pdfItem("Pending", 18, 220),
  ];

  return [page1, page2, page3, page4];
}

describe("Charles Schwab statement format helpers", () => {
  it("parses same-month and cross-month statement periods", () => {
    expect(schwabPeriod("October 1-31, 2025")).toEqual({
      from: "2025-10-01",
      through: "2025-10-31",
    });
    expect(schwabPeriod("September 30 - October 31, 2025")).toEqual({
      from: "2025-09-30",
      through: "2025-10-31",
    });
    expect(schwabPeriod("December 15, 2024 - January 15, 2025")).toEqual({
      from: "2024-12-15",
      through: "2025-01-15",
    });
    expect(() => schwabPeriod("October 31-1, 2025")).toThrow();
  });

  it("parses Schwab currency, parenthesized negatives, and N/A", () => {
    expect(schwabNumber("$10,290,455.92")).toBe("10290455.92");
    expect(schwabNumber("($110,000.00)")).toBe("-110000");
    expect(schwabNumber("(40.0000)")).toBe("-40");
    expect(schwabNumber("N/A")).toBeNull();
    expect(schwabNumber("--")).toBeNull();
  });
});

describe("Charles Schwab monthly statement observations", () => {
  it("extracts closing total, cash, and positions without leaking account number", () => {
    const { identity, snapshot } = extractMonthlySchwab(schwabFixture());
    expect(identity).toBe("16886083");
    expect(snapshot).toMatchObject({
      date: "2025-10-31",
      currency: "USD",
      totalValue: "1250",
      cashValue: "150",
      positionsComplete: true,
      format: "schwab-monthly-v1",
      sourcePage: 1,
    });
    expect(snapshot.positions).toEqual([
      { label: "AAPL APPLE INC", quantity: "4", marketValue: "800", costBasis: "600" },
      { label: "VCEB VANGUARD ESG U.S.", quantity: "5", marketValue: "300", costBasis: null },
    ]);
    expect(JSON.stringify(snapshot)).not.toContain("1688-6083");
    expect(JSON.stringify(snapshot)).not.toContain("16886083");
  });

  it("marks positionsComplete false when positions do not reconcile to Ending Account Value", () => {
    const pages = schwabFixture();
    pages[0].find((c) => c.str === "$1,250.00" && c.transform[5] === 126)!.str = "$1,300.00";
    const { snapshot } = extractMonthlySchwab(pages);
    expect(snapshot.totalValue).toBe("1300");
    expect(snapshot.positionsComplete).toBe(false);
  });
});

describe("Charles Schwab daily statement history", () => {
  it("reconciles settlement-date cash, keeps in-kind transfers non-cash, and ignores pending activity", () => {
    const { identity, draft } = extractSchwabHistory(schwabFixture());
    expect(identity).toBe("16886083");
    expect(draft.format).toBe("schwab-history-v1");
    expect(draft.from).toBe("2025-10-01");
    expect(draft.through).toBe("2025-10-31");
    expect(draft.openingCash?.value).toBe("200");
    expect(draft.cashReconciliation).toEqual({
      knownMovementTotal: "-50",
      difference: "0",
      matches: true,
    });
    expect(draft.issues).toEqual([]);
    expect(draft.evidence.activities.map((a) => [a.date, a.action, a.quantity, a.cash])).toEqual([
      ["2025-10-02", "BUY", "0.5", "-100"],
      ["2025-10-02", "INCOME", null, "50"],
      ["2025-10-15", "TRANSFER_IN", "5", null],
    ]);
  });

  it("flags cashMismatch when opening cash does not reconcile with settled movements", () => {
    const pages = schwabFixture();
    pages[1].find((c) => c.str === "$200.00")!.str = "$205.00";
    const { draft } = extractSchwabHistory(pages);
    expect(draft.cashReconciliation).toMatchObject({ difference: "5", matches: false });
    expect(draft.issues).toContainEqual({ code: "cashMismatch" });
  });

  it("blocks daily import when Transaction Details header is skipped even if Bank Sweep is present and cash matches", () => {
    const pages = schwabFixture();
    pages[1].find((c) => c.str === "$200.00")!.str = "$150.00";
    pages[3].find((c) => c.str === "Amount($)")!.str = "Amount ($)";
    const { draft } = extractSchwabHistory(pages);
    expect(draft.evidence.activities).toEqual([]);
    expect(draft.cashReconciliation.matches).toBeNull();
    expect(draft.issues).toContainEqual({ code: "missingActivitySections" });
  });

  it.each(["Spinoff", "Stock Split"])(
    "flags %s corporate actions as unsupported in daily mode",
    (actionLabel) => {
      const pages = schwabFixture();
      pages[3].find((c) => c.str === "Account Transfer")!.str = actionLabel;
      const { draft } = extractSchwabHistory(pages);
      const row = draft.evidence.activities.find((a) => a.section === "transfers");
      expect(row?.action).toBe("UNSUPPORTED");
      expect(draft.issues).toEqual(
        expect.arrayContaining([expect.objectContaining({ code: "unsupportedActivity" })]),
      );
    },
  );

  it("allows a verified cash-only close after selling the final position", () => {
    const pages = schwabFixture();
    // Ending value = $1,000 cash only; opening cash $200 + sale $800 = $1,000.
    pages[0].find((c) => c.str === "$1,250.00" && c.transform[5] === 126)!.str = "$1,000.00";
    pages[1].find((c) => c.str === "$150.00")!.str = "$1,000.00";
    pages.splice(2, 1); // No closing Positions sections.
    pages[2] = [
      pdfItem("Schwab One® Account", 78, 566),
      pdfItem("Statement Period", 408, 541),
      pdfItem("October 1-31, 2025", 408, 528),
      pdfItem("Transaction Details", 18, 505),
      pdfItem("Date", 18, 482),
      pdfItem("Category", 45, 482),
      pdfItem("Action", 96, 482),
      pdfItem("CUSIP", 175, 482),
      pdfItem("Description", 248, 482),
      atRight("Quantity", 491, 482),
      atRight("Amount($)", 688, 482),
      pdfItem("10/10 Sale", 18, 465),
      pdfItem("AAPL", 175, 465),
      pdfItem("APPLE INC", 248, 465),
      atRight("(4.0000)", 491, 465),
      atRight("800.00", 692, 465),
      pdfItem("Total Transactions", 23, 390),
      atRight("$800.00", 691, 390),
    ];
    const { draft } = extractSchwabHistory(pages);
    expect(draft.closing.positions).toEqual([]);
    expect(draft.closing.positionsComplete).toBe(true);
    expect(draft.cashReconciliation).toEqual({
      knownMovementTotal: "800",
      difference: "0",
      matches: true,
    });
    expect(draft.issues).toEqual([]);
  });
});

describe("statement parser dispatcher", () => {
  it("routes Merrill and Schwab statements cleanly and namespaces identities", () => {
    const merrill = extractMonthlyStatement(monthlyFixture());
    const schwab = extractMonthlyStatement(schwabFixture());
    expect(merrill.identity).toBe("merrill:SYNTH01");
    expect(merrill.snapshot.format).toBe("merrill-cma-monthly-v1");
    expect(schwab.identity).toBe("schwab:16886083");
    expect(schwab.snapshot.format).toBe("schwab-monthly-v1");

    const schwabDaily = extractStatementHistory(schwabFixture());
    expect(schwabDaily.identity).toBe("schwab:16886083");
    expect(schwabDaily.draft.issues).toEqual([]);
  });

  it("does not misclassify a Merrill statement holding CHARLES SCHWAB CORP as ambiguous", () => {
    const pages = monthlyFixture();
    pages[1].find((c) => c.str === "Synthetic equity")!.str = "CHARLES SCHWAB CORP";
    const parsed = extractMonthlyStatement(pages);
    expect(parsed.identity).toBe("merrill:SYNTH01");
    expect(parsed.snapshot.format).toBe("merrill-cma-monthly-v1");
  });

  it("rejects unrecognized or ambiguous brokerage statements", () => {
    expect(() => extractMonthlyStatement([[pdfItem("Unknown Broker", 10, 10)]])).toThrow();
    const mixed = schwabFixture();
    mixed[0].push(
      pdfItem("Merrill", 10, 10),
      pdfItem("Account Number: SYNTH-01", 10, 20),
      pdfItem("October 01, 2025 - October 31, 2025", 10, 30),
    );
    expect(() => extractMonthlyStatement(mixed)).toThrow();
  });
});
