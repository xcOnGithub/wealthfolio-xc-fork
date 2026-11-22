import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Button, PrivacyAmount, useDateFormatting } from "@wealthfolio/ui";
import {
  getAssets,
  listDailyStatements,
  listStatements,
  previewDailyStatements,
  saveDailyStatements,
  refreshStatementQuotes,
} from "@/adapters";
import type { DailyStatementPreview, DailyStatementWrite } from "@/adapters/shared/statements";
import { readBasisFile } from "@/pages/settings/accounts/components/basis-pdf";
import { validateBasisFiles } from "@/pages/settings/accounts/components/opening-basis-batch";
import { QueryKeys } from "@/lib/query-keys";
import type { StatementHistoryDraft } from "./merrill-history";
import { extractStatementHistory } from "./statement-parser";
import { dailyStatementWrite } from "./daily-review";

interface Review {
  draft: StatementHistoryDraft;
  name: string;
  links: Record<string, string>;
  revision: string | null;
  snapshotRevision: string | null;
}
export function DailyStatementPanel({
  accountId,
  currency,
}: {
  accountId: string;
  currency: string;
}) {
  const { t } = useTranslation();
  const dates = useDateFormatting();
  const client = useQueryClient();
  const { data: assets, isError: assetError } = useQuery({
    queryKey: ["statementAssets"],
    queryFn: getAssets,
  });
  const {
    data: saved,
    isPending,
    isError,
    refetch,
  } = useQuery({
    queryKey: ["dailyStatements", accountId],
    queryFn: () => listDailyStatements(accountId),
  });
  const [reviews, setReviews] = useState<Review[]>([]);
  const [previews, setPreviews] = useState<DailyStatementPreview[]>([]);
  const [previewWrites, setPreviewWrites] = useState<DailyStatementWrite[]>([]);
  const [busy, setBusy] = useState(false);
  const [confirmed, setConfirmed] = useState(false);
  const [error, setError] = useState<string>();
  const [refreshFailures, setRefreshFailures] = useState<number>();
  const generation = useRef(0);
  useEffect(
    () => () => {
      generation.current++;
    },
    [],
  );
  const invalidate = async () => {
    await Promise.all([
      client.invalidateQueries({ queryKey: QueryKeys.statementHistory(accountId) }),
      client.invalidateQueries({ queryKey: [QueryKeys.STATEMENT_VALUES, accountId] }),
      client.invalidateQueries({ queryKey: [QueryKeys.NET_WORTH_HISTORY] }),
      client.invalidateQueries({ queryKey: [QueryKeys.NET_WORTH] }),
      refetch(),
    ]);
  };
  const clearPreview = () => {
    generation.current++;
    setPreviews([]);
    setPreviewWrites([]);
    setConfirmed(false);
    setError(undefined);
    setRefreshFailures(undefined);
  };
  const upload = async (files: File[]) => {
    clearPreview();
    const current = generation.current;
    setReviews([]);
    setBusy(true);
    try {
      validateBasisFiles(files);
      if (files.some((f) => !f.name.toLowerCase().endsWith(".pdf"))) throw new Error("format");
      const [daily, monthly] = await Promise.all([
        listDailyStatements(accountId),
        listStatements(accountId),
      ]);
      const batch: Review[] = [];
      let identity: string | undefined;
      let chars = 0;
      let pageCount = 0;
      for (const file of files) {
        let parsed: ReturnType<typeof extractStatementHistory> | undefined;
        await readBasisFile(file, (pages) => {
          pageCount += pages.length;
          chars += pages.reduce((n, p) => n + p.reduce((m, i) => m + i.str.length, 0), 0);
          if (chars > 1_000_000 || pageCount > 100) throw new Error("size");
          parsed = extractStatementHistory(pages);
          return "";
        });
        if (current !== generation.current) return;
        if (
          !parsed ||
          parsed.draft.closing.currency !== currency ||
          (identity && identity !== parsed.identity)
        )
          throw new Error("account");
        identity = parsed.identity;
        if (batch.some((r) => r.draft.through === parsed!.draft.through))
          throw new Error("duplicate");
        batch.push({
          draft: parsed.draft,
          name: file.name,
          links: {},
          revision:
            daily.find((s) => s.interval.through === parsed!.draft.through)?.revision ?? null,
          snapshotRevision:
            monthly.find((s) => s.snapshot.date === parsed!.draft.through)?.revision ?? null,
        });
      }
      if (current === generation.current) setReviews(batch);
    } catch {
      if (current === generation.current) setError(t("account:statements.error"));
    } finally {
      if (current === generation.current) setBusy(false);
    }
  };
  const calculate = async (refresh = false) => {
    clearPreview();
    const current = generation.current;
    setBusy(true);
    try {
      const writes = reviews.map((r) =>
        dailyStatementWrite(r.draft, r.links, r.revision, r.snapshotRevision),
      );
      if (refresh) {
        const failures = await refreshStatementQuotes(accountId, writes);
        if (current !== generation.current) return;
        setRefreshFailures(failures);
        await invalidate();
      }
      const result = await previewDailyStatements(accountId, writes);
      if (current === generation.current) {
        setPreviews(result);
        setPreviewWrites(writes);
      }
    } catch {
      if (current === generation.current) setError(t("account:statements.daily.reviewError"));
    } finally {
      if (current === generation.current) setBusy(false);
    }
  };
  const save = async () => {
    if (!confirmed || !previews.length) return;
    setBusy(true);
    setError(undefined);
    try {
      await saveDailyStatements(accountId, previewWrites, true);
      setReviews([]);
      clearPreview();
      await invalidate();
    } catch {
      setError(t("account:statements.daily.reviewError"));
      setConfirmed(false);
      await invalidate();
    } finally {
      setBusy(false);
    }
  };
  const available = assets?.filter(
    (a) => a.kind === "INVESTMENT" && a.instrumentType === "EQUITY" && a.quoteCcy === "USD",
  );
  const selectAsset = (review: Review, index: number, key: string, label: string) => (
    <label className="grid gap-1" key={key}>
      <bdi>{label}</bdi>
      <select
        className="bg-background w-full rounded border p-2"
        value={review.links[key] ?? ""}
        disabled={busy}
        onChange={(e) => {
          clearPreview();
          setReviews((old) =>
            old.map((r, i) =>
              i === index ? { ...r, links: { ...r.links, [key]: e.target.value } } : r,
            ),
          );
        }}
      >
        <option value="">{t("account:statements.daily.selectAsset")}</option>
        {available?.map((a) => (
          <option key={a.id} value={a.id}>
            {a.displayCode ?? a.instrumentSymbol ?? a.id} · {a.name}
          </option>
        ))}
      </select>
    </label>
  );
  return (
    <div className="space-y-3">
      <p className="text-muted-foreground text-sm">{t("account:statements.daily.help")}</p>
      {(error || isError || assetError) && (
        <p role="alert">{error ?? t("account:statements.error")}</p>
      )}
      <label className="grid gap-2">
        {t("account:statements.upload")}
        <input
          type="file"
          accept=".pdf,application/pdf"
          multiple
          disabled={busy || isPending || isError || assetError}
          onChange={(e) => {
            const files = Array.from(e.target.files ?? []);
            e.target.value = "";
            if (files.length) void upload(files);
          }}
        />
      </label>
      {reviews.map((r, index) => (
        <section className="space-y-3 rounded border p-3" key={r.draft.through}>
          <p className="break-all">
            <bdi>{r.name}</bdi> · {dates.formatCalendarDate(r.draft.from)} –{" "}
            {dates.formatCalendarDate(r.draft.through)}
          </p>
          {(r.revision || r.snapshotRevision) && (
            <p role="alert">
              {t(r.revision ? "account:statements.daily.replace" : "account:statements.replace")}
            </p>
          )}
          <p>
            {t("account:statements.daily.cashCheck")}{" "}
            <bdi>
              {r.draft.openingCash ? (
                <PrivacyAmount value={Number(r.draft.openingCash.value)} currency={currency} />
              ) : (
                "—"
              )}{" "}
              +{" "}
              <PrivacyAmount
                value={Number(r.draft.cashReconciliation.knownMovementTotal)}
                currency={currency}
              />{" "}
              ={" "}
              {r.draft.closing.cashValue === null ? (
                "—"
              ) : (
                <PrivacyAmount value={Number(r.draft.closing.cashValue)} currency={currency} />
              )}
            </bdi>
          </p>
          <p>
            {t("account:statements.daily.sections", {
              sections: Object.keys(r.draft.evidence.sections).join(", "),
              events: r.draft.evidence.activities.length,
            })}
          </p>
          {r.draft.issues.map((issue, i) => (
            <p role="alert" key={i}>
              {t(`account:statements.daily.issues.${issue.code}`)}{" "}
              {issue.source &&
                t("account:statements.daily.reference", {
                  page: issue.source.page,
                  line: issue.source.line,
                })}
            </p>
          ))}
          <p className="text-sm">{t("account:statements.daily.mappingHelp")}</p>
          {r.draft.closing.positions.map((p, i) =>
            selectAsset(r, index, `holding:${i}`, `${p.label} · ${p.quantity}`),
          )}
          <details>
            <summary>{t("account:statements.daily.events")}</summary>
            <div className="space-y-2">
              {r.draft.evidence.activities.map((e) => (
                <div key={e.id} className="rounded border p-2 text-sm">
                  <p>
                    <bdi>
                      {e.date ?? "—"} · {e.action} · {e.description}
                    </bdi>
                  </p>
                  <p>
                    {t("account:statements.daily.reference", { page: e.page, line: e.line })} ·{" "}
                    <bdi>{e.quantity ?? "—"}</bdi> ·{" "}
                    {e.cash !== null && (
                      <PrivacyAmount value={Number(e.cash)} currency={currency} />
                    )}
                  </p>
                  {!["sweep", "pending"].includes(e.section) &&
                    ["BUY", "SELL", "TRANSFER_IN", "TRANSFER_OUT"].includes(e.action) &&
                    selectAsset(r, index, `activity:${e.id}`, e.security ?? e.description)}
                </div>
              ))}
            </div>
          </details>
        </section>
      ))}
      {!!reviews.length && (
        <div className="flex flex-wrap gap-2">
          <Button
            disabled={busy || reviews.some((r) => r.draft.issues.length > 0)}
            onClick={() => void calculate()}
          >
            {t("account:statements.daily.preview")}
          </Button>
          <Button
            variant="outline"
            disabled={busy || reviews.some((r) => r.draft.issues.length > 0)}
            onClick={() => void calculate(true)}
          >
            {t("account:statements.daily.refresh")}
          </Button>
          <Button
            variant="outline"
            disabled={busy}
            onClick={() => {
              clearPreview();
              setReviews([]);
            }}
          >
            {t("common:cancel")}
          </Button>
        </div>
      )}
      {refreshFailures !== undefined && (
        <p role="status">
          {t("account:statements.daily.refreshResult", { count: refreshFailures })}
        </p>
      )}
      {previews.map((p) => (
        <div className="space-y-2" key={p.priced.through}>
          <p>{t(`account:statements.daily.handoffs.${p.liveHandoff}`)}</p>
          <div className="max-h-72 overflow-auto">
            <table className="w-full text-start text-sm">
              <thead>
                <tr>
                  <th>{t("account:statements.daily.date")}</th>
                  <th>{t("account:statements.daily.value")}</th>
                  <th>{t("account:statements.daily.quality")}</th>
                </tr>
              </thead>
              <tbody>
                {p.priced.days.map((d) => (
                  <tr key={d.date}>
                    <td>{dates.formatCalendarDate(d.date)}</td>
                    <td>
                      {d.value === null ? (
                        "—"
                      ) : (
                        <PrivacyAmount value={Number(d.value)} currency={currency} />
                      )}
                    </td>
                    <td>{t(`account:statements.daily.qualities.${d.quality}`)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {p.priced.days.some((d) => d.value === null) && (
            <p>{t("account:statements.daily.fallback")}</p>
          )}
        </div>
      ))}
      {!!previews.length && (
        <>
          <label className="flex items-start gap-2">
            <input
              type="checkbox"
              checked={confirmed}
              disabled={busy}
              onChange={(e) => setConfirmed(e.target.checked)}
            />
            {t("account:statements.daily.confirm")}
          </label>
          <Button disabled={busy || !confirmed} onClick={() => void save()}>
            {t("common:save")}
          </Button>
        </>
      )}
      {saved?.some(
        (s, i) =>
          i > 0 &&
          new Date(s.interval.from).getTime() - new Date(saved[i - 1].interval.through).getTime() >
            86_400_000,
      ) && <p role="status">{t("account:statements.daily.gaps")}</p>}
      {saved?.map((s) => (
        <div key={s.revision}>
          <p>
            {t("account:statements.daily.saved", {
              from: dates.formatCalendarDate(s.interval.from),
              through: dates.formatCalendarDate(s.interval.through),
            })}
          </p>
          <p>{t(`account:statements.daily.handoffs.${s.liveHandoff ?? "unverified"}`)}</p>
          <p>
            {t("account:statements.daily.coverage", {
              priced: s.priced?.days.filter((d) => d.value !== null).length ?? 0,
              total:
                s.priced?.days.length ??
                Math.round(
                  (new Date(s.interval.through).getTime() - new Date(s.interval.from).getTime()) /
                    86_400_000,
                ) + 1,
            })}
          </p>
          <Button
            variant="outline"
            disabled={busy}
            onClick={() => {
              void (async () => {
                setBusy(true);
                setError(undefined);
                try {
                  const monthly = await listStatements(accountId);
                  const snapshot = monthly.find((m) => m.snapshot.date === s.interval.through);
                  if (!snapshot) throw new Error("changed");
                  const failures = await refreshStatementQuotes(accountId, [
                    {
                      interval: s.interval,
                      closingAssetIds: s.closingAssetIds,
                      snapshot: snapshot.snapshot,
                      expectedRevision: s.revision,
                      expectedSnapshotRevision: snapshot.revision,
                    },
                  ]);
                  setRefreshFailures(failures);
                  await invalidate();
                } catch {
                  setError(t("account:statements.daily.reviewError"));
                } finally {
                  setBusy(false);
                }
              })();
            }}
          >
            {t("account:statements.daily.refresh")}
          </Button>
        </div>
      ))}
      {!!saved?.length && (
        <p className="text-muted-foreground text-sm">{t("account:statements.daily.deleteHelp")}</p>
      )}
    </div>
  );
}
