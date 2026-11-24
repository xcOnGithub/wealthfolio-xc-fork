import { useEffect, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { useMutation } from "@tanstack/react-query";
import {
  AmountDisplay,
  Badge,
  Button,
  Checkbox,
  Input,
  Page,
  PageContent,
  PageHeader,
  useDateFormatting,
  useNumberFormatting,
} from "@wealthfolio/ui";
import { Card, CardContent, CardHeader, CardTitle } from "@wealthfolio/ui/components/ui/card";
import { Textarea } from "@wealthfolio/ui/components/ui/textarea";
import { previewFamilyDigest } from "@/adapters";
import { previousFamilyMonth } from "@/features/family/calendar";
import { useFamily } from "@/features/family/use-family";
import { selectedProfileId } from "@/features/profiles/session";
import { useBalancePrivacy } from "@/hooks/use-balance-privacy";
import { useSettingsContext } from "@/lib/settings-provider";
import type {
  FamilyBulkItemAction,
  FamilyItem,
  FamilyItemAction,
  MonthlyFamilyDigest,
} from "@/features/family/types";
import { FamilySettingsPanel } from "./family-settings";

export function ItemCard({
  item,
  busy,
  onAction,
  selected = false,
  onToggleSelect,
}: {
  item: FamilyItem;
  busy: boolean;
  onAction: (action: FamilyItemAction) => void;
  selected?: boolean;
  onToggleSelect?: (shiftKey: boolean) => void;
}) {
  const { t } = useTranslation("family");
  const dates = useDateFormatting();
  const { isBalanceHidden } = useBalancePrivacy();
  const [note, setNote] = useState(item.note);
  const shiftPressedRef = useRef(false);
  return (
    <Card className={selected ? "border-primary/60 bg-accent/30" : undefined}>
      <CardContent className="flex items-start gap-3 p-4">
        {onToggleSelect && (
          <Checkbox
            className="mt-1"
            checked={selected}
            disabled={busy}
            aria-label={t("selectItem", { title: item.title })}
            onClick={(event) => {
              shiftPressedRef.current = event.shiftKey;
            }}
            onCheckedChange={() => {
              const shiftKey = shiftPressedRef.current;
              shiftPressedRef.current = false;
              onToggleSelect(shiftKey);
            }}
          />
        )}
        <div className="min-w-0 flex-1 space-y-3">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div className="min-w-0 space-y-1">
              <div className="flex flex-wrap items-center gap-2">
                <Badge variant="outline">
                  {t(`kind.${item.kind}`, item.kind.replaceAll("_", " "))}
                </Badge>
                <span className="text-muted-foreground text-xs">{t(`status.${item.status}`)}</span>
              </div>
              <h2 className="font-semibold">{item.title}</h2>
              <p className="text-muted-foreground text-sm break-words">
                {isBalanceHidden ? "••••" : item.detail}
              </p>
            </div>
            {item.amount != null && item.currency && (
              <AmountDisplay
                isHidden={isBalanceHidden}
                value={item.amount}
                currency={item.currency}
                className="font-semibold"
              />
            )}
          </div>
          {item.snoozedUntil && (
            <p className="text-muted-foreground text-sm">
              {t("snoozedUntil", { date: dates.formatDate(item.snoozedUntil) })}
            </p>
          )}
          <div className="flex flex-wrap gap-2">
            <Button variant="outline" size="sm" render={<Link to={item.href} />}>
              {t("review")}
            </Button>
            {item.status === "open" && (
              <Button
                variant="ghost"
                size="sm"
                disabled={busy}
                onClick={() => onAction({ id: item.id, action: "acknowledge" })}
              >
                {t("acknowledge")}
              </Button>
            )}
            {item.status !== "resolved" ? (
              <>
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={busy}
                  onClick={() =>
                    onAction({
                      id: item.id,
                      action: "snooze",
                      snoozedUntil: new Date(Date.now() + 7 * 86400000).toISOString(),
                    })
                  }
                >
                  {t("snooze")}
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={busy}
                  onClick={() => onAction({ id: item.id, action: "resolve" })}
                >
                  {t("resolve")}
                </Button>
              </>
            ) : item.active ? (
              <Button
                variant="ghost"
                size="sm"
                disabled={busy}
                onClick={() => onAction({ id: item.id, action: "reopen" })}
              >
                {t("reopen")}
              </Button>
            ) : null}
          </div>
          <details>
            <summary className="cursor-pointer text-sm">
              {t("familyNote")}
              {item.note ? " •" : ""}
            </summary>
            <div className="mt-3 space-y-2">
              <Textarea
                aria-label={t("familyNote")}
                maxLength={2000}
                value={note}
                onChange={(e) => setNote(e.target.value)}
              />
              <Button
                variant="outline"
                size="sm"
                disabled={busy}
                onClick={() => onAction({ id: item.id, action: "note", note })}
              >
                {t("saveNote")}
              </Button>
            </div>
          </details>
        </div>
      </CardContent>
    </Card>
  );
}
export function DigestCard({ digest }: { digest: MonthlyFamilyDigest }) {
  const { t } = useTranslation("family");
  const numbers = useNumberFormatting();
  const dates = useDateFormatting();
  const { isBalanceHidden } = useBalancePrivacy();
  return (
    <div className="space-y-5">
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        {(
          [
            ["netWorthChange", digest.netWorthChange],
            ["income", digest.income],
            ["spending", digest.spending],
          ] as const
        ).map(([label, value]) => (
          <Card key={label}>
            <CardContent className="space-y-2 p-4">
              <p className="text-muted-foreground text-sm">{t(label)}</p>
              <AmountDisplay
                isHidden={isBalanceHidden}
                value={value}
                currency={digest.currency}
                className="text-lg font-semibold"
              />
            </CardContent>
          </Card>
        ))}
        <Card>
          <CardContent className="space-y-2 p-4">
            <p className="text-muted-foreground text-sm">{t("savingsRate")}</p>
            <p className="text-lg font-semibold">
              {isBalanceHidden
                ? "••••"
                : digest.savingsRate == null
                  ? "—"
                  : numbers.formatPercent(digest.savingsRate)}
            </p>
          </CardContent>
        </Card>
      </div>
      <p className="text-muted-foreground text-sm">{t("savingsHelp")}</p>
      <Card>
        <CardHeader>
          <CardTitle>{t("spendingMovers")}</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          {digest.movers.length === 0 && (
            <p className="text-muted-foreground text-sm">{t("noComparisons")}</p>
          )}
          {digest.movers.map((m, index) => (
            <div key={`${m.name}-${index}`} className="flex justify-between gap-4 text-sm">
              <span>{m.name}</span>
              <span className="text-end">
                <AmountDisplay
                  isHidden={isBalanceHidden}
                  value={m.current}
                  currency={digest.currency}
                />{" "}
                ·{" "}
                <AmountDisplay
                  isHidden={isBalanceHidden}
                  value={m.change}
                  currency={digest.currency}
                  colorFormat
                />
              </span>
            </div>
          ))}
        </CardContent>
      </Card>
      <Card>
        <CardHeader>
          <CardTitle>{t("goals")}</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          {digest.goals.length === 0 && (
            <p className="text-muted-foreground text-sm">{t("noGoals")}</p>
          )}
          {digest.goals.map((g, index) => (
            <div key={`${g.title}-${index}`} className="flex justify-between gap-4 text-sm">
              <span>{g.title}</span>
              <span>
                {isBalanceHidden
                  ? "••••"
                  : g.progress == null
                    ? "—"
                    : numbers.formatPercent(g.progress)}
              </span>
            </div>
          ))}
        </CardContent>
      </Card>
      <Card>
        <CardHeader>
          <CardTitle>{t("upcomingExpenses")}</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          {digest.upcomingBills.length === 0 && (
            <p className="text-muted-foreground text-sm">{t("noBills")}</p>
          )}
          {digest.upcomingBills.map((bill) => (
            <div key={bill.id} className="flex justify-between gap-4 text-sm">
              <span>
                {bill.name} · {dates.formatCalendarDate(bill.dueDate)}
              </span>
              <AmountDisplay
                isHidden={isBalanceHidden}
                value={bill.amount}
                currency={bill.currency}
              />
            </div>
          ))}
        </CardContent>
      </Card>
      <Card>
        <CardContent className="space-y-3 p-4">
          <h2 className="font-semibold">{t("meetingPrompt")}</h2>
          <p className="text-muted-foreground text-sm">{t("meetingQuestions")}</p>
          <p className="text-muted-foreground text-sm">{t("scheduleHelp")}</p>
        </CardContent>
      </Card>
      {digest.warnings.length > 0 && (
        <div role="status" className="space-y-2 text-sm">
          {digest.warnings.map((warning) => (
            <p key={warning}>{warning}</p>
          ))}
        </div>
      )}
    </div>
  );
}
export default function FamilyPage() {
  const { t } = useTranslation("family");
  const family = useFamily();
  const { settings: appSettings } = useSettingsContext();
  const [params, setParams] = useSearchParams();
  const tab = params.get("tab") ?? "inbox";
  const [filter, setFilter] = useState("attention");
  const [limit, setLimit] = useState(100);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(() => new Set());
  const [lastSelectedId, setLastSelectedId] = useState<string | null>(null);
  const [month, setMonth] = useState(() => previousFamilyMonth(new Date(), appSettings?.timezone));
  const preview = useMutation({ mutationFn: previewFamilyDigest });
  const profile = selectedProfileId();
  const refresh = family.refresh.mutate;
  const resetPreview = preview.reset;
  useEffect(() => {
    setSelectedIds(new Set());
    setLastSelectedId(null);
    resetPreview();
    refresh();
  }, [refresh, resetPreview, profile]);
  const data = family.data;
  const digest = preview.data?.month === month ? preview.data : data?.digests[month];
  const items = (data?.items ?? [])
    .filter(
      (i) =>
        filter === "all" ||
        (filter === "attention"
          ? i.active && (i.status === "open" || i.status === "acknowledged")
          : i.status === filter),
    )
    .sort((a, b) => b.firstSeen.localeCompare(a.firstSeen));
  const visibleItems = items.slice(0, limit);
  const selectedItems = items.filter((item) => selectedIds.has(item.id));
  const selectedVisibleCount = visibleItems.filter((item) => selectedIds.has(item.id)).length;
  const allVisibleSelected =
    visibleItems.length > 0 && selectedVisibleCount === visibleItems.length;
  const someVisibleSelected = selectedVisibleCount > 0 && !allVisibleSelected;
  const allFilteredSelected = items.length > 0 && selectedItems.length === items.length;
  const acknowledgeIds = selectedItems
    .filter((item) => item.status === "open")
    .map((item) => item.id);
  const snoozeOrResolveIds = selectedItems
    .filter((item) => item.status !== "resolved")
    .map((item) => item.id);
  const reopenIds = selectedItems
    .filter((item) => item.status === "resolved" && item.active)
    .map((item) => item.id);
  const itemBusy = family.action.isPending || family.bulkAction.isPending;
  const clearSelection = () => {
    setSelectedIds(new Set());
    setLastSelectedId(null);
  };
  const toggleItemSelection = (itemId: string, shiftKey: boolean) => {
    setSelectedIds((previous) => {
      const next = new Set(previous);
      const willSelect = !next.has(itemId);
      if (shiftKey && lastSelectedId) {
        const start = visibleItems.findIndex((item) => item.id === lastSelectedId);
        const end = visibleItems.findIndex((item) => item.id === itemId);
        if (start !== -1 && end !== -1) {
          const [from, to] = start <= end ? [start, end] : [end, start];
          for (let index = from; index <= to; index += 1) {
            const id = visibleItems[index].id;
            if (willSelect) {
              next.add(id);
            } else {
              next.delete(id);
            }
          }
          return next;
        }
      }
      if (willSelect) {
        next.add(itemId);
      } else {
        next.delete(itemId);
      }
      return next;
    });
    setLastSelectedId(itemId);
  };
  const toggleSelectAllVisible = () => {
    setLastSelectedId(null);
    if (allVisibleSelected) {
      setSelectedIds(new Set());
    } else {
      setSelectedIds(new Set(visibleItems.map((item) => item.id)));
    }
  };
  const runBulkAction = (input: FamilyBulkItemAction) => {
    if (input.ids.length === 0) return;
    family.bulkAction.mutate(input, {
      onSuccess: () => {
        clearSelection();
      },
    });
  };
  const error =
    family.isError ||
    family.refresh.isError ||
    family.action.isError ||
    family.bulkAction.isError ||
    family.retry.isError;
  return (
    <Page>
      <PageHeader
        heading={t("title")}
        text={t("subtitle")}
        actions={
          <Button variant="outline" disabled={family.refresh.isPending} onClick={() => refresh()}>
            {family.refresh.isPending ? t("refreshing") : t("refresh")}
          </Button>
        }
      />
      <PageContent className="space-y-6 pt-4">
        <nav className="flex flex-wrap gap-2" aria-label={t("navigation")}>
          {(["inbox", "digest", "settings"] as const).map((value) => (
            <Button
              key={value}
              variant={tab === value ? "default" : "outline"}
              onClick={() => {
                if (value !== tab) clearSelection();
                setParams((previous) => {
                  const next = new URLSearchParams(previous);
                  next.set("tab", value);
                  return next;
                });
              }}
            >
              {t(value)}
            </Button>
          ))}
        </nav>
        {error && (
          <p role="alert" className="text-destructive text-sm">
            {t("loadError")}
          </p>
        )}
        {!data && <p role="status">{t("loading")}</p>}
        {data && (
          <>
            {!data.settings.enabled && (
              <p role="status" className="text-muted-foreground text-sm">
                {t("disabled")}
              </p>
            )}
            {data.warnings.length > 0 && (
              <details className="text-sm">
                <summary>{t("dataNotes", { count: data.warnings.length })}</summary>
                <div className="mt-2 space-y-2">
                  {data.warnings.map((w) => (
                    <p key={w}>{w}</p>
                  ))}
                </div>
              </details>
            )}
            {tab === "inbox" && (
              <section className="space-y-4" aria-label={t("inbox")}>
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <h2 className="font-semibold">{t("needsAttention", { count: items.length })}</h2>
                  <label className="text-sm">
                    <span className="sr-only">{t("filter")}</span>
                    <select
                      className="border-input bg-background rounded-md border p-2"
                      value={filter}
                      onChange={(e) => {
                        setFilter(e.target.value);
                        setLimit(100);
                        clearSelection();
                      }}
                      aria-label={t("filter")}
                    >
                      {(["attention", "snoozed", "resolved", "all"] as const).map((value) => (
                        <option key={value} value={value}>
                          {t(`filterValues.${value}`)}
                        </option>
                      ))}
                    </select>
                  </label>
                </div>
                <p className="text-muted-foreground text-sm">{t("resolveHelp")}</p>
                {items.length > 0 && (
                  <div
                    role="region"
                    aria-label={t("bulkActions")}
                    className={
                      selectedItems.length > 0
                        ? "bg-card/95 border-border sticky top-2 z-10 flex flex-wrap items-center justify-between gap-3 rounded-lg border px-4 py-2.5 shadow-sm backdrop-blur"
                        : "flex flex-wrap items-center justify-between gap-3 px-1 py-1"
                    }
                  >
                    <div className="flex flex-wrap items-center gap-3">
                      <label className="flex cursor-pointer items-center gap-2 text-sm">
                        <Checkbox
                          checked={allVisibleSelected}
                          indeterminate={someVisibleSelected}
                          disabled={itemBusy}
                          onCheckedChange={toggleSelectAllVisible}
                          aria-label={t("selectAll")}
                        />
                        <span className="font-medium">
                          {selectedItems.length > 0
                            ? t("selectedCount", { count: selectedItems.length })
                            : t("selectAll")}
                        </span>
                      </label>
                      {allVisibleSelected && !allFilteredSelected && (
                        <Button
                          variant="link"
                          size="sm"
                          className="h-auto p-0"
                          disabled={itemBusy}
                          onClick={() => setSelectedIds(new Set(items.map((item) => item.id)))}
                        >
                          {t("selectAllMatching", { count: items.length })}
                        </Button>
                      )}
                    </div>
                    {selectedItems.length > 0 && (
                      <div className="flex flex-wrap items-center gap-2">
                        {acknowledgeIds.length > 0 && (
                          <Button
                            variant="outline"
                            size="sm"
                            disabled={itemBusy}
                            onClick={() =>
                              runBulkAction({ ids: acknowledgeIds, action: "acknowledge" })
                            }
                          >
                            {t("acknowledge")}
                          </Button>
                        )}
                        {snoozeOrResolveIds.length > 0 && (
                          <>
                            <Button
                              variant="outline"
                              size="sm"
                              disabled={itemBusy}
                              onClick={() =>
                                runBulkAction({
                                  ids: snoozeOrResolveIds,
                                  action: "snooze",
                                  snoozedUntil: new Date(Date.now() + 7 * 86400000).toISOString(),
                                })
                              }
                            >
                              {t("snooze")}
                            </Button>
                            <Button
                              variant="default"
                              size="sm"
                              disabled={itemBusy}
                              onClick={() =>
                                runBulkAction({ ids: snoozeOrResolveIds, action: "resolve" })
                              }
                            >
                              {t("resolve")}
                            </Button>
                          </>
                        )}
                        {reopenIds.length > 0 && (
                          <Button
                            variant="outline"
                            size="sm"
                            disabled={itemBusy}
                            onClick={() => runBulkAction({ ids: reopenIds, action: "reopen" })}
                          >
                            {t("reopen")}
                          </Button>
                        )}
                        <Button
                          variant="ghost"
                          size="sm"
                          disabled={itemBusy}
                          onClick={clearSelection}
                        >
                          {t("clearSelection")}
                        </Button>
                      </div>
                    )}
                  </div>
                )}
                {items.length === 0 && (
                  <Card>
                    <CardContent className="space-y-2 p-6">
                      <h3 className="font-semibold">{t("allClear")}</h3>
                      <p className="text-muted-foreground text-sm">{t("allClearHelp")}</p>
                    </CardContent>
                  </Card>
                )}
                {visibleItems.map((item) => (
                  <ItemCard
                    key={item.id}
                    item={item}
                    busy={itemBusy}
                    selected={selectedIds.has(item.id)}
                    onToggleSelect={(shiftKey) => toggleItemSelection(item.id, shiftKey)}
                    onAction={(input) => family.action.mutate(input)}
                  />
                ))}
                {items.length > limit && (
                  <Button variant="outline" onClick={() => setLimit((n) => n + 100)}>
                    {t("showMore")}
                  </Button>
                )}
              </section>
            )}
            {tab === "digest" && (
              <section className="space-y-4" aria-label={t("digest")}>
                <div className="flex flex-wrap items-end gap-3">
                  <label className="space-y-1 text-sm">
                    <span>{t("month")}</span>
                    <Input
                      type="month"
                      max={previousFamilyMonth(new Date(), appSettings?.timezone)}
                      value={month}
                      onChange={(e) => setMonth(e.target.value)}
                    />
                  </label>
                  <Button
                    variant="outline"
                    disabled={!month || preview.isPending}
                    onClick={() => preview.mutate(month)}
                  >
                    {preview.isPending ? t("loading") : t("preview")}
                  </Button>
                </div>
                {preview.isError && (
                  <p role="alert" className="text-destructive text-sm">
                    {t("loadError")}
                  </p>
                )}
                {digest ? (
                  <DigestCard digest={digest} />
                ) : (
                  <p className="text-muted-foreground text-sm">{t("previewHelp")}</p>
                )}
              </section>
            )}
            {tab === "settings" && (
              <div className="space-y-6">
                {family.settings.isSuccess && <p role="status">{t("saved")}</p>}
                <FamilySettingsPanel
                  key={`${profile}-${data.configRevision}`}
                  snapshot={data}
                  currency={appSettings?.baseCurrency ?? "USD"}
                  busy={family.settings.isPending}
                  onSave={family.settings.mutateAsync}
                />
                <Card>
                  <CardHeader>
                    <CardTitle>{t("deliveryHistory")}</CardTitle>
                  </CardHeader>
                  <CardContent className="space-y-4">
                    <p className="text-muted-foreground text-sm">{t("retryHelp")}</p>
                    {data.deliveries.length === 0 && (
                      <p className="text-muted-foreground text-sm">{t("noDeliveries")}</p>
                    )}
                    {[...data.deliveries]
                      .reverse()
                      .slice(0, 50)
                      .map((d) => (
                        <div
                          key={d.id}
                          className="flex flex-wrap items-center justify-between gap-3 border-b pb-3 text-sm"
                        >
                          <div>
                            <p className="font-medium">{d.notification.subject}</p>
                            <p className="text-muted-foreground">
                              {d.channel} · {t(`deliveryStatus.${d.status}`)}
                            </p>
                            {d.error && (
                              <p className="text-muted-foreground">{t("deliveryError")}</p>
                            )}
                          </div>
                          {d.configRevision === data.configRevision &&
                            ["failed", "sending"].includes(d.status) && (
                              <Button
                                variant="outline"
                                size="sm"
                                disabled={family.retry.isPending}
                                onClick={() => family.retry.mutate(d.id)}
                              >
                                {t("retry")}
                              </Button>
                            )}
                        </div>
                      ))}
                  </CardContent>
                </Card>
              </div>
            )}
          </>
        )}
      </PageContent>
    </Page>
  );
}
