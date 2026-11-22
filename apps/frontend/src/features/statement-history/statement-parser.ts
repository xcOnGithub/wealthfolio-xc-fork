import type { StatementSnapshot } from "@/adapters/shared/statements";
import type { PdfTextItem } from "@/pages/settings/accounts/components/basis-pdf";
import { merrillIdentity } from "./merrill-format";
import { extractMerrillHistory, type StatementHistoryDraft } from "./merrill-history";
import { extractMonthlyMerrill } from "./merrill-statement";
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

function detectBrokerage(pages: PdfTextItem[][]): "merrill" | "schwab" {
  const merrill = canIdentify(() => merrillIdentity(pages));
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
