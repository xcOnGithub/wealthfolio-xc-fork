import Decimal from "decimal.js";
import type { PlaidInvestmentPreview } from "@/adapters/shared/plaid";
import { parseCsv } from "@/adapters";
import { createDefaultParseConfig } from "@/pages/activity/import/utils/default-activity-template";
import { readBasisFile, type PdfBasisExclusion } from "./basis-pdf";
import { BROKERAGE_BASIS_FORMATS } from "./brokerage-basis-formats";
import {
  BasisError,
  basisDate,
  calculateBasis,
  matchBasis,
  type BasisCandidate,
} from "./opening-basis";

const Money = Decimal.clone({ precision: 40 });
export interface TransferBasisDependencies {
  openingCosts: Record<string, string>;
  transferCosts: Record<string, string>;
  disposedCosts: Record<string, string>;
}
export interface InvestmentReviewCosts extends TransferBasisDependencies {
  transferEvidence: Record<string, TransferBasisDependencies>;
}
export interface ReviewCostUpdate {
  openingCosts?: Record<string, string>;
  transferCosts?: Record<string, string>;
  disposedCosts?: Record<string, string>;
  fill?: { id: string; unitCost: string; dependencies: TransferBasisDependencies };
}

/** Update costs and invalidate dependent helper fills atomically, before approval
 * can observe them. Manual edits take ownership of their own value. Dependencies
 * refer only to inputs actually used by the calculation, and can form a chain.
 */
export function updateInvestmentReviewCosts(
  previous: InvestmentReviewCosts,
  update: ReviewCostUpdate,
): InvestmentReviewCosts {
  const next = {
    openingCosts: { ...previous.openingCosts, ...update.openingCosts },
    transferCosts: { ...previous.transferCosts, ...update.transferCosts },
    disposedCosts: { ...previous.disposedCosts, ...update.disposedCosts },
    transferEvidence: { ...previous.transferEvidence },
  };
  for (const id of Object.keys(update.transferCosts ?? {})) delete next.transferEvidence[id];
  if (update.fill && !next.transferCosts[update.fill.id]?.trim()) {
    const { id, unitCost, dependencies } = update.fill;
    next.transferCosts[id] = unitCost;
    next.transferEvidence[id] = dependencies;
  }
  let invalidated: boolean;
  do {
    invalidated = false;
    for (const [id, dependencies] of Object.entries(next.transferEvidence)) {
      if (
        Object.entries(dependencies.openingCosts).some(
          ([key, value]) => next.openingCosts[key] !== value,
        ) ||
        Object.entries(dependencies.transferCosts).some(
          ([key, value]) => next.transferCosts[key] !== value,
        ) ||
        Object.entries(dependencies.disposedCosts).some(
          ([key, value]) => next.disposedCosts[key] !== value,
        )
      ) {
        next.transferCosts[id] = "";
        delete next.transferEvidence[id];
        invalidated = true;
      }
    }
  } while (invalidated);
  return next;
}

export interface TransferStatement {
  name: string;
  asOf: string;
  candidates: BasisCandidate[];
  exclusions: PdfBasisExclusion[];
  sources: Record<number, { page: string; line: string }>;
}

/** Use the same bounded, versioned PDF extraction, independently of the opening cutoff. */
export async function readTransferStatement(
  file: File,
  formatId: string,
  currency: string,
  excludeUnavailableBasis: boolean,
  admit: () => void,
): Promise<TransferStatement> {
  const format = BROKERAGE_BASIS_FORMATS.find((f) => f.id === formatId && f.kind === "pdf");
  if (!format?.extractPdf || !/\.pdf$/i.test(file.name)) throw new BasisError("brokerageFormat");
  const exclusions: PdfBasisExclusion[] = [];
  const text = await readBasisFile(file, (pages) =>
    format.extractPdf!(pages, undefined, {
      excludeUnavailableBasis,
      onExcluded: (entry) => exclusions.push(entry),
    }),
  );
  admit();
  const parsed = await parseCsv(
    new File([text], "transfer-basis.csv", { type: "text/csv" }),
    createDefaultParseConfig(currency),
  );
  admit();
  if (parsed.errors.length || parsed.rowCount !== parsed.rows.length || !parsed.rows.length)
    throw new BasisError("structure");
  const table = format.normalize(parsed, currency);
  const asOf = basisDate(table.rows[0][table.columns.date!]);
  const nextDay = new Date(`${asOf}T00:00:00Z`);
  nextDay.setUTCDate(nextDay.getUTCDate() + 1);
  // This is a statement's own end-of-day checkpoint, never an opening-position proposal.
  const result = calculateBasis({
    ...table,
    mode: "positions",
    asOf,
    startDate: nextDay.toISOString().slice(0, 10),
    currency,
    completeHistory: false,
  });
  return {
    name: file.name,
    asOf,
    candidates: result.candidates,
    exclusions,
    sources: Object.fromEntries(
      table.rows.map((row, i) => [
        i + 1,
        {
          page: row[table.headers.indexOf("Source page")],
          line: row[table.headers.indexOf("Source line")],
        },
      ]),
    ),
  };
}

function amount(value: string | undefined): Decimal {
  if (!value || !/^\d+(?:\.\d+)?$/.test(value)) throw new BasisError("missingCost");
  const n = new Money(value);
  if (!n.isFinite() || n.gt("1e20") || n.decimalPlaces() > 18) throw new BasisError("number");
  return n;
}

/** Isolate one transfer's book cost; never use market value, proceeds or inferred lot relief. */
export function calculateTransferBasis(input: {
  preview: PlaidInvestmentPreview;
  statement: TransferStatement;
  candidate: BasisCandidate;
  transferId: string;
  openingCosts: Record<string, string>;
  transferCosts: Record<string, string>;
  disposedCosts: Record<string, string>;
}) {
  const { preview, statement, candidate, openingCosts, transferCosts, disposedCosts } = input;
  const target = preview.records.find((r) => r.id === input.transferId);
  if (
    !target?.securityId ||
    target.type !== "TRANSFER_IN" ||
    !target.requiresTransferCost ||
    target.transferUnitCost != null
  )
    throw new BasisError("transferTarget");
  if (preview.issues.length) throw new BasisError("previewIssues");
  if (
    basisDate(statement.asOf) < basisDate(target.date) ||
    target.date < basisDate(preview.startDate)
  )
    throw new BasisError("transferDate");
  const opening = preview.openingPositions.find((p) => p.securityId === target.securityId);
  let quantity = opening ? amount(opening.quantity) : new Money(0);
  const prior = opening
    ? quantity.mul(amount(opening.unitCost ?? openingCosts[opening.securityId]))
    : new Money(0);
  const dependencies: TransferBasisDependencies = {
    openingCosts: opening
      ? { [opening.securityId]: opening.unitCost ?? openingCosts[opening.securityId] }
      : {},
    transferCosts: {},
    disposedCosts: {},
  };
  let purchases = new Money(0),
    otherTransfers = new Money(0),
    disposed = new Money(0);
  const seen = new Set<string>();
  const records = preview.records
    .filter((r) => r.securityId === target.securityId && r.date <= statement.asOf)
    .sort(
      (a, b) =>
        a.date.localeCompare(b.date) ||
        Number(!["BUY", "TRANSFER_IN"].includes(a.type)) -
          Number(!["BUY", "TRANSFER_IN"].includes(b.type)) ||
        a.id.localeCompare(b.id),
    );
  for (const row of records) {
    basisDate(row.date);
    if (seen.has(row.id) || row.date < preview.startDate) throw new BasisError("duplicate");
    seen.add(row.id);
    const q = amount(row.quantity);
    if (["BUY", "TRANSFER_IN", "SELL", "TRANSFER_OUT"].includes(row.type)) {
      if (q.lte(0)) throw new BasisError("number");
      if (row.type === "BUY") {
        // Preview cash is net of explicit fees; those fees are already included in buy basis.
        if (!row.cash.startsWith("-")) throw new BasisError("number");
        purchases = purchases.plus(amount(row.cash.slice(1)));
        quantity = quantity.plus(q);
      } else if (row.type === "TRANSFER_IN") {
        quantity = quantity.plus(q);
        if (row.id !== target.id) {
          const cost = row.transferUnitCost ?? transferCosts[row.id];
          if (!cost?.trim()) throw new BasisError("ambiguousTransfers");
          otherTransfers = otherTransfers.plus(q.mul(amount(cost)));
          dependencies.transferCosts[row.id] = cost;
        }
      } else {
        quantity = quantity.minus(q);
        disposed = disposed.plus(amount(disposedCosts[row.id]));
        dependencies.disposedCosts[row.id] = disposedCosts[row.id];
      }
      if (quantity.lt(0)) throw new BasisError("incomplete");
    } else if (
      !["DIVIDEND", "INTEREST", "FEE", "TAX", "DEPOSIT", "WITHDRAWAL"].includes(row.type) ||
      !q.isZero()
    ) {
      throw new BasisError("action");
    }
  }
  const match = matchBasis(candidate, quantity.toFixed(), statement.exclusions);
  if (!match) throw new BasisError("transferQuantity");
  const ending = amount(candidate.totalCost);
  const transferBasis = ending.minus(prior).minus(purchases).minus(otherTransfers).plus(disposed);
  if (transferBasis.lt(0) || transferBasis.gt("1e20")) throw new BasisError("incomplete");
  // A later incoming transfer cannot fund an earlier disposal's book cost.
  // Check the chronology too, rather than merely balancing month-end totals.
  let runningBasis = prior;
  let runningQuantity = opening ? amount(opening.quantity) : new Money(0);
  // Only allow the representation error of unit costs rounded to the supported
  // 18 places. This is not a general monetary or share-quantity tolerance.
  const rounding = (quantity: string, cost: string) =>
    amount(cost).decimalPlaces() === 18
      ? amount(quantity).mul("0.0000000000000000005")
      : new Money(0);
  let allowance = opening
    ? rounding(opening.quantity, opening.unitCost ?? openingCosts[opening.securityId])
    : new Money(0);
  for (const row of records) {
    if (row.type === "BUY") {
      runningBasis = runningBasis.plus(amount(row.cash.slice(1)));
      runningQuantity = runningQuantity.plus(amount(row.quantity));
    } else if (row.type === "TRANSFER_IN") {
      runningQuantity = runningQuantity.plus(amount(row.quantity));
      if (row.id === target.id) runningBasis = runningBasis.plus(transferBasis);
      else {
        const cost = row.transferUnitCost ?? transferCosts[row.id];
        runningBasis = runningBasis.plus(amount(row.quantity).mul(amount(cost)));
        allowance = allowance.plus(rounding(row.quantity, cost));
      }
    } else if (["SELL", "TRANSFER_OUT"].includes(row.type)) {
      runningBasis = runningBasis.minus(amount(disposedCosts[row.id]));
      runningQuantity = runningQuantity.minus(amount(row.quantity));
    }
    if (
      runningBasis.lt(allowance.neg()) ||
      (runningQuantity.isZero() && runningBasis.abs().gt(allowance))
    )
      throw new BasisError("incomplete");
  }
  const unitCost = transferBasis.div(amount(target.quantity)).toDecimalPlaces(18).toFixed();
  amount(unitCost);
  if (transferBasis.gt(0) && new Money(unitCost).isZero()) throw new BasisError("number");
  return {
    dependencies,
    unitCost,
    totalCost: transferBasis.toFixed(),
    prior: prior.toFixed(),
    purchases: purchases.toFixed(),
    otherTransfers: otherTransfers.toFixed(),
    disposed: disposed.toFixed(),
    ending: ending.toFixed(),
    quantity: quantity.toFixed(),
    excludedQuantity: match.excludedQuantity,
    statementUnitCost: match.unitCost,
  };
}
