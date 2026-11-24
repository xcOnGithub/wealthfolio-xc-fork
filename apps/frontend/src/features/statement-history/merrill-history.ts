import Decimal from "decimal.js";
import type { StatementSnapshot } from "@/adapters/shared/statements";
import { BasisError, pdfRows, type PdfTextItem } from "./statement-pdf";
import {
  MAX_STATEMENT_PAGES,
  merrillIdentity,
  merrillNumber,
  merrillPeriod,
  normalizeSecurityLabel,
  splitMerrillStatements,
  type MerrillStatementSource,
} from "./merrill-format";
import {
  extractMerrillActivity,
  type StatementEvidence,
  type StatementSection,
  type StatementSource,
} from "./merrill-activity";
import { extractMonthlyMerrill } from "./merrill-statement";

const Money = Decimal.clone({ precision: 40 });
const SETTLED_ACTIVITY_SECTIONS: ReadonlySet<StatementSection> = new Set([
  "securities",
  "transfers",
  "income",
  "cash",
]);
export interface StatementHistoryIssue {
  code:
    | "missingActivitySections"
    | "missingOpeningCash"
    | "missingClosingCash"
    | "incompleteClosingPositions"
    | "unknownCashMovement"
    | "undatedCashMovement"
    | "cashMismatch"
    | "unsupportedActivity"
    | "unsettledTrades";
  source?: StatementSource;
}
export type MerrillHistoryIssue = StatementHistoryIssue;
/** Transient source facts for review, not ledger activities or daily valuations. */
export interface StatementHistoryDraft {
  format: "merrill-cma-history-v1" | "schwab-history-v1";
  from: string;
  through: string;
  datePolicy: "settlement";
  closing: StatementSnapshot;
  evidence: StatementEvidence;
  /** Transient links from event descriptions to validated closing row labels. */
  activityHoldingLabels: Record<string, string | null>;
  /** Explicit tickers indexed by normalized closing labels. Kept in the browser. */
  holdingTickers: ReadonlyMap<string, string | null>;
  /** Explicit tickers from the broker's transaction symbol column, when available. */
  activityTickers?: ReadonlyMap<string, string>;
  openingCash: { value: string; source: StatementSource } | null;
  cashReconciliation: {
    knownMovementTotal: string;
    difference: string | null;
    matches: boolean | null;
  };
  issues: StatementHistoryIssue[];
}
export type MerrillHistoryDraft = StatementHistoryDraft;

function openingCash(
  pages: PdfTextItem[][],
  pageNumbers?: number[],
): MerrillHistoryDraft["openingCash"] {
  const matches: NonNullable<MerrillHistoryDraft["openingCash"]>[] = [];
  for (const [pageIndex, page] of pages.entries()) {
    const pageNumber = pageNumbers?.[pageIndex] ?? pageIndex + 1;
    const rows = pdfRows(page);
    for (const [lineIndex, row] of rows.entries()) {
      if (!row.cells.some((c) => c.str.trim() === "Opening Cash/Money Accounts")) continue;
      const headers = page
        .filter((c) => c.str.trim() === "This Statement" && c.transform[5] > row.y)
        .sort((a, b) => a.transform[5] - b.transform[5]);
      if (!headers.length) continue;
      if (headers[1] && Math.abs(headers[0].transform[5] - headers[1].transform[5]) < 2)
        throw new BasisError("brokerageFormat", lineIndex + 1, pageNumber);
      const anchor = headers[0].transform[4] + headers[0].width;
      const values = row.cells.filter(
        (c) => c.str.trim() && Math.abs(c.transform[4] + c.width - anchor) < 3,
      );
      if (values.length !== 1) throw new BasisError("brokerageFormat", lineIndex + 1, pageNumber);
      const value = merrillNumber(values[0].str);
      if (value === null) continue;
      if (new Money(value).abs().gt("1e15"))
        throw new BasisError("number", lineIndex + 1, pageNumber);
      matches.push({ value, source: { page: pageNumber, line: lineIndex + 1 } });
    }
  }
  if (matches.length > 1) throw new BasisError("duplicate");
  return matches[0] ?? null;
}

type PageMarker = { current: number; total: number } | "ambiguous" | null;

function collectMarkers(
  matches: Iterable<RegExpExecArray>,
  found: Map<string, { current: number; total: number }>,
) {
  for (const match of matches) {
    const current = Number(match[1]);
    const total = Number(match[2]);
    if (current >= 1 && total >= 1 && total <= MAX_STATEMENT_PAGES) {
      found.set(`${current}/${total}`, { current, total });
    }
  }
}

function uniqueMarker(found: Map<string, { current: number; total: number }>): PageMarker {
  if (found.size === 1) return found.values().next().value!;
  return found.size > 1 ? "ambiguous" : null;
}

function statementPageMarker(page: PdfTextItem[]): PageMarker {
  const rows = pdfRows(page).map((row) => ({
    y: row.y,
    text: [...row.cells]
      .sort((a, b) => a.transform[4] - b.transform[4])
      .map((c) => c.str.trim())
      .join(" "),
  }));

  const explicitPageMatches = new Map<string, { current: number; total: number }>();
  for (const row of rows) {
    collectMarkers(row.text.matchAll(/\bPage\s*:?\s*(\d+)\s+of\s+(\d+)\b/gi), explicitPageMatches);
  }
  const fromExplicit = uniqueMarker(explicitPageMatches);
  if (fromExplicit !== null) return fromExplicit;

  const cellMatches = new Map<string, { current: number; total: number }>();
  for (const cell of page) {
    const match = /^(\d+)\s+of\s+(\d+)$/.exec(cell.str.trim());
    if (match) collectMarkers([match], cellMatches);
  }
  const fromCells = uniqueMarker(cellMatches);
  if (fromCells !== null) return fromCells;

  const footerMatches = new Map<string, { current: number; total: number }>();
  for (const row of rows) {
    if (row.y <= 150) {
      collectMarkers(row.text.matchAll(/\b(\d+)\s+of\s+(\d+)\b/gi), footerMatches);
    }
  }
  return uniqueMarker(footerMatches);
}

function isCoverOnlyPage(page: PdfTextItem[]): boolean {
  if (page.some((c) => !!merrillPeriod(c.str))) return false;
  const text = pdfRows(page)
    .map((row) =>
      [...row.cells]
        .sort((a, b) => a.transform[4] - b.transform[4])
        .map((c) => c.str.trim())
        .join(" "),
    )
    .join("\n");
  return !(
    /\bCMA\b.*\b(?:ASSETS|TRANSACTIONS|MONEY ACCOUNT TRANSACTIONS)\b/i.test(text) ||
    /\b(?:NET PORTFOLIO VALUE|Opening Cash\/Money Accounts|Closing Cash\/Money Accounts)\b/i.test(
      text,
    ) ||
    /\b(?:UNSETTLED TRADES|SECURITIES YOU TRANSFERRED IN\/OUT|REALIZED GAINS\/\(LOSSES\)|SECURITY TRANSACTIONS|DIVIDENDS\/INTEREST INCOME TRANSACTIONS|CASH\/OTHER TRANSACTIONS)\b/i.test(
      text,
    )
  );
}

function hasMissingStatementPages(pages: PdfTextItem[][]): boolean {
  const markers = pages.map(statementPageMarker);
  if (markers.some((m) => m === "ambiguous")) return true;
  const firstNumbered = markers.findIndex((m) => m !== null);
  if (firstNumbered < 0) return false;
  let lastNumbered = markers.length - 1;
  while (lastNumbered > firstNumbered && markers[lastNumbered] === null) {
    lastNumbered--;
  }
  // Allow unnumbered cover-only pages at month boundaries (both leading covers and
  // next-month covers grouped onto the prior month by splitMerrillStatements).
  for (let i = 0; i < firstNumbered; i++) {
    if (!isCoverOnlyPage(pages[i])) return true;
  }
  for (let i = lastNumbered + 1; i < pages.length; i++) {
    if (!isCoverOnlyPage(pages[i])) return true;
  }
  const numbered = markers.slice(firstNumbered, lastNumbered + 1);
  if (numbered.some((m) => m === null)) return true;
  const validNumbered = numbered as { current: number; total: number }[];
  const total = validNumbered[0].total;
  const seen = new Set<number>();
  for (const marker of validNumbered) {
    if (marker.total !== total || marker.current > total || seen.has(marker.current)) return true;
    seen.add(marker.current);
  }
  return seen.size !== total;
}

/** Independent of Plaid and acquisition basis. The identity is returned separately
 * for account verification and must never be included in persisted draft facts.
 */
export function extractMerrillHistory(
  pages: PdfTextItem[][],
  options?: { pageNumbers?: number[]; source?: MerrillStatementSource },
): {
  identity: string;
  draft: MerrillHistoryDraft;
} {
  const source = options?.source ?? merrillIdentity(pages);
  const { identity, from, through } = source;
  const {
    snapshot: closing,
    holdingLabels,
    holdingTickers,
  } = extractMonthlyMerrill(pages, {
    pageNumbers: options?.pageNumbers,
    source,
  });
  const evidence = extractMerrillActivity(pages, {
    pageNumbers: options?.pageNumbers,
    source,
  });
  const activityHoldingLabels: Record<string, string | null> = {};
  for (const row of evidence.activities) {
    const label = holdingLabels.get(normalizeSecurityLabel(row.description));
    if (label !== undefined) activityHoldingLabels[row.id] = label;
  }
  const opening = openingCash(pages, options?.pageNumbers);
  const hasSettledActivitySection = (Object.keys(evidence.sections) as StatementSection[]).some(
    (section) => SETTLED_ACTIVITY_SECTIONS.has(section),
  );
  const missingPages = hasMissingStatementPages(pages);
  const issues: MerrillHistoryIssue[] = [];
  if (!hasSettledActivitySection || missingPages) issues.push({ code: "missingActivitySections" });
  if (!opening) issues.push({ code: "missingOpeningCash" });
  if (closing.cashValue === null) issues.push({ code: "missingClosingCash" });
  if (!closing.positionsComplete) issues.push({ code: "incompleteClosingPositions" });
  let knownMovementTotal = new Money(0),
    complete = hasSettledActivitySection && !missingPages;
  for (const row of evidence.activities) {
    const rowSource = { page: row.page, line: row.line };
    // Merrill CMA closing cash and positions are reported on a settlement-date basis;
    // UNSETTLED TRADES are informational and settle in the next statement period.
    if (row.section === "pending" || row.section === "sweep") continue;
    if (row.action === "UNSUPPORTED")
      issues.push({ code: "unsupportedActivity", source: rowSource });
    if (["TRANSFER_IN", "TRANSFER_OUT"].includes(row.action)) continue; // In-kind value is not cash.
    if (row.cash === null) {
      issues.push({ code: "unknownCashMovement", source: rowSource });
      complete = false;
    } else knownMovementTotal = knownMovementTotal.plus(row.cash);
    if (!row.date) {
      issues.push({ code: "undatedCashMovement", source: rowSource });
      complete = false;
    }
  }
  const difference =
    opening && closing.cashValue !== null && complete
      ? new Money(opening.value).plus(knownMovementTotal).minus(closing.cashValue)
      : null;
  const matches = difference === null ? null : difference.abs().lte("0.01");
  if (matches === false) issues.push({ code: "cashMismatch" });
  return {
    identity,
    draft: {
      format: "merrill-cma-history-v1",
      from,
      through,
      datePolicy: "settlement",
      closing,
      evidence,
      activityHoldingLabels,
      holdingTickers,
      openingCash: opening,
      cashReconciliation: {
        knownMovementTotal: knownMovementTotal.toFixed(),
        difference: difference?.toFixed() ?? null,
        matches,
      },
      issues,
    },
  };
}

/** Extract one or more monthly statement drafts from a single-month or bundled Merrill CMA PDF. */
export function extractMerrillHistoryStatements(
  pages: PdfTextItem[][],
): ReturnType<typeof extractMerrillHistory>[] {
  return splitMerrillStatements(pages).map((segment) =>
    extractMerrillHistory(segment.pages, {
      pageNumbers: segment.pageNumbers,
      source: segment,
    }),
  );
}
