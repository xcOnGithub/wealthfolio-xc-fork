import Decimal from "decimal.js";
import type { StatementSnapshot } from "@/adapters/shared/statements";
import { pdfRows, type PdfTextItem } from "@/pages/settings/accounts/components/basis-pdf";
import { BasisError } from "@/pages/settings/accounts/components/opening-basis";
import {
  merrillIdentity,
  merrillNumber,
  normalizeSecurityLabel,
  splitMerrillStatements,
  type MerrillStatementSource,
} from "./merrill-format";
import {
  extractMerrillActivity,
  type StatementEvidence,
  type StatementSource,
} from "./merrill-activity";
import { extractMonthlyMerrill } from "./merrill-statement";

const Money = Decimal.clone({ precision: 40 });
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
  const evidence = extractMerrillActivity(pages, [], {
    pageNumbers: options?.pageNumbers,
    source,
  });
  const activityHoldingLabels: Record<string, string | null> = {};
  for (const row of evidence.activities) {
    const label = holdingLabels.get(normalizeSecurityLabel(row.description));
    if (label !== undefined) activityHoldingLabels[row.id] = label;
  }
  const opening = openingCash(pages, options?.pageNumbers);
  const issues: MerrillHistoryIssue[] = [];
  if (!Object.keys(evidence.sections).length) issues.push({ code: "missingActivitySections" });
  if (!opening) issues.push({ code: "missingOpeningCash" });
  if (closing.cashValue === null) issues.push({ code: "missingClosingCash" });
  if (!closing.positionsComplete) issues.push({ code: "incompleteClosingPositions" });
  let knownMovementTotal = new Money(0),
    complete = Object.keys(evidence.sections).length > 0;
  for (const row of evidence.activities) {
    const rowSource = { page: row.page, line: row.line };
    if (row.section === "pending") {
      issues.push({ code: "unsettledTrades", source: rowSource });
      continue;
    }
    if (row.section === "sweep") continue;
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
