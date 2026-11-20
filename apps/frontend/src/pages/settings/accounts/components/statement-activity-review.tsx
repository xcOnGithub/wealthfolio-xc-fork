import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button, useDateFormatting } from "@wealthfolio/ui";
import { BasisError } from "./opening-basis";
import { proposeDisposedBasis, type InvestmentRecord } from "./statement-activity-match";
import type { DisposedBasisSource, ReviewCostUpdate, TransferStatement } from "./transfer-basis";

interface Props {
  statement: TransferStatement;
  security: string;
  records: InvestmentRecord[];
  costs: Record<string, string>;
  sources: Record<string, DisposedBasisSource>;
  disabled: boolean;
  onUpdate: (update: ReviewCostUpdate) => void;
}
const control = "border-input bg-background min-w-0 rounded-md border px-3 py-2 text-sm";

export function StatementActivityReview({
  statement,
  security,
  records,
  costs,
  sources,
  disabled,
  onUpdate,
}: Props) {
  const { t } = useTranslation();
  const dates = useDateFormatting();
  const key = "connect:plaid.investments.statementActivity";
  const sourceKey = "connect:plaid.investments.transferEvidence.source";
  const [recordId, setRecordId] = useState("");
  const [rowIds, setRowIds] = useState<string[]>([]);
  const [verified, setVerified] = useState(false);
  const [error, setError] = useState<string>();
  if (!statement.activity || !statement.documentId) return null;
  const activity = statement.activity;
  const record = records.find((r) => r.id === recordId);
  const lots = activity.dispositions.filter((r) => !r.security || r.security === security);
  function changeRows(next: string[]) {
    onUpdate({
      invalidateDisposed: { documentId: statement.documentId!, rowIds: [...rowIds, ...next] },
    });
    setRowIds(next);
    setVerified(false);
    setError(undefined);
  }
  return (
    <section className="space-y-3 rounded-md border p-3 text-sm" aria-label={t(`${key}.title`)}>
      <h4 className="font-medium">{t(`${key}.title`)}</h4>
      <p>{t(`${key}.help`)}</p>
      <p>
        {t(`${key}.coverage`, {
          from: dates.formatCalendarDate(activity.from),
          to: dates.formatCalendarDate(activity.through),
          activities: activity.activities.length,
          dispositions: activity.dispositions.length,
        })}
      </p>
      <details>
        <summary className="cursor-pointer">{t(`${key}.activity`)}</summary>
        <ul className="max-h-60 space-y-2 overflow-auto">
          {activity.activities.map((r) => (
            <li key={r.id}>
              <bdi>{r.description}</bdi> · {r.date ? dates.formatCalendarDate(r.date) : "—"} ·{" "}
              {t(`${key}.sections.${r.section}`)} · {r.quantity ?? "—"} · {r.cash ?? "—"}
              {r.transferValue !== null && (
                <>
                  {" "}
                  · {t(`${key}.transferValue`)}: {r.transferValue}
                </>
              )}
              {r.action === "UNSUPPORTED" && r.section !== "pending" && (
                <p className="text-destructive">{t(`${key}.unsupported`)}</p>
              )}
              <p>{t(sourceKey, { page: r.page, line: r.line })}</p>
            </li>
          ))}
        </ul>
      </details>
      <label className="grid gap-1">
        {t(`${key}.sale`)}
        <select
          className={control}
          disabled={disabled || !security}
          value={recordId}
          onChange={(e) => {
            const id = e.target.value;
            setRecordId(id);
            setRowIds(
              sources[id]?.documentId === statement.documentId
                ? sources[id].rows.map((r) => r.id)
                : [],
            );
            setVerified(false);
            setError(undefined);
          }}
        >
          <option value="">{t("connect:plaid.investments.evidence.unmapped")}</option>
          {records
            .filter((r) => r.type === "SELL")
            .map((r) => (
              <option key={r.id} value={r.id}>
                {r.name} · {dates.formatCalendarDate(r.date)} · {r.quantity} · {r.cash}
              </option>
            ))}
        </select>
      </label>
      {record && (
        <>
          <p>{t(`${key}.lots`)}</p>
          {lots.length === 0 && <p>{t(`${key}.noLots`)}</p>}
          {lots.map((r) => {
            const assigned = Object.entries(sources).find(
              ([id, s]) =>
                id !== recordId &&
                s.documentId === statement.documentId &&
                s.rows.some((x) => x.id === r.id),
            );
            return (
              <label key={r.id} className="flex items-start gap-2">
                <input
                  type="checkbox"
                  disabled={disabled || !!assigned || r.basis === null || r.proceeds === null}
                  checked={rowIds.includes(r.id)}
                  onChange={(e) =>
                    changeRows(
                      e.target.checked ? [...rowIds, r.id] : rowIds.filter((id) => id !== r.id),
                    )
                  }
                />
                <span>
                  <bdi>{r.description}</bdi> · {dates.formatCalendarDate(r.date)} · {r.quantity} ·{" "}
                  {t(`${key}.basis`)}: {r.basis ?? "—"} · {t(`${key}.proceeds`)}:{" "}
                  {r.proceeds ?? "—"} · {t(sourceKey, { page: r.page, line: r.line })}
                  {assigned && <> · {t(`${key}.assigned`)}</>}
                </span>
              </label>
            );
          })}
          <label className="flex items-start gap-2">
            <input
              type="checkbox"
              disabled={disabled || rowIds.length === 0}
              checked={verified}
              onChange={(e) => setVerified(e.target.checked)}
            />
            {t(`${key}.confirm`)}
          </label>
          <Button
            type="button"
            disabled={disabled || !verified || !rowIds.length || !!costs[recordId]?.trim()}
            onClick={() => {
              setError(undefined);
              try {
                const proposal = proposeDisposedBasis({
                  record,
                  statement,
                  security,
                  rowIds,
                  verifiedDates: verified,
                });
                onUpdate({
                  fillDisposed: {
                    id: recordId,
                    value: proposal.value,
                    source: {
                      documentId: statement.documentId!,
                      filename: statement.name,
                      rows: proposal.rows,
                    },
                  },
                });
                setVerified(false);
              } catch (failure) {
                setError(failure instanceof BasisError ? failure.code : "parse");
              }
            }}
          >
            {t(`${key}.apply`)}
          </Button>
          {costs[recordId]?.trim() && (
            <p>{t("connect:plaid.investments.transferEvidence.preserve")}</p>
          )}
          {sources[recordId] && (
            <p>
              <bdi>{sources[recordId].filename}</bdi> ·{" "}
              {sources[recordId].rows
                .map((r) => t(sourceKey, { page: r.page, line: r.line }))
                .join("; ")}
            </p>
          )}
        </>
      )}
      {error && (
        <p role="alert" className="text-destructive">
          {t(`connect:plaid.investments.evidence.errors.${error}`)}
        </p>
      )}
    </section>
  );
}
