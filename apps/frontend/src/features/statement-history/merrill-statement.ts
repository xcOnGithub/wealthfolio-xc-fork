import Decimal from "decimal.js";
import type { StatementSnapshot, StatementPosition } from "@/adapters/shared/statements";
import { pdfRows, type PdfTextItem } from "@/pages/settings/accounts/components/basis-pdf";
import { basisDate } from "@/pages/settings/accounts/components/opening-basis";

const Money = Decimal.clone({ precision: 40 });
const months = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];
const period = /^([A-Za-z]+)\s+(\d{1,2}),\s+(\d{4})\s*-\s*([A-Za-z]+)\s+(\d{1,2}),\s+(\d{4})$/;
const right = (item: PdfTextItem) => item.transform[4] + item.width;
const fail = () => new Error("statementFormat");
function money(value: string): string {
  const clean = value.trim().replace(/^\$/, "");
  if (
    !/^\(?-?(?:\d+|\d{1,3}(?:,\d{3})+)(?:\.\d{1,18})?\)?$/.test(clean) ||
    clean.startsWith("(") !== clean.endsWith(")")
  )
    throw fail();
  const n = new Money(clean.replaceAll(",", "").replace(/^\((.*)\)$/, "-$1"));
  if (!n.isFinite() || n.abs().gt("1e15")) throw fail();
  return n.toFixed();
}
/** Exact Merrill CMA summary format; closing values are anchored to the dated column. */
export function extractMonthlyMerrill(pages: PdfTextItem[][]): {
  snapshot: StatementSnapshot;
  identity: string;
} {
  if (!pages.some((page) => page.some((item) => /\bMerrill\b/i.test(item.str)))) throw fail();
  const dates = new Set<string>(),
    ranges = new Set<string>(),
    identities = new Set<string>();
  for (const page of pages) {
    for (const item of page) {
      const account = /Account\s+Number:\s*([A-Z0-9]+(?:-[A-Z0-9]+)+)/i.exec(item.str);
      if (account) identities.add(account[1].replaceAll("-", "").toUpperCase());
      const match = period.exec(item.str.trim());
      if (match) {
        const month = months.indexOf(match[4]);
        const firstMonth = months.indexOf(match[1]);
        if (month < 0 || firstMonth < 0) throw fail();
        const start = basisDate(
          `${match[3]}-${String(firstMonth + 1).padStart(2, "0")}-${match[2].padStart(2, "0")}`,
        );
        const end = basisDate(
          `${match[6]}-${String(month + 1).padStart(2, "0")}-${match[5].padStart(2, "0")}`,
        );
        if (start > end) throw fail();
        dates.add(end);
        ranges.add(`${start}/${end}`);
      }
    }
  }
  if (dates.size !== 1 || ranges.size !== 1 || identities.size !== 1) throw fail();
  const date = [...dates][0],
    closing = new Date(`${date}T12:00:00Z`);
  const heading = `${months[closing.getUTCMonth()]} ${closing.getUTCDate()}`;
  const summaries: { value: string; page: number; cash: string | null }[] = [];
  for (const [index, page] of pages.entries()) {
    const rows = pdfRows(page);
    for (const row of rows) {
      const label = row.cells.find((item) => item.str.trim() === "NET PORTFOLIO VALUE");
      if (!label) continue;
      const headers = page.filter(
        (item) => item.transform[5] > row.y && item.str.trim() === heading,
      );
      if (headers.length !== 1) throw fail();
      const column = right(headers[0]);
      const at = (cells: PdfTextItem[]) =>
        cells.filter((item) => item.str.trim() && Math.abs(right(item) - column) < 3);
      const values = at(row.cells);
      if (values.length !== 1) throw fail();
      const cashRow = rows.find(
        (r) => r.y > row.y && r.cells.some((c) => c.str.trim() === "Cash/Money Accounts"),
      );
      const cash = cashRow ? at(cashRow.cells) : [];
      summaries.push({
        value: money(values[0].str),
        page: index + 1,
        cash: cash.length === 1 ? money(cash[0].str) : null,
      });
    }
  }
  if (summaries.length !== 1) throw fail();
  const summary = summaries[0];
  const positions: StatementPosition[] = [];
  let detailsValid = true;
  // Detailed rows are optional. Failure never turns unknown basis into zero.
  for (const page of pages) {
    if (!page.some((item) => /\bCMA\b.*\bASSETS\b/i.test(item.str))) continue;
    let columns: { q: number; b: number; m: number } | undefined;
    let previousLabel = "";
    let cashSection = false;
    const rows = pdfRows(page);
    for (const [lineIndex, row] of rows.entries()) {
      if (row.cells.some((c) => c.str.trim() === "CASH/MONEY ACCOUNTS")) cashSection = true;
      if (
        row.cells.some((c) =>
          /^(?:EQUITIES|MUTUAL FUNDS\/CLOSED END FUNDS\/UITs\/ETPs)$/.test(c.str.trim()),
        )
      )
        cashSection = false;
      const find = (label: string) => row.cells.find((c) => c.str.trim() === label);
      if (find("Quantity") && find("Cost Basis") && find("Market Value")) {
        if (
          !rows[lineIndex - 1]?.cells.some(
            (c) => c.str.trim() === "Total" && Math.abs(right(c) - right(find("Cost Basis")!)) < 3,
          )
        ) {
          detailsValid = false;
          columns = undefined;
          continue;
        }
        columns = {
          q: right(find("Quantity")!),
          b: right(find("Cost Basis")!),
          m: right(find("Market Value")!),
        };
        previousLabel = "";
        continue;
      }
      if (!columns || cashSection) continue;
      if (find("TOTAL")) {
        columns = undefined;
        continue;
      }
      const at = (x: number) => row.cells.filter((c) => c.str.trim() && Math.abs(right(c) - x) < 3);
      const quantity = at(columns.q);
      if (!quantity.length) continue;
      try {
        const basis = at(columns.b),
          market = at(columns.m);
        if (quantity.length !== 1 || basis.length !== 1 || market.length !== 1) throw fail();
        const label =
          row.cells
            .filter((c) => c.transform[4] < columns!.q - 80 && c.str.trim())
            .sort((a, b) => a.transform[4] - b.transform[4])
            .map((c) => c.str.trim())
            .join(" ") || previousLabel;
        if (!label) throw fail();
        previousLabel = label;
        positions.push({
          label,
          quantity: money(quantity[0].str),
          marketValue: money(market[0].str),
          costBasis: ["--", "-", "N/A", "NA"].includes(basis[0].str.trim())
            ? null
            : money(basis[0].str),
        });
      } catch {
        detailsValid = false;
      }
    }
  }
  const complete =
    detailsValid &&
    positions.length > 0 &&
    summary.cash !== null &&
    new Money(summary.cash)
      .plus(positions.reduce((sum, p) => sum.plus(p.marketValue!), new Money(0)))
      .minus(summary.value)
      .abs()
      .lte("0.01");
  return {
    identity: [...identities][0],
    snapshot: {
      date,
      currency: "USD",
      totalValue: summary.value,
      cashValue: summary.cash,
      positions,
      positionsComplete: complete,
      format: "merrill-cma-monthly-v1",
      sourcePage: summary.page,
    },
  };
}
