import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  Button,
  PrivacyAmount,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  useDateFormatting,
} from "@wealthfolio/ui";
import {
  getAssets,
  listDailyStatements,
  listStatements,
  previewDailyStatements,
  saveDailyStatements,
  refreshStatementQuotes,
  registerStatementSecurities,
} from "@/adapters";
import type { DailyStatementPreview, DailyStatementWrite } from "@/adapters/shared/statements";
import { readBasisFile, validateBasisFiles } from "./statement-pdf";
import { QueryKeys } from "@/lib/query-keys";
import { MAX_STATEMENT_BATCH, MAX_STATEMENT_PAGES } from "./merrill-format";
import type { StatementHistoryDraft } from "./merrill-history";
import { extractStatementHistories } from "./statement-parser";
import { dailyStatementWrite } from "./daily-review";
import {
  activitySecurityLabel,
  resolveReviewLinks,
  reviewedSecurityMatches,
  securityReviewGroups,
  statementTickerLinks,
} from "./security-review-matches";

interface Review {
  draft: StatementHistoryDraft;
  name: string;
  groups: Record<string, string>;
  savedLinks: Record<string, string>;
  suggestedLinks?: Record<string, string>;
  edits: Record<string, string>;
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
  const {
    data: assets,
    isError: assetError,
    isPending: assetsPending,
  } = useQuery({
    queryKey: [QueryKeys.ASSETS],
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
  const reviewLinks = useMemo(() => resolveReviewLinks(reviews), [reviews]);
  const [previews, setPreviews] = useState<DailyStatementPreview[]>([]);
  const [previewWrites, setPreviewWrites] = useState<DailyStatementWrite[]>([]);
  const [busy, setBusy] = useState(false);
  const [confirmed, setConfirmed] = useState(false);
  const [error, setError] = useState<string>();
  const [registrationError, setRegistrationError] = useState(false);
  const [refreshFailures, setRefreshFailures] = useState<number>();
  const generation = useRef(0);
  const available = assets?.filter(
    (a) => a.kind === "INVESTMENT" && a.instrumentType === "EQUITY" && a.quoteCcy === "USD",
  );
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
      client.invalidateQueries({ queryKey: [QueryKeys.STATEMENT_ACTIVITIES, accountId] }),
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
    setRegistrationError(false);
    setBusy(true);
    try {
      validateBasisFiles(files);
      if (files.some((f) => !f.name.toLowerCase().endsWith(".pdf"))) throw new Error("format");
      const [daily, monthly] = await Promise.all([
        listDailyStatements(accountId),
        listStatements(accountId),
      ]);
      const preselect = reviewedSecurityMatches(
        daily,
        monthly,
        new Set(available?.map((a) => a.id)),
      );
      const batch: Review[] = [];
      let identity: string | undefined;
      let chars = 0;
      let pageCount = 0;
      for (const file of files) {
        let parsed: ReturnType<typeof extractStatementHistories> | undefined;
        await readBasisFile(
          file,
          (pages) => {
            pageCount += pages.length;
            chars += pages.reduce((n, p) => n + p.reduce((m, i) => m + i.str.length, 0), 0);
            if (chars > 1_000_000 || pageCount > MAX_STATEMENT_PAGES) throw new Error("size");
            parsed = extractStatementHistories(pages);
            return "";
          },
          { maxPages: MAX_STATEMENT_PAGES },
        );
        if (current !== generation.current) return;
        if (!parsed?.length) throw new Error("account");
        for (const item of parsed) {
          if (item.draft.closing.currency !== currency || (identity && identity !== item.identity))
            throw new Error("account");
          identity = item.identity;
          if (batch.some((r) => r.draft.through === item.draft.through))
            throw new Error("duplicate");
          if (batch.length >= MAX_STATEMENT_BATCH) throw new Error("size");
          batch.push({
            draft: item.draft,
            name: file.name,
            groups: securityReviewGroups(item.draft),
            savedLinks: preselect(item.draft),
            edits: {},
            revision:
              daily.find((s) => s.interval.through === item.draft.through)?.revision ?? null,
            snapshotRevision:
              monthly.find((s) => s.snapshot.date === item.draft.through)?.revision ?? null,
          });
        }
      }
      const knownLinks = resolveReviewLinks(batch);
      const tickerLinks = batch.map(({ draft }, index) =>
        Object.entries(statementTickerLinks(draft)).filter(
          ([row]) => knownLinks[index][row] === undefined,
        ),
      );
      const tickers = [
        ...new Set(tickerLinks.flatMap((links) => links.map(([, ticker]) => ticker))),
      ];
      const resolved: Awaited<ReturnType<typeof registerStatementSecurities>> = tickers.length
        ? await registerStatementSecurities(accountId, tickers).catch(() => {
            if (current === generation.current) setRegistrationError(true);
            return {};
          })
        : {};
      const registered = Object.values(resolved);
      if (registered.length) {
        // Make the new options available in this review before the catalog refetch finishes.
        client.setQueryData([QueryKeys.ASSETS], (previous: Awaited<ReturnType<typeof getAssets>>) =>
          Array.from(new Map([...(previous ?? []), ...registered].map((a) => [a.id, a])).values()),
        );
        void client.invalidateQueries({ queryKey: [QueryKeys.ASSETS] });
      }
      if (current !== generation.current) return;
      setReviews(
        batch.map((review, i) => ({
          ...review,
          suggestedLinks: Object.fromEntries(
            tickerLinks[i].flatMap(([row, ticker]) =>
              resolved[ticker] ? [[row, resolved[ticker].id]] : [],
            ),
          ),
        })),
      );
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
      const writes = reviews.map((r, index) =>
        dailyStatementWrite(r.draft, reviewLinks[index], r.revision, r.snapshotRevision),
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
  const selectAsset = (index: number, key: string, label: string) => (
    <label className="grid gap-1" key={key}>
      <bdi>{label}</bdi>
      <select
        className="bg-background w-full rounded border p-2"
        value={reviewLinks[index][key] ?? ""}
        disabled={busy}
        onChange={(e) => {
          const assetId = e.currentTarget.value;
          clearPreview();
          setReviews((old) =>
            old.map((r, i) => (i === index ? { ...r, edits: { ...r.edits, [key]: assetId } } : r)),
          );
        }}
      >
        <option value="">{t("account:statements.daily.selectAsset")}</option>
        {available?.map((a) => (
          <option key={a.id} value={a.id}>
            {a.displayCode ?? a.instrumentSymbol ?? a.id}
            {a.name ? ` · ${a.name}` : ""}
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
      {registrationError && reviews.length > 0 && (
        <p role="alert">{t("account:statements.daily.registrationError")}</p>
      )}
      <p className="text-muted-foreground text-sm">
        {t("account:statements.daily.registrationHelp")}
      </p>
      <label className="grid gap-2">
        {t("account:statements.upload")}
        <input
          type="file"
          accept=".pdf,application/pdf"
          multiple
          disabled={busy || isPending || isError || assetsPending || assetError}
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
            selectAsset(index, `holding:${i}`, `${p.label} · ${p.quantity}`),
          )}
          <details>
            <summary>{t("account:statements.daily.events")}</summary>
            <div className="space-y-2">
              {r.draft.evidence.activities.map((e) => {
                const security = activitySecurityLabel(e);
                return (
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
                    {security !== undefined && selectAsset(index, `activity:${e.id}`, security)}
                  </div>
                );
              })}
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
          <Table containerClassName="max-h-72 overflow-auto">
            <TableHeader>
              <TableRow>
                <TableHead className="h-9 px-3">{t("account:statements.daily.date")}</TableHead>
                <TableHead className="h-9 px-3">{t("account:statements.daily.value")}</TableHead>
                <TableHead className="h-9 px-3">{t("account:statements.daily.quality")}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {p.priced.days.map((d) => (
                <TableRow key={d.date}>
                  <TableCell className="px-3 py-2">{dates.formatCalendarDate(d.date)}</TableCell>
                  <TableCell className="px-3 py-2 tabular-nums">
                    {d.value === null ? (
                      "—"
                    ) : (
                      <PrivacyAmount value={Number(d.value)} currency={currency} />
                    )}
                  </TableCell>
                  <TableCell className="px-3 py-2">
                    {t(`account:statements.daily.qualities.${d.quality}`)}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
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
