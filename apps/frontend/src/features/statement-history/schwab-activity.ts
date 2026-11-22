import Decimal from "decimal.js";
import { BasisError, basisDate } from "@/pages/settings/accounts/components/opening-basis";
import { pdfRows, type PdfTextItem } from "@/pages/settings/accounts/components/basis-pdf";
import type { StatementActivity, StatementEvidence, StatementSection } from "./merrill-activity";
import { schwabIdentity, schwabNumber } from "./schwab-format";

const Money = Decimal.clone({ precision: 40 });
const right = (item: PdfTextItem) => item.transform[4] + item.width;

const STOP_TRANSACTION_SECTION =
  /^(?:Total Transactions|Bank Sweep Activity|Cost Basis Lot Details|Pending\s*\/\s*Open Activity|Endnotes For Your Account|Terms and Conditions)$/i;

function schwabRowDate(raw: string, from: string, through: string): string {
  const m = /^(\d{1,2})\/(\d{1,2})(?:\/(\d{2}|\d{4}))?$/.exec(raw.trim());
  if (!m) throw new BasisError("date");
  const years = m[3]
    ? [m[3].length === 4 ? m[3] : `${through.slice(0, 2)}${m[3]}`]
    : [...new Set([from.slice(0, 4), through.slice(0, 4)])];
  const dates = years.map((year) =>
    basisDate(`${year}-${m[1].padStart(2, "0")}-${m[2].padStart(2, "0")}`),
  );
  if (m[3]) {
    if (dates[0] < from || dates[0] > through) throw new BasisError("cutoff");
    return dates[0];
  }
  const inPeriod = dates.filter((date) => date >= from && date <= through);
  if (inPeriod.length !== 1) throw new BasisError("date");
  return inPeriod[0];
}

interface TransactionColumns {
  dateX: number;
  categoryX: number;
  actionX: number;
  symbolX: number;
  descX: number;
  qtyRight: number;
  feeRight?: number;
  amountRight: number;
}

function classifySchwabRow(
  category: string,
  action: string,
  quantity: string | null,
  amount: string | null,
): {
  section: StatementSection;
  action: StatementActivity["action"];
  quantity: string | null;
  cash: string | null;
  transferValue: string | null;
} {
  const cat = category.toLowerCase();
  const act = action.toLowerCase();

  if (cat === "purchase") {
    if (quantity === null || amount === null) {
      return {
        section: "securities",
        action: "UNSUPPORTED",
        quantity,
        cash: amount,
        transferValue: null,
      };
    }
    const q = new Money(quantity);
    const c = new Money(amount);
    if (q.lte(0) || c.gt(0)) {
      return {
        section: "securities",
        action: "UNSUPPORTED",
        quantity,
        cash: amount,
        transferValue: null,
      };
    }
    return {
      section: "securities",
      action: "BUY",
      quantity: q.toFixed(),
      cash: c.toFixed(),
      transferValue: null,
    };
  }

  if (cat === "sale") {
    if (quantity === null || amount === null) {
      return {
        section: "securities",
        action: "UNSUPPORTED",
        quantity,
        cash: amount,
        transferValue: null,
      };
    }
    const q = new Money(quantity);
    const c = new Money(amount);
    if (q.eq(0) || c.lt(0)) {
      return {
        section: "securities",
        action: "UNSUPPORTED",
        quantity,
        cash: amount,
        transferValue: null,
      };
    }
    return {
      section: "securities",
      action: "SELL",
      quantity: q.abs().toFixed(),
      cash: c.toFixed(),
      transferValue: null,
    };
  }

  if (cat === "dividend") {
    if (quantity !== null && !new Money(quantity).eq(0)) {
      if (act.includes("reinvest") && amount !== null && new Money(amount).lte(0)) {
        const q = new Money(quantity);
        if (q.gt(0)) {
          return {
            section: "securities",
            action: "BUY",
            quantity: q.toFixed(),
            cash: new Money(amount).toFixed(),
            transferValue: null,
          };
        }
      }
      return {
        section: "securities",
        action: "UNSUPPORTED",
        quantity,
        cash: amount,
        transferValue: null,
      };
    }
    return {
      section: "income",
      action: "INCOME",
      quantity: null,
      cash: amount,
      transferValue: null,
    };
  }

  if (cat === "interest") {
    if (quantity !== null && !new Money(quantity).eq(0)) {
      return {
        section: "income",
        action: "UNSUPPORTED",
        quantity,
        cash: amount,
        transferValue: null,
      };
    }
    return {
      section: "income",
      action: "INCOME",
      quantity: null,
      cash: amount,
      transferValue: null,
    };
  }

  if (["deposit", "withdrawal", "expense", "fees", "tax", "journal", "moneylink"].includes(cat)) {
    if (quantity !== null && !new Money(quantity).eq(0)) {
      return {
        section: "cash",
        action: "UNSUPPORTED",
        quantity,
        cash: amount,
        transferValue: null,
      };
    }
    return { section: "cash", action: "CASH", quantity: null, cash: amount, transferValue: null };
  }

  if (cat === "other" || cat === "other activity") {
    if (
      quantity !== null &&
      !new Money(quantity).eq(0) &&
      /^(?:account\s+transfer|transfer|journaled\s+shares)$/i.test(act.trim())
    ) {
      const q = new Money(quantity);
      return {
        section: "transfers",
        action: q.gt(0) ? "TRANSFER_IN" : "TRANSFER_OUT",
        quantity: q.abs().toFixed(),
        cash: null,
        transferValue: amount,
      };
    }
    return {
      section: "transfers",
      action: "UNSUPPORTED",
      quantity,
      cash: amount,
      transferValue: null,
    };
  }

  return { section: "cash", action: "UNSUPPORTED", quantity, cash: amount, transferValue: null };
}

export interface SchwabActivityExtraction {
  evidence: StatementEvidence;
  /** True only when every Transaction Details block header was parsed, or the statement explicitly reports no activity. */
  activitySourceParsed: boolean;
}

/** Extract settlement-date transaction evidence and completeness proof from Charles Schwab monthly statements. */
export function extractSchwabActivityDetailed(pages: PdfTextItem[][]): SchwabActivityExtraction {
  const { from, through } = schwabIdentity(pages);
  const sections: Partial<Record<StatementSection, number>> = {};
  const activities: StatementActivity[] = [];
  let inTransactions = false;
  let blockHeaderParsed = false;
  let parsedTransactionTable = false;
  let unparsedTransactionTable = false;
  let explicitNoActivity = false;
  let currentDate: string | null = null;
  let seq = 0;

  const closeTransactionBlock = () => {
    if (inTransactions && !blockHeaderParsed) {
      unparsedTransactionTable = true;
    }
    inTransactions = false;
    blockHeaderParsed = false;
  };

  for (const [pageIndex, page] of pages.entries()) {
    const rows = pdfRows(page);
    let columns: TransactionColumns | undefined;

    for (let lineIndex = 0; lineIndex < rows.length; lineIndex++) {
      const row = rows[lineIndex];

      if (
        row.cells.some((c) =>
          /\bno\s+(?:transactions?|activity)\s+(?:during|for|in)\s+this\s+(?:statement\s+)?period\b/i.test(
            c.str.trim(),
          ),
        )
      ) {
        explicitNoActivity = true;
      }

      if (row.cells.some((c) => /^Bank Sweep Activity$/i.test(c.str.trim()))) {
        sections.sweep = (sections.sweep ?? 0) + 1;
        closeTransactionBlock();
        columns = undefined;
        continue;
      }

      if (row.cells.some((c) => /^Transaction Details(?:\s*\(continued\))?$/i.test(c.str.trim()))) {
        closeTransactionBlock();
        inTransactions = true;
        blockHeaderParsed = false;
        columns = undefined;
        continue;
      }

      if (row.cells.some((c) => STOP_TRANSACTION_SECTION.test(c.str.trim()))) {
        closeTransactionBlock();
        columns = undefined;
        continue;
      }

      if (!inTransactions) continue;

      const find = (re: RegExp) => row.cells.find((c) => re.test(c.str.trim()));
      const dateH = find(/^Date$/i);
      const catH = find(/^Category$/i);
      const actH = find(/^Action$/i);
      const symH = find(/^(?:Symbol\/)?CUSIP$/i) ?? find(/^Symbol$/i);
      const descH = find(/^Description$/i);
      const qtyH = find(/^Quantity$/i);
      const amtH = find(/^Amount\(\$\)$/i);
      if (dateH && catH && actH && descH && qtyH && amtH) {
        const feeH = find(/^Interest\(\$\)$/i) ?? find(/^Charges\/Interest\(\$\)$/i);
        blockHeaderParsed = true;
        parsedTransactionTable = true;
        columns = {
          dateX: dateH.transform[4],
          categoryX: catH.transform[4],
          actionX: actH.transform[4],
          symbolX: symH?.transform[4] ?? descH.transform[4] - 70,
          descX: descH.transform[4],
          qtyRight: right(qtyH),
          feeRight: feeH ? right(feeH) : undefined,
          amountRight: right(amtH),
        };
        continue;
      }

      if (!columns) continue;

      // Parse Date and Category (which pdfjs may merge into a single cell like "10/02 Sale").
      const leftCells = row.cells
        .filter(
          (c) => c.transform[4] >= columns!.dateX - 5 && c.transform[4] < columns!.actionX - 5,
        )
        .sort((a, b) => a.transform[4] - b.transform[4]);

      let dateToken: string | undefined;
      let categoryText = "";

      for (const cell of leftCells) {
        const text = cell.str.trim();
        if (!text) continue;
        const merged = /^(\d{1,2}\/\d{1,2}(?:\/\d{2,4})?)\s+(.+)$/.exec(text);
        if (merged && !dateToken) {
          dateToken = merged[1];
          categoryText = [categoryText, merged[2]].filter(Boolean).join(" ");
        } else if (/^\d{1,2}\/\d{1,2}(?:\/\d{2,4})?$/.test(text) && !dateToken) {
          dateToken = text;
        } else {
          categoryText = [categoryText, text].filter(Boolean).join(" ");
        }
      }

      if (dateToken) {
        currentDate = schwabRowDate(dateToken, from, through);
      }

      // Skip continuation lines (e.g. "Activity" under "Other", fee notes, wrapped descriptions).
      if (!categoryText || /^Activity$/i.test(categoryText)) continue;

      const inBand = (minX: number, maxX: number) =>
        row.cells
          .filter((c) => c.transform[4] >= minX && c.transform[4] < maxX && c.str.trim())
          .sort((a, b) => a.transform[4] - b.transform[4])
          .map((c) => c.str.trim())
          .join(" ");

      const actionText = inBand(columns.actionX - 5, columns.symbolX - 5);
      const symbolText = inBand(columns.symbolX - 5, columns.descX - 5);
      const descText = inBand(columns.descX - 5, columns.qtyRight - 55);

      const atRight = (target: number, tol = 10) => {
        const matches = row.cells.filter((c) => {
          const s = c.str.trim();
          if (!s || /^[a-z](?:,[a-z])*$/i.test(s) || s === ",") return false;
          return Math.abs(right(c) - target) <= tol;
        });
        if (matches.length > 1)
          throw new BasisError("brokerageFormat", lineIndex + 1, pageIndex + 1);
        return matches[0]?.str.trim();
      };

      const qtyRaw = atRight(columns.qtyRight, 8);
      const feeRaw = columns.feeRight !== undefined ? atRight(columns.feeRight, 8) : undefined;
      const amtRaw = atRight(columns.amountRight, 10);

      const qty = qtyRaw ? schwabNumber(qtyRaw) : null;
      const fee = feeRaw ? schwabNumber(feeRaw) : null;
      const amt = amtRaw ? schwabNumber(amtRaw) : null;

      const classified = classifySchwabRow(categoryText, actionText, qty, amt);
      sections[classified.section] = (sections[classified.section] ?? 0) + 1;

      const security = [symbolText, descText].filter(Boolean).join(" ").trim() || undefined;
      const description = [categoryText, actionText, security].filter(Boolean).join(" · ");

      activities.push({
        id: `schwab-${++seq}`,
        page: pageIndex + 1,
        line: lineIndex + 1,
        section: classified.section,
        description,
        security,
        date: currentDate,
        dateKind: currentDate ? "settlement" : "unknown",
        action: classified.action,
        quantity: classified.quantity,
        cash: classified.cash,
        fee,
        transferValue: classified.transferValue,
      });
    }
    closeTransactionBlock();
  }

  return {
    evidence: {
      from,
      through,
      sections,
      activities,
      dispositions: [],
    },
    activitySourceParsed:
      !unparsedTransactionTable && (parsedTransactionTable || explicitNoActivity),
  };
}

/** Extract settlement-date transaction evidence from Charles Schwab monthly statements. */
export function extractSchwabActivity(pages: PdfTextItem[][]): StatementEvidence {
  return extractSchwabActivityDetailed(pages).evidence;
}
