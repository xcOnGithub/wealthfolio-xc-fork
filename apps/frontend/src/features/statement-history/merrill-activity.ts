import Decimal from "decimal.js";
import { BasisError, basisDate, pdfRows, type PdfTextItem } from "./statement-pdf";
import {
  merrillIdentity,
  merrillPeriod,
  merrillNumber as number,
  type MerrillStatementSource,
} from "./merrill-format";

const Money = Decimal.clone({ precision: 40 });
export type StatementSection =
  | "securities"
  | "transfers"
  | "realized"
  | "income"
  | "cash"
  | "sweep"
  | "pending";
export interface StatementSource {
  page: number;
  line: number;
}
export interface StatementActivity extends StatementSource {
  id: string;
  section: StatementSection;
  description: string;
  security?: string;
  date: string | null;
  dateKind: "settlement" | "liquidation" | "trade" | "unknown";
  action: "BUY" | "SELL" | "TRANSFER_IN" | "TRANSFER_OUT" | "CASH" | "INCOME" | "UNSUPPORTED";
  quantity: string | null;
  cash: string | null;
  fee: string | null;
  transferValue: string | null;
}
export interface StatementDisposition extends StatementSource {
  id: string;
  description: string;
  security?: string;
  date: string;
  acquired: string | null;
  quantity: string;
  basis: string | null;
  proceeds: string | null;
}
export interface StatementEvidence {
  from: string;
  through: string;
  sections: Partial<Record<StatementSection, number>>;
  activities: StatementActivity[];
  dispositions: StatementDisposition[];
}
const normalizeName = (s: string) => s.toUpperCase().replace(/[^A-Z0-9]/g, "");
function cleanDescription(s: string) {
  return s
    .split(
      /\b(?:CLIENT ENTERED|DISCRETION NOT EXERCISED|ORDERS MAY BE ROUTED|AS PRINCIPAL OR AGENT)\b/i,
    )[0]
    .trim()
    .replace(/\s+/g, " ");
}
function statementDate(
  raw: string,
  from: string,
  through: string,
  acquisition = false,
): string | null {
  if (
    !raw.trim() ||
    /^various$/i.test(raw.trim()) ||
    (acquisition && /^(?:--|-|N\/A|NA)$/i.test(raw.trim()))
  )
    return null;
  const m = /^(\d{1,2})\/(\d{1,2})(?:\/(\d{2}|\d{4}))?$/.exec(raw.trim());
  if (!m) throw new BasisError("date");
  const years = m[3]
    ? [m[3].length === 4 ? m[3] : `${through.slice(0, 2)}${m[3]}`]
    : [...new Set([from.slice(0, 4), through.slice(0, 4)])];
  if (acquisition && !m[3]) throw new BasisError("date");
  if (acquisition && m[3]?.length === 2 && Number(years[0]) > Number(through.slice(0, 4)))
    years[0] = String(Number(years[0]) - 100);
  const dates = years.map((year) =>
    basisDate(`${year}-${m[1].padStart(2, "0")}-${m[2].padStart(2, "0")}`),
  );
  if (m[3]) return dates[0];
  const inPeriod = dates.filter((date) => date >= from && date <= through);
  if (inPeriod.length !== 1) throw new BasisError("date");
  return inPeriod[0];
}
interface Columns {
  description: number;
  blocks?: { date: number; description: number; debit: number; credit: number }[];
  type?: number;
  dates: number[];
  dateKind: StatementActivity["dateKind"];
  quantity?: number;
  net?: number;
  debit?: number;
  credit?: number;
  fee?: number;
  value?: number;
  basis?: number;
  proceeds?: number;
}
const right = (c: PdfTextItem) => c.transform[4] + c.width;
function at(cells: PdfTextItem[], anchor?: number) {
  if (anchor === undefined) return "";
  const matches = cells.filter((c) => Math.abs(right(c) - anchor) < 6);
  if (matches.length > 1) throw new BasisError("structure");
  return matches[0]?.str.trim() ?? "";
}
/** Blank/-- means an unused side of a cash movement; N/A stays unknown. */
function cashMovement(
  cells: PdfTextItem[],
  debitAnchor?: number,
  creditAnchor?: number,
): string | null {
  const raw = [at(cells, debitAnchor), at(cells, creditAnchor)];
  const values = raw.map(number);
  if (values.some((v, i) => v === null && !["", "--", "-"].includes(raw[i]))) return null;
  const [debit, credit] = values;
  return debit === null && credit === null
    ? null
    : new Money(credit ?? 0).minus(new Money(debit ?? 0).abs()).toFixed();
}
function identifyHeader(
  cells: PdfTextItem[],
  previous: PdfTextItem[],
  section: StatementSection,
): Columns | undefined {
  const find = (s: string) => cells.find((c) => c.str.trim() === s);
  const description = find("Description");
  if (!description) return;
  const dates = cells
    .filter((c) => c.str.trim() === "Date")
    .sort((a, b) => a.transform[4] - b.transform[4]);
  if (!dates.length) return;
  const q = find("Quantity"),
    cost = find("Cost Basis"),
    sale = find("Sale Amount"),
    type = find("Transaction Type");
  if (section === "realized") {
    if (
      !q ||
      !cost ||
      !sale ||
      dates.length !== 2 ||
      !previous.some((c) => c.str.includes("Liquidation"))
    )
      throw new BasisError("brokerageFormat");
    return {
      description: description.transform[4],
      dates: dates.map(right),
      dateKind: "liquidation",
      quantity: right(q),
      basis: right(cost),
      proceeds: right(sale),
    };
  }
  if (section === "sweep") {
    const descriptions = cells.filter((c) => c.str.trim() === "Description");
    const debits = cells.filter((c) => c.str.trim() === "Withdrawals");
    const credits = cells.filter((c) => c.str.trim() === "Deposits");
    if (
      dates.length !== 2 ||
      descriptions.length !== 2 ||
      debits.length !== 2 ||
      credits.length !== 2
    )
      throw new BasisError("brokerageFormat");
    return {
      description: description.transform[4],
      dates: [],
      dateKind: "unknown",
      blocks: dates.map((d, i) => ({
        date: d.transform[4],
        description: descriptions[i].transform[4],
        debit: right(debits[i]),
        credit: right(credits[i]),
      })),
    };
  }
  if (section === "pending") {
    const pendingType = find("Type"),
      amount = find("Amount");
    if (
      !q ||
      !pendingType ||
      !amount ||
      dates.length !== 2 ||
      !previous.some((c) => c.str.trim() === "Settlement")
    )
      throw new BasisError("brokerageFormat");
    return {
      description: description.transform[4],
      type: pendingType.transform[4],
      dates: dates.map((c) => c.transform[4]),
      dateKind: "trade",
      quantity: right(q),
      net: right(amount),
    };
  }
  if (!type || !q || dates.length !== 1) throw new BasisError("brokerageFormat");
  const result: Columns = {
    description: description.transform[4],
    type: type.transform[4],
    dates: [dates[0].transform[4]],
    dateKind: section === "securities" ? "settlement" : "unknown",
    quantity: right(q),
  };
  if (section === "securities") {
    const credit = find("Credit"),
      fees = find("Trading Fees"),
      amount = find("Amount");
    if (!credit || !fees || !amount || !previous.some((c) => c.str.includes("Settlement")))
      throw new BasisError("brokerageFormat");
    result.net = right(credit);
    result.fee = right(fees);
  } else if (section === "transfers") {
    const value = find("Value of Securities");
    if (!value) throw new BasisError("brokerageFormat");
    result.value = right(value);
  } else if (section === "income") {
    const income = find("Income");
    if (!income) throw new BasisError("brokerageFormat");
    result.net = right(income);
  } else {
    const debit = find("Debit"),
      credit = find("Credit");
    if (!debit || !credit) throw new BasisError("brokerageFormat");
    result.debit = right(debit);
    result.credit = right(credit);
  }
  return result;
}
function sectionName(text: string): StatementSection | undefined {
  if (text.startsWith("UNSETTLED TRADES")) return "pending";
  if (/^SECURITIES YOU TRANSFERRED IN\/OUT$/.test(text)) return "transfers";
  if (text.startsWith("REALIZED GAINS/(LOSSES)")) return "realized";
  if (/^SECURITY TRANSACTIONS(?:\s+\(continued\))?$/i.test(text)) return "securities";
  if (text.startsWith("DIVIDENDS/INTEREST INCOME TRANSACTIONS")) return "income";
  if (text.startsWith("CASH/OTHER TRANSACTIONS")) return "cash";
  if (/^(?:YOUR )?CMA MONEY ACCOUNT TRANSACTIONS/.test(text)) return "sweep";
  return undefined;
}

/** Browser-local activity evidence, independent of acquisition-cost availability. */
export function extractMerrillActivity(
  pages: PdfTextItem[][],
  positions: { security: string; description: string }[] = [],
  options?: { pageNumbers?: number[]; source?: MerrillStatementSource },
): StatementEvidence {
  const { identity: account, from, through } = options?.source ?? merrillIdentity(pages);
  const evidence: StatementEvidence = {
    from,
    through,
    sections: {},
    activities: [],
    dispositions: [],
  };
  const bindSecurity = (description: string) => {
    const name = normalizeName(description);
    const matches = positions.filter((p) =>
      [
        p.description,
        p.description.replace(
          new RegExp(`\\s+${p.security.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`),
          "",
        ),
      ].some((s) => normalizeName(s) === name),
    );
    return matches.length === 1 ? matches[0].security : undefined;
  };
  let section: StatementSection | undefined;
  const pageFingerprints = new Set<string>();
  let totals: Record<string, Decimal> = {};
  let unknownTotals = new Set<string>();
  for (const [pageIndex, items] of pages.entries()) {
    const pageNumber = options?.pageNumbers?.[pageIndex] ?? pageIndex + 1;
    const rows = pdfRows(items).map((r) => ({
      ...r,
      cells: [...r.cells].sort((a, b) => a.transform[4] - b.transform[4]),
    }));
    if (
      !rows.some((r) =>
        /CMA (?:TRANSACTIONS|MONEY ACCOUNT TRANSACTIONS)/.test(r.cells.map((c) => c.str).join(" ")),
      )
    )
      continue;
    const identity = /Account\s+Number:\s*([A-Z0-9]+(?:-[A-Z0-9]+)+)/i.exec(
      items.map((c) => c.str).join(" "),
    );
    if (
      !identity ||
      identity[1].replace(/[^A-Z0-9]/gi, "").toUpperCase() !== account ||
      !items.some((c) => !!merrillPeriod(c.str))
    )
      throw new BasisError("brokerageAccount", undefined, pageNumber);
    // Equal-valued rows can be legitimate fills or tax lots. Reject an exact
    // repeated page instead, retaining its printed page label in the identity.
    const signature = JSON.stringify(
      rows.flatMap((row) =>
        row.cells.map((c) => [c.str.trim(), c.transform[4], c.transform[5], c.width]),
      ),
    );
    if (pageFingerprints.has(signature)) throw new BasisError("duplicate", undefined, pageNumber);
    pageFingerprints.add(signature);
    let columns: Columns | undefined;
    let needsHeader = false;
    let sawHeader = false;
    let pending: StatementActivity | StatementDisposition | undefined;
    let descriptionPrefix = "";
    const add = (field: string, value: string | null) => {
      if (value === null) unknownTotals.add(field);
      else totals[field] = (totals[field] ?? new Money(0)).plus(value);
    };
    const describe = () => {
      if (pending) pending.security = bindSecurity(pending.description);
    };
    for (const [lineIndex, row] of rows.entries()) {
      const page = pageNumber,
        line = lineIndex + 1;
      const text = row.cells
        .map((c) => c.str.trim())
        .join(" ")
        .trim();
      try {
        const next = sectionName(text);
        if (next) {
          if (needsHeader) throw new BasisError("brokerageFormat");
          needsHeader = true;
          describe();
          if (next !== section) {
            totals = {};
            unknownTotals = new Set();
          }
          section = next;
          columns = undefined;
          pending = undefined;
          descriptionPrefix = "";
          evidence.sections[next] ??= 0;
          continue;
        }
        if (!section) continue;
        const header = identifyHeader(row.cells, rows[lineIndex - 1]?.cells ?? [], section);
        if (header) {
          if (section === "cash") {
            totals.cash ??= new Money(0);
            totals.debit ??= new Money(0);
            totals.credit ??= new Money(0);
          }
          sawHeader = true;
          needsHeader = false;
          describe();
          columns = header;
          pending = undefined;
          descriptionPrefix = "";
          continue;
        }
        if (!columns) continue;
        if (/^Sub[\s-]*totals?\b/i.test(text)) {
          describe();
          pending = undefined;
          descriptionPrefix = "";
          continue;
        }
        if (/^(?:NET\s+)?TOTALS?\b/i.test(text)) {
          describe();
          if (section === "cash" && /^NET\s+TOTALS?\b/i.test(text)) {
            const printedNet = cashMovement(row.cells, columns.debit, columns.credit);
            if (
              printedNet !== null &&
              !unknownTotals.has("cash") &&
              (totals.cash ?? new Money(0)).minus(printedNet).abs().gt("0.01")
            )
              throw new BasisError("brokerageReconcile");
          } else {
            for (const [field, anchor] of Object.entries({
              cash: columns.net,
              debit: columns.debit,
              credit: columns.credit,
              value: columns.value,
              basis: columns.basis,
              proceeds: columns.proceeds,
            })) {
              const value = number(at(row.cells, anchor));
              const printed =
                value !== null && field === "debit" ? new Money(value).abs().toFixed() : value;
              if (
                printed !== null &&
                !unknownTotals.has(field) &&
                totals[field] !== undefined &&
                totals[field].minus(printed).abs().gt("0.01")
              )
                throw new BasisError("brokerageReconcile");
            }
          }
          columns = undefined;
          pending = undefined;
          descriptionPrefix = "";
          // A printed TOTAL closes this block. Continuation headers without a
          // TOTAL keep accumulating, but separately closed purchase/sale blocks
          // must not inherit one another's sums or unknown-field flags.
          totals = {};
          unknownTotals = new Set();
          continue;
        }
        const numeric = (anchor?: number) => number(at(row.cells, anchor));
        if (columns.blocks) {
          for (const [blockIndex, block] of columns.blocks.entries()) {
            const date = row.cells.find(
              (c) => Math.abs(c.transform[4] - block.date) < 6 && /^\d{1,2}\/\d{1,2}/.test(c.str),
            );
            if (!date) continue;
            const description = row.cells
              .filter((c) => c.transform[4] >= block.description - 8 && right(c) < block.debit - 30)
              .map((c) => c.str)
              .join(" ");
            evidence.activities.push({
              id: `sweep:${page}:${line}:${blockIndex}`,
              page,
              line,
              section: "sweep",
              description,
              date: statementDate(date.str, from, through),
              dateKind: "unknown",
              action: "CASH",
              quantity: null,
              cash: cashMovement(row.cells, block.debit, block.credit),
              fee: null,
              transferValue: null,
            });
            evidence.sections.sweep = (evidence.sections.sweep ?? 0) + 1;
            if (evidence.activities.length + evidence.dispositions.length > 10000)
              throw new BasisError("size");
          }
          continue;
        }
        const qty = numeric(columns.quantity);
        const dateCell = row.cells.find(
          (c) =>
            Math.abs(c.transform[4] - columns!.dates[0]) < 3 &&
            /^\d{1,2}\/\d{1,2}/.test(c.str.trim()),
        );
        const dateText =
          section === "realized" ? at(row.cells, columns.dates[1]) : (dateCell?.str ?? "");
        const firstNumeric =
          section === "realized"
            ? qty !== null || numeric(columns.basis) !== null || numeric(columns.proceeds) !== null
            : !!dateCell ||
              qty !== null ||
              numeric(columns.net) !== null ||
              numeric(columns.value) !== null;
        const descEnd = section === "realized" ? (columns.quantity ?? 0) - 40 : (columns.type ?? 0);
        const desc = cleanDescription(
          row.cells
            .filter(
              (c) => c.transform[4] >= columns!.description - 8 && c.transform[4] < descEnd - 2,
            )
            .map((c) => c.str.trim())
            .join(" "),
        );
        if (!firstNumeric) {
          if (
            !["income", "cash"].includes(section) &&
            row.cells.some(
              (c) =>
                /^\d{1,2}\/\d{1,2}(?:\/\d{2,4})?$/.test(c.str.trim()) ||
                (c.transform[4] >= (columns!.quantity ?? Infinity) - 50 &&
                  /^\(?-?\$?\d[\d,.]*\)?$/.test(c.str.trim())),
            )
          )
            throw new BasisError("brokerageRecord");
          if (desc && !/^Sub[\s-]*totals?\b/i.test(desc)) {
            if (pending) {
              pending.description = cleanDescription(`${pending.description} ${desc}`);
              describe();
            } else descriptionPrefix = cleanDescription(`${descriptionPrefix} ${desc}`);
          }
          continue;
        }
        describe();
        const description = cleanDescription(`${descriptionPrefix} ${desc}`);
        descriptionPrefix = "";
        if (!description) throw new BasisError("security");
        const date = statementDate(dateText, from, through);
        if (!["income", "realized"].includes(section) && !date) throw new BasisError("date");
        if (section !== "pending" && date && (date < from || date > through))
          throw new BasisError("cutoff");
        if (section === "realized") {
          if (!date || qty === null || new Money(qty).lte(0))
            throw new BasisError("brokerageBasis");
          const basis = numeric(columns.basis),
            proceeds = numeric(columns.proceeds);
          if (
            (basis !== null && new Money(basis).lt(0)) ||
            (proceeds !== null && new Money(proceeds).lt(0))
          )
            throw new BasisError("number");
          const disposition: StatementDisposition = {
            id: `sale:${page}:${line}`,
            page,
            line,
            description,
            date,
            acquired: statementDate(at(row.cells, columns.dates[0]), from, date, true),
            quantity: qty,
            basis,
            proceeds,
          };
          if (disposition.acquired && disposition.acquired > date) throw new BasisError("date");
          evidence.dispositions.push(disposition);
          pending = disposition;
          add("basis", basis);
          add("proceeds", proceeds);
        } else {
          const type = row.cells
            .filter(
              (c) =>
                c.transform[4] >= (columns!.type ?? 0) - 2 &&
                c.transform[4] < (columns!.quantity ?? 0) - 40,
            )
            .map((c) => c.str)
            .join(" ")
            .trim();
          let action: StatementActivity["action"] =
            section === "income"
              ? "INCOME"
              : section === "cash"
                ? "CASH"
                : /^Purchase\b/i.test(type)
                  ? "BUY"
                  : /^Sale\b/i.test(type)
                    ? "SELL"
                    : /^(?:Security\s+)?Transfer\s+In\b/i.test(type)
                      ? "TRANSFER_IN"
                      : /^(?:Security\s+)?Transfer\s+Out\b/i.test(type)
                        ? "TRANSFER_OUT"
                        : "UNSUPPORTED";
          if (/\b(?:split|reinvest|return of capital|basis adjustment)\b/i.test(type))
            action = "UNSUPPORTED";
          if (section === "pending") action = "UNSUPPORTED";
          if (
            ["BUY", "SELL", "TRANSFER_IN", "TRANSFER_OUT"].includes(action) &&
            (qty === null || new Money(qty).isZero())
          )
            throw new BasisError("number");
          const net = numeric(columns.net),
            debit = numeric(columns.debit),
            credit = numeric(columns.credit);
          const cash =
            columns.net !== undefined
              ? net
              : cashMovement(row.cells, columns.debit, columns.credit);
          const activity: StatementActivity = {
            id: `activity:${page}:${line}`,
            page,
            line,
            section,
            description,
            date,
            dateKind: date ? columns.dateKind : "unknown",
            action,
            quantity: qty === null ? null : new Money(qty).abs().toFixed(),
            cash,
            fee: numeric(columns.fee),
            transferValue: numeric(columns.value),
          };
          evidence.activities.push(activity);
          pending = activity;
          if (section === "cash") {
            // An empty debit/credit column means no movement on that side;
            // an explicit unavailable amount remains unknown for totals.
            const absent = (anchor?: number) => ["", "--", "-"].includes(at(row.cells, anchor));
            add(
              "debit",
              debit !== null
                ? new Money(debit).abs().toFixed()
                : absent(columns.debit)
                  ? "0"
                  : null,
            );
            add("credit", credit ?? (absent(columns.credit) ? "0" : null));
          }
          add("cash", cash);
          add("value", activity.transferValue);
        }
        evidence.sections[section] = (evidence.sections[section] ?? 0) + 1;
        if (evidence.activities.length + evidence.dispositions.length > 10000)
          throw new BasisError("size");
      } catch (error) {
        throw new BasisError(
          error instanceof BasisError ? error.code : "brokerageRecord",
          line,
          page,
        );
      }
    }
    describe();
    if (needsHeader || !sawHeader) throw new BasisError("brokerageFormat", undefined, pageNumber);
  }
  return evidence;
}
