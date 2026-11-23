import { printedTicker, rememberTicker } from "./statement-tickers";
import Decimal from "decimal.js";
import type { StatementSnapshot, StatementPosition } from "@/adapters/shared/statements";
import { pdfRows, type PdfTextItem } from "./statement-pdf";
import {
  MERRILL_MONTHS as months,
  merrillIdentity,
  merrillNumber,
  normalizeSecurityLabel,
  splitMerrillStatements,
  type MerrillStatementSource,
} from "./merrill-format";

const Money = Decimal.clone({ precision: 40 });
const right = (item: PdfTextItem) => item.transform[4] + item.width;
const fail = () => new Error("statementFormat");
function money(value: string): string {
  try {
    const parsed = merrillNumber(value);
    if (parsed === null || new Money(parsed).abs().gt("1e15")) throw fail();
    return parsed;
  } catch {
    throw fail();
  }
}
function reportedMoney(value: string): string | null {
  return ["--", "-", "N/A", "NA"].includes(value.trim().toUpperCase()) ? null : money(value);
}
/** Exact Merrill CMA summary format; closing values are anchored to the dated column. */
export function extractMonthlyMerrill(
  pages: PdfTextItem[][],
  options?: { pageNumbers?: number[]; source?: MerrillStatementSource },
): {
  snapshot: StatementSnapshot;
  identity: string;
  holdingLabels: ReadonlyMap<string, string | null>;
  holdingTickers: ReadonlyMap<string, string | null>;
} {
  let identity: string, date: string;
  try {
    const source = options?.source ?? merrillIdentity(pages);
    identity = source.identity;
    date = source.through;
  } catch {
    throw fail();
  }
  const closing = new Date(`${date}T12:00:00Z`);
  const heading = `${months[closing.getUTCMonth()]} ${closing.getUTCDate()}`;
  const summaries: { value: string; page: number; cash: string | null }[] = [];
  for (const [index, page] of pages.entries()) {
    const pageNumber = options?.pageNumbers?.[index] ?? index + 1;
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
        page: pageNumber,
        cash: cash.length === 1 ? reportedMoney(cash[0].str) : null,
      });
    }
  }
  if (summaries.length !== 1) throw fail();
  const summary = summaries[0];
  const positions: StatementPosition[] = [];
  const holdingLabels = new Map<string, string | null>();
  const holdingTickers = new Map<string, string | null>();
  const linkLabel = (alias: string, label: string) => {
    const key = normalizeSecurityLabel(alias);
    if (!key) return;
    const known = holdingLabels.get(key);
    if (known === undefined) holdingLabels.set(key, label);
    else if (known !== null && normalizeSecurityLabel(known) !== normalizeSecurityLabel(label))
      holdingLabels.set(key, null);
  };
  let detailsValid = true;
  // Detailed rows are optional. Failure never turns unknown basis into zero.
  for (const page of pages) {
    if (!page.some((item) => /\bCMA\b.*\bASSETS\b/i.test(item.str))) continue;
    let columns: { q: number; b: number; m: number; symbol?: number } | undefined;
    let previousLabel = "";
    let cashSection = false;
    let tickerSection = false;
    const rows = pdfRows(page);
    for (const [lineIndex, row] of rows.entries()) {
      if (row.cells.some((c) => c.str.trim() === "CASH/MONEY ACCOUNTS")) {
        cashSection = true;
        tickerSection = false;
      }
      if (
        row.cells.some((c) =>
          /^(?:EQUITIES|MUTUAL FUNDS\/CLOSED END FUNDS\/UITs\/ETPs)(?: \(continued\))?$/i.test(
            c.str.trim(),
          ),
        )
      ) {
        cashSection = false;
        tickerSection = true;
      }
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
          symbol: find("Symbol")?.transform[4],
        };
        previousLabel = "";
        continue;
      }
      if (!columns || cashSection) continue;
      if (find("TOTAL")) {
        tickerSection = false;
        columns = undefined;
        continue;
      }
      const at = (x: number) => row.cells.filter((c) => c.str.trim() && Math.abs(right(c) - x) < 3);
      const quantity = at(columns.q);
      if (!quantity.length) {
        if (columns.symbol === undefined && previousLabel) {
          const text = row.cells.map((cell) => cell.str.trim()).join(" ");
          const symbol = /\bSYMBOL:\s*([A-Z]{1,6}(?:[.-][A-Z])?)(?=\s|$)/.exec(text)?.[1];
          if (symbol) {
            linkLabel(symbol, previousLabel);
            const ticker = tickerSection ? printedTicker(symbol) : undefined;
            if (ticker) rememberTicker(holdingTickers, previousLabel, ticker);
          }
        }
        continue;
      }
      try {
        const basis = at(columns.b),
          market = at(columns.m);
        if (quantity.length !== 1 || basis.length !== 1 || market.length !== 1) throw fail();
        const labelCells = row.cells
          .filter((c) => c.transform[4] < columns!.q - 80 && c.str.trim())
          .sort((a, b) => a.transform[4] - b.transform[4]);
        const label = labelCells.map((c) => c.str.trim()).join(" ") || previousLabel;
        if (!label || label.length > 256 || positions.length >= 1000) throw fail();
        previousLabel = label;
        const q = money(quantity[0].str),
          b = reportedMoney(basis[0].str),
          m = reportedMoney(market[0].str);
        // These optional rows must satisfy the stored-position contract. Unsupported
        // details cannot prevent saving a separately verified closing account total.
        if (new Money(q).lt(0) || (b !== null && new Money(b).lt(0))) throw fail();
        if (m === null) detailsValid = false;
        positions.push({ label, quantity: q, marketValue: m, costBasis: b });
        linkLabel(label, label);
        const symbolColumn = columns.symbol;
        if (symbolColumn !== undefined) {
          const symbols = labelCells.filter(
            (cell) =>
              cell.transform[4] >= symbolColumn - 2 && cell.transform[4] < symbolColumn + 65,
          );
          const ticker = symbols.length === 1 ? printedTicker(symbols[0].str) : undefined;
          if (ticker) {
            if (tickerSection) rememberTicker(holdingTickers, label, ticker);
            linkLabel(ticker, label);
            linkLabel(
              labelCells
                .filter((cell) => cell !== symbols[0])
                .map((cell) => cell.str.trim())
                .join(" "),
              label,
            );
          }
        }
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
    identity,
    holdingLabels,
    holdingTickers,
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

/** Extract one or more monthly snapshots from a single-month or bundled Merrill CMA PDF. */
export function extractMonthlyMerrillStatements(
  pages: PdfTextItem[][],
): ReturnType<typeof extractMonthlyMerrill>[] {
  return splitMerrillStatements(pages).map((segment) =>
    extractMonthlyMerrill(segment.pages, {
      pageNumbers: segment.pageNumbers,
      source: segment,
    }),
  );
}
