import Decimal from "decimal.js";
import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Button, PrivacyAmount as AmountDisplay, useDateFormatting } from "@wealthfolio/ui";
import { listStatements, saveStatements, deleteStatement } from "@/adapters";
import type { StatementSnapshot, StoredStatement } from "@/adapters/shared/statements";
import { readBasisFile } from "@/pages/settings/accounts/components/basis-pdf";
import { validateBasisFiles } from "@/pages/settings/accounts/components/opening-basis-batch";
import { extractMonthlyMerrill } from "./merrill-statement";
import { QueryKeys } from "@/lib/query-keys";

const Money = Decimal.clone({ precision: 40 });

export function StatementHistoryPanel({
  accountId,
  currency,
}: {
  accountId: string;
  currency: string;
}) {
  const { t } = useTranslation();
  const dates = useDateFormatting();
  const queryClient = useQueryClient();
  const queryKey = QueryKeys.statementHistory(accountId);
  const {
    data: stored,
    isError,
    isPending,
  } = useQuery({ queryKey, queryFn: () => listStatements(accountId) });
  const [drafts, setDrafts] = useState<StatementSnapshot[]>([]);
  const [draftNames, setDraftNames] = useState<Record<string, string>>({});
  const [reviewedPrevious, setReviewedPrevious] = useState<
    Record<string, StoredStatement | undefined>
  >({});
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  const [errorFile, setErrorFile] = useState<number>();
  const [deleting, setDeleting] = useState<StoredStatement>();
  const admission = useRef(0);
  useEffect(
    () => () => {
      admission.current++;
    },
    [],
  );
  const invalidate = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey }),
      queryClient.invalidateQueries({ queryKey: [QueryKeys.STATEMENT_VALUES, accountId] }),
      queryClient.invalidateQueries({ queryKey: [QueryKeys.NET_WORTH_HISTORY] }),
      queryClient.invalidateQueries({ queryKey: [QueryKeys.NET_WORTH] }),
    ]);
  };
  const upload = async (files: File[]) => {
    const generation = ++admission.current;
    const reviewedStored = stored ?? [];
    let fileIndex: number | undefined;
    setErrorFile(undefined);
    setDrafts([]);
    setDraftNames({});
    setConfirmed(false);
    setError(false);
    setBusy(true);
    try {
      validateBasisFiles(files);
      if (files.some((file) => !file.name.toLowerCase().endsWith(".pdf")))
        throw new Error("format");
      const batch: StatementSnapshot[] = [];
      const names: Record<string, string> = {};
      let identity: string | undefined;
      let chars = 0;
      for (const [index, file] of files.entries()) {
        fileIndex = index + 1;
        if (generation !== admission.current) return;
        let parsed: ReturnType<typeof extractMonthlyMerrill> | undefined;
        await readBasisFile(file, (pages) => {
          chars += pages.reduce((n, p) => n + p.reduce((m, i) => m + i.str.length, 0), 0);
          if (chars > 1_000_000) throw new Error("size");
          parsed = extractMonthlyMerrill(pages);
          return "";
        });
        if (generation !== admission.current) return;
        if (
          !parsed ||
          parsed.snapshot.currency !== currency ||
          (identity && identity !== parsed.identity)
        )
          throw new Error("account");
        identity = parsed.identity;
        const snapshot = parsed.snapshot;
        const previous = batch.find((s) => s.date === snapshot.date);
        if (previous && JSON.stringify(previous) !== JSON.stringify(parsed.snapshot))
          throw new Error("conflict");
        if (!previous) {
          batch.push(parsed.snapshot);
          names[parsed.snapshot.date] = file.name;
        }
      }
      setDrafts(batch);
      setDraftNames(names);
      setReviewedPrevious(
        Object.fromEntries(
          batch.map((s) => [s.date, reviewedStored.find((old) => old.snapshot.date === s.date)]),
        ),
      );
    } catch {
      if (generation === admission.current) {
        setError(true);
        setErrorFile(fileIndex);
      }
    } finally {
      if (generation === admission.current) setBusy(false);
    }
  };
  const save = async () => {
    setBusy(true);
    setError(false);
    try {
      await saveStatements(
        accountId,
        drafts.map((snapshot) => ({
          snapshot,
          expectedRevision: reviewedPrevious[snapshot.date]?.revision ?? null,
        })),
        confirmed,
      );
      setDrafts([]);
      setConfirmed(false);
      await invalidate();
    } catch {
      setError(true);
      setConfirmed(false);
      await invalidate();
    } finally {
      setBusy(false);
    }
  };
  const remove = async () => {
    if (!deleting) return;
    setBusy(true);
    setError(false);
    try {
      await deleteStatement(accountId, deleting.snapshot.date, deleting.revision);
      setDeleting(undefined);
      await invalidate();
    } catch {
      setError(true);
    } finally {
      setBusy(false);
    }
  };
  const renderSnapshot = (snapshot: StatementSnapshot, review = false) => (
    <div className="space-y-2 rounded-md border p-3" key={snapshot.date}>
      <p>
        <time dateTime={snapshot.date}>{dates.formatCalendarDate(snapshot.date)}</time>
        {" · "}
        <AmountDisplay value={Number(snapshot.totalValue)} currency={snapshot.currency} />
      </p>
      {review && (
        <p className="text-muted-foreground text-sm break-all">
          <bdi>{draftNames[snapshot.date]}</bdi>
        </p>
      )}
      <p className="text-muted-foreground text-sm">
        {t("account:statements.source", { page: snapshot.sourcePage })}
      </p>
      {(!snapshot.positionsComplete || snapshot.positions.some((p) => p.costBasis === null)) && (
        <p className="text-sm">{t("account:statements.partial")}</p>
      )}
      {review &&
        reviewedPrevious[snapshot.date] &&
        JSON.stringify(reviewedPrevious[snapshot.date]!.snapshot) !== JSON.stringify(snapshot) && (
          <div role="alert">
            <p>{t("account:statements.replace")}</p>
            <p>
              {t("common:previous")}:{" "}
              <AmountDisplay
                value={Number(reviewedPrevious[snapshot.date]!.snapshot.totalValue)}
                currency={snapshot.currency}
              />
            </p>
          </div>
        )}
      {snapshot.positions.length > 0 && (
        <details>
          <summary>{t("account:statements.positions")}</summary>
          <div className="overflow-x-auto">
            <table className="w-full text-start text-sm">
              <thead>
                <tr>
                  <th>{t("account:statements.positions")}</th>
                  <th>{t("account:statements.quantity")}</th>
                  <th>{t("account:statements.market")}</th>
                  <th>{t("account:statements.basis")}</th>
                  <th>{t("holdings:unrealized_pnl")}</th>
                </tr>
              </thead>
              <tbody>
                {snapshot.positions.map((p, index) => (
                  <tr key={index}>
                    <td>
                      <bdi>{p.label}</bdi>
                    </td>
                    <td>
                      <bdi>{p.quantity}</bdi>
                    </td>
                    <td>
                      {p.marketValue === null ? (
                        "—"
                      ) : (
                        <AmountDisplay value={Number(p.marketValue)} currency={snapshot.currency} />
                      )}
                    </td>
                    <td>
                      {p.costBasis === null ? (
                        "—"
                      ) : (
                        <AmountDisplay value={Number(p.costBasis)} currency={snapshot.currency} />
                      )}
                    </td>
                    <td>
                      {p.costBasis === null || p.marketValue === null ? (
                        "—"
                      ) : (
                        <AmountDisplay
                          value={Number(new Money(p.marketValue).minus(p.costBasis).toFixed())}
                          currency={snapshot.currency}
                        />
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </details>
      )}
    </div>
  );
  return (
    <details className="mb-4 rounded-md border p-4">
      <summary className="cursor-pointer font-medium">{t("account:statements.title")}</summary>
      <div className="mt-3 space-y-3">
        <p className="text-muted-foreground text-sm">{t("account:statements.help")}</p>
        {(error || isError) && (
          <p role="alert">
            {errorFile && t("connect:plaid.investments.evidence.batchFile", { file: errorFile })}{" "}
            {t("account:statements.error")}
          </p>
        )}
        <label className="grid gap-2">
          {t("account:statements.upload")}
          <input
            type="file"
            accept=".pdf,application/pdf"
            multiple
            disabled={busy || isPending || isError}
            onChange={(e) => {
              const files = Array.from(e.target.files ?? []);
              e.target.value = "";
              if (files.length) void upload(files);
            }}
          />
        </label>
        {drafts.map((s) => renderSnapshot(s, true))}
        {drafts.length > 0 && (
          <>
            <label className="flex items-start gap-2">
              <input
                type="checkbox"
                checked={confirmed}
                disabled={busy}
                onChange={(e) => setConfirmed(e.target.checked)}
              />
              {t("account:statements.confirm")}
            </label>
            <div className="flex gap-2">
              <Button disabled={busy || !confirmed || isError} onClick={() => void save()}>
                {t("common:save")}
              </Button>
              <Button
                variant="outline"
                disabled={busy}
                onClick={() => {
                  admission.current++;
                  setDrafts([]);
                  setConfirmed(false);
                }}
              >
                {t("common:cancel")}
              </Button>
            </div>
          </>
        )}
        {stored?.map((s) => (
          <div key={s.revision}>
            {renderSnapshot(s.snapshot)}
            <Button variant="ghost" disabled={busy} onClick={() => setDeleting(s)}>
              {t("common:delete")}
            </Button>
          </div>
        ))}
        {deleting && (
          <div role="alert" className="space-y-2">
            <p>
              {t("account:statements.deleteConfirm", {
                date: dates.formatCalendarDate(deleting.snapshot.date),
              })}
            </p>
            <Button disabled={busy} onClick={() => void remove()}>
              {t("common:confirm")}
            </Button>
            <Button variant="outline" disabled={busy} onClick={() => setDeleting(undefined)}>
              {t("common:cancel")}
            </Button>
          </div>
        )}
      </div>
    </details>
  );
}
