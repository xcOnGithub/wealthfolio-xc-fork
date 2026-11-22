import Decimal from "decimal.js";
import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Button, PrivacyAmount as AmountDisplay, useDateFormatting } from "@wealthfolio/ui";
import { listStatements, saveStatements, deleteStatement, saveFamilyTask } from "@/adapters";
import type { StatementSnapshot, StoredStatement } from "@/adapters/shared/statements";
import { readBasisFile } from "@/pages/settings/accounts/components/basis-pdf";
import { validateBasisFiles } from "@/pages/settings/accounts/components/opening-basis-batch";
import { MAX_STATEMENT_BATCH, MAX_STATEMENT_PAGES } from "./merrill-format";
import { extractMonthlyStatements } from "./statement-parser";
import { selectedProfileId } from "@/features/profiles/session";
import { QueryKeys } from "@/lib/query-keys";

import { DailyStatementPanel } from "./daily-statement-panel";

const Money = Decimal.clone({ precision: 40 });

function sameSnapshotContent(a: StatementSnapshot, b: StatementSnapshot): boolean {
  const { sourcePage: _a, ...aContent } = a;
  const { sourcePage: _b, ...bContent } = b;
  return JSON.stringify(aContent) === JSON.stringify(bContent);
}

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
  const [mode, setMode] = useState("monthly");
  const [drafts, setDrafts] = useState<StatementSnapshot[]>([]);
  const [draftNames, setDraftNames] = useState<Record<string, string>>({});
  const [reviewedPrevious, setReviewedPrevious] = useState<
    Record<string, StoredStatement | undefined>
  >({});
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  const [reminderError, setReminderError] = useState(false);
  const reminderWrites = useRef(Promise.resolve());
  const rememberReview = (resolved: boolean) => {
    const profile = selectedProfileId();
    const attempt = admission.current;
    // A fast save must clear its earlier preview reminder even if requests are slow.
    reminderWrites.current = reminderWrites.current.then(async () => {
      if (attempt !== admission.current || profile !== selectedProfileId()) return;
      try {
        await saveFamilyTask({
          accountId,
          resolved,
          title: t("family:statementReviewTitle"),
          detail: t("family:statementReviewDetail"),
        });
        if (attempt !== admission.current || profile !== selectedProfileId()) return;
        setReminderError(false);
        await queryClient.invalidateQueries({ queryKey: ["family-money", profile] });
      } catch {
        if (attempt === admission.current && profile === selectedProfileId())
          setReminderError(true);
      }
    });
    return reminderWrites.current;
  };
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
      queryClient.invalidateQueries({ queryKey: [QueryKeys.STATEMENT_ACTIVITIES, accountId] }),
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
      let pageCount = 0;
      for (const [index, file] of files.entries()) {
        fileIndex = index + 1;
        if (generation !== admission.current) return;
        let parsed: ReturnType<typeof extractMonthlyStatements> | undefined;
        await readBasisFile(
          file,
          (pages) => {
            pageCount += pages.length;
            chars += pages.reduce((n, p) => n + p.reduce((m, i) => m + i.str.length, 0), 0);
            if (chars > 1_000_000 || pageCount > MAX_STATEMENT_PAGES) throw new Error("size");
            parsed = extractMonthlyStatements(pages);
            return "";
          },
          { maxPages: MAX_STATEMENT_PAGES },
        );
        if (generation !== admission.current) return;
        if (!parsed?.length) throw new Error("account");
        for (const item of parsed) {
          if (item.snapshot.currency !== currency || (identity && identity !== item.identity))
            throw new Error("account");
          identity = item.identity;
          const snapshot = item.snapshot;
          const previous = batch.find((s) => s.date === snapshot.date);
          if (previous && !sameSnapshotContent(previous, snapshot)) throw new Error("conflict");
          if (!previous) {
            if (batch.length >= MAX_STATEMENT_BATCH) throw new Error("size");
            batch.push(snapshot);
            names[snapshot.date] = file.name;
          }
        }
      }
      setDrafts(batch);
      void rememberReview(false);
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
      void rememberReview(true);
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
        !sameSnapshotContent(reviewedPrevious[snapshot.date]!.snapshot, snapshot) && (
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
        {reminderError && (
          <p role="status" className="text-muted-foreground text-sm">
            {t("family:statementReminderError")}
          </p>
        )}

        <p className="text-muted-foreground text-sm">{t("account:statements.help")}</p>
        <label className="grid gap-2">
          {t("account:statements.daily.mode")}
          <select
            className="bg-background rounded border p-2"
            value={mode}
            disabled={busy}
            onChange={(e) => setMode(e.target.value)}
          >
            <option value="monthly">{t("account:statements.daily.monthly")}</option>
            <option value="daily">{t("account:statements.daily.title")}</option>
          </select>
        </label>
        {mode === "daily" ? (
          <DailyStatementPanel key={accountId} accountId={accountId} currency={currency} />
        ) : (
          <>
            {(error || isError) && (
              <p role="alert">
                {errorFile &&
                  t("connect:plaid.investments.evidence.batchFile", { file: errorFile })}{" "}
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
          </>
        )}
      </div>
    </details>
  );
}
