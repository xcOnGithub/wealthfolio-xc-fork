import Decimal from "decimal.js";
import type { PdfTextItem } from "./statement-pdf";
import { normalizeSecurityLabel } from "./merrill-format";
import type { StatementHistoryDraft, StatementHistoryIssue } from "./merrill-history";
import { extractSchwabActivityDetailed } from "./schwab-activity";
import { schwabIdentity } from "./schwab-format";
import { extractMonthlySchwab, extractSchwabCashBalances } from "./schwab-statement";

const Money = Decimal.clone({ precision: 40 });

/** Independent of Plaid and acquisition basis. The identity is returned separately
 * for account verification and must never be included in persisted draft facts.
 */
export function extractSchwabHistory(pages: PdfTextItem[][]): {
  identity: string;
  draft: StatementHistoryDraft;
} {
  const { identity, from, through } = schwabIdentity(pages);
  const { snapshot: closing, holdingTickers } = extractMonthlySchwab(pages);
  const { evidence, activitySourceParsed, activityTickers } = extractSchwabActivityDetailed(pages);
  const holdingLabels = new Map<string, string | null>();
  const linkLabel = (key: string, label: string) => {
    const existing = holdingLabels.get(key);
    holdingLabels.set(key, existing === undefined || existing === label ? label : null);
  };
  for (const { label } of closing.positions) {
    const key = normalizeSecurityLabel(label);
    if (!key) continue;
    linkLabel(key, label);
    const ticker = holdingTickers.get(key);
    if (ticker) linkLabel(normalizeSecurityLabel(ticker), label);
  }
  const activityHoldingLabels: Record<string, string | null> = {};
  for (const row of evidence.activities) {
    if (!row.security) continue;
    const label = holdingLabels.get(normalizeSecurityLabel(row.security));
    if (label !== undefined) activityHoldingLabels[row.id] = label;
  }
  const cashBalances = extractSchwabCashBalances(pages);
  const opening = cashBalances.opening
    ? {
        value: cashBalances.opening.value,
        source: { page: cashBalances.opening.page, line: cashBalances.opening.line },
      }
    : null;

  const issues: StatementHistoryIssue[] = [];
  if (!activitySourceParsed) issues.push({ code: "missingActivitySections" });
  if (!opening) issues.push({ code: "missingOpeningCash" });
  if (closing.cashValue === null) issues.push({ code: "missingClosingCash" });
  if (!closing.positionsComplete) issues.push({ code: "incompleteClosingPositions" });

  let knownMovementTotal = new Money(0);
  let complete = activitySourceParsed;

  for (const row of evidence.activities) {
    const source = { page: row.page, line: row.line };
    if (row.section === "sweep") continue;
    if (row.action === "UNSUPPORTED") issues.push({ code: "unsupportedActivity", source });
    if (["TRANSFER_IN", "TRANSFER_OUT"].includes(row.action)) continue;
    if (row.cash === null) {
      issues.push({ code: "unknownCashMovement", source });
      complete = false;
    } else {
      knownMovementTotal = knownMovementTotal.plus(row.cash);
    }
    if (!row.date) {
      issues.push({ code: "undatedCashMovement", source });
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
      format: "schwab-history-v1",
      from,
      through,
      datePolicy: "settlement",
      closing,
      evidence,
      activityHoldingLabels,
      holdingTickers,
      activityTickers: activitySourceParsed ? activityTickers : undefined,
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
