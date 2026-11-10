import Decimal from "decimal.js";

const Money = Decimal.clone({ precision: 40 });
export const basisFields = [
  "security",
  "quantity",
  "totalCost",
  "unitCost",
  "date",
  "action",
  "price",
  "fee",
  "currency",
] as const;
export type BasisField = (typeof basisFields)[number];
export type BasisColumns = Partial<Record<BasisField, number>>;
export interface BasisCandidate {
  security: string;
  quantity: string;
  totalCost: string;
  unitCost: string;
  rows: number[];
}
export interface BasisCheckpoint {
  asOf: string;
  currency: string;
  candidates: BasisCandidate[];
}
export class BasisError extends Error {
  constructor(
    public readonly code: string,
    public readonly row?: number,
  ) {
    super(code);
  }
}

// US statement numbers only. Never strip unknown characters or guess a locale.
function amount(value: string, row: number): Decimal {
  const text = value.trim().replace(/^\$/, "");
  if (!/^(?:\d+|\d{1,3}(?:,\d{3})+)(?:\.\d+)?$/.test(text)) {
    throw new BasisError("number", row);
  }
  const result = new Money(text.replaceAll(",", ""));
  if (!result.isFinite() || result.gt("1e20") || result.decimalPlaces() > 18) {
    throw new BasisError("number", row);
  }
  return result;
}

export function basisDate(value: string): string {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) throw new BasisError("date");
  const date = new Date(`${value}T00:00:00Z`);
  if (!Number.isFinite(date.getTime()) || date.toISOString().slice(0, 10) !== value) {
    throw new BasisError("date");
  }
  return value;
}

export function openingDate(startDate: string): string {
  const date = new Date(`${basisDate(startDate)}T00:00:00Z`);
  date.setUTCDate(date.getUTCDate() - 1);
  return date.toISOString().slice(0, 10);
}

export function suggestBasisColumns(headers: string[]): BasisColumns {
  const aliases: Record<BasisField, string[]> = {
    security: ["symbol", "ticker", "security", "description"],
    quantity: ["quantity", "shares", "units"],
    totalCost: ["cost basis", "total cost basis", "total cost", "adjusted cost basis"],
    unitCost: ["average cost", "average cost per share", "unit cost", "cost per share"],
    date: ["date", "trade date"],
    action: ["action", "type", "transaction type"],
    price: ["price", "purchase price", "price per share"],
    fee: ["fee", "fees", "commission"],
    currency: ["currency"],
  };
  return Object.fromEntries(
    basisFields.flatMap((field) => {
      const matches = headers.flatMap((header, index) =>
        aliases[field].includes(header.toLowerCase().trim()) ? [index] : [],
      );
      return matches.length === 1 ? [[field, matches[0]]] : [];
    }),
  );
}

/** Pure evidence calculation, never an activity import or a tax-lot inference. */
export function calculateBasis(input: {
  headers: string[];
  rows: string[][];
  columns: BasisColumns;
  mode: "positions" | "transactions";
  asOf: string;
  startDate: string;
  currency: string;
  completeHistory: boolean;
  checkpoint?: BasisCheckpoint;
  sourceRows?: number[];
}): { candidates: BasisCandidate[]; excluded: number } {
  const { headers, rows, columns, mode, currency } = input;
  const cutoff = openingDate(input.startDate);
  if (basisDate(input.asOf) > cutoff || (mode === "transactions" && input.asOf !== cutoff))
    throw new BasisError("cutoff");
  if (
    input.checkpoint &&
    (input.checkpoint.currency !== currency || basisDate(input.checkpoint.asOf) >= cutoff)
  )
    throw new BasisError("cutoff");
  if (mode === "transactions" && !input.completeHistory) throw new BasisError("history");
  const required: BasisField[] =
    mode === "positions" ? ["security", "quantity"] : ["security", "quantity", "date", "action"];
  if (
    required.some((field) => columns[field] === undefined) ||
    (mode === "positions" && columns.totalCost === undefined && columns.unitCost === undefined)
  ) {
    throw new BasisError("columns");
  }
  const indices = Object.values(columns);
  if (
    new Set(indices).size !== indices.length ||
    indices.some((i) => !Number.isInteger(i) || i < 0 || i >= headers.length)
  ) {
    throw new BasisError("columns");
  }
  if (rows.length === 0 || rows.length > 10000) throw new BasisError("size");
  if (
    input.sourceRows &&
    (input.sourceRows.length !== rows.length ||
      new Set(input.sourceRows).size !== rows.length ||
      input.sourceRows.some((row) => !Number.isInteger(row) || row < 1))
  )
    throw new BasisError("structure");
  const entries = rows
    .map((row, index) => {
      const line = input.sourceRows?.[index] ?? index + 1;
      if (row.length !== headers.length) throw new BasisError("structure", line);
      const get = (field: BasisField) =>
        columns[field] === undefined ? "" : (row[columns[field]!] ?? "").trim();
      let date = input.asOf;
      if (mode === "transactions" || columns.date !== undefined) {
        const raw = get("date");
        // Explicit US dates; no day/month auto-detection.
        const us = /^(\d{1,2})\/(\d{1,2})\/(\d{4})$/.exec(raw);
        date = basisDate(us ? `${us[3]}-${us[1].padStart(2, "0")}-${us[2].padStart(2, "0")}` : raw);
      }
      return { line, date, get };
    })
    .sort((a, b) => a.date.localeCompare(b.date) || a.line - b.line);
  const positions = new Map<string, { quantity: Decimal; cost: Decimal; rows: number[] }>();
  if (mode === "transactions" && input.checkpoint) {
    for (const candidate of input.checkpoint.candidates) {
      if (positions.has(candidate.security)) throw new BasisError("duplicate");
      positions.set(candidate.security, {
        quantity: amount(candidate.quantity, 0),
        cost: amount(candidate.totalCost, 0),
        rows: [],
      });
    }
  }
  const seen = new Set<string>();
  let excluded = 0;
  for (const { line, date, get } of entries) {
    if (
      mode === "transactions" &&
      (date > cutoff || (input.checkpoint && date <= input.checkpoint.asOf))
    ) {
      excluded++;
      continue;
    }
    if (mode === "positions" && date !== input.asOf) throw new BasisError("cutoff", line);
    if (columns.currency !== undefined && get("currency").toUpperCase() !== currency)
      throw new BasisError("currency", line);
    const security = get("security").toUpperCase();
    if (!security || security.length > 200) throw new BasisError("security", line);
    const quantity = amount(get("quantity"), line);
    if (quantity.lte(0)) throw new BasisError("number", line);
    if (mode === "positions") {
      if (positions.has(security)) throw new BasisError("duplicate", line);
      const cost =
        columns.totalCost === undefined
          ? quantity.times(amount(get("unitCost"), line))
          : amount(get("totalCost"), line);
      if (
        columns.totalCost !== undefined &&
        columns.unitCost !== undefined &&
        cost
          .minus(quantity.times(amount(get("unitCost"), line)))
          .abs()
          .gt("0.01")
      ) {
        throw new BasisError("conflict", line);
      }
      positions.set(security, { quantity, cost, rows: [line] });
      continue;
    }
    const action = get("action").toUpperCase();
    if (!["BUY", "SELL"].includes(action)) throw new BasisError("action", line);
    const old = positions.get(security) ?? { quantity: new Money(0), cost: new Money(0), rows: [] };
    let cost: Decimal;
    if (get("totalCost")) cost = amount(get("totalCost"), line);
    else if (action === "BUY" && columns.price !== undefined && columns.fee !== undefined) {
      cost = quantity.times(amount(get("price"), line)).plus(amount(get("fee"), line));
    } else throw new BasisError("saleBasis", line);
    const fingerprint = JSON.stringify([
      security,
      date,
      action,
      quantity.toFixed(),
      cost.toFixed(),
    ]);
    if (seen.has(fingerprint)) throw new BasisError("duplicate", line);
    seen.add(fingerprint);
    const nextQuantity =
      action === "BUY" ? old.quantity.plus(quantity) : old.quantity.minus(quantity);
    const nextCost = action === "BUY" ? old.cost.plus(cost) : old.cost.minus(cost);
    if (nextQuantity.lt(0) || nextCost.lt(0) || (nextQuantity.isZero() && !nextCost.isZero())) {
      throw new BasisError("incomplete", line);
    }
    positions.set(security, { quantity: nextQuantity, cost: nextCost, rows: [...old.rows, line] });
  }
  const candidates = [...positions]
    .filter(([, p]) => p.quantity.gt(0))
    .map(([security, p]) => ({
      security,
      quantity: p.quantity.toFixed(),
      totalCost: p.cost.toFixed(),
      unitCost: p.cost.div(p.quantity).toDecimalPlaces(18).toFixed(),
      rows: p.rows,
    }));
  if (!candidates.length) throw new BasisError("empty");
  if (candidates.some((p) => new Money(p.totalCost).gt(0) && new Money(p.unitCost).isZero()))
    throw new BasisError("number");
  return { candidates, excluded };
}

export function matchingBasis(candidate: BasisCandidate, quantity: string): boolean {
  try {
    return new Money(candidate.quantity).eq(quantity);
  } catch {
    return false;
  }
}
