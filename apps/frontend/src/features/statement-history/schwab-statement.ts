import Decimal from "decimal.js";
import type { StatementPosition, StatementSnapshot } from "@/adapters/shared/statements";
import { pdfRows, type PdfTextItem } from "@/pages/settings/accounts/components/basis-pdf";
import { schwabIdentity, schwabNumber } from "./schwab-format";

const Money = Decimal.clone({ precision: 40 });
const right = (item: PdfTextItem) => item.transform[4] + item.width;
const fail = () => new Error("statementFormat");

const POSITION_SECTION =
  /^Positions\s*-\s*(Fixed Income|Equities|Mutual Funds|Exchange Traded Funds|Other Assets)$/i;
const POSITION_TOTAL =
  /^Total\s+(Fixed Income|Equities|Mutual Funds|Exchange Traded Funds|Other Assets)$/i;
const STOP_SECTION =
  /^(?:Cost Basis Lot Details|Transactions\s*-\s*Summary|Transaction Details|Bank Sweep Activity|Pending\s*\/\s*Open Activity|Endnotes For Your Account|Terms and Conditions)$/i;

function money(value: string): string {
  try {
    const parsed = schwabNumber(value);
    if (parsed === null || new Money(parsed).abs().gt("1e15")) throw fail();
    return parsed;
  } catch {
    throw fail();
  }
}

function reportedMoney(value: string): string | null {
  const cleaned = value.trim().replace(/\s+[a-z](?:,[a-z])*$/i, "");
  return ["--", "-", "N/A", "NA"].includes(cleaned.toUpperCase()) ? null : money(cleaned);
}

/** Extract opening and closing combined cash from the Cash and Cash Investments table. */
export function extractSchwabCashBalances(pages: PdfTextItem[][]): {
  opening: { value: string; page: number; line: number } | null;
  closing: { value: string; page: number; line: number } | null;
} {
  const openings: { value: string; page: number; line: number }[] = [];
  const closings: { value: string; page: number; line: number }[] = [];

  for (const [pageIndex, page] of pages.entries()) {
    const rows = pdfRows(page);
    let inCashSection = false;
    let beginRight: number | undefined;
    let endRight: number | undefined;

    for (let lineIndex = 0; lineIndex < rows.length; lineIndex++) {
      const row = rows[lineIndex];
      if (row.cells.some((c) => c.str.trim() === "Cash and Cash Investments")) {
        inCashSection = true;
        beginRight = undefined;
        endRight = undefined;
        continue;
      }
      if (!inCashSection) continue;
      if (
        row.cells.some(
          (c) => POSITION_SECTION.test(c.str.trim()) || STOP_SECTION.test(c.str.trim()),
        )
      ) {
        inCashSection = false;
        continue;
      }

      const beginHeader = row.cells.find((c) => c.str.trim() === "Beginning");
      const endHeader = row.cells.find((c) => c.str.trim() === "Ending");
      if (beginHeader && endHeader) {
        beginRight = right(beginHeader);
        endRight = right(endHeader);
        continue;
      }

      if (
        beginRight !== undefined &&
        endRight !== undefined &&
        row.cells.some((c) => c.str.trim() === "Total Cash and Cash Investments")
      ) {
        const at = (col: number) =>
          row.cells.filter((c) => c.str.trim() && Math.abs(right(c) - col) < 6);
        const beginCells = at(beginRight);
        const endCells = at(endRight);
        if (beginCells.length === 1) {
          const val = reportedMoney(beginCells[0].str);
          if (val !== null) {
            openings.push({ value: val, page: pageIndex + 1, line: lineIndex + 1 });
          }
        }
        if (endCells.length === 1) {
          const val = reportedMoney(endCells[0].str);
          if (val !== null) {
            closings.push({ value: val, page: pageIndex + 1, line: lineIndex + 1 });
          }
        }
        inCashSection = false;
      }
    }
  }

  if (openings.length > 1 || closings.length > 1) throw fail();
  return {
    opening: openings[0] ?? null,
    closing: closings[0] ?? null,
  };
}

interface PositionColumns {
  fixedIncome: boolean;
  symbolX: number;
  descX: number;
  descMaxX: number;
  qtyRight: number;
  marketRight: number;
  basisRight: number;
}

/** Exact Charles Schwab brokerage monthly statement summary and positions extractor. */
export function extractMonthlySchwab(pages: PdfTextItem[][]): {
  snapshot: StatementSnapshot;
  identity: string;
} {
  let identity: string;
  let date: string;
  try {
    const source = schwabIdentity(pages);
    identity = source.identity;
    date = source.through;
  } catch {
    throw fail();
  }

  const summaries: { value: string; page: number }[] = [];
  for (const [index, page] of pages.entries()) {
    const rows = pdfRows(page);
    for (const row of rows) {
      const label = row.cells.find((c) => c.str.trim() === "Ending Account Value");
      if (!label) continue;
      const headers = page.filter(
        (c) => c.transform[5] > row.y && c.str.trim() === "This Statement",
      );
      if (headers.length !== 1) throw fail();
      const column = right(headers[0]);
      const values = row.cells.filter((c) => c.str.trim() && Math.abs(right(c) - column) < 6);
      if (values.length !== 1) throw fail();
      summaries.push({ value: money(values[0].str), page: index + 1 });
    }
  }
  if (summaries.length !== 1) throw fail();
  const summary = summaries[0];
  const cashBalances = extractSchwabCashBalances(pages);
  const cashValue = cashBalances.closing?.value ?? null;

  const positions: StatementPosition[] = [];
  let detailsValid = true;

  for (const page of pages) {
    const rows = pdfRows(page);
    let currentSection: string | undefined;
    let columns: PositionColumns | undefined;

    for (let lineIndex = 0; lineIndex < rows.length; lineIndex++) {
      const row = rows[lineIndex];
      const sectionCell = row.cells.find((c) => POSITION_SECTION.test(c.str.trim()));
      if (sectionCell) {
        if (currentSection && !columns) detailsValid = false;
        currentSection = sectionCell.str.trim();
        columns = undefined;
        continue;
      }
      if (row.cells.some((c) => STOP_SECTION.test(c.str.trim()))) {
        if (currentSection && !columns) detailsValid = false;
        currentSection = undefined;
        columns = undefined;
        continue;
      }
      if (!currentSection) continue;
      if (row.cells.some((c) => POSITION_TOTAL.test(c.str.trim()))) {
        if (!columns) detailsValid = false;
        currentSection = undefined;
        columns = undefined;
        continue;
      }

      const find = (re: RegExp) => row.cells.find((c) => re.test(c.str.trim()));
      const symHeader = find(/^(?:Symbol|CUSIP)$/i);
      const descHeader = find(/^Description$/i);
      const qtyHeader = find(/^Quantity(?:\/Par)?$/i);
      const mktHeader = find(/^(?:Market\s+Value\(\$\)|Value\(\$\))$/i);
      const basisHeader = find(/^(?:Cost\s+Basis\(\$\)|Orig\s+Cost\s+Basis(?:\(\$\))?)$/i);

      if (symHeader && descHeader && qtyHeader && mktHeader && basisHeader) {
        const isFixedIncome = /Fixed Income/i.test(currentSection);
        const basisRightEdge = (() => {
          // On Fixed Income, "Orig Cost Basis" and "($)" may be split into adjacent cells.
          const parenCell = row.cells.find(
            (c) =>
              c.str.trim() === "($)" &&
              c.transform[4] >= basisHeader.transform[4] &&
              c.transform[4] - right(basisHeader) < 10,
          );
          return parenCell ? right(parenCell) : right(basisHeader);
        })();
        const descMaxX = isFixedIncome
          ? (find(/^Coupon$/i)?.transform[4] ?? qtyHeader.transform[4] - 50)
          : qtyHeader.transform[4] - 40;
        columns = {
          fixedIncome: isFixedIncome,
          symbolX: symHeader.transform[4],
          descX: descHeader.transform[4],
          descMaxX,
          qtyRight: right(qtyHeader),
          marketRight: right(mktHeader),
          basisRight: basisRightEdge,
        };
        continue;
      }

      if (!columns) continue;

      const atRight = (cells: PdfTextItem[], target: number, tol = 8) =>
        cells.filter((c) => {
          const s = c.str.trim();
          if (!s || /^[a-z](?:,[a-z])*$/i.test(s) || s === ",") return false;
          return Math.abs(right(c) - target) <= tol;
        });

      const qtyCells = atRight(row.cells, columns.qtyRight, 6);
      if (!qtyCells.length) continue;

      try {
        const marketCells = atRight(row.cells, columns.marketRight, 6);
        if (qtyCells.length !== 1 || marketCells.length !== 1) throw fail();

        let basisCells = atRight(row.cells, columns.basisRight, 8);
        // Fixed Income renders Adj Cost Basis ~2pt below the main row (before Orig Cost Basis ~12pt below).
        if (!basisCells.length && columns.fixedIncome && lineIndex + 1 < rows.length) {
          const nextRow = rows[lineIndex + 1];
          if (row.y - nextRow.y <= 5) {
            basisCells = atRight(nextRow.cells, columns.basisRight, 8);
          }
        }
        if (basisCells.length > 1) throw fail();

        const symbolText = row.cells
          .filter(
            (c) =>
              c.transform[4] >= columns!.symbolX - 5 &&
              c.transform[4] < columns!.descX - 5 &&
              c.str.trim(),
          )
          .sort((a, b) => a.transform[4] - b.transform[4])
          .map((c) => c.str.trim())
          .join(" ");

        const descCellsFor = (cells: PdfTextItem[]) =>
          cells
            .filter(
              (c) =>
                c.transform[4] >= columns!.descX - 5 &&
                c.transform[4] < columns!.descMaxX &&
                c.str.trim() &&
                c.str.trim() !== ",",
            )
            .sort((a, b) => a.transform[4] - b.transform[4])
            .map((c) => c.str.trim())
            .join(" ");

        let descText = descCellsFor(row.cells);
        // Fixed Income renders the security description ~3pt above the main row.
        if (!descText && columns.fixedIncome && lineIndex > 0) {
          const prevRow = rows[lineIndex - 1];
          if (prevRow.y - row.y <= 6) {
            descText = descCellsFor(prevRow.cells);
          }
        }

        const label = [symbolText, descText].filter(Boolean).join(" ").trim();
        if (!label || label.length > 256 || positions.length >= 1000) throw fail();

        const q = money(qtyCells[0].str);
        const m = reportedMoney(marketCells[0].str);
        const b = basisCells.length === 1 ? reportedMoney(basisCells[0].str) : null;

        if (new Money(q).lt(0) || (b !== null && new Money(b).lt(0))) throw fail();
        if (m === null) detailsValid = false;
        positions.push({ label, quantity: q, marketValue: m, costBasis: b });
      } catch {
        detailsValid = false;
      }
    }
    if (currentSection && !columns) detailsValid = false;
  }

  const complete =
    detailsValid &&
    cashValue !== null &&
    new Money(cashValue)
      .plus(positions.reduce((sum, p) => sum.plus(p.marketValue!), new Money(0)))
      .minus(summary.value)
      .abs()
      .lte("0.01");

  return {
    identity,
    snapshot: {
      date,
      currency: "USD",
      totalValue: summary.value,
      cashValue,
      positions,
      positionsComplete: complete,
      format: "schwab-monthly-v1",
      sourcePage: summary.page,
    },
  };
}
