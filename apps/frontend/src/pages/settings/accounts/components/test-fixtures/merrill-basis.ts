import type { PdfTextItem } from "../basis-pdf";
import { MERRILL_ACTIVITY_HEADERS } from "../merrill-basis-csv";

// Entirely invented records. No customer account, security or amount is copied.
export function merrillCsvFixture() {
  return {
    headers: [...MERRILL_ACTIVITY_HEADERS],
    rows: [
      [
        "5/1/2025",
        "5/2/2025",
        "Settled",
        "Synthetic account",
        "Synthetic owner",
        "SYNTH-01",
        "DividendAndInterest",
        "DIVIDEND",
        "Synthetic issuer",
        "DEMO",
        "--",
        "--",
        "2.00",
      ],
      [
        "5/2/2025",
        "5/5/2025",
        "Settled",
        "Synthetic account",
        "Synthetic owner",
        "SYNTH-01",
        "SecurityTransactions",
        "PURCHASE",
        "Synthetic equity",
        "DEMO",
        "2",
        "10",
        "(21.00)",
      ],
    ],
  };
}
export function pdfItem(str: string, x: number, y: number, width = 30): PdfTextItem {
  return { str, transform: [1, 0, 0, 1, x, y], width };
}
export const atRight = (str: string, right: number, y: number) => pdfItem(str, right - 30, y);
export function statementHeader(account = "SYNTH-01", period = "May 01, 2025 - May 12, 2025") {
  return [
    pdfItem("Merrill", 36, 780),
    pdfItem(`Account Number: ${account}`, 36, 760),
    pdfItem("CMA® ASSETS", 36, 720),
    pdfItem(period, 400, 720),
  ];
}
export function positionHeader(y: number, symbol = true) {
  return [
    atRight("Total", 400, y + 10),
    pdfItem("Description", 36, y),
    ...(symbol ? [pdfItem("Symbol", 180, y)] : []),
    atRight("Quantity", 300, y),
    atRight("Cost Basis", 400, y),
    atRight("Market Price", 500, y),
    atRight("Market Value", 600, y),
  ];
}
export function merrillPdfFixture(): PdfTextItem[][] {
  return [
    [
      ...statementHeader(),
      pdfItem("EQUITIES", 36, 600),
      ...positionHeader(590),
      pdfItem("Synthetic equity", 36, 570),
      pdfItem("DEMO", 180, 570),
      atRight("2", 300, 570),
      atRight("21.00", 400, 570),
      atRight("100.00", 500, 570),
      atRight("200.00", 600, 570),
      pdfItem("TOTAL", 36, 540),
      atRight("21.00", 400, 540),
      atRight("200.00", 600, 540),
      pdfItem("MUTUAL FUNDS/CLOSED END FUNDS/UITs/ETPs", 36, 500),
      ...positionHeader(490, false),
      pdfItem("Synthetic fund", 36, 470),
      atRight("3", 300, 470),
      atRight("45.00", 400, 470),
      atRight("100.00", 500, 470),
      atRight("300.00", 600, 470),
      pdfItem("CURRENT YIELD 1.00% SYMBOL: FUNDX INITIAL PURCHASE DATE 01/01/2020", 36, 456, 400),
      pdfItem("TOTAL", 36, 440),
      atRight("45.00", 400, 440),
      atRight("300.00", 600, 440),
    ],
  ];
}
