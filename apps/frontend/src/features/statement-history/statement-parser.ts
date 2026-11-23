import type { StatementSnapshot } from "@/adapters/shared/statements";
import type { PdfTextItem } from "./statement-pdf";
import { merrillIdentity, splitMerrillStatements } from "./merrill-format";
import {
  extractMerrillHistory,
  extractMerrillHistoryStatements,
  type StatementHistoryDraft,
} from "./merrill-history";
import { extractMonthlyMerrill, extractMonthlyMerrillStatements } from "./merrill-statement";
import { isSchwabStatement, schwabIdentity } from "./schwab-format";
import { extractSchwabHistory } from "./schwab-history";
import { extractMonthlySchwab } from "./schwab-statement";

function canIdentify(fn: () => unknown): boolean {
  try {
    fn();
    return true;
  } catch {
    return false;
  }
}

function detectBrokerage(pages: PdfTextItem[][], allowMultiMonth = false): "merrill" | "schwab" {
  const merrill = canIdentify(() =>
    allowMultiMonth ? splitMerrillStatements(pages) : merrillIdentity(pages),
  );
  const schwab = isSchwabStatement(pages) && canIdentify(() => schwabIdentity(pages));
  if (merrill && !schwab) return "merrill";
  if (schwab && !merrill) return "schwab";
  throw new Error("statementFormat");
}

/** Dispatch monthly statement extraction across supported brokerages. */
export function extractMonthlyStatement(pages: PdfTextItem[][]): {
  snapshot: StatementSnapshot;
  identity: string;
} {
  const brokerage = detectBrokerage(pages);
  const parsed =
    brokerage === "schwab" ? extractMonthlySchwab(pages) : extractMonthlyMerrill(pages);
  return {
    snapshot: parsed.snapshot,
    identity: `${brokerage}:${parsed.identity}`,
  };
}

/** Dispatch single-month or bundled multi-month snapshot extraction across supported brokerages. */
export function extractMonthlyStatements(pages: PdfTextItem[][]): {
  snapshot: StatementSnapshot;
  identity: string;
}[] {
  const brokerage = detectBrokerage(pages, true);
  if (brokerage === "schwab") {
    const parsed = extractMonthlySchwab(pages);
    return [{ snapshot: parsed.snapshot, identity: `schwab:${parsed.identity}` }];
  }
  return extractMonthlyMerrillStatements(pages).map((parsed) => ({
    snapshot: parsed.snapshot,
    identity: `merrill:${parsed.identity}`,
  }));
}

/** Dispatch daily history statement extraction across supported brokerages. */
export function extractStatementHistory(pages: PdfTextItem[][]): {
  draft: StatementHistoryDraft;
  identity: string;
} {
  const brokerage = detectBrokerage(pages);
  const parsed =
    brokerage === "schwab" ? extractSchwabHistory(pages) : extractMerrillHistory(pages);
  return {
    draft: parsed.draft,
    identity: `${brokerage}:${parsed.identity}`,
  };
}

/** Dispatch single-month or bundled multi-month daily history extraction across supported brokerages. */
export function extractStatementHistories(pages: PdfTextItem[][]): {
  draft: StatementHistoryDraft;
  identity: string;
}[] {
  const brokerage = detectBrokerage(pages, true);
  if (brokerage === "schwab") {
    const parsed = extractSchwabHistory(pages);
    return [{ draft: parsed.draft, identity: `schwab:${parsed.identity}` }];
  }
  return extractMerrillHistoryStatements(pages).map((parsed) => ({
    draft: parsed.draft,
    identity: `merrill:${parsed.identity}`,
  }));
}
