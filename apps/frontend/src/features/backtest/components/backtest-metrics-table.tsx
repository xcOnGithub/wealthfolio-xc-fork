import type { BacktestResponse, BacktestSeriesResult } from "@/lib/types/backtest";
import {
  Badge,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  useAmountFormatting,
  useDateFormatting,
  useNumberFormatting,
} from "@wealthfolio/ui";
import { useTranslation } from "react-i18next";
import { formatBacktestSeriesLabel } from "../formatters";

interface BacktestMetricsTableProps {
  result: BacktestResponse;
}

export function BacktestMetricsTable({ result }: BacktestMetricsTableProps) {
  const { t } = useTranslation(["backtest"]);
  const { formatAmount } = useAmountFormatting();
  const { formatPercent, formatDecimal } = useNumberFormatting();
  const { formatDate } = useDateFormatting();

  const columns: BacktestSeriesResult[] = [
    result.strategy,
    ...(result.buyAndHold ? [result.buyAndHold] : []),
    ...(result.benchmark ? [result.benchmark] : []),
  ];

  const primary = result.strategy.metrics;

  const formatRatio = (val: number | null) =>
    val == null ? "—" : formatDecimal(val, { minimumFractionDigits: 2, maximumFractionDigits: 2 });

  const formatYearMetric = (item: { year: number; returnPct: number } | null) => {
    if (!item) return "—";
    return `${item.year} (${formatPercent(item.returnPct, { digits: 1, signDisplay: "exceptZero" })})`;
  };

  const rows: {
    key: string;
    label: string;
    render: (series: BacktestSeriesResult) => string;
  }[] = [
    {
      key: "finalValue",
      label: t("backtest:metrics.final_value"),
      render: (s) => formatAmount(s.metrics.finalValue, "USD"),
    },
    {
      key: "totalInvested",
      label: t("backtest:metrics.total_invested"),
      render: (s) => formatAmount(s.metrics.totalInvested, "USD"),
    },
    {
      key: "totalProfit",
      label: t("backtest:metrics.net_profit"),
      render: (s) => formatAmount(s.metrics.totalProfit, "USD"),
    },
    {
      key: "totalReturn",
      label: t("backtest:metrics.total_return"),
      render: (s) => formatPercent(s.metrics.totalReturn, { digits: 2, signDisplay: "exceptZero" }),
    },
    {
      key: "twr",
      label: t("backtest:metrics.twr"),
      render: (s) => formatPercent(s.metrics.twr, { digits: 2, signDisplay: "exceptZero" }),
    },
    {
      key: "cagr",
      label: t("backtest:metrics.cagr"),
      render: (s) =>
        s.metrics.cagr == null
          ? "—"
          : formatPercent(s.metrics.cagr, { digits: 2, signDisplay: "exceptZero" }),
    },
    {
      key: "maxDrawdown",
      label: t("backtest:metrics.max_drawdown"),
      render: (s) => formatPercent(s.metrics.maxDrawdown, { digits: 2 }),
    },
    {
      key: "volatility",
      label: t("backtest:metrics.volatility"),
      render: (s) =>
        s.metrics.annualizedVolatility == null
          ? "—"
          : formatPercent(s.metrics.annualizedVolatility, { digits: 2 }),
    },
    {
      key: "sharpe",
      label: t("backtest:metrics.sharpe_ratio"),
      render: (s) => formatRatio(s.metrics.sharpeRatio),
    },
    {
      key: "sortino",
      label: t("backtest:metrics.sortino_ratio"),
      render: (s) => formatRatio(s.metrics.sortinoRatio),
    },
    {
      key: "bestYear",
      label: t("backtest:metrics.best_year"),
      render: (s) => formatYearMetric(s.metrics.bestYear),
    },
    {
      key: "worstYear",
      label: t("backtest:metrics.worst_year"),
      render: (s) => formatYearMetric(s.metrics.worstYear),
    },
    {
      key: "dividends",
      label: t("backtest:metrics.total_dividends"),
      render: (s) => formatAmount(s.metrics.totalDividends, "USD"),
    },
    {
      key: "fees",
      label: t("backtest:metrics.total_fees"),
      render: (s) => formatAmount(s.metrics.totalFees, "USD"),
    },
    {
      key: "rebalances",
      label: t("backtest:metrics.rebalances"),
      render: (s) => String(s.metrics.rebalanceCount),
    },
    {
      key: "turnover",
      label: t("backtest:metrics.annualized_turnover"),
      render: (s) => formatPercent(s.metrics.annualizedTurnover, { digits: 1 }),
    },
  ];

  return (
    <div className="space-y-4">
      {/* Headline KPI Cards */}
      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
        <Card>
          <CardHeader className="pb-1.5">
            <CardDescription>{t("backtest:metrics.final_value")}</CardDescription>
            <CardTitle className="text-xl tabular-nums">
              {formatAmount(primary.finalValue, "USD")}
            </CardTitle>
          </CardHeader>
          <CardContent className="text-muted-foreground text-xs tabular-nums">
            {t("backtest:metrics.invested_caption", {
              amount: formatAmount(primary.totalInvested, "USD"),
            })}
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-1.5">
            <CardDescription>{t("backtest:metrics.cagr")}</CardDescription>
            <CardTitle className="text-xl tabular-nums">
              {primary.cagr == null
                ? "—"
                : formatPercent(primary.cagr, { digits: 2, signDisplay: "exceptZero" })}
            </CardTitle>
          </CardHeader>
          <CardContent className="text-muted-foreground text-xs tabular-nums">
            {t("backtest:metrics.twr_caption", {
              value: formatPercent(primary.twr, { digits: 2, signDisplay: "exceptZero" }),
            })}
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-1.5">
            <CardDescription>{t("backtest:metrics.max_drawdown")}</CardDescription>
            <CardTitle className="text-destructive text-xl tabular-nums">
              {formatPercent(primary.maxDrawdown, { digits: 2 })}
            </CardTitle>
          </CardHeader>
          <CardContent className="text-muted-foreground text-xs">
            {primary.maxDrawdownPeakDate && primary.maxDrawdownTroughDate
              ? `${formatDate(primary.maxDrawdownPeakDate)} → ${formatDate(primary.maxDrawdownTroughDate)}`
              : "—"}
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-1.5">
            <CardDescription>{t("backtest:metrics.sharpe_ratio")}</CardDescription>
            <CardTitle className="text-xl tabular-nums">
              {formatRatio(primary.sharpeRatio)}
            </CardTitle>
          </CardHeader>
          <CardContent className="text-muted-foreground text-xs tabular-nums">
            {t("backtest:metrics.volatility_caption", {
              value:
                primary.annualizedVolatility == null
                  ? "—"
                  : formatPercent(primary.annualizedVolatility, { digits: 2 }),
            })}
          </CardContent>
        </Card>
      </div>

      {/* Detailed Comparison Table */}
      <Card>
        <CardHeader>
          <CardTitle className="text-base">{t("backtest:metrics.comparison_title")}</CardTitle>
          <CardDescription>{t("backtest:metrics.comparison_description")}</CardDescription>
        </CardHeader>
        <CardContent className="overflow-x-auto">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t("backtest:metrics.metric_column")}</TableHead>
                {columns.map((col, idx) => (
                  <TableHead key={`${col.descriptor.kind}-${idx}`} className="text-end">
                    {formatBacktestSeriesLabel(col.descriptor, t)}
                  </TableHead>
                ))}
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((row) => (
                <TableRow key={row.key}>
                  <TableCell className="text-muted-foreground font-medium">{row.label}</TableCell>
                  {columns.map((col, idx) => (
                    <TableCell key={`${row.key}-${idx}`} className="text-end tabular-nums">
                      {row.render(col)}
                    </TableCell>
                  ))}
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </CardContent>
      </Card>

      {/* Ending Allocation & Data Coverage */}
      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle className="text-base">
              {t("backtest:metrics.ending_holdings_title")}
            </CardTitle>
            <CardDescription>
              {t("backtest:metrics.ending_holdings_description", {
                date: formatDate(result.effectiveEndDate),
              })}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>{t("backtest:fields.ticker")}</TableHead>
                  <TableHead className="text-end">{t("backtest:fields.weight")}</TableHead>
                  <TableHead className="text-end">{t("backtest:metrics.ending_value")}</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {result.strategy.endingHoldings.map((item) => (
                  <TableRow key={item.isCash ? "__cash__" : item.symbol}>
                    <TableCell className="font-medium">
                      {item.isCash ? t("backtest:holdings.usd_cash") : item.symbol}
                    </TableCell>
                    <TableCell className="text-end tabular-nums">
                      {formatPercent(item.weight, { digits: 1 })}
                    </TableCell>
                    <TableCell className="text-end tabular-nums">
                      {formatAmount(item.value, "USD")}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="text-base">{t("backtest:coverage.title")}</CardTitle>
            <CardDescription>
              {t("backtest:coverage.description", {
                start: formatDate(result.effectiveStartDate),
                end: formatDate(result.effectiveEndDate),
                days: result.tradingDays,
              })}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>{t("backtest:fields.ticker")}</TableHead>
                  <TableHead>{t("backtest:coverage.range")}</TableHead>
                  <TableHead className="text-end">{t("backtest:coverage.dividends")}</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {result.symbolCoverage.map((cov) => (
                  <TableRow
                    key={`${cov.symbol}-${cov.exchangeMic ?? ""}-${cov.isBenchmark ? "bench" : "req"}`}
                  >
                    <TableCell className="font-medium">
                      <div className="flex flex-wrap items-center gap-1.5">
                        <span>{cov.symbol}</span>
                        {cov.exchangeMic && (
                          <Badge variant="outline" className="font-mono text-xs">
                            {cov.exchangeMic}
                          </Badge>
                        )}
                        {cov.isBenchmark && (
                          <Badge variant="secondary" className="text-xs">
                            {t("backtest:fields.benchmark")}
                          </Badge>
                        )}
                      </div>
                    </TableCell>
                    <TableCell className="text-muted-foreground text-xs tabular-nums">
                      {formatDate(cov.firstDate)} – {formatDate(cov.lastDate)}
                    </TableCell>
                    <TableCell className="text-end">
                      {cov.dividendsAvailable ? (
                        <Badge variant="secondary" className="tabular-nums">
                          {cov.dividendEventsCount}
                        </Badge>
                      ) : (
                        <Badge variant="outline">{t("backtest:coverage.unavailable")}</Badge>
                      )}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
