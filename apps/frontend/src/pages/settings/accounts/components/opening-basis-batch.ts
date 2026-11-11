import { parseCsv } from "@/adapters";
import { createDefaultParseConfig } from "@/pages/activity/import/utils/default-activity-template";
import {
  MAX_BASIS_FILE_BYTES,
  MAX_BASIS_TEXT,
  readBasisFile,
  type PdfBasisExclusion,
} from "./basis-pdf";
import { BROKERAGE_BASIS_FORMATS } from "./brokerage-basis-formats";
import {
  BasisError,
  calculateBasis,
  openingDate,
  type BasisColumns,
  type BasisCheckpoint,
} from "./opening-basis";
import type { NormalizedBasisTable } from "./merrill-basis-csv";

export const MAX_BASIS_FILES = 5;
export const MAX_BASIS_BATCH_BYTES = 25 * 1024 * 1024;
export class BasisBatchError extends BasisError {
  constructor(
    code: string,
    public readonly fileIndex?: number,
    row?: number,
  ) {
    super(code, row);
  }
}
export interface BasisOrigin {
  file: number;
  name: string;
  row: number;
  page?: string;
  line?: string;
}
export interface PreparedBasisDocument {
  fileIndex: number;
  name: string;
  formatId: string;
  kind: "csv" | "pdf";
  excludedPositions?: PdfBasisExclusion[];
  table: Pick<NormalizedBasisTable, "headers" | "rows" | "columns"> & {
    sourceRows?: number[];
    cashRows?: number[];
  };
}
export interface BasisBatch {
  excludedPositions?: (PdfBasisExclusion & { file: number; name: string })[];
  headers: string[];
  rows: string[][];
  columns: BasisColumns;
  mode: "positions" | "transactions";
  asOf: string;
  hasStatement: boolean;
  checkpoint?: BasisCheckpoint;
  origins: Record<number, BasisOrigin>;
  checkpointOrigins: Record<number, BasisOrigin>;
  documents: {
    fileIndex: number;
    name: string;
    formatId: string;
    records: number;
    cash: number;
    role: "baseline" | "superseded" | "transactions" | "covered";
  }[];
}

export function validateBasisFiles(files: File[]): void {
  if (!files.length || files.length > MAX_BASIS_FILES) throw new BasisBatchError("batchCount");
  if (
    files.some((file) => !file.size || file.size > MAX_BASIS_FILE_BYTES) ||
    files.reduce((sum, file) => sum + file.size, 0) > MAX_BASIS_BATCH_BYTES
  )
    throw new BasisBatchError("batchSize");
}

function origins(document: PreparedBasisDocument): Record<number, BasisOrigin> {
  return Object.fromEntries(
    document.table.rows.map((row, index) => [
      index + 1,
      {
        file: document.fileIndex,
        name: document.name,
        row: document.table.sourceRows?.[index] ?? index + 1,
        page: row[document.table.headers.indexOf("Source page")],
        line: row[document.table.headers.indexOf("Source line")],
      },
    ]),
  );
}

/** Statements are checkpoints, never additive holdings. Choose the latest valid baseline. */
export function combineBasisDocuments(
  documents: PreparedBasisDocument[],
  currency: string,
  startDate: string,
): BasisBatch {
  if (!documents.length || documents.length > MAX_BASIS_FILES)
    throw new BasisBatchError("batchCount");
  const cutoff = openingDate(startDate);
  const statements = documents
    .filter((doc) => doc.kind === "pdf")
    .map((doc) => {
      const asOf = doc.table.rows[0]?.[doc.table.columns.date!];
      if (!asOf) throw new BasisBatchError("date", doc.fileIndex);
      try {
        const result = calculateBasis({
          ...doc.table,
          mode: "positions",
          asOf,
          startDate,
          currency,
          completeHistory: false,
        });
        return { doc, asOf, candidates: result.candidates };
      } catch (error) {
        throw new BasisBatchError(
          error instanceof BasisError ? error.code : "parse",
          doc.fileIndex,
        );
      }
    })
    .sort((a, b) => b.asOf.localeCompare(a.asOf) || a.doc.fileIndex - b.doc.fileIndex);
  for (const statement of statements) {
    const sameDate = statements.find((other) => other.asOf === statement.asOf);
    const signature = (item: typeof statement) =>
      JSON.stringify({
        positions: item.candidates
          .map((p) => [p.security, p.quantity, p.totalCost])
          .sort((a, b) => a[0].localeCompare(b[0])),
        exclusions: (item.doc.excludedPositions ?? [])
          .map((p) => [p.label, p.quantity])
          .sort((a, b) => a[0].localeCompare(b[0])),
      });
    if (sameDate && signature(sameDate) !== signature(statement))
      throw new BasisBatchError("batchConflict", statement.doc.fileIndex);
  }
  const excludedPositions = documents.flatMap((doc) =>
    (doc.excludedPositions ?? []).map((p) => ({ ...p, file: doc.fileIndex, name: doc.name })),
  );
  const baseline = statements[0];
  const transactions = documents.filter((doc) => doc.kind === "csv");
  const positionsOnly = Boolean(baseline && (!transactions.length || baseline.asOf === cutoff));
  const summaries: BasisBatch["documents"] = documents.map((doc) => ({
    fileIndex: doc.fileIndex,
    name: doc.name,
    formatId: doc.formatId,
    records: doc.table.rows.length,
    cash: doc.table.cashRows?.length ?? 0,
    role:
      doc.kind === "pdf"
        ? doc === baseline?.doc
          ? "baseline"
          : "superseded"
        : positionsOnly
          ? "covered"
          : "transactions",
  }));
  if (positionsOnly && baseline)
    return {
      ...baseline.doc.table,
      mode: "positions",
      asOf: baseline.asOf,
      hasStatement: true,
      origins: origins(baseline.doc),
      checkpointOrigins: {},
      documents: summaries,
      excludedPositions,
    };
  const headers = [
    "Security",
    "Quantity",
    "Total cost basis",
    "Date",
    "Action",
    "Currency",
    "Source file",
    "Source row",
  ];
  const columns: BasisColumns = {
    security: 0,
    quantity: 1,
    totalCost: 2,
    date: 3,
    action: 4,
    currency: 5,
  };
  const rows: string[][] = [];
  const sourceOrigins: Record<number, BasisOrigin> = {};
  for (const doc of transactions) {
    for (const [index, row] of doc.table.rows.entries()) {
      const fields = ["security", "quantity", "totalCost", "date", "action", "currency"] as const;
      if (fields.some((field) => doc.table.columns[field] === undefined))
        throw new BasisBatchError("brokerageFormat", doc.fileIndex);
      const sourceRow = doc.table.sourceRows?.[index] ?? index + 1;
      rows.push([
        ...fields.map((field) => row[doc.table.columns[field]!]),
        String(doc.fileIndex),
        String(sourceRow),
      ]);
      sourceOrigins[rows.length] = { file: doc.fileIndex, name: doc.name, row: sourceRow };
    }
  }
  if (!rows.length && !baseline) throw new BasisBatchError("noBasisTrades");
  return {
    headers,
    rows,
    columns,
    mode: "transactions",
    asOf: "",
    hasStatement: Boolean(baseline),
    checkpoint: baseline
      ? { asOf: baseline.asOf, currency, candidates: baseline.candidates }
      : undefined,
    origins: sourceOrigins,
    checkpointOrigins: baseline ? origins(baseline.doc) : {},
    documents: summaries,
    excludedPositions,
  };
}

/** Sequential reads bound PDF-worker memory. No partial batch is returned on any failure. */
export async function prepareBasisBatch(
  files: File[],
  selectedFormat: string,
  currency: string,
  startDate: string,
  admit: () => void = () => {},
  excludeUnavailableBasis = false,
): Promise<BasisBatch> {
  validateBasisFiles(files);
  const chosen = BROKERAGE_BASIS_FORMATS.find((format) => format.id === selectedFormat);
  if (!chosen) throw new BasisBatchError("batchFormat");
  const formats = files.map((file, index) => {
    const kind = /\.pdf$/i.test(file.name) ? "pdf" : /\.csv$/i.test(file.name) ? "csv" : undefined;
    const matches = BROKERAGE_BASIS_FORMATS.filter(
      (format) => format.brokerage === chosen.brokerage && format.kind === kind,
    );
    const format = kind === chosen.kind ? chosen : matches.length === 1 ? matches[0] : undefined;
    if (!format) throw new BasisBatchError("brokerageFormat", index + 1);
    return format;
  });
  const documents: PreparedBasisDocument[] = [];
  let textSize = 0,
    rowCount = 0;
  let account: string | undefined;
  for (const [index, file] of files.entries()) {
    admit();
    try {
      const format = formats[index];
      let identity: string | undefined;
      const identify = (value: string) => {
        identity = value;
      };
      const excludedPositions: PdfBasisExclusion[] = [];
      const text = await readBasisFile(
        file,
        format.extractPdf
          ? (pages) =>
              format.extractPdf!(pages, identify, {
                excludeUnavailableBasis,
                onExcluded: (p) => excludedPositions.push(p),
              })
          : undefined,
      );
      admit();
      textSize += text.length;
      if (textSize > MAX_BASIS_TEXT) throw new BasisError("batchSize");
      const parsed = await parseCsv(
        new File([text], `opening-basis-${index + 1}.csv`, { type: "text/csv" }),
        createDefaultParseConfig(currency),
      );
      admit();
      rowCount += parsed.rows.length;
      if (rowCount > 10000) throw new BasisError("batchSize");
      if (
        parsed.errors.length ||
        !parsed.rows.length ||
        parsed.rowCount !== parsed.rows.length ||
        parsed.headers.length < 2 ||
        parsed.headers.length > 50
      )
        throw new BasisError("structure");
      const table = format.normalize(parsed, currency, {
        onAccount: identify,
        allowCashOnly: true,
      });
      if (!identity || (account && identity !== account)) throw new BasisError("brokerageAccount");
      account = identity;
      documents.push({
        fileIndex: index + 1,
        name: file.name,
        formatId: format.id,
        kind: format.kind,
        table,
        excludedPositions,
      });
    } catch (error) {
      throw new BasisBatchError(
        error instanceof BasisError ? error.code : "parse",
        index + 1,
        error instanceof BasisError ? error.row : undefined,
      );
    }
  }
  admit();
  return combineBasisDocuments(documents, currency, startDate);
}
