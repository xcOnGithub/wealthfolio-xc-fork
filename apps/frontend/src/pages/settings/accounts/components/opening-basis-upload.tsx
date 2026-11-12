import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button, useDateFormatting, useNumberFormatting } from "@wealthfolio/ui";
import { parseCsv } from "@/adapters";
import type { PlaidInvestmentPreview } from "@/adapters/shared/plaid";
import { createDefaultParseConfig } from "@/pages/activity/import/utils/default-activity-template";
import { MAX_BASIS_TEXT, readBasisFile, type PdfBasisExclusion } from "./basis-pdf";
import { BROKERAGE_BASIS_FORMATS } from "./brokerage-basis-formats";
import {
  BasisBatchError,
  prepareBasisBatch,
  type BasisBatch,
  type BasisOrigin,
} from "./opening-basis-batch";
import {
  BasisError,
  basisFields,
  calculateBasis,
  matchingBasis,
  openingDate,
  suggestBasisColumns,
  type BasisCandidate,
  type BasisColumns,
  type BasisCheckpoint,
} from "./opening-basis";

interface Props {
  preview: PlaidInvestmentPreview;
  busy: boolean;
  onApply: (costs: Record<string, string>) => void;
}
type ExcludedBasisPosition = PdfBasisExclusion & { name: string; file: number };

const control = "border-input bg-background min-w-0 rounded-md border px-3 py-2 text-sm";

export function OpeningBasisUpload({ preview, busy, onApply }: Props) {
  const { t } = useTranslation();
  const dates = useDateFormatting();
  const numbers = useNumberFormatting();
  const [excludeUnavailableBasis, setExcludeUnavailableBasis] = useState(false);
  const [excludedPositions, setExcludedPositions] = useState<ExcludedBasisPosition[]>([]);
  const [checkpointExclusions, setCheckpointExclusions] = useState<ExcludedBasisPosition[]>([]);
  const displayedExclusions = [...checkpointExclusions, ...excludedPositions];
  const key = "connect:plaid.investments.evidence";
  const [text, setText] = useState("");
  const [headerRow, setHeaderRow] = useState(1);
  const [table, setTable] = useState<{
    headers: string[];
    rows: string[][];
    sourceRows?: number[];
    cashRows?: number[];
  }>();
  const [formatId, setFormatId] = useState("");
  const format = BROKERAGE_BASIS_FORMATS.find((item) => item.id === formatId);
  const [columns, setColumns] = useState<BasisColumns>({});
  const [mode, setMode] = useState<"positions" | "transactions">("positions");
  const [asOf, setAsOf] = useState("");
  const [complete, setComplete] = useState(false);
  const [checkpoint, setCheckpoint] = useState<BasisCheckpoint>();
  const [batch, setBatch] = useState<BasisBatch>();
  const [batchFiles, setBatchFiles] = useState<string[]>([]);
  const [checkpointOrigins, setCheckpointOrigins] = useState<Record<number, BasisOrigin>>({});
  const [result, setResult] = useState<{ candidates: BasisCandidate[]; excluded: number }>();
  const [assignments, setAssignments] = useState<Record<string, string>>({});
  const [confirmed, setConfirmed] = useState(false);
  const [error, setError] = useState<{ code: string; row?: number; file?: number }>();
  const [loading, setLoading] = useState(false);
  const generation = useRef(0);
  useEffect(
    () => () => {
      generation.current++;
    },
    [],
  );
  const disabled = busy || loading;
  function invalidate() {
    generation.current++;
    setResult(undefined);
    setAssignments({});
    setConfirmed(false);
    setError(undefined);
  }
  function report(error: unknown) {
    const origin =
      error instanceof BasisError && !(error instanceof BasisBatchError) && error.row
        ? batch?.origins[error.row]
        : undefined;
    setError(
      error instanceof BasisError
        ? {
            code: error.code,
            row: origin?.row ?? error.row,
            file: error instanceof BasisBatchError ? error.fileIndex : origin?.file,
          }
        : { code: "parse" },
    );
  }
  async function upload(file: File) {
    invalidate();
    setBatch(undefined);
    setExcludedPositions([]);
    setBatchFiles([]);
    setText("");
    setTable(undefined);
    setColumns({});
    setAsOf("");
    setComplete(false);
    setHeaderRow(1);
    setLoading(true);
    const request = generation.current;
    try {
      if (formatId !== "manual" && !format) throw new BasisError("brokerageFormat");
      if (format && !file.name.toLowerCase().endsWith(`.${format.kind}`))
        throw new BasisError("brokerageFormat");
      const excluded: PdfBasisExclusion[] = [];
      const value = await readBasisFile(
        file,
        format?.extractPdf
          ? (pages) =>
              format.extractPdf!(pages, undefined, {
                excludeUnavailableBasis,
                onExcluded: (p) => excluded.push(p),
              })
          : undefined,
      );
      if (generation.current === request) {
        setText(value);
        setExcludedPositions(excluded.map((p) => ({ ...p, name: file.name, file: 1 })));
      }
    } catch (error) {
      if (generation.current === request) report(error);
    } finally {
      if (generation.current === request) setLoading(false);
    }
  }
  async function uploadBatch(files: File[]) {
    invalidate();
    setBatch(undefined);
    setExcludedPositions([]);
    setBatchFiles(files.map((file) => file.name));
    setText("");
    setTable(undefined);
    setColumns({});
    setAsOf("");
    setComplete(false);
    setLoading(true);
    const request = generation.current;
    try {
      const prepared = await prepareBasisBatch(
        files,
        formatId,
        preview.currency,
        preview.startDate,
        () => {
          if (generation.current !== request) throw new BasisError("parse");
        },
        excludeUnavailableBasis,
      );
      if (generation.current !== request) return;
      setBatch(prepared);
      const omissions = prepared.excludedPositions ?? [];
      const baseline = prepared.documents.find((doc) => doc.role === "baseline");
      setExcludedPositions(
        prepared.checkpoint ? omissions.filter((p) => p.file !== baseline?.fileIndex) : omissions,
      );
      if (prepared.hasStatement)
        setCheckpointExclusions(
          prepared.checkpoint ? omissions.filter((p) => p.file === baseline?.fileIndex) : [],
        );
      setTable(prepared);
      setColumns(prepared.columns);
      setMode(prepared.mode);
      setAsOf(prepared.asOf);
      if (prepared.hasStatement) {
        setCheckpoint(prepared.checkpoint);
        setCheckpointOrigins(prepared.checkpointOrigins);
      }
    } catch (error) {
      if (generation.current === request) report(error);
    } finally {
      if (generation.current === request) setLoading(false);
    }
  }
  function sourceRows(rows: number[], opening = false) {
    const references = opening ? checkpointOrigins : batch?.origins;
    return rows
      .map((row) => {
        const origin = references?.[row];
        return origin ? t(`${key}.${origin.page ? "batchPage" : "batchRow"}`, { ...origin }) : row;
      })
      .join("; ");
  }
  async function readTable() {
    invalidate();
    setTable(undefined);
    setLoading(true);
    const request = generation.current;
    try {
      if (formatId !== "manual" && !format) throw new BasisError("brokerageFormat");
      if (!text.trim() || text.length > MAX_BASIS_TEXT) throw new BasisError("size");
      if (!Number.isInteger(headerRow) || headerRow < 1) throw new BasisError("structure");
      const config = {
        ...createDefaultParseConfig(preview.currency),
        headerRowIndex: 0,
        skipTopRows: headerRow - 1,
      };
      const parsed = await parseCsv(
        new File([text], "opening-basis.csv", { type: "text/csv" }),
        config,
      );
      if (generation.current !== request) return;
      if (
        parsed.errors.length ||
        parsed.rows.length > 10000 ||
        !parsed.rows.length ||
        parsed.headers.length < 2 ||
        parsed.headers.length > 50
      )
        throw new BasisError("structure");
      const normalized = format?.normalize(parsed, preview.currency, {
        allowCashOnly: Boolean(checkpoint),
      });
      setTable(normalized ?? { headers: parsed.headers, rows: parsed.rows });
      setColumns(normalized?.columns ?? suggestBasisColumns(parsed.headers));
      if (format) {
        setMode(format.mode);
        if (format.kind === "pdf") setAsOf(normalized!.rows[0][3]);
      }
    } catch (error) {
      if (generation.current === request) report(error);
    } finally {
      if (generation.current === request) setLoading(false);
    }
  }
  function calculate() {
    invalidate();
    if (!table) return;
    try {
      setResult(
        calculateBasis({
          ...table,
          columns,
          mode,
          asOf,
          startDate: preview.startDate,
          currency: preview.currency,
          completeHistory: complete,
          checkpoint: mode === "transactions" ? checkpoint : undefined,
        }),
      );
    } catch (error) {
      report(error);
    }
  }
  const selected = result?.candidates.filter((candidate) => assignments[candidate.security]);
  const compatible =
    asOf === openingDate(preview.startDate) &&
    Boolean(selected?.length) &&
    selected!.every((candidate) => {
      const position = preview.openingPositions.find(
        (p) => p.securityId === assignments[candidate.security],
      );
      return position && matchingBasis(candidate, position.quantity);
    }) &&
    new Set(selected?.map((c) => assignments[c.security])).size === selected?.length;
  return (
    <details className="space-y-3 rounded-md border p-3">
      <summary className="cursor-pointer font-medium">{t(`${key}.title`)}</summary>
      <p className="text-sm">{t(`${key}.help`)}</p>
      <p className="text-muted-foreground text-sm">{t(`${key}.privacy`)}</p>
      <label className="grid gap-1 text-sm">
        {t(`${key}.brokerageFormat`)}
        <select
          aria-label={t(`${key}.brokerageFormat`)}
          className={control}
          value={formatId}
          disabled={disabled}
          onChange={(event) => {
            invalidate();
            setText("");
            setTable(undefined);
            setColumns({});
            setAsOf("");
            setComplete(false);
            setFormatId(event.target.value);
            setExcludedPositions([]);
            setBatch(undefined);
            setBatchFiles([]);
          }}
        >
          <option value="" disabled>
            {t(`${key}.chooseFormat`)}
          </option>
          <option value="manual">{t(`${key}.manualFormat`)}</option>
          {BROKERAGE_BASIS_FORMATS.map((item) => (
            <option key={item.id} value={item.id}>
              {t(`${key}.formats.${item.id}`)}
            </option>
          ))}
        </select>
      </label>
      {format && <p className="text-sm">{t(`${key}.formatHelp`)}</p>}
      {format?.brokerage === "merrill_edge" && (
        <label className="flex items-start gap-2 text-sm">
          <input
            type="checkbox"
            checked={excludeUnavailableBasis}
            disabled={disabled}
            onChange={(e) => {
              invalidate();
              setExcludeUnavailableBasis(e.target.checked);
              setCheckpointExclusions([]);
              setExcludedPositions([]);
              setText("");
              setTable(undefined);
              setColumns({});
              setAsOf("");
              setComplete(false);
              setCheckpoint(undefined);
              setCheckpointOrigins({});
              setBatch(undefined);
              setBatchFiles([]);
            }}
          />
          {t(`${key}.excludeUnavailableBasis`)}
        </label>
      )}
      {displayedExclusions.length > 0 && (
        <div role="status" className="space-y-1 text-sm">
          <p>{t(`${key}.excludedBasisHelp`, { total: displayedExclusions.length })}</p>
          <ul className="max-h-40 overflow-auto">
            {displayedExclusions.map((p, index) => (
              <li key={`${p.name}-${p.file}-${p.page}-${p.line}-${index}`}>
                <bdi>{p.label}</bdi> · {numbers.formatQuantity(p.quantity)} ·{" "}
                {t(`${key}.batchPage`, { file: p.file, name: p.name, page: p.page, line: p.line })}
              </li>
            ))}
          </ul>
        </div>
      )}
      {checkpoint && (
        <div className="space-y-2 text-sm">
          <p>
            {t(`${key}.checkpoint`, {
              date: dates.formatCalendarDate(checkpoint.asOf),
              count: checkpoint.candidates.length,
            })}
          </p>
          <div className="max-h-48 overflow-auto">
            <table className="w-full text-sm">
              <thead>
                <tr>
                  {["security", "quantity", "totalCost", "sourceRows"].map((field) => (
                    <th key={field} className="px-2 text-start">
                      {t(`${key}.fields.${field}`)}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {checkpoint.candidates.map((p) => (
                  <tr key={p.security}>
                    <td className="px-2">{p.security}</td>
                    <td className="px-2">{p.quantity}</td>
                    <td className="px-2">{p.totalCost}</td>
                    <td className="px-2">{sourceRows(p.rows, true)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <Button
            type="button"
            variant="outline"
            disabled={disabled}
            onClick={() => {
              invalidate();
              setCheckpoint(undefined);
              setCheckpointOrigins({});
              setCheckpointExclusions([]);
              setComplete(false);
            }}
          >
            {t(`${key}.clearCheckpoint`)}
          </Button>
        </div>
      )}
      <label className="grid gap-1 text-sm">
        {t(`${key}.file`)}
        <input
          type="file"
          multiple
          accept=".pdf,.csv,application/pdf,text/csv"
          disabled={disabled || !formatId}
          onChange={(event) => {
            const files = Array.from(event.target.files ?? []);
            event.target.value = "";
            if (files.length === 1) void upload(files[0]);
            else if (files.length) void uploadBatch(files);
          }}
        />
      </label>
      <p className="text-muted-foreground text-sm">{t(`${key}.batchHelp`)}</p>
      {batchFiles.length > 0 && (
        <ol className="space-y-1 text-sm" aria-label={t(`${key}.batchFiles`)}>
          {batchFiles.map((name, index) => {
            const doc = batch?.documents[index];
            return (
              <li key={index}>
                {t(`${key}.batchFile`, { file: index + 1 })} <bdi className="break-all">{name}</bdi>
                {doc && (
                  <>
                    {" "}
                    · {t(`${key}.batchRoles.${doc.role}`)} ·{" "}
                    {t(`${key}.rows`, { count: doc.records })}
                    {doc.cash > 0 && <> · {t(`${key}.batchCash`, { count: doc.cash })}</>}
                  </>
                )}
              </li>
            );
          })}
        </ol>
      )}
      {loading && <p role="status">{t(`${key}.loading`)}</p>}
      {text && (
        <>
          <label className="grid gap-1 text-sm">
            {t(`${key}.table`)}
            <textarea
              dir="ltr"
              aria-label={t(`${key}.table`)}
              className={`${control} h-48 w-full font-mono`}
              value={text}
              disabled={disabled || Boolean(format)}
              onChange={(e) => {
                invalidate();
                setTable(undefined);
                setText(e.target.value);
              }}
            />
          </label>
          {!format && <p className="text-muted-foreground text-sm">{t(`${key}.tableHelp`)}</p>}
          <label className="grid gap-1 text-sm">
            {t(`${key}.header`)}
            <input
              className={control}
              type="number"
              min={1}
              value={headerRow}
              disabled={disabled || Boolean(format)}
              onChange={(e) => {
                invalidate();
                setTable(undefined);
                setHeaderRow(Number(e.target.value));
              }}
            />
          </label>
          <Button
            type="button"
            variant="outline"
            disabled={disabled}
            onClick={() => void readTable()}
          >
            {t(`${key}.read`)}
          </Button>
        </>
      )}
      {table && (
        <div className="space-y-3">
          <p className="text-sm">{t(`${key}.rows`, { count: table.rows.length })}</p>
          {table.cashRows && (
            <p className="text-sm">
              {t(`${key}.cashRows`, {
                count: table.cashRows.length,
                rows: table.cashRows.join(", "),
              })}
            </p>
          )}
          <label className="grid gap-1 text-sm">
            {t(`${key}.mode`)}
            <select
              aria-label={t(`${key}.mode`)}
              className={control}
              value={mode}
              disabled={disabled || Boolean(format)}
              onChange={(e) => {
                invalidate();
                setComplete(false);
                setMode(e.target.value as typeof mode);
              }}
            >
              <option value="positions">{t(`${key}.positions`)}</option>
              <option value="transactions">{t(`${key}.transactions`)}</option>
            </select>
          </label>
          <p className="text-sm">{t(`${key}.costHelp`)}</p>
          <div className="grid gap-2 sm:grid-cols-2">
            {basisFields
              .filter(
                (field) => mode === "transactions" || !["action", "price", "fee"].includes(field),
              )
              .map((field) => (
                <label key={field} className="grid gap-1 text-sm">
                  {t(`${key}.fields.${field}`)}
                  <select
                    aria-label={t(`${key}.fields.${field}`)}
                    className={control}
                    value={columns[field] ?? ""}
                    disabled={disabled || Boolean(format)}
                    onChange={(e) => {
                      invalidate();
                      setColumns((old) => {
                        const next = { ...old };
                        if (e.target.value === "") delete next[field];
                        else next[field] = Number(e.target.value);
                        return next;
                      });
                    }}
                  >
                    <option value="">{t(`${key}.unmapped`)}</option>
                    {table.headers.map((header, i) => (
                      <option key={i} value={i}>
                        {i + 1}: {header}
                      </option>
                    ))}
                  </select>
                </label>
              ))}
          </div>
          <label className="grid gap-1 text-sm">
            {t(`${key}.asOf`)}
            <input
              className={control}
              type="date"
              value={asOf}
              disabled={disabled || (batch ? batch.mode === "positions" : format?.kind === "pdf")}
              onChange={(e) => {
                invalidate();
                setAsOf(e.target.value);
              }}
            />
          </label>
          <p className="text-sm">
            {t(`${key}.cutoffHelp`, {
              date: dates.formatCalendarDate(openingDate(preview.startDate)),
              currency: preview.currency,
            })}
          </p>
          {mode === "transactions" && (
            <label className="flex items-start gap-2 text-sm">
              <input
                type="checkbox"
                checked={complete}
                disabled={disabled}
                onChange={(e) => {
                  invalidate();
                  setComplete(e.target.checked);
                }}
              />
              {t(`${key}.complete`)}
            </label>
          )}
          <Button type="button" variant="outline" disabled={disabled} onClick={calculate}>
            {t(`${key}.calculate`)}
          </Button>
        </div>
      )}
      {error && (
        <p role="alert" className="text-destructive text-sm">
          {error.file && t(`${key}.batchFile`, { file: error.file })}{" "}
          {t(`${key}.errors.${error.code}`)} {error.row && t(`${key}.row`, { row: error.row })}
        </p>
      )}
      {result && (
        <div className="space-y-3">
          {mode === "positions" && asOf < openingDate(preview.startDate) && (
            <>
              <p className="text-sm">{t(`${key}.bridgeHelp`)}</p>
              <Button
                type="button"
                variant="outline"
                disabled={disabled}
                onClick={() => {
                  setCheckpoint({
                    asOf,
                    currency: preview.currency,
                    candidates: result.candidates,
                  });
                  setCheckpointOrigins(batch?.origins ?? {});
                  setCheckpointExclusions(excludedPositions);
                  setExcludedPositions([]);
                  setBatch(undefined);
                  setBatchFiles([]);
                  invalidate();
                  setText("");
                  setTable(undefined);
                  setColumns({});
                  setMode("transactions");
                  setAsOf("");
                  setComplete(false);
                }}
              >
                {t(`${key}.useCheckpoint`)}
              </Button>
            </>
          )}
          <p className="text-sm">{t(`${key}.excluded`, { count: result.excluded })}</p>
          <div className="overflow-auto">
            <table className="w-full text-sm">
              <thead>
                <tr>
                  {["security", "quantity", "totalCost", "unitCost", "sourceRows", "match"].map(
                    (field) => (
                      <th key={field} className="px-2 text-start">
                        {t(`${key}.fields.${field}`)}
                      </th>
                    ),
                  )}
                </tr>
              </thead>
              <tbody>
                {result.candidates.map((candidate) => (
                  <tr key={candidate.security}>
                    <td className="px-2">{candidate.security}</td>
                    <td className="px-2">{candidate.quantity}</td>
                    <td className="px-2">{candidate.totalCost}</td>
                    <td className="px-2">{candidate.unitCost}</td>
                    <td className="px-2">{sourceRows(candidate.rows)}</td>
                    <td className="px-2">
                      <select
                        className={control}
                        aria-label={t(`${key}.matchLabel`, { security: candidate.security })}
                        value={assignments[candidate.security] ?? ""}
                        disabled={disabled}
                        onChange={(e) => {
                          setConfirmed(false);
                          setAssignments((old) => ({
                            ...old,
                            [candidate.security]: e.target.value,
                          }));
                        }}
                      >
                        <option value="">{t(`${key}.unmapped`)}</option>
                        {preview.openingPositions.map((p) => (
                          <option
                            key={p.securityId}
                            value={p.securityId}
                            disabled={!matchingBasis(candidate, p.quantity)}
                          >
                            {p.name} · {p.quantity}
                          </option>
                        ))}
                      </select>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <p className="text-sm">{t(`${key}.matchHelp`)}</p>
          <label className="flex items-start gap-2 text-sm">
            <input
              type="checkbox"
              checked={confirmed}
              disabled={disabled}
              onChange={(e) => setConfirmed(e.target.checked)}
            />
            {t(`${key}.confirm`)}
          </label>
          <Button
            type="button"
            disabled={disabled || !confirmed || !compatible}
            onClick={() => {
              if (!compatible || !confirmed) return;
              onApply(
                Object.fromEntries(selected!.map((c) => [assignments[c.security], c.unitCost])),
              );
              setConfirmed(false);
            }}
          >
            {t(`${key}.apply`)}
          </Button>
        </div>
      )}
    </details>
  );
}
