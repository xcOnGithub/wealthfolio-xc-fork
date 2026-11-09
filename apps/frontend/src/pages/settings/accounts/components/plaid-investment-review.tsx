import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button, PrivacyAmount, useDateFormatting, useNumberFormatting } from "@wealthfolio/ui";
import type { PlaidInvestmentPreview } from "@/adapters/shared/plaid";

interface Props {
  preview: PlaidInvestmentPreview;
  busy: boolean;
  onApprove: (costs: Record<string, string>) => void;
  onClose: () => void;
}

export function PlaidInvestmentReview({ preview, busy, onApprove, onClose }: Props) {
  const { t } = useTranslation();
  const dates = useDateFormatting();
  const numbers = useNumberFormatting();
  const [costs, setCosts] = useState<Record<string, string>>(
    Object.fromEntries(preview.openingPositions.map((p) => [p.securityId, p.unitCost ?? ""])),
  );
  const [confirmed, setConfirmed] = useState(false);
  const [page, setPage] = useState(0);
  const size = 50;
  const invalidCosts = preview.openingPositions.some((p) => {
    const value = costs[p.securityId]?.trim();
    return !value || !Number.isFinite(Number(value)) || Number(value) < 0;
  });
  return (
    <section
      className="space-y-3 rounded-md border p-3"
      aria-label={t("connect:plaid.investments.review")}
    >
      <h3 className="font-semibold">{t("connect:plaid.investments.review")}</h3>
      <p>{t("connect:plaid.investments.help")}</p>
      <p>
        {t("connect:plaid.investments.coverage", {
          date: dates.formatCalendarDate(preview.startDate),
          transactions: preview.sourceTransactionCount,
        })}
      </p>
      <p>
        {t("connect:plaid.investments.openingCash")}:{" "}
        <PrivacyAmount value={Number(preview.openingCash)} currency={preview.currency} />
      </p>
      {preview.openingPositions.map((p) => (
        <label key={p.securityId} className="grid gap-2">
          <span>
            {p.name} · {numbers.formatQuantity(Number(p.quantity))} ·{" "}
            {t("connect:plaid.investments.unitCost")}
          </span>
          <input
            className="border-input bg-background rounded-md border px-3 py-2 text-sm"
            inputMode="decimal"
            value={costs[p.securityId] ?? ""}
            disabled={busy || !preview.initial}
            aria-label={`${t("connect:plaid.investments.unitCost")} ${p.name}`}
            onChange={(e) => setCosts((old) => ({ ...old, [p.securityId]: e.target.value }))}
          />
        </label>
      ))}
      {preview.issues.length > 0 && (
        <div role="alert" className="space-y-2">
          <p>{t("connect:plaid.investments.blocked")}</p>
          <ul className="max-h-60 space-y-1 overflow-auto">
            {preview.issues.map((issue) => (
              <li key={`${issue.recordId}-${issue.reason}`}>
                {issue.name ?? t("connect:plaid.investments.account")}:{" "}
                {t(`connect:plaid.investments.issues.${issue.reason}`, {
                  defaultValue: issue.reason,
                })}
              </li>
            ))}
          </ul>
        </div>
      )}
      <div className="overflow-auto">
        <table className="w-full text-sm">
          <thead>
            <tr>
              <th className="text-start">{t("common:date")}</th>
              <th className="text-start">{t("common:description")}</th>
              <th className="text-start">{t("common:type")}</th>
              <th className="text-end">{t("common:amount")}</th>
            </tr>
          </thead>
          <tbody>
            {preview.records.slice(page * size, (page + 1) * size).map((r) => (
              <tr key={r.id}>
                <td>{dates.formatCalendarDate(r.date)}</td>
                <td>
                  {r.name ?? "—"}
                  {r.securityId && (
                    <span className="text-muted-foreground block">
                      {numbers.formatQuantity(Number(r.quantity))} @{" "}
                      <PrivacyAmount value={Number(r.price)} currency={preview.currency} />
                    </span>
                  )}
                  {Number(r.fee) !== 0 && (
                    <span className="text-muted-foreground block">
                      {t("connect:plaid.investments.fee")}:{" "}
                      <PrivacyAmount value={Number(r.fee)} currency={preview.currency} />
                    </span>
                  )}
                </td>
                <td>{r.type}</td>
                <td className="text-end">
                  <PrivacyAmount value={Number(r.cash)} currency={preview.currency} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {preview.records.length > size && (
        <div className="flex gap-2">
          <Button variant="outline" disabled={page === 0} onClick={() => setPage((p) => p - 1)}>
            {t("connect:plaid.investments.previous")}
          </Button>
          <Button
            variant="outline"
            disabled={(page + 1) * size >= preview.records.length}
            onClick={() => setPage((p) => p + 1)}
          >
            {t("connect:plaid.investments.next")}
          </Button>
        </div>
      )}
      <label className="flex items-center gap-2">
        <input
          type="checkbox"
          checked={confirmed}
          disabled={busy}
          onChange={(e) => setConfirmed(e.target.checked)}
        />
        {t(
          preview.initial
            ? "connect:plaid.investments.confirmInitial"
            : "connect:plaid.investments.confirmUpdate",
        )}
      </label>
      <div className="flex gap-2">
        <Button
          disabled={busy || !confirmed || invalidCosts || preview.issues.length > 0}
          onClick={() => onApprove(costs)}
        >
          {t("common:confirm")}
        </Button>
        <Button variant="ghost" onClick={onClose}>
          {t("common:close")}
        </Button>
      </div>
    </section>
  );
}
