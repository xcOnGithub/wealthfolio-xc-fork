import { listStatementActivities } from "@/adapters";
import { QueryKeys } from "@/lib/query-keys";
import { useInfiniteQuery } from "@tanstack/react-query";
import { Button, PrivacyAmount, useBalancePrivacy, useDateFormatting } from "@wealthfolio/ui";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@wealthfolio/ui/components/ui/table";
import { useId } from "react";
import { useTranslation } from "react-i18next";

export function useStatementActivities(accountId: string, enabled: boolean) {
  return useInfiniteQuery({
    queryKey: [QueryKeys.STATEMENT_ACTIVITIES, accountId],
    queryFn: ({ pageParam }) => listStatementActivities(accountId, pageParam),
    initialPageParam: 0,
    getNextPageParam: (page) => page.nextOffset ?? undefined,
    enabled,
  });
}

export function StatementActivitiesPanel({
  query,
  onAddActivity,
}: {
  query: ReturnType<typeof useStatementActivities>;
  onAddActivity?: () => void;
}) {
  const { t } = useTranslation();
  const { formatCalendarDate } = useDateFormatting();
  const { isBalanceHidden } = useBalancePrivacy();
  const titleId = useId();
  const rows = query.data?.pages.flatMap((page) => page.rows) ?? [];
  if (!query.isLoading && !query.isError && !rows.length) return null;

  return (
    <section aria-labelledby={titleId} className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <h3 id={titleId} className="font-medium">
            {t("account:statements.activities.title")}
          </h3>
          <p className="text-muted-foreground text-sm">{t("account:statements.activities.help")}</p>
        </div>
        {onAddActivity && rows.length > 0 && (
          <Button variant="outline" size="sm" onClick={onAddActivity}>
            {t("activity:add_activity")}
          </Button>
        )}
      </div>
      {query.isLoading && (
        <p role="status" className="text-sm">
          {t("common:loading")}
        </p>
      )}
      {rows.length > 0 && (
        <Table aria-labelledby={titleId}>
          <TableHeader>
            <TableRow>
              <TableHead>{t("common:date")}</TableHead>
              <TableHead>{t("activity:activity_details")}</TableHead>
              <TableHead className="text-end">{t("common:quantity")}</TableHead>
              <TableHead className="text-end">
                {t("account:statements.activities.netCash")}
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((row) => (
              <TableRow key={`${row.statementDate}:${row.sourceId}`}>
                <TableCell>
                  <time dateTime={row.date}>{formatCalendarDate(row.date)}</time>
                </TableCell>
                <TableCell>
                  {row.kind === "CASH"
                    ? t("account:statements.activities.cashMovement")
                    : t(`activity:type_${row.kind.toLowerCase()}`)}
                  {row.assetId && (
                    <p className="text-muted-foreground">
                      <bdi>{row.assetLabel ?? t("activity:type_unknown")}</bdi>
                    </p>
                  )}
                </TableCell>
                <TableCell className="text-end tabular-nums">
                  <bdi>{row.quantity === null ? "—" : isBalanceHidden ? "••••" : row.quantity}</bdi>
                </TableCell>
                <TableCell className="text-end tabular-nums">
                  <PrivacyAmount value={Number(row.cash)} currency={row.currency} />
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
      {query.isError && (
        <p role="alert" className="text-sm">
          {t("account:statements.activities.loadError")}
        </p>
      )}
      {(query.hasNextPage || query.isError) && (
        <Button
          variant="outline"
          size="sm"
          disabled={query.isFetching}
          onClick={() => {
            if (query.isError && !query.isFetchNextPageError) void query.refetch();
            else void query.fetchNextPage();
          }}
        >
          {t(
            query.isFetching
              ? "common:loading"
              : query.isError
                ? "common:retry"
                : "common:load_more",
          )}
        </Button>
      )}
    </section>
  );
}
