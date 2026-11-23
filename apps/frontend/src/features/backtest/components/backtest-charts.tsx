import type { BacktestResponse, BacktestSeriesResult } from "@/lib/types/backtest";
import {
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  useAmountFormatting,
  useDateFormatting,
  useNumberFormatting,
} from "@wealthfolio/ui";
import { ChartConfig, ChartContainer, ChartTooltip } from "@wealthfolio/ui/components/ui/chart";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Area,
  Bar,
  BarChart,
  CartesianGrid,
  ComposedChart,
  Line,
  ReferenceLine,
  ResponsiveContainer,
  XAxis,
  YAxis,
} from "recharts";
import { formatBacktestSeriesLabel } from "../formatters";

const SERIES_COLORS = {
  strategy: "var(--chart-1)",
  buyAndHold: "var(--chart-2)",
  benchmark: "var(--chart-3)",
  invested: "var(--muted-foreground)",
} as const;

const MAX_CHART_POINTS = 320;
const noVerticalGridPoints = () => [];

function thinPoints<T>(points: T[], maxPoints: number): T[] {
  if (points.length <= maxPoints) return points;
  const stride = Math.ceil(points.length / maxPoints);
  return points.filter((_, index) => index % stride === 0 || index === points.length - 1);
}

interface BacktestChartsProps {
  result: BacktestResponse;
}

interface TooltipPayloadItem {
  dataKey?: string | number;
  name?: string;
  value?: number | string;
  color?: string;
}

function CustomSeriesTooltip({
  active,
  payload,
  label,
  formatLabel,
  formatValue,
}: {
  active?: boolean;
  payload?: readonly TooltipPayloadItem[];
  label?: string | number;
  formatLabel: (val: string) => string;
  formatValue: (val: number) => string;
}) {
  if (!active || !payload?.length) return null;

  return (
    <div className="border-border/60 bg-background min-w-44 rounded-lg border px-3 py-2 text-xs shadow-md">
      <div className="text-muted-foreground mb-1.5 font-medium">{formatLabel(String(label))}</div>
      <div className="space-y-1">
        {payload.map((entry) => {
          if (entry.value == null) return null;
          return (
            <div key={String(entry.dataKey)} className="flex items-center justify-between gap-4">
              <div className="flex items-center gap-1.5">
                <span
                  className="inline-block size-2 shrink-0 rounded-full"
                  style={{ backgroundColor: entry.color }}
                />
                <span className="text-muted-foreground truncate">{entry.name}</span>
              </div>
              <span className="font-medium tabular-nums">{formatValue(Number(entry.value))}</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

export function BacktestCharts({ result }: BacktestChartsProps) {
  const { t } = useTranslation(["backtest"]);
  const { formatAmount, formatCompactAmount } = useAmountFormatting();
  const { formatPercent } = useNumberFormatting();
  const { formatDate } = useDateFormatting();
  const [growthMode, setGrowthMode] = useState<"value" | "return">("value");

  const strategyLabel = formatBacktestSeriesLabel(result.strategy.descriptor, t);
  const buyAndHoldLabel = result.buyAndHold
    ? formatBacktestSeriesLabel(result.buyAndHold.descriptor, t)
    : t("backtest:strategies.buy_and_hold");
  const benchmarkLabel = result.benchmark
    ? formatBacktestSeriesLabel(result.benchmark.descriptor, t)
    : t("backtest:fields.benchmark");

  const hasDca = result.strategy.metrics.totalInvested > (result.strategy.series[0]?.invested ?? 0);

  const growthData = useMemo(() => {
    const bhByDate = new Map(result.buyAndHold?.series.map((p) => [p.date, p]) ?? []);
    const benchByDate = new Map(result.benchmark?.series.map((p) => [p.date, p]) ?? []);

    const combined = result.strategy.series.map((p) => {
      const bh = bhByDate.get(p.date);
      const bench = benchByDate.get(p.date);
      return {
        date: p.date,
        strategyValue: p.value,
        buyAndHoldValue: bh?.value ?? null,
        benchmarkValue: bench?.value ?? null,
        investedValue: p.invested,
        strategyReturn: p.cumulativeReturn,
        buyAndHoldReturn: bh?.cumulativeReturn ?? null,
        benchmarkReturn: bench?.cumulativeReturn ?? null,
        strategyDrawdown: p.drawdown,
        buyAndHoldDrawdown: bh?.drawdown ?? null,
        benchmarkDrawdown: bench?.drawdown ?? null,
      };
    });

    return thinPoints(combined, MAX_CHART_POINTS);
  }, [result]);

  const annualData = useMemo(() => {
    const years = new Set<number>();
    const addYears = (s: BacktestSeriesResult | null) => {
      s?.annualReturns.forEach((a) => years.add(a.year));
    };
    addYears(result.strategy);
    addYears(result.buyAndHold);
    addYears(result.benchmark);

    const strategyMap = new Map(result.strategy.annualReturns.map((a) => [a.year, a.returnPct]));
    const bhMap = new Map(result.buyAndHold?.annualReturns.map((a) => [a.year, a.returnPct]) ?? []);
    const benchMap = new Map(
      result.benchmark?.annualReturns.map((a) => [a.year, a.returnPct]) ?? [],
    );

    return [...years]
      .sort((a, b) => a - b)
      .map((year) => ({
        year: String(year),
        strategy: strategyMap.get(year) ?? null,
        buyAndHold: bhMap.get(year) ?? null,
        benchmark: benchMap.get(year) ?? null,
      }));
  }, [result]);

  const chartConfig: ChartConfig = useMemo(
    () => ({
      strategy: { label: strategyLabel, color: SERIES_COLORS.strategy },
      buyAndHold: {
        label: buyAndHoldLabel,
        color: SERIES_COLORS.buyAndHold,
      },
      benchmark: {
        label: benchmarkLabel,
        color: SERIES_COLORS.benchmark,
      },
      invested: {
        label: t("backtest:charts.total_invested"),
        color: SERIES_COLORS.invested,
      },
    }),
    [benchmarkLabel, buyAndHoldLabel, strategyLabel, t],
  );

  const formatShortDate = (dateStr: string) => {
    try {
      return formatDate(dateStr, { month: "short", year: "2-digit" });
    } catch {
      return dateStr;
    }
  };

  const formatFullDate = (dateStr: string) => {
    try {
      return formatDate(dateStr);
    } catch {
      return dateStr;
    }
  };

  return (
    <div className="space-y-4">
      {/* Portfolio Growth Chart */}
      <Card>
        <CardHeader className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
          <div>
            <CardTitle className="text-base">{t("backtest:charts.growth_title")}</CardTitle>
            <CardDescription>{t("backtest:charts.growth_description")}</CardDescription>
          </div>
          <div className="flex items-center gap-1 rounded-lg border p-0.5">
            <Button
              type="button"
              size="sm"
              variant={growthMode === "value" ? "secondary" : "ghost"}
              className="h-7 px-2.5 text-xs"
              onClick={() => setGrowthMode("value")}
            >
              {t("backtest:charts.mode_value")}
            </Button>
            <Button
              type="button"
              size="sm"
              variant={growthMode === "return" ? "secondary" : "ghost"}
              className="h-7 px-2.5 text-xs"
              onClick={() => setGrowthMode("return")}
            >
              {t("backtest:charts.mode_return")}
            </Button>
          </div>
        </CardHeader>
        <CardContent>
          <div className="mb-3 flex flex-wrap items-center gap-4 text-xs">
            <div className="flex items-center gap-1.5">
              <span
                className="inline-block size-2.5 rounded-full"
                style={{ backgroundColor: SERIES_COLORS.strategy }}
              />
              <span className="font-medium">{strategyLabel}</span>
            </div>
            {result.buyAndHold && (
              <div className="flex items-center gap-1.5">
                <span
                  className="inline-block size-2.5 rounded-full"
                  style={{ backgroundColor: SERIES_COLORS.buyAndHold }}
                />
                <span>{buyAndHoldLabel}</span>
              </div>
            )}
            {result.benchmark && (
              <div className="flex items-center gap-1.5">
                <span
                  className="inline-block size-2.5 rounded-full"
                  style={{ backgroundColor: SERIES_COLORS.benchmark }}
                />
                <span>{benchmarkLabel}</span>
              </div>
            )}
            {growthMode === "value" && hasDca && (
              <div className="flex items-center gap-1.5">
                <span
                  className="inline-block h-0.5 w-3 border-t-2 border-dashed"
                  style={{ borderColor: SERIES_COLORS.invested }}
                />
                <span className="text-muted-foreground">{t("backtest:charts.total_invested")}</span>
              </div>
            )}
          </div>

          <ChartContainer config={chartConfig} className="h-75 w-full">
            <ResponsiveContainer width="100%" height="100%">
              <ComposedChart data={growthData} margin={{ top: 8, right: 12, left: 4, bottom: 0 }}>
                <CartesianGrid
                  strokeDasharray="3 3"
                  vertical={false}
                  verticalCoordinatesGenerator={noVerticalGridPoints}
                  className="stroke-border/50"
                />
                <XAxis
                  dataKey="date"
                  tickFormatter={formatShortDate}
                  minTickGap={40}
                  tickLine={false}
                  axisLine={false}
                  className="text-xs"
                />
                <YAxis
                  width={68}
                  tickLine={false}
                  axisLine={false}
                  className="text-xs"
                  tickFormatter={(val: number) =>
                    growthMode === "value"
                      ? formatCompactAmount(val, "USD", true, "narrowSymbol")
                      : formatPercent(val, { digits: 0 })
                  }
                />
                {growthMode === "return" && (
                  <ReferenceLine y={0} stroke="currentColor" strokeOpacity={0.25} />
                )}
                <ChartTooltip
                  content={({ active, payload, label }) => (
                    <CustomSeriesTooltip
                      active={active}
                      payload={payload as readonly TooltipPayloadItem[] | undefined}
                      label={label}
                      formatLabel={formatFullDate}
                      formatValue={(val) =>
                        growthMode === "value"
                          ? formatAmount(val, "USD")
                          : formatPercent(val, { digits: 2, signDisplay: "exceptZero" })
                      }
                    />
                  )}
                />
                {growthMode === "value" && hasDca && (
                  <Line
                    type="monotone"
                    dataKey="investedValue"
                    name={t("backtest:charts.total_invested")}
                    stroke={SERIES_COLORS.invested}
                    strokeWidth={1.5}
                    strokeDasharray="4 4"
                    dot={false}
                    isAnimationActive={false}
                  />
                )}
                {result.benchmark && (
                  <Line
                    type="monotone"
                    dataKey={growthMode === "value" ? "benchmarkValue" : "benchmarkReturn"}
                    name={benchmarkLabel}
                    stroke={SERIES_COLORS.benchmark}
                    strokeWidth={1.75}
                    dot={false}
                    isAnimationActive={false}
                  />
                )}
                {result.buyAndHold && (
                  <Line
                    type="monotone"
                    dataKey={growthMode === "value" ? "buyAndHoldValue" : "buyAndHoldReturn"}
                    name={buyAndHoldLabel}
                    stroke={SERIES_COLORS.buyAndHold}
                    strokeWidth={1.75}
                    dot={false}
                    isAnimationActive={false}
                  />
                )}
                <Area
                  type="monotone"
                  dataKey={growthMode === "value" ? "strategyValue" : "strategyReturn"}
                  name={strategyLabel}
                  stroke={SERIES_COLORS.strategy}
                  fill={SERIES_COLORS.strategy}
                  fillOpacity={0.12}
                  strokeWidth={2.25}
                  dot={false}
                  isAnimationActive={false}
                />
              </ComposedChart>
            </ResponsiveContainer>
          </ChartContainer>
        </CardContent>
      </Card>

      {/* Drawdown & Annual Returns */}
      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle className="text-base">{t("backtest:charts.drawdown_title")}</CardTitle>
            <CardDescription>{t("backtest:charts.drawdown_description")}</CardDescription>
          </CardHeader>
          <CardContent>
            <ChartContainer config={chartConfig} className="h-55 w-full">
              <ResponsiveContainer width="100%" height="100%">
                <ComposedChart data={growthData} margin={{ top: 8, right: 12, left: 4, bottom: 0 }}>
                  <CartesianGrid
                    strokeDasharray="3 3"
                    vertical={false}
                    verticalCoordinatesGenerator={noVerticalGridPoints}
                    className="stroke-border/50"
                  />
                  <XAxis
                    dataKey="date"
                    tickFormatter={formatShortDate}
                    minTickGap={40}
                    tickLine={false}
                    axisLine={false}
                    className="text-xs"
                  />
                  <YAxis
                    width={56}
                    tickLine={false}
                    axisLine={false}
                    className="text-xs"
                    domain={["auto", 0]}
                    tickFormatter={(val: number) => formatPercent(val, { digits: 0 })}
                  />
                  <ReferenceLine y={0} stroke="currentColor" strokeOpacity={0.25} />
                  <ChartTooltip
                    content={({ active, payload, label }) => (
                      <CustomSeriesTooltip
                        active={active}
                        payload={payload as readonly TooltipPayloadItem[] | undefined}
                        label={label}
                        formatLabel={formatFullDate}
                        formatValue={(val) => formatPercent(val, { digits: 2 })}
                      />
                    )}
                  />
                  {result.benchmark && (
                    <Line
                      type="monotone"
                      dataKey="benchmarkDrawdown"
                      name={benchmarkLabel}
                      stroke={SERIES_COLORS.benchmark}
                      strokeWidth={1.5}
                      dot={false}
                      isAnimationActive={false}
                    />
                  )}
                  {result.buyAndHold && (
                    <Line
                      type="monotone"
                      dataKey="buyAndHoldDrawdown"
                      name={buyAndHoldLabel}
                      stroke={SERIES_COLORS.buyAndHold}
                      strokeWidth={1.5}
                      dot={false}
                      isAnimationActive={false}
                    />
                  )}
                  <Area
                    type="monotone"
                    dataKey="strategyDrawdown"
                    name={strategyLabel}
                    stroke={SERIES_COLORS.strategy}
                    fill={SERIES_COLORS.strategy}
                    fillOpacity={0.18}
                    strokeWidth={2}
                    dot={false}
                    isAnimationActive={false}
                  />
                </ComposedChart>
              </ResponsiveContainer>
            </ChartContainer>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="text-base">{t("backtest:charts.annual_returns_title")}</CardTitle>
            <CardDescription>{t("backtest:charts.annual_returns_description")}</CardDescription>
          </CardHeader>
          <CardContent>
            <ChartContainer config={chartConfig} className="h-55 w-full">
              <ResponsiveContainer width="100%" height="100%">
                <BarChart data={annualData} margin={{ top: 8, right: 12, left: 4, bottom: 0 }}>
                  <CartesianGrid
                    strokeDasharray="3 3"
                    vertical={false}
                    verticalCoordinatesGenerator={noVerticalGridPoints}
                    className="stroke-border/50"
                  />
                  <XAxis dataKey="year" tickLine={false} axisLine={false} className="text-xs" />
                  <YAxis
                    width={56}
                    tickLine={false}
                    axisLine={false}
                    className="text-xs"
                    tickFormatter={(val: number) => formatPercent(val, { digits: 0 })}
                  />
                  <ReferenceLine y={0} stroke="currentColor" strokeOpacity={0.3} />
                  <ChartTooltip
                    content={({ active, payload, label }) => (
                      <CustomSeriesTooltip
                        active={active}
                        payload={payload as readonly TooltipPayloadItem[] | undefined}
                        label={label}
                        formatLabel={(yr) => yr}
                        formatValue={(val) =>
                          formatPercent(val, { digits: 2, signDisplay: "exceptZero" })
                        }
                      />
                    )}
                  />
                  <Bar
                    dataKey="strategy"
                    name={strategyLabel}
                    fill={SERIES_COLORS.strategy}
                    radius={[3, 3, 0, 0]}
                    isAnimationActive={false}
                  />
                  {result.buyAndHold && (
                    <Bar
                      dataKey="buyAndHold"
                      name={buyAndHoldLabel}
                      fill={SERIES_COLORS.buyAndHold}
                      radius={[3, 3, 0, 0]}
                      isAnimationActive={false}
                    />
                  )}
                  {result.benchmark && (
                    <Bar
                      dataKey="benchmark"
                      name={benchmarkLabel}
                      fill={SERIES_COLORS.benchmark}
                      radius={[3, 3, 0, 0]}
                      isAnimationActive={false}
                    />
                  )}
                </BarChart>
              </ResponsiveContainer>
            </ChartContainer>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
