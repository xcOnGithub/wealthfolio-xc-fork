import {
  merrillPdfFixture,
  pdfItem,
  atRight,
} from "@/pages/settings/accounts/components/test-fixtures/merrill-basis";
export function monthlyFixture() {
  const pages = merrillPdfFixture();
  pages.unshift([
    pdfItem("Merrill", 36, 780),
    pdfItem("Account Number: SYNTH-01", 36, 760),
    pdfItem("May 01, 2025 - May 12, 2025", 400, 720),
    atRight("May 12", 600, 600),
    atRight("April 30", 700, 600),
    pdfItem("Cash/Money Accounts", 36, 580),
    atRight("10.00", 600, 580),
    atRight("5.00", 700, 580),
    pdfItem("TOTAL ASSETS", 36, 560),
    atRight("510.00", 600, 560),
    pdfItem("TOTAL LIABILITIES", 36, 540),
    atRight("0.00", 600, 540),
    pdfItem("NET PORTFOLIO VALUE", 36, 520),
    atRight("510.00", 600, 520),
    atRight("400.00", 700, 520),
  ]);
  return pages;
}
