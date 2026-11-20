import { atRight, merrillPdfFixture, pdfItem } from "./merrill-basis";
import type { PdfTextItem } from "../basis-pdf";

// Invented securities, dates, identity and amounts; layout only resembles the export.
export function activityHeader() {
  return [
    pdfItem("Merrill", 36, 780),
    pdfItem("Account Number: SYNTH-01", 36, 760),
    pdfItem("May 01, 2025 - May 12, 2025", 400, 740),
    pdfItem("YOUR", 36, 720),
    pdfItem("CMA TRANSACTIONS", 80, 720),
  ];
}
export function tradesHeader(y: number) {
  return [
    pdfItem("Settlement", 36, y + 10),
    pdfItem("Date", 36, y),
    pdfItem("Description", 84, y),
    pdfItem("Transaction Type", 220, y),
    atRight("Quantity", 430, y),
    atRight("Amount", 510, y),
    atRight("Trading Fees", 590, y),
    atRight("Credit", 673, y),
  ];
}
export function saleHeader(y: number) {
  return [
    pdfItem("Acquired", 324, y + 10),
    pdfItem("Liquidation", 374, y + 10),
    pdfItem("Description", 37, y),
    atRight("Quantity", 306, y),
    atRight("Date", 354, y),
    atRight("Date", 402, y),
    atRight("Sale Amount", 485, y),
    atRight("Cost Basis", 566, y),
    atRight("Year to Date", 750, y),
  ];
}
export function saleRow(
  y: number,
  quantity = "1",
  basis = "9.00",
  proceeds = "15.00",
): PdfTextItem[] {
  return [
    pdfItem("Synthetic equity", 37, y),
    atRight(quantity, 308, y),
    atRight("01/01/20", 356, y),
    atRight("05/06/25", 404, y),
    atRight(proceeds, 488, y),
    atRight(basis, 569, y),
    atRight("999.99", 750, y),
  ];
}
export function merrillActivityFixture(): PdfTextItem[][] {
  return [
    ...merrillPdfFixture(),
    [
      ...activityHeader(),
      pdfItem("SECURITY TRANSACTIONS", 37, 680),
      ...tradesHeader(650),
      pdfItem("05/06", 37, 630),
      pdfItem("Synthetic equity", 79, 630),
      pdfItem("Sale", 220, 630),
      atRight("1", 432, 630),
      atRight("15.00", 675, 630),
      pdfItem("TOTAL", 37, 610),
      atRight("15.00", 673, 610),
      pdfItem("REALIZED GAINS/(LOSSES)", 37, 580),
      ...saleHeader(550),
      ...saleRow(530),
      pdfItem("TOTAL", 37, 510),
      atRight("9.00", 566, 510),
      atRight("15.00", 485, 510),
      pdfItem("SECURITIES YOU TRANSFERRED IN/OUT", 37, 480),
      pdfItem("Date", 37, 460),
      pdfItem("Description", 91, 460),
      pdfItem("Transaction Type", 293, 460),
      atRight("Quantity", 506, 460),
      atRight("Value of Securities", 672, 460),
      pdfItem("05/02", 37, 440),
      pdfItem("Synthetic equity", 91, 440),
      pdfItem("Security Transfer In", 293, 440),
      atRight("2", 506, 440),
      atRight("200", 672, 440),
    ],
  ];
}

export function cashActivityPage(
  section: "cash" | "income",
  type = "Withdrawal",
  quantity = "--",
): PdfTextItem[] {
  return [
    ...activityHeader(),
    pdfItem(
      section === "cash" ? "CASH/OTHER TRANSACTIONS" : "DIVIDENDS/INTEREST INCOME TRANSACTIONS",
      37,
      680,
    ),
    pdfItem("Date", 37, 650),
    pdfItem("Description", 70, 650),
    pdfItem("Transaction Type", 250, 650),
    atRight("Quantity", 430, 650),
    ...(section === "cash"
      ? [atRight("Debit", 673, 650), atRight("Credit", 750, 650)]
      : [atRight("Income", 673, 650), atRight("Year To Date", 750, 650)]),
    pdfItem("05/02", 37, 630),
    pdfItem("Synthetic equity", 70, 630),
    pdfItem(type, 250, 630),
    atRight(quantity, 430, 630),
    atRight("4", 673, 630),
  ];
}
