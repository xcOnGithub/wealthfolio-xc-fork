import Decimal from "decimal.js";

import { BasisError, basisDate } from "./opening-basis";
import { pdfRows, type PdfTextItem } from "./basis-pdf";

const Money = Decimal.clone({ precision: 40 });

type Section = "equities" | "funds" | "cash";
interface Position {
  security: string;
  quantity: string;
  cost: string;
  page: number;
  line: number;
}
const months = [
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
const range = /^([A-Za-z]+)\s+(\d{1,2}),\s+(\d{4})\s*-\s*([A-Za-z]+)\s+(\d{1,2}),\s+(\d{4})$/;
function date(month: string, day: string, year: string): string {
  const index = months.indexOf(month);
  if (index < 0) throw new BasisError("brokerageFormat");
  return basisDate(`${year}-${String(index + 1).padStart(2, "0")}-${day.padStart(2, "0")}`);
}
function money(value: string): Decimal {
  if (!/^(?:\d+|\d{1,3}(?:,\d{3})+)(?:\.\d+)?$/.test(value.trim()))
    throw new BasisError("brokerageBasis");
  const amount = new Money(value.trim().replaceAll(",", ""));
  if (!amount.isFinite() || amount.gt("1e20") || amount.decimalPlaces() > 18)
    throw new BasisError("brokerageBasis");
  return amount;
}
function ticker(value: string): string {
  if (!/^[A-Z]{1,6}(?:[.-][A-Z])?$/.test(value)) throw new BasisError("security");
  return value;
}

/** CMA Assets v1: header-anchored columns, explicit symbols and section-total reconciliation. */
export function extractMerrillStatement(
  pages: PdfTextItem[][],
  onAccount?: (identity: string) => void,
): string {
  if (!pages.some((page) => page.some((item) => /\bMerrill\b/i.test(item.str))))
    throw new BasisError("brokerageFormat");
  const positions: Position[] = [];
  const accounts = new Set<string>();
  const periods = new Set<string>();
  let section: Section | undefined;
  let columns: { quantity: number; basis: number; symbol?: number } | undefined;
  let pending: Position | undefined;
  let sectionCost = new Money(0);
  let open = false;
  let sections = 0;
  let announced = false;
  for (const [pageIndex, page] of pages.entries()) {
    const assets = page.find((item) => /\bCMA\b.*\bASSETS\b/i.test(item.str));
    if (!assets) continue;
    const period = page.find(
      (item) =>
        Math.abs(item.transform[5] - assets.transform[5]) < 4 && range.test(item.str.trim()),
    );
    const match = period ? range.exec(period.str.trim()) : undefined;
    if (!match || date(match[1], match[2], match[3]) > date(match[4], match[5], match[6]))
      throw new BasisError("brokerageFormat");
    periods.add(date(match[4], match[5], match[6]));
    const account = /Account\s+Number:\s*([A-Z0-9]+(?:-[A-Z0-9]+)+)/i.exec(
      page.map((item) => item.str).join(" "),
    );
    if (!account) throw new BasisError("brokerageAccount");
    accounts.add(account[1]);
    if (accounts.size !== 1 || periods.size !== 1) throw new BasisError("brokerageAccount");
    columns = undefined; // Continuation pages must establish their own column geometry.
    const rows = pdfRows(page);
    for (const [lineIndex, row] of rows.entries()) {
      const cells = row.cells.filter((item) => item.str.trim());
      const text = cells.map((item) => item.str.trim()).join(" ");
      const heading = cells.find((item) =>
        /^(?:EQUITIES|MUTUAL FUNDS\/CLOSED END FUNDS\/UITs\/ETPs|CASH\/MONEY ACCOUNTS)$/.test(
          item.str.trim(),
        ),
      );
      if (heading) {
        if (announced) throw new BasisError("brokerageFormat");
        const next: Section =
          heading.str.trim() === "EQUITIES"
            ? "equities"
            : heading.str.trim() === "CASH/MONEY ACCOUNTS"
              ? "cash"
              : "funds";
        if (open && section !== next) throw new BasisError("brokerageReconcile");
        section = next;
        columns = undefined;
        announced = true;
      }
      const find = (label: string) => cells.find((item) => item.str.trim() === label);
      const description = find("Description"),
        quantity = find("Quantity"),
        basis = find("Cost Basis");
      if (description && quantity && basis && find("Market Price") && find("Market Value")) {
        if (!section || !announced) throw new BasisError("brokerageFormat");
        const totalBasisHeader = rows[lineIndex - 1]?.cells.some(
          (item) =>
            item.str.trim() === "Total" &&
            Math.abs(item.transform[4] + item.width - basis.transform[4] - basis.width) < 3,
        );
        if (!totalBasisHeader) throw new BasisError("brokerageFormat");
        announced = false;
        columns = {
          quantity: quantity.transform[4] + quantity.width,
          basis: basis.transform[4] + basis.width,
          symbol: find("Symbol")?.transform[4],
        };
        if (section !== "cash" && !open) {
          open = true;
          sectionCost = new Money(0);
        }
        continue;
      }
      if (!columns || section === "cash" || !open) continue;
      // Values are right aligned below their labeled headers. Never select market value as basis.
      const at = (right: number) =>
        cells.filter((item) => Math.abs(item.transform[4] + item.width - right) < 3);
      const qty = at(columns.quantity),
        cost = at(columns.basis);
      const total = cells.some((item) => item.str.trim() === "TOTAL");
      if (total && !qty.length) {
        if (pending || cost.length !== 1 || sectionCost.minus(money(cost[0].str)).abs().gt("0.01"))
          throw new BasisError("brokerageReconcile");
        open = false;
        sections++;
        columns = undefined;
        continue;
      }
      if (/\bSubtotal\b/.test(text) && !qty.length && !cost.length) continue;
      const symbolMatch = /\bSYMBOL:\s*([A-Z][A-Z0-9.-]*)\b/.exec(text);
      if (symbolMatch && pending) {
        pending.security = ticker(symbolMatch[1]);
        pending = undefined;
        continue;
      }
      if (!qty.length) {
        // Continuation descriptions and footnotes may not introduce uncounted basis.
        if (
          cost.length ||
          cells.some(
            (item) =>
              item.transform[4] > columns!.quantity - 50 && /^[\d,.]+$/.test(item.str.trim()),
          )
        )
          throw new BasisError("brokerageFormat");
        continue;
      }
      if (pending || qty.length !== 1 || cost.length !== 1) throw new BasisError("brokerageBasis");
      const q = money(qty[0].str),
        b = money(cost[0].str);
      if (q.lte(0)) throw new BasisError("brokerageBasis");
      const position: Position = {
        security: "",
        quantity: q.toFixed(),
        cost: b.toFixed(),
        page: pageIndex + 1,
        line: lineIndex + 1,
      };
      if (columns.symbol !== undefined) {
        const symbols = cells.filter(
          (item) =>
            item.transform[4] >= columns!.symbol! - 2 && item.transform[4] < columns!.symbol! + 65,
        );
        if (symbols.length !== 1) throw new BasisError("security");
        position.security = ticker(symbols[0].str.trim());
      } else pending = position;
      positions.push(position);
      sectionCost = sectionCost.plus(b);
    }
    if (announced) throw new BasisError("brokerageFormat");
  }
  if (open || pending || !sections || !positions.length || positions.some((p) => !p.security))
    throw new BasisError("brokerageReconcile");
  if (new Set(positions.map((p) => p.security)).size !== positions.length)
    throw new BasisError("duplicate");
  onAccount?.([...accounts][0].replace(/[^A-Z0-9]/gi, "").toUpperCase());
  // Only normalized positions leave the extractor; account names/numbers are not copied.
  return [
    "Security,Quantity,Total cost basis,Date,Currency,Source page,Source line",
    ...positions.map((p) =>
      [p.security, p.quantity, p.cost, [...periods][0], "USD", p.page, p.line].join(","),
    ),
  ].join("\n");
}
