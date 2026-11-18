import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button, useDateFormatting } from "@wealthfolio/ui";
import type { PlaidInvestmentPreview } from "@/adapters/shared/plaid";
import { BROKERAGE_BASIS_FORMATS } from "./brokerage-basis-formats";
import { BasisError } from "./opening-basis";
import {
  calculateTransferBasis,
  readTransferStatement,
  type TransferStatement,
  type TransferBasisDependencies,
} from "./transfer-basis";

interface Props {
  preview: PlaidInvestmentPreview;
  busy: boolean;
  openingCosts: Record<string, string>;
  transferCosts: Record<string, string>;
  disposedCosts: Record<string, string>;
  onDisposedCostChange: (id: string, value: string) => void;
  onApply: (id: string, cost: string, dependencies: TransferBasisDependencies) => void;
}
const control = "border-input bg-background min-w-0 rounded-md border px-3 py-2 text-sm";

export function TransferBasisUpload({
  preview,
  busy,
  openingCosts,
  transferCosts,
  disposedCosts,
  onDisposedCostChange,
  onApply,
}: Props) {
  const { t } = useTranslation();
  const dates = useDateFormatting();
  const key = "connect:plaid.investments.transferEvidence";
  const evidence = "connect:plaid.investments.evidence";
  const [formatId, setFormatId] = useState("");
  const [exclude, setExclude] = useState(false);
  const [statement, setStatement] = useState<TransferStatement>();
  const [transferId, setTransferId] = useState("");
  const [security, setSecurity] = useState("");
  const [confirmedContext, setConfirmedContext] = useState<string>();
  const [result, setResult] = useState<{
    context: string;
    value: ReturnType<typeof calculateTransferBasis>;
  }>();
  const [error, setError] = useState<string>();
  const [loading, setLoading] = useState(false);
  const generation = useRef(0);
  useEffect(
    () => () => {
      generation.current++;
    },
    [],
  );
  const context = JSON.stringify([
    preview,
    statement,
    transferId,
    security,
    openingCosts,
    transferCosts,
    disposedCosts,
  ]);
  const current = result?.context === context ? result.value : undefined;
  const disabled = busy || loading;
  const target = preview.records.find((r) => r.id === transferId);
  const dispositions = preview.records.filter(
    (r) =>
      target &&
      statement &&
      r.securityId === target.securityId &&
      r.date <= statement.asOf &&
      ["SELL", "TRANSFER_OUT"].includes(r.type),
  );
  function reset() {
    generation.current++;
    setStatement(undefined);
    setResult(undefined);
    setError(undefined);
    setSecurity("");
    setConfirmedContext(undefined);
  }
  async function upload(file: File) {
    reset();
    setLoading(true);
    const request = generation.current;
    const admit = () => {
      if (request !== generation.current) throw new Error("stale");
    };
    try {
      const next = await readTransferStatement(file, formatId, preview.currency, exclude, admit);
      admit();
      setStatement(next);
    } catch (failure) {
      if (request === generation.current)
        setError(failure instanceof BasisError ? failure.code : "parse");
    } finally {
      if (request === generation.current) setLoading(false);
    }
  }
  return (
    <details className="space-y-3 rounded-md border p-3">
      <summary className="cursor-pointer font-medium">{t(`${key}.title`)}</summary>
      <p className="text-sm">{t(`${key}.help`)}</p>
      <label className="grid gap-1 text-sm">
        {t(`${evidence}.brokerageFormat`)}
        <select
          aria-label={t(`${evidence}.brokerageFormat`)}
          className={control}
          value={formatId}
          disabled={disabled}
          onChange={(e) => {
            reset();
            setFormatId(e.target.value);
          }}
        >
          <option value="">{t(`${evidence}.unmapped`)}</option>
          {BROKERAGE_BASIS_FORMATS.filter((f) => f.kind === "pdf").map((f) => (
            <option key={f.id} value={f.id}>
              {t(`${evidence}.formats.${f.id}`)}
            </option>
          ))}
        </select>
      </label>
      <label className="flex items-start gap-2 text-sm">
        <input
          type="checkbox"
          checked={exclude}
          disabled={disabled}
          onChange={(e) => {
            reset();
            setExclude(e.target.checked);
          }}
        />
        {t(`${evidence}.excludeUnavailableBasis`)}
      </label>
      <label className="grid gap-1 text-sm">
        {t(`${key}.file`)}
        <input
          type="file"
          accept=".pdf,application/pdf"
          disabled={disabled || !formatId}
          onChange={(e) => {
            const file = e.target.files?.[0];
            e.target.value = "";
            if (file) void upload(file);
          }}
        />
      </label>
      {loading && <p role="status">{t(`${evidence}.loading`)}</p>}
      {statement && (
        <>
          <p className="text-sm">
            <bdi>{statement.name}</bdi> · {dates.formatCalendarDate(statement.asOf)} ·{" "}
            <bdi>{preview.currency}</bdi>
          </p>
          {statement.exclusions.length > 0 && (
            <div role="status" className="text-sm">
              <p>{t(`${evidence}.excludedBasisHelp`, { total: statement.exclusions.length })}</p>
              {statement.exclusions.map((p, i) => (
                <p key={i}>
                  <bdi>{p.label}</bdi> · {p.quantity} ·{" "}
                  {t(`${key}.source`, { page: p.page, line: p.line })}
                </p>
              ))}
            </div>
          )}
          <label className="grid gap-1 text-sm">
            {t(`${key}.target`)}
            <select
              aria-label={t(`${key}.target`)}
              className={control}
              value={transferId}
              disabled={disabled}
              onChange={(e) => {
                setTransferId(e.target.value);
                setResult(undefined);
                setConfirmedContext(undefined);
                setError(undefined);
              }}
            >
              <option value="">{t(`${evidence}.unmapped`)}</option>
              {preview.records
                .filter((r) => r.requiresTransferCost && r.transferUnitCost == null)
                .map((r) => (
                  <option key={r.id} value={r.id}>
                    {r.name} · {dates.formatCalendarDate(r.date)} · {r.quantity}
                  </option>
                ))}
            </select>
          </label>
          <label className="grid gap-1 text-sm">
            {t(`${key}.position`)}
            <select
              aria-label={t(`${key}.position`)}
              className={control}
              value={security}
              disabled={disabled}
              onChange={(e) => {
                setSecurity(e.target.value);
                setResult(undefined);
                setConfirmedContext(undefined);
                setError(undefined);
              }}
            >
              <option value="">{t(`${evidence}.unmapped`)}</option>
              {statement.candidates.map((p) => (
                <option key={p.security} value={p.security}>
                  {p.security} · {p.quantity} · {p.totalCost} {preview.currency}
                </option>
              ))}
            </select>
          </label>
          {dispositions.map((r) => (
            <label key={r.id} className="grid gap-1 text-sm">
              <span>
                {t(`${key}.disposed`)} · <bdi>{r.name}</bdi> · {dates.formatCalendarDate(r.date)} ·{" "}
                {r.quantity}
              </span>
              <input
                className={control}
                inputMode="decimal"
                disabled={disabled}
                value={disposedCosts[r.id] ?? ""}
                onChange={(e) => {
                  onDisposedCostChange(r.id, e.target.value);
                  setConfirmedContext(undefined);
                }}
              />
            </label>
          ))}
          <Button
            type="button"
            variant="outline"
            disabled={disabled || !transferId || !security}
            onClick={() => {
              setResult(undefined);
              setConfirmedContext(undefined);
              setError(undefined);
              try {
                const candidate = statement.candidates.find((p) => p.security === security);
                if (!candidate) return;
                setResult({
                  context,
                  value: calculateTransferBasis({
                    preview,
                    statement,
                    candidate,
                    transferId,
                    openingCosts,
                    transferCosts,
                    disposedCosts,
                  }),
                });
              } catch (failure) {
                setError(failure instanceof BasisError ? failure.code : "parse");
              }
            }}
          >
            {t(`${evidence}.calculate`)}
          </Button>
          {current && (
            <div className="space-y-3 text-sm">
              <p>{t(`${key}.calculation`, { ...current, currency: preview.currency })}</p>
              <p>{t(`${key}.proposal`, { ...current, currency: preview.currency })}</p>
              {statement.candidates
                .find((p) => p.security === security)
                ?.rows.map((row) => (
                  <p key={row}>{t(`${key}.source`, statement.sources[row])}</p>
                ))}
              {current.excludedQuantity !== "0" && (
                <p>
                  {t(`${evidence}.fractionalMatch`, {
                    quantity: current.quantity,
                    excluded: current.excludedQuantity,
                    total: current.ending,
                    currency: preview.currency,
                    unitCost: current.statementUnitCost,
                  })}
                </p>
              )}
              <label className="flex items-start gap-2">
                <input
                  type="checkbox"
                  disabled={disabled}
                  checked={confirmedContext === context}
                  onChange={(e) => setConfirmedContext(e.target.checked ? context : undefined)}
                />
                {t(`${key}.confirm`, {
                  from: dates.formatCalendarDate(preview.startDate),
                  to: dates.formatCalendarDate(statement.asOf),
                })}
              </label>
              <Button
                type="button"
                disabled={
                  disabled ||
                  confirmedContext !== context ||
                  Boolean(transferCosts[transferId]?.trim())
                }
                onClick={() => {
                  if (
                    !current ||
                    confirmedContext !== context ||
                    transferCosts[transferId]?.trim() ||
                    target?.transferUnitCost != null
                  )
                    return;
                  onApply(transferId, current.unitCost, current.dependencies);
                  setConfirmedContext(undefined);
                  setResult(undefined);
                }}
              >
                {t(`${key}.apply`)}
              </Button>
              {transferCosts[transferId]?.trim() && <p>{t(`${key}.preserve`)}</p>}
            </div>
          )}
        </>
      )}
      {error && (
        <p role="alert" className="text-destructive text-sm">
          {t(`${key}.errors.${error}`, { defaultValue: t(`${evidence}.errors.${error}`) })}
        </p>
      )}
    </details>
  );
}
