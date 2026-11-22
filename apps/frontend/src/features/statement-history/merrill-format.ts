import Decimal from "decimal.js";
import type { PdfTextItem } from "@/pages/settings/accounts/components/basis-pdf";
import { BasisError, basisDate } from "@/pages/settings/accounts/components/opening-basis";

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

/** Transient identity only. Callers must not persist or log the account identifier. */
export function merrillIdentity(pages: PdfTextItem[][]) {
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
export function normalizeSecurityLabel(label: string): string {
  return label.normalize("NFC").trim().replace(/\s+/g, " ").toLowerCase();
}
