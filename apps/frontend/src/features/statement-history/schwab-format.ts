import Decimal from "decimal.js";
import { BasisError, basisDate, pdfRows, type PdfTextItem } from "./statement-pdf";
import { MERRILL_MONTHS as MONTHS } from "./merrill-format";

const Money = Decimal.clone({ precision: 40 });
const SAME_MONTH_PERIOD = /^([A-Za-z]+)\s+(\d{1,2})\s*[-–—]\s*(\d{1,2}),\s+(\d{4})$/;
const CROSS_MONTH_PERIOD =
  /^([A-Za-z]+)\s+(\d{1,2})(?:,\s*(\d{4}))?\s*[-–—]\s*([A-Za-z]+)\s+(\d{1,2}),\s+(\d{4})$/;
const ACCOUNT_NUMBER = /^[A-Z0-9]{3,}(?:-[A-Z0-9]{2,})+$/i;

/** US statement numbers, including ($110,000.00), (40.0000), and N/A. Missing is never zero. */
export function schwabNumber(raw: string): string | null {
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

export function schwabPeriod(text: string): { from: string; through: string } | undefined {
  const trimmed = text.trim();
  const toDate = (month: string, day: string, year: string) => {
    const index = MONTHS.findIndex((m) => m.toLowerCase() === month.toLowerCase());
    if (index < 0) throw new BasisError("date");
    return basisDate(`${year}-${String(index + 1).padStart(2, "0")}-${day.padStart(2, "0")}`);
  };
  const same = SAME_MONTH_PERIOD.exec(trimmed);
  if (same) {
    const from = toDate(same[1], same[2], same[4]);
    const through = toDate(same[1], same[3], same[4]);
    if (from > through) throw new BasisError("date");
    return { from, through };
  }
  const cross = CROSS_MONTH_PERIOD.exec(trimmed);
  if (cross) {
    const from = toDate(cross[1], cross[2], cross[3] ?? cross[6]);
    const through = toDate(cross[4], cross[5], cross[6]);
    if (from > through) throw new BasisError("date");
    return { from, through };
  }
  return undefined;
}

export function isSchwabStatement(pages: PdfTextItem[][]): boolean {
  return pages.some((page) =>
    page.some((c) =>
      /\bSchwab\s+One\b|schwab\.com\/stmt|\bCharles\s+Schwab\s+&\s+Co\.|\bSchwab\s+Institutional\b/i.test(
        c.str,
      ),
    ),
  );
}

/** Transient identity only. Callers must not persist or log the account identifier. */
export function schwabIdentity(pages: PdfTextItem[][]) {
  if (!isSchwabStatement(pages)) throw new BasisError("brokerageFormat");
  const identities = new Set<string>();
  const ranges = new Map<string, { from: string; through: string }>();

  for (const page of pages) {
    const joined = page.map((c) => c.str).join(" ");
    for (const match of joined.matchAll(/Account\s+Number:\s*([A-Z0-9]+(?:-[A-Z0-9]+)+)/gi)) {
      identities.add(match[1].replaceAll("-", "").toUpperCase());
    }
    const rows = pdfRows(page);
    for (let i = 0; i < rows.length; i++) {
      const row = rows[i];
      const acctHeader = row.cells.find((c) => c.str.trim() === "Account Number");
      if (acctHeader) {
        for (let j = i + 1; j < Math.min(rows.length, i + 4); j++) {
          if (row.y - rows[j].y > 25) break;
          const value = rows[j].cells.find(
            (c) =>
              Math.abs(c.transform[4] - acctHeader.transform[4]) < 15 &&
              ACCOUNT_NUMBER.test(c.str.trim()),
          );
          if (value) {
            identities.add(value.str.trim().replaceAll("-", "").toUpperCase());
            break;
          }
        }
      }
      const periodHeader = row.cells.find((c) => c.str.trim() === "Statement Period");
      if (periodHeader) {
        for (let j = i + 1; j < Math.min(rows.length, i + 4); j++) {
          if (row.y - rows[j].y > 25) break;
          const candidate = rows[j].cells.find(
            (c) => Math.abs(c.transform[4] - periodHeader.transform[4]) < 15,
          );
          if (candidate) {
            const period = schwabPeriod(candidate.str);
            if (period) {
              ranges.set(`${period.from}/${period.through}`, period);
              break;
            }
          }
        }
      }
    }
    for (const c of page) {
      const period = schwabPeriod(c.str);
      if (period) ranges.set(`${period.from}/${period.through}`, period);
    }
  }

  if (identities.size !== 1 || ranges.size !== 1) throw new BasisError("brokerageAccount");
  return { identity: [...identities][0], ...ranges.values().next().value! };
}
