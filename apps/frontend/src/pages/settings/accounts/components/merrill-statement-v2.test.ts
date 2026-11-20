import { describe, it, expect } from "vitest";
import { extractMerrillStatementV2 } from "./merrill-statement-v2";
import { extractMerrillStatement } from "./merrill-basis-pdf";
import {
  merrillActivityFixture,
  activityHeader,
  tradesHeader,
  saleRow,
  saleHeader,
} from "./test-fixtures/merrill-statement-v2";
import { pdfItem, atRight, merrillPdfFixture } from "./test-fixtures/merrill-basis";

describe("Merrill statement activity v2", () => {
  it("keeps v1 positions and extracts later activity, sale basis and transfer value separately", () => {
    const pages = merrillActivityFixture(),
      r = extractMerrillStatementV2(pages);
    expect(r.positionsCsv).toBe(extractMerrillStatement(pages));
    expect(r.evidence.sections).toEqual({ securities: 1, realized: 1, transfers: 1 });
    expect(r.evidence.activities[0]).toMatchObject({
      security: "DEMO",
      date: "2025-05-06",
      dateKind: "settlement",
      action: "SELL",
      cash: "15",
      quantity: "1",
      page: 2,
    });
    expect(r.evidence.dispositions[0]).toMatchObject({
      security: "DEMO",
      date: "2025-05-06",
      acquired: "2020-01-01",
      basis: "9",
      proceeds: "15",
    });
    expect(r.evidence.activities[1]).toMatchObject({
      action: "TRANSFER_IN",
      transferValue: "200",
      cash: null,
    });
  });
  it("does not depend on page number, and keeps liquidation separate from acquisition and YTD", () => {
    const pages = merrillActivityFixture();
    pages.splice(1, 0, []);
    const r = extractMerrillStatementV2(pages).evidence;
    expect(r.dispositions[0]).toMatchObject({ page: 3, basis: "9", date: "2025-05-06" });
  });
  it("reconciles totals across repeated section headings and pages", () => {
    const pages = merrillActivityFixture();
    const trade = pages[1].filter((c) => c.transform[5] >= 630);
    pages.splice(1, 1, trade, [
      ...activityHeader(),
      pdfItem("SECURITY TRANSACTIONS", 37, 680),
      ...tradesHeader(650),
      pdfItem("TOTAL", 37, 610),
      atRight("15.00", 673, 610),
    ]);
    expect(extractMerrillStatementV2(pages).evidence.activities).toHaveLength(1);
  });
  it("preserves unavailable sale basis as unknown", () => {
    const pages = [
      ...merrillPdfFixture(),
      [
        ...activityHeader(),
        pdfItem("REALIZED GAINS/(LOSSES)", 37, 580),
        ...saleHeader(550),
        ...saleRow(530, "1", "--"),
        pdfItem("TOTAL", 37, 510),
        atRight("25", 566, 510),
      ],
    ];
    expect(extractMerrillStatementV2(pages).evidence.dispositions[0].basis).toBeNull();
  });
  it("never invents identity from quantity", () => {
    const pages = merrillActivityFixture();
    const item = pages[1].find((c) => c.str === "Synthetic equity" && c.transform[5] === 530)!;
    item.str = "Unrecognized issuer";
    expect(extractMerrillStatementV2(pages).evidence.dispositions[0].security).toBeUndefined();
  });
  it.each(["Security Split", "Reinvestment", "Return of Capital"])(
    "preserves unsupported basis-changing action %s",
    (type) => {
      const pages = merrillActivityFixture();
      pages[1].find((c) => c.str === "Security Transfer In")!.str = type;
      expect(extractMerrillStatementV2(pages).evidence.activities[1].action).toBe("UNSUPPORTED");
    },
  );
  it.each([
    [
      "account",
      (p: ReturnType<typeof merrillActivityFixture>) => {
        p[1].find((c) => c.str === "Account Number: SYNTH-01")!.str = "Account Number: OTHER-02";
      },
    ],
    [
      "period",
      (p: ReturnType<typeof merrillActivityFixture>) => {
        p[1].find((c) => c.str.includes("May 01"))!.str = "June 01, 2025 - June 12, 2025";
      },
    ],
    [
      "header",
      (p: ReturnType<typeof merrillActivityFixture>) => {
        p[1].find((c) => c.str === "Cost Basis")!.str = "Market Value";
      },
    ],
    [
      "total",
      (p: ReturnType<typeof merrillActivityFixture>) => {
        p[1].find((c) => c.str === "15.00" && c.transform[5] === 610)!.str = "25.00";
      },
    ],
    [
      "double negative",
      (p: ReturnType<typeof merrillActivityFixture>) => {
        p[1].find((c) => c.str === "9.00" && c.transform[5] === 530)!.str = "(-9.00)";
      },
    ],
    [
      "missing date",
      (p: ReturnType<typeof merrillActivityFixture>) => {
        p[1] = p[1].filter((c) => !(c.str === "05/06" && c.transform[5] === 630));
      },
    ],
  ])("rejects unsafe %s with safe source location", (_, mutate) => {
    const pages = merrillActivityFixture();
    mutate(pages);
    expect(() => extractMerrillStatementV2(pages)).toThrow();
  });
  it("rejects a recognized section with missing columns", () => {
    expect(() =>
      extractMerrillStatementV2([
        ...merrillPdfFixture(),
        [...activityHeader(), pdfItem("SECURITY TRANSACTIONS", 37, 680)],
      ]),
    ).toThrow();
  });
  it("parses two cash-sweep columns as informational cash, never basis", () => {
    const page = [...activityHeader(), pdfItem("YOUR CMA MONEY ACCOUNT TRANSACTIONS", 37, 680)];
    for (const x of [0, 360])
      page.push(
        pdfItem("Date", 40 + x, 650),
        pdfItem("Description", 84 + x, 650),
        atRight("Withdrawals", 260 + x, 650),
        atRight("Deposits", 340 + x, 650),
        pdfItem("05/02", 40 + x, 630),
        pdfItem("Sweep", 84 + x, 630),
        atRight("10", 340 + x, 630),
      );
    const r = extractMerrillStatementV2([...merrillPdfFixture(), page]).evidence;
    expect(r.sections.sweep).toBe(2);
    expect(r.activities.every((x) => x.action === "CASH" && x.transferValue === null)).toBe(true);
  });
});

it("rejects an unfamiliar activity layout rather than silently reporting no sales", () => {
  expect(() =>
    extractMerrillStatementV2([
      ...merrillPdfFixture(),
      [...activityHeader(), pdfItem("UNRECOGNIZED ACTIVITY FORMAT", 37, 680)],
    ]),
  ).toThrow("brokerageFormat");
});
it("recognizes shifted columns and wrapped descriptions without fuzzy identity matching", () => {
  const pages = merrillActivityFixture();
  for (const c of pages[1]) c.transform[4] += 12;
  const sale = pages[1].find((c) => c.str === "Synthetic equity" && c.transform[5] === 530)!;
  sale.str = "Synthetic";
  pages[1].push(pdfItem("equity", 49, 520));
  expect(extractMerrillStatementV2(pages).evidence.dispositions[0]).toMatchObject({
    description: "Synthetic equity",
    security: "DEMO",
  });
});
it("keeps unsettled trades separate from settled history", () => {
  const page = [
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
  ];
  const e = extractMerrillStatementV2([...merrillPdfFixture(), page]).evidence;
  expect(e.activities[0]).toMatchObject({
    section: "pending",
    dateKind: "trade",
    date: "2025-05-12",
    cash: "-10",
    action: "UNSUPPORTED",
  });
  expect(e.dispositions).toHaveLength(0);
});
it("detects duplicated sale evidence instead of adding its cost twice", () => {
  const pages = merrillActivityFixture();
  pages[1].push(...saleRow(520));
  expect(() => extractMerrillStatementV2(pages)).toThrow();
});

it("extracts income and cash but never treats subtotals or YTD as new activity", () => {
  const page = [
    ...activityHeader(),
    pdfItem("DIVIDENDS/INTEREST INCOME TRANSACTIONS", 37, 680),
    pdfItem("Date", 37, 650),
    pdfItem("Description", 70, 650),
    pdfItem("Transaction Type", 250, 650),
    atRight("Quantity", 430, 650),
    atRight("Income", 673, 650),
    atRight("Year To Date", 750, 650),
    pdfItem("Synthetic dividend", 70, 630),
    pdfItem("Dividend", 250, 630),
    atRight("2", 673, 630),
    atRight("99", 750, 630),
    pdfItem("Sub Total", 70, 610),
    atRight("2", 673, 610),
    atRight("99", 750, 610),
    pdfItem("CASH/OTHER TRANSACTIONS", 37, 580),
    pdfItem("Date", 37, 550),
    pdfItem("Description", 70, 550),
    pdfItem("Transaction Type", 250, 550),
    atRight("Quantity", 430, 550),
    atRight("Debit", 673, 550),
    atRight("Credit", 750, 550),
    pdfItem("05/02", 37, 530),
    pdfItem("Cash payment", 70, 530),
    pdfItem("Withdrawal", 250, 530),
    atRight("4", 673, 530),
  ];
  const e = extractMerrillStatementV2([...merrillPdfFixture(), page]).evidence;
  expect(e.sections).toEqual({ income: 1, cash: 1 });
  expect(e.activities).toMatchObject([
    { action: "INCOME", date: null, cash: "2" },
    { action: "CASH", cash: "-4", date: "2025-05-02" },
  ]);
});

it("stops when financial cells no longer align with the recognized header", () => {
  const pages = merrillActivityFixture();
  for (const c of pages[1]) if (c.transform[5] === 530) c.transform[4] += 15;
  expect(() => extractMerrillStatementV2(pages)).toThrow();
});
