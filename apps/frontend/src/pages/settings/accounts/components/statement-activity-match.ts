import Decimal from "decimal.js";
import type { PlaidInvestmentPreview } from "@/adapters/shared/plaid";
import { BasisError, basisDate } from "./opening-basis";
import type { TransferStatement } from "./transfer-basis";
import type { StatementDisposition } from "./merrill-statement-v2";

const Money = Decimal.clone({ precision: 40 });
export type InvestmentRecord = PlaidInvestmentPreview["records"][number];
/** Explicitly selected lots establish identity. Quantity alone never matches a security.
 * Sale amount must reconcile with provider cash plus its explicit fee. A differing
 * trade/settlement date needs separate user verification; no automatic date window.
 */
export function proposeDisposedBasis(input: {
  record: InvestmentRecord;
  statement: TransferStatement;
  security: string;
  rowIds: string[];
  verifiedDates: boolean;
}) {
  const { record, statement, security, rowIds, verifiedDates } = input;
  if (
    !statement.activity ||
    record.type !== "SELL" ||
    !record.securityId ||
    !security ||
    !rowIds.length ||
    new Set(rowIds).size !== rowIds.length
  )
    throw new BasisError("brokerageRecord");
  const rows = rowIds.map((id) => statement.activity!.dispositions.find((r) => r.id === id));
  if (rows.some((r) => !r || (r.security && r.security !== security)))
    throw new BasisError("security");
  let quantity = new Money(0),
    basis = new Money(0),
    proceeds = new Money(0);
  for (const r of rows as StatementDisposition[]) {
    basisDate(r.date);
    if (
      r.date < statement.activity.from ||
      r.date > statement.asOf ||
      (r.date !== record.date && !verifiedDates)
    )
      throw new BasisError("date");
    if (r.basis === null || r.proceeds === null) throw new BasisError("missingCost");
    for (const n of [r.quantity, r.basis, r.proceeds]) {
      if (
        !/^\d+(?:\.\d+)?$/.test(n) ||
        new Money(n).gt("1e20") ||
        new Money(n).decimalPlaces() > 18
      )
        throw new BasisError("number");
    }
    quantity = quantity.plus(r.quantity);
    basis = basis.plus(r.basis);
    proceeds = proceeds.plus(r.proceeds);
  }
  if (basis.gt("1e20") || quantity.gt("1e20") || proceeds.gt("1e20"))
    throw new BasisError("number");
  if (!quantity.eq(record.quantity)) throw new BasisError("transferQuantity");
  if (
    !/^\d+(?:\.\d+)?$/.test(record.cash) ||
    !/^\d+(?:\.\d+)?$/.test(record.fee) ||
    proceeds.minus(new Money(record.cash).plus(record.fee)).abs().gt("0.01")
  )
    throw new BasisError("brokerageReconcile");
  return {
    value: basis.toFixed(),
    rows: (rows as StatementDisposition[]).map(({ id, page, line }) => ({ id, page, line })),
  };
}
