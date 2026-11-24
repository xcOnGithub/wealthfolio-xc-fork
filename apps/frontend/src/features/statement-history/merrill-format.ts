import Decimal from "decimal.js";
import { BasisError, basisDate, type PdfTextItem } from "./statement-pdf";

const Money = Decimal.clone({ precision: 40 });
export const MERRILL_MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];
export const MERRILL_PERIOD =
  /^([A-Za-z]+)\s+(\d{1,2}),\s+(\d{4})\s*-\s*([A-Za-z]+)\s+(\d{1,2}),\s+(\d{4})$/;

/** US statement numbers, including .0001 and (.02). Missing is never zero. */
export function merrillNumber(raw: string): string | null {
  const value = raw.trim();
  if (!value || /^(?:--|-|N\/A|NA)$/i.test(value)) return null;
  if (
    value.length > 128 ||
    value.startsWith("(") !== value.endsWith(")") ||
    !/^\(?(?:-?\$?|\$-?)(?:(?:\d+|\d{1,3}(?:,\d{3})+)(?:\.\d+)?|\.\d+)\)?$/.test(value) ||
    (value.startsWith("(") && value.includes("-"))
  )
    throw new BasisError("number");
  const n = new Money(value.replace(/[$,()]/g, ""));
  if (!n.isFinite() || n.abs().gt("1e20") || n.decimalPlaces() > 18) throw new BasisError("number");
  return (value.startsWith("(") ? n.neg() : n).toFixed();
}

export function merrillPeriod(text: string): { from: string; through: string } | undefined {
  const match = MERRILL_PERIOD.exec(text.trim());
  if (!match) return;
  const date = (month: string, day: string, year: string) => {
    const index = MERRILL_MONTHS.indexOf(month);
    if (index < 0) throw new BasisError("date");
    return basisDate(`${year}-${String(index + 1).padStart(2, "0")}-${day.padStart(2, "0")}`);
  };
  const from = date(match[1], match[2], match[3]),
    through = date(match[4], match[5], match[6]);
  if (from > through) throw new BasisError("date");
  return { from, through };
}

export const MAX_STATEMENT_BATCH = 60;
export const MAX_STATEMENT_PAGES = 1000;

export interface MerrillStatementSource {
  identity: string;
  from: string;
  through: string;
}

export interface MerrillStatementPages extends MerrillStatementSource {
  pages: PdfTextItem[][];
  pageNumbers: number[];
}

/** Transient identity only. Callers must not persist or log the account identifier. */
export function merrillIdentity(pages: PdfTextItem[][]): MerrillStatementSource {
  if (!pages.some((page) => page.some((c) => /\bMerrill\b/i.test(c.str))))
    throw new BasisError("brokerageFormat");
  const identities = new Set<string>(),
    ranges = new Map<string, { from: string; through: string }>();
  for (const page of pages) {
    for (const match of page
      .map((c) => c.str)
      .join(" ")
      .matchAll(/Account\s+Number:\s*([A-Z0-9]+(?:-[A-Z0-9]+)+)/gi))
      identities.add(match[1].replaceAll("-", "").toUpperCase());
    for (const c of page) {
      const period = merrillPeriod(c.str);
      if (period) ranges.set(`${period.from}/${period.through}`, period);
    }
  }
  if (identities.size !== 1 || ranges.size !== 1) throw new BasisError("brokerageAccount");
  return { identity: [...identities][0], ...ranges.values().next().value! };
}

/** Split a single-month or bundled multi-month Merrill CMA PDF into contiguous period segments. */
export function splitMerrillStatements(pages: PdfTextItem[][]): MerrillStatementPages[] {
  if (!pages.some((page) => page.some((c) => /\bMerrill\b/i.test(c.str))))
    throw new BasisError("brokerageFormat");
  const identities = new Set<string>();
  for (const page of pages) {
    for (const match of page
      .map((c) => c.str)
      .join(" ")
      .matchAll(/Account\s+Number:\s*([A-Z0-9]+(?:-[A-Z0-9]+)+)/gi))
      identities.add(match[1].replaceAll("-", "").toUpperCase());
  }
  if (identities.size !== 1) throw new BasisError("brokerageAccount");
  const identity = [...identities][0];

  const segments: (MerrillStatementPages & { key: string })[] = [];
  const seenKeys = new Set<string>();
  const leadingUndated: { page: PdfTextItem[]; pageNumber: number }[] = [];

  for (const [index, page] of pages.entries()) {
    const pageNumber = index + 1;
    const pageRanges = new Map<string, { from: string; through: string }>();
    for (const c of page) {
      const period = merrillPeriod(c.str);
      if (period) pageRanges.set(`${period.from}/${period.through}`, period);
    }
    if (pageRanges.size > 1) throw new BasisError("brokerageAccount", undefined, pageNumber);
    const entry = pageRanges.entries().next().value as
      | [string, { from: string; through: string }]
      | undefined;
    if (!entry) {
      const current = segments.at(-1);
      if (current) {
        current.pages.push(page);
        current.pageNumbers.push(pageNumber);
      } else {
        leadingUndated.push({ page, pageNumber });
      }
      continue;
    }
    const [key, period] = entry;
    const current = segments.at(-1);
    if (current && current.key === key) {
      current.pages.push(page);
      current.pageNumbers.push(pageNumber);
      continue;
    }
    if (seenKeys.has(key)) throw new BasisError("brokerageAccount", undefined, pageNumber);
    seenKeys.add(key);
    segments.push({
      key,
      identity,
      from: period.from,
      through: period.through,
      pages: [...leadingUndated.map((u) => u.page), page],
      pageNumbers: [...leadingUndated.map((u) => u.pageNumber), pageNumber],
    });
    leadingUndated.length = 0;
  }

  if (!segments.length) throw new BasisError("brokerageAccount");
  const throughDates = new Set(segments.map((s) => s.through));
  if (throughDates.size !== segments.length) throw new BasisError("brokerageAccount");

  return segments
    .sort((a, b) => a.through.localeCompare(b.through))
    .map(({ key: _key, ...segment }) => segment);
}

export function normalizeSecurityLabel(label: string): string {
  return label.normalize("NFC").trim().replace(/\s+/g, " ").toLowerCase();
}

export const MERRILL_FOOTER_MAX_Y = 100;

const PRECEDING_PROSE_CUE =
  /\b(?:see|refer|check|view|read|consult|review|continued|shown|listed|pages|on|in|to|from|at|per|through|for|with|by|of|and|or|series|class|tranche|part|tier|distribution|installment|payment|dividend|transfer|lot|share|shares|unit|units)[\s:,;-]+$/i;
const FOLLOWING_PROSE_CUE =
  /^\s+(?:for|in|to|of|on|at|by|from|with|and|or|is|are|was|were|has|have|shows?|lists?|contains?|describes?|includes?|details?|section|table|schedule|days?|months?|shares?|accounts?)\b/i;

/** Extract non-prose printed page markers (e.g. "1 of 10", "Page 4 of 5") from a footer-band row. */
export function merrillFooterRowMarkers(row: {
  y: number;
  cells: PdfTextItem[];
}): { current: number; total: number }[] {
  if (row.y > MERRILL_FOOTER_MAX_Y) return [];
  const sorted = [...row.cells].sort((a, b) => a.transform[4] - b.transform[4]);
  // Dated transaction rows and multi-decimal holding rows in the bottom band are data, not footers.
  if (
    sorted.some(
      (c) => c.transform[4] < 120 && /^\d{1,2}\/\d{1,2}(?:\/\d{2,4})?$/.test(c.str.trim()),
    ) ||
    sorted.filter((c) => /^\d{1,2}\/\d{1,2}\/\d{2,4}$/.test(c.str.trim())).length >= 2 ||
    sorted.filter((c) => /^\(?-?\$?(?:\d+|\d{1,3}(?:,\d{3})+)\.\d{2,}\)?$/.test(c.str.trim()))
      .length >= 2
  ) {
    return [];
  }
  const text = sorted.map((c) => c.str.trim()).join(" ");
  const found = new Map<string, { current: number; total: number }>();
  for (const match of text.matchAll(/\b(Page\s*:?\s*)?(\d+)\s+of\s+(\d+)\b/gi)) {
    const index = match.index ?? 0;
    const before = text.slice(0, index);
    const after = text.slice(index + match[0].length);
    if (
      PRECEDING_PROSE_CUE.test(before) ||
      FOLLOWING_PROSE_CUE.test(after) ||
      (!match[1] && /[a-z]\s+$/.test(before))
    ) {
      continue;
    }
    const current = Number(match[2]);
    const total = Number(match[3]);
    if (current >= 1 && total >= 1 && total <= MAX_STATEMENT_PAGES) {
      found.set(`${current}/${total}`, { current, total });
    }
  }
  return [...found.values()];
}
