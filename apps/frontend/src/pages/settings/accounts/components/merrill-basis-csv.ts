import Decimal from "decimal.js";

import { BasisError, basisDate, type BasisColumns } from "./opening-basis";

const Money = Decimal.clone({ precision: 40 });

// Header vocabulary only. Fixtures use invented accounts, securities and amounts.
export const MERRILL_ACTIVITY_HEADERS = [
  "Trade Date",
  "Settlement Date",
  "Pending/Settled",
  "Account Nickname",
  "Account Registration",
  "Account #",
  "Type",
  "Description 1",
  "Description 2",
  "Symbol/CUSIP #",
  "Quantity",
  "Price ($)",
  "Amount ($)",
];
export interface BasisTable {
  headers: string[];
  rows: string[][];
}
export interface NormalizedBasisTable extends BasisTable {
  columns: BasisColumns;
  sourceRows: number[];
  cashRows: number[];
}
export interface BasisSourceOptions {
  onAccount?: (identity: string) => void;
  allowCashOnly?: boolean;
}

function number(value: string, row: number): Decimal {
  const text = value.trim();
  const raw =
    text.startsWith("(") && text.endsWith(")")
      ? `-${text.slice(1, -1).replace(/^\$/, "")}`
      : text.replace(/^\$/, "");
  if (!/^-?(?:\d+|\d{1,3}(?:,\d{3})+)(?:\.\d+)?$/.test(raw)) throw new BasisError("number", row);
  const result = new Money(raw.replaceAll(",", ""));
  if (!result.isFinite() || result.abs().gt("1e20") || result.decimalPlaces() > 18)
    throw new BasisError("number", row);
  return result;
}
function date(value: string): string {
  const match = /^(\d{1,2})\/(\d{1,2})\/(\d{4})$/.exec(value.trim());
  if (!match) throw new BasisError("date");
  return basisDate(`${match[3]}-${match[1].padStart(2, "0")}-${match[2].padStart(2, "0")}`);
}

/** Merrill Settled Activity CSV v1. Never infer sale basis from cash proceeds. */
export function normalizeMerrillActivity(
  table: BasisTable,
  currency: string,
  options: BasisSourceOptions = {},
): NormalizedBasisTable {
  if (currency !== "USD") throw new BasisError("currency");
  if (
    table.headers.length !== MERRILL_ACTIVITY_HEADERS.length ||
    table.headers.some((header, i) => header.trim() !== MERRILL_ACTIVITY_HEADERS[i])
  )
    throw new BasisError("brokerageFormat");
  if (!table.rows.length || table.rows.length > 10000) throw new BasisError("size");
  const accounts = new Set<string>();
  const result: NormalizedBasisTable = {
    headers: ["Security", "Quantity", "Total cost basis", "Date", "Action", "Currency"],
    rows: [],
    columns: { security: 0, quantity: 1, totalCost: 2, date: 3, action: 4, currency: 5 },
    sourceRows: [],
    cashRows: [],
  };
  for (const [index, cells] of table.rows.entries()) {
    const row = index + 1;
    if (cells.length !== MERRILL_ACTIVITY_HEADERS.length) throw new BasisError("structure", row);
    const [
      tradeDate,
      settlementDate,
      status,
      ,
      ,
      account,
      type,
      description1,
      ,
      security,
      quantity,
      price,
      cash,
    ] = cells.map((v) => v.trim());
    if (!account) throw new BasisError("brokerageAccount", row);
    accounts.add(account.replace(/\s/g, ""));
    if (accounts.size !== 1) throw new BasisError("brokerageAccount", row);
    if (status !== "Settled") throw new BasisError("brokeragePending", row);
    const trade = date(tradeDate);
    if (date(settlementDate) < trade) throw new BasisError("date", row);
    // Description 1 is the operation; Description 2 is the security name.
    // Fund names such as “short treasury bond” do not describe transaction economics.
    const description = description1.toUpperCase();
    if (type === "DividendAndInterest") {
      if (
        !["DIVIDEND", "INTEREST"].includes(description) ||
        !["", "--"].includes(quantity) ||
        !["", "--"].includes(price) ||
        number(cash, row).lte(0)
      )
        throw new BasisError("brokerageRecord", row);
      result.cashRows.push(row);
      continue;
    }
    if (
      type !== "SecurityTransactions" ||
      description !== "PURCHASE" ||
      /\b(?:SALE|SOLD|SELL)\b/.test(description)
    )
      throw new BasisError(
        type === "SecurityTransactions" && /\b(?:SALE|SOLD|SELL)\b/.test(description)
          ? "saleBasis"
          : "brokerageRecord",
        row,
      );
    if (!/^[A-Z]{1,6}(?:[.-][A-Z])?$/.test(security)) throw new BasisError("security", row);
    const qty = number(quantity, row),
      unitPrice = number(price, row),
      netCash = number(cash, row);
    if (
      qty.lte(0) ||
      unitPrice.lte(0) ||
      netCash.gte(0) ||
      netCash.abs().plus("0.01").lt(qty.times(unitPrice))
    )
      throw new BasisError("brokerageRecord", row);
    // The purchase's net debit includes its acquisition charges. No assumed zero-fee column.
    result.rows.push([security, qty.toFixed(), netCash.abs().toFixed(), trade, "BUY", "USD"]);
    result.sourceRows.push(row);
  }
  if (!result.rows.length && !options.allowCashOnly) throw new BasisError("noBasisTrades");
  options.onAccount?.([...accounts][0].replace(/[^A-Z0-9]/gi, "").toUpperCase());
  return result;
}
