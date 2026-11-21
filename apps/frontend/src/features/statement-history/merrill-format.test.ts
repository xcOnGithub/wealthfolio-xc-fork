import { it, expect } from "vitest";
import { merrillIdentity, merrillNumber } from "./merrill-format";
import { monthlyFixture } from "./test-fixtures/monthly-statement";

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
