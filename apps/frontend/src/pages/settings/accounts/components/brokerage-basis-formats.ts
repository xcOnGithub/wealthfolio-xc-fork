import { BasisError, suggestBasisColumns, type BasisColumns } from "./opening-basis";
import type { PdfTextItem, PdfBasisReview } from "./basis-pdf";
import {
  normalizeMerrillActivity,
  type BasisTable,
  type BasisSourceOptions,
} from "./merrill-basis-csv";
import { extractMerrillStatementV2 } from "./merrill-statement-v2";
import { extractMerrillStatement } from "./merrill-basis-pdf";

interface BrokerageBasisFormat {
  id: string;
  brokerage: string;
  kind: "pdf" | "csv";
  mode: "positions" | "transactions";
  extractPdf?: (
    pages: PdfTextItem[][],
    onAccount?: (identity: string) => void,
    review?: PdfBasisReview,
  ) => string;
  transferOnly?: boolean;
  extractStatement?: typeof extractMerrillStatementV2;
  normalize: (
    table: BasisTable,
    currency: string,
    options?: BasisSourceOptions,
  ) => BasisTable & {
    columns: BasisColumns;
    sourceRows?: number[];
    cashRows?: number[];
    outsideRows?: number[];
  };
}

// Add a separately tested version when an institution changes its export layout.
export const BROKERAGE_BASIS_FORMATS: BrokerageBasisFormat[] = [
  {
    id: "merrill-settled-activity-v1",
    brokerage: "merrill_edge",
    kind: "csv",
    mode: "transactions",
    normalize: normalizeMerrillActivity,
  },
  {
    id: "merrill-cma-statement-v1",
    brokerage: "merrill_edge",
    kind: "pdf",
    mode: "positions",
    extractPdf: extractMerrillStatement,
    normalize: normalizePositions,
  },
  {
    id: "merrill-cma-statement-v2",
    brokerage: "merrill_edge",
    kind: "pdf",
    mode: "positions",
    transferOnly: true,
    extractStatement: extractMerrillStatementV2,
    normalize: normalizePositions,
  },
];
function normalizePositions(table: BasisTable, currency: string) {
  if (currency !== "USD") throw new BasisError("currency");
  const headers = [
    "Security",
    "Quantity",
    "Total cost basis",
    "Date",
    "Currency",
    "Source page",
    "Source line",
  ];
  if (
    table.headers.length !== headers.length ||
    table.headers.some((header, i) => header !== headers[i]) ||
    !table.rows.length
  )
    throw new BasisError("brokerageFormat");
  return { ...table, columns: suggestBasisColumns(table.headers) };
}
