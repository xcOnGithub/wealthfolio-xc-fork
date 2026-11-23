import { it, expect } from "vitest";
import { merrillIdentity, merrillNumber, splitMerrillStatements } from "./merrill-format";
import { monthlyFixture } from "./test-fixtures/monthly-statement";
import { pdfItem } from "./test-fixtures/merrill-pdf";

it.each([
  [".0002", "0.0002"],
  ["(.02)", "-0.02"],
  ["-.25", "-0.25"],
  ["$.25", "0.25"],
  ["($1,234.56)", "-1234.56"],
  ["-$1.25", "-1.25"],
  ["$-1.25", "-1.25"],
  ["0", "0"],
  ["0.000000000000000001", "0.000000000000000001"],
  ["--", null],
  ["n/a", null],
  ["NA", null],
  ["", null],
])("reads statement numeric form %j", (raw, expected) => {
  expect(merrillNumber(raw)).toBe(expected);
});
it.each([
  "(-.02)",
  "($-.02)",
  "1,23.45",
  "1.000,25",
  "1e2",
  "Infinity",
  "NaN",
  "+1",
  ".2%",
  "1.2*",
  "0.0000000000000000001",
  "100000000000000000001",
  "(1.00",
  "1.00)",
  ".",
  "1.",
])("rejects unsafe numeric form %j", (raw) => {
  expect(() => merrillNumber(raw)).toThrow("number");
});
it("requires complete period and identity consistency across the document", () => {
  const pages = monthlyFixture();
  expect(merrillIdentity(pages)).toEqual({
    identity: "SYNTH01",
    from: "2025-05-01",
    through: "2025-05-12",
  });
  pages[0].find((c) => c.str.startsWith("May 01"))!.str = "April 01, 2025 - May 12, 2025";
  expect(() => merrillIdentity(pages)).toThrow("brokerageAccount");
});

function monthPages(month: string, endDay: number) {
  const pages = monthlyFixture();
  for (const page of pages) {
    for (const item of page) {
      item.str = item.str
        .replaceAll("May 01, 2025 - May 12, 2025", `${month} 01, 2025 - ${month} ${endDay}, 2025`)
        .replaceAll("May 12", `${month} ${endDay}`);
    }
  }
  return pages;
}

it("splits bundled multi-month PDFs in ascending order while preserving original page numbers", () => {
  const april = monthPages("April", 30);
  const march = monthPages("March", 31);
  const cover = [pdfItem("Merrill", 36, 780), pdfItem("Statement Package", 36, 740)];
  const disclosure = [pdfItem("Important Disclosures", 36, 740)];
  // Reverse chronological bundle: cover (p1), April (p2-3), disclosure (p4), March (p5-6)
  const segments = splitMerrillStatements([cover, ...april, disclosure, ...march]);
  expect(segments).toHaveLength(2);
  expect(segments[0]).toMatchObject({
    identity: "SYNTH01",
    from: "2025-03-01",
    through: "2025-03-31",
    pageNumbers: [5, 6],
  });
  expect(segments[1]).toMatchObject({
    identity: "SYNTH01",
    from: "2025-04-01",
    through: "2025-04-30",
    pageNumbers: [1, 2, 3, 4],
  });
});

it("rejects interleaved periods, mixed accounts, and ambiguous pages in bundled PDFs", () => {
  const march = monthPages("March", 31);
  const april = monthPages("April", 30);
  // Interleaved March -> April -> March
  expect(() => splitMerrillStatements([march[0], ...april, march[1]])).toThrow("brokerageAccount");
  // Mixed account numbers
  const other = monthPages("April", 30);
  other[0].find((c) => c.str.startsWith("Account Number:"))!.str = "Account Number: OTHER-02";
  expect(() => splitMerrillStatements([...march, ...other])).toThrow("brokerageAccount");
  // Two periods on a single page
  const ambiguous = monthPages("March", 31);
  ambiguous[0].push(pdfItem("April 01, 2025 - April 30, 2025", 400, 700));
  expect(() => splitMerrillStatements(ambiguous)).toThrow("brokerageAccount");
});
