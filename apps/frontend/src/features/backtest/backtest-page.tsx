import { runBacktest } from "@/adapters";
import TickerSearchInput from "@/components/ticker-search";
import { useHoldings } from "@/hooks/use-holdings";
import type { SymbolSearchResult } from "@/lib/types";
import type {
  BacktestAssetInput,
  BacktestRequest,
  BacktestResponse,
  BacktestStrategy,
  ContributionFrequency,
  DividendMode,
  MomentumWeighting,
  RebalanceFrequency,
  SignalFrequency,
} from "@/lib/types/backtest";
import { useQuery } from "@tanstack/react-query";
import {
  AlertFeedback,
  Badge,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Icons,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Separator,
  Skeleton,
  Switch,
  useDateFormatting,
} from "@wealthfolio/ui";
import { format, subYears } from "date-fns";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { BacktestCharts } from "./components/backtest-charts";
import { BacktestMetricsTable } from "./components/backtest-metrics-table";
import { formatBacktestWarning } from "./formatters";
import {
  ALLOWED_US_INSTRUMENT_TYPES,
  BACKTEST_PRESETS,
  KNOWN_US_EXCHANGE_MICS,
  extractUsdHoldingsWeights,
  type BacktestPreset,
} from "./presets";

type StrategyKind = BacktestStrategy["type"];

const MAX_ASSETS = 10;

function formatIsoDate(date: Date): string {
  return format(date, "yyyy-MM-dd");
}

function defaultDateRange(): { startDate: string; endDate: string } {
  const end = new Date();
  const start = subYears(end, 5);
  return { startDate: formatIsoDate(start), endDate: formatIsoDate(end) };
}

export function BacktestPage() {
  const { t } = useTranslation(["backtest", "common"]);
  const { formatDate } = useDateFormatting();
  const { holdings } = useHoldings({ type: "all" });

  const initialDates = useMemo(() => defaultDateRange(), []);

  // Portfolio universe
  const [assets, setAssets] = useState<BacktestAssetInput[]>([
    { symbol: "VTI", weight: 60, exchangeMic: "ARCX", instrumentType: "ETF" },
    { symbol: "BND", weight: 40, exchangeMic: "XNAS", instrumentType: "ETF" },
  ]);
  const [tickerInput, setTickerInput] = useState("");
  const [importNotice, setImportNotice] = useState<string | null>(null);

  // Period & cash flows
  const [startDate, setStartDate] = useState(initialDates.startDate);
  const [endDate, setEndDate] = useState(initialDates.endDate);
  const [initialCapital, setInitialCapital] = useState("10000");
  const [contributionAmount, setContributionAmount] = useState("0");
  const [contributionFrequency, setContributionFrequency] =
    useState<ContributionFrequency>("monthly");
  const [dividendMode, setDividendMode] = useState<DividendMode>("reinvest");
  const [feeBps, setFeeBps] = useState("0");
  const [benchmarkSymbol, setBenchmarkSymbol] = useState("SPY");
  const [benchmarkExchangeMic, setBenchmarkExchangeMic] = useState<string | null>("ARCX");
  const [benchmarkInstrumentType, setBenchmarkInstrumentType] = useState<string | null>("ETF");

  // Strategy & strategy-specific parameters
  const [strategyKind, setStrategyKind] = useState<StrategyKind>("periodicRebalance");
  const [rebalanceFrequency, setRebalanceFrequency] = useState<RebalanceFrequency>("quarterly");
  const [driftThresholdPct, setDriftThresholdPct] = useState("5");
  const [smaDays, setSmaDays] = useState("200");
  const [smaCheckFrequency, setSmaCheckFrequency] = useState<SignalFrequency>("monthly");
  const [smaDefensiveSymbol, setSmaDefensiveSymbol] = useState("SHY");
  const [smaDefensiveExchangeMic, setSmaDefensiveExchangeMic] = useState<string | null>("XNAS");
  const [smaDefensiveInstrumentType, setSmaDefensiveInstrumentType] = useState<string | null>(
    "ETF",
  );
  const [momentumLookbackDays, setMomentumLookbackDays] = useState("126");
  const [momentumTopN, setMomentumTopN] = useState("1");
  const [momentumFrequency, setMomentumFrequency] = useState<RebalanceFrequency>("monthly");
  const [momentumWeighting, setMomentumWeighting] = useState<MomentumWeighting>("equal");
  const [absoluteMomentum, setAbsoluteMomentum] = useState(true);
  const [momentumDefensiveSymbol, setMomentumDefensiveSymbol] = useState("SHY");
  const [momentumDefensiveExchangeMic, setMomentumDefensiveExchangeMic] = useState<string | null>(
    "XNAS",
  );
  const [momentumDefensiveInstrumentType, setMomentumDefensiveInstrumentType] = useState<
    string | null
  >("ETF");

  // Submitted request triggers in-session cached execution
  const [submittedRequest, setSubmittedRequest] = useState<BacktestRequest | null>(null);
  const [validationError, setValidationError] = useState<string | null>(null);

  const usdHoldings = useMemo(() => extractUsdHoldingsWeights(holdings), [holdings]);

  const weightSum = useMemo(
    () => assets.reduce((sum, item) => sum + (Number.isFinite(item.weight) ? item.weight : 0), 0),
    [assets],
  );
  const requiresWeights = strategyKind !== "momentumRotation";
  const isWeightSumValid = !requiresWeights || (weightSum >= 99.9 && weightSum <= 100.1);

  const {
    data: result,
    isFetching,
    isError,
    error,
  } = useQuery<BacktestResponse, Error>({
    queryKey: ["backtest-experiment", submittedRequest],
    queryFn: () => {
      if (!submittedRequest) {
        throw new Error("Missing backtest request");
      }
      return runBacktest(submittedRequest);
    },
    enabled: submittedRequest !== null,
    staleTime: 5 * 60 * 1000,
    retry: false,
  });

  const applyPreset = (preset: BacktestPreset) => {
    setImportNotice(null);
    setValidationError(null);
    setAssets(preset.assets.map((a) => ({ ...a })));
    setBenchmarkSymbol(preset.benchmarkSymbol);
    setBenchmarkExchangeMic(preset.benchmarkExchangeMic);
    setBenchmarkInstrumentType(preset.benchmarkInstrumentType);
    setDividendMode(preset.dividendMode);

    const s = preset.strategy;
    setStrategyKind(s.type);
    if (s.type === "periodicRebalance") {
      setRebalanceFrequency(s.frequency);
    } else if (s.type === "driftBandRebalance") {
      setDriftThresholdPct(String(s.thresholdPct));
    } else if (s.type === "smaTrendFilter") {
      setSmaDays(String(s.smaDays));
      setSmaCheckFrequency(s.checkFrequency);
      setSmaDefensiveSymbol(s.defensiveSymbol ?? "");
      setSmaDefensiveExchangeMic(s.defensiveExchangeMic ?? null);
      setSmaDefensiveInstrumentType(s.defensiveInstrumentType ?? null);
    } else if (s.type === "momentumRotation") {
      setMomentumLookbackDays(String(s.lookbackDays));
      setMomentumTopN(String(Math.min(s.topN, preset.assets.length)));
      setMomentumFrequency(s.rebalanceFrequency);
      setMomentumWeighting(s.weighting);
      setAbsoluteMomentum(s.absoluteMomentum);
      setMomentumDefensiveSymbol(s.defensiveSymbol ?? "");
      setMomentumDefensiveExchangeMic(s.defensiveExchangeMic ?? null);
      setMomentumDefensiveInstrumentType(s.defensiveInstrumentType ?? null);
    }
  };

  const handleImportCurrentHoldings = () => {
    setValidationError(null);
    if (usdHoldings.length === 0) {
      setImportNotice(t("backtest:notices.no_usd_holdings"));
      return;
    }
    setAssets(usdHoldings.map((h) => ({ ...h })));
    setImportNotice(
      t("backtest:notices.imported_holdings", {
        count: usdHoldings.length,
      }),
    );
  };

  const handleAddTicker = (rawSymbol: string, searchResult?: SymbolSearchResult) => {
    const clean = rawSymbol.trim().toUpperCase();
    if (!clean) return;
    setImportNotice(null);
    setValidationError(null);

    if (assets.length >= MAX_ASSETS) {
      setValidationError(t("backtest:validation.max_assets", { count: MAX_ASSETS }));
      return;
    }
    if (assets.some((a) => a.symbol === clean)) {
      setValidationError(t("backtest:validation.duplicate_ticker", { symbol: clean }));
      return;
    }

    const exchangeMic =
      (searchResult?.canonicalExchangeMic || searchResult?.exchangeMic || "")
        .trim()
        .toUpperCase() || null;
    const instrumentType = (searchResult?.quoteType || "").trim().toUpperCase() || null;
    const currency = (searchResult?.currency || "").trim().toUpperCase() || null;

    if (
      (currency && currency !== "USD") ||
      (exchangeMic && !KNOWN_US_EXCHANGE_MICS.has(exchangeMic)) ||
      (instrumentType && !ALLOWED_US_INSTRUMENT_TYPES.has(instrumentType))
    ) {
      setValidationError(t("backtest:validation.non_us_listing", { symbol: clean }));
      return;
    }

    const remaining = Math.max(0, Math.round((100 - weightSum) * 10) / 10);
    setAssets((prev) => [
      ...prev,
      { symbol: clean, weight: remaining, exchangeMic, instrumentType },
    ]);
    setTickerInput("");
  };

  const handleRemoveTicker = (symbol: string) => {
    setAssets((prev) => prev.filter((a) => a.symbol !== symbol));
  };

  const handleWeightChange = (symbol: string, value: string) => {
    const parsed = value === "" ? 0 : Number(value);
    if (!Number.isFinite(parsed)) return;
    setAssets((prev) =>
      prev.map((item) => (item.symbol === symbol ? { ...item, weight: parsed } : item)),
    );
  };

  const handleEqualizeWeights = () => {
    if (assets.length === 0) return;
    const base = Math.floor(1000 / assets.length) / 10;
    const updated = assets.map((item) => ({ ...item, weight: base }));
    const sum = updated.reduce((acc, item) => acc + item.weight, 0);
    const diff = Math.round((100 - sum) * 10) / 10;
    updated[0].weight = Math.round((updated[0].weight + diff) * 10) / 10;
    setAssets(updated);
  };

  const applyDateHorizon = (years: number) => {
    const end = new Date();
    const start = subYears(end, years);
    setStartDate(formatIsoDate(start));
    setEndDate(formatIsoDate(end));
  };

  const buildRequest = (): BacktestRequest | null => {
    if (assets.length === 0) {
      setValidationError(t("backtest:validation.min_assets"));
      return null;
    }
    if (!startDate || !endDate || endDate <= startDate) {
      setValidationError(t("backtest:validation.invalid_dates"));
      return null;
    }
    const capital = Number(initialCapital);
    if (!Number.isFinite(capital) || capital <= 0) {
      setValidationError(t("backtest:validation.invalid_capital"));
      return null;
    }
    const fee = Number(feeBps || "0");
    if (!Number.isFinite(fee) || fee < 0 || fee > 500) {
      setValidationError(t("backtest:validation.invalid_fee"));
      return null;
    }
    const dca = Number(contributionAmount || "0");
    if (!Number.isFinite(dca) || dca < 0) {
      setValidationError(t("backtest:validation.invalid_contribution"));
      return null;
    }
    if (!isWeightSumValid) {
      setValidationError(t("backtest:validation.weight_sum", { total: weightSum.toFixed(1) }));
      return null;
    }

    let strategy: BacktestStrategy;
    switch (strategyKind) {
      case "buyAndHold":
        strategy = { type: "buyAndHold" };
        break;
      case "periodicRebalance":
        strategy = { type: "periodicRebalance", frequency: rebalanceFrequency };
        break;
      case "driftBandRebalance": {
        const threshold = Number(driftThresholdPct);
        if (!Number.isFinite(threshold) || threshold <= 0 || threshold >= 100) {
          setValidationError(t("backtest:validation.invalid_drift"));
          return null;
        }
        strategy = { type: "driftBandRebalance", thresholdPct: threshold };
        break;
      }
      case "smaTrendFilter": {
        const days = Math.round(Number(smaDays));
        if (!Number.isFinite(days) || days < 5 || days > 400) {
          setValidationError(t("backtest:validation.invalid_lookback"));
          return null;
        }
        const defSym = smaDefensiveSymbol.trim() ? smaDefensiveSymbol.trim().toUpperCase() : null;
        strategy = {
          type: "smaTrendFilter",
          smaDays: days,
          checkFrequency: smaCheckFrequency,
          defensiveSymbol: defSym,
          defensiveExchangeMic: defSym ? smaDefensiveExchangeMic : null,
          defensiveInstrumentType: defSym ? smaDefensiveInstrumentType : null,
        };
        break;
      }
      case "momentumRotation": {
        const lookback = Math.round(Number(momentumLookbackDays));
        const topN = Math.round(Number(momentumTopN));
        if (!Number.isFinite(lookback) || lookback < 5 || lookback > 400) {
          setValidationError(t("backtest:validation.invalid_lookback"));
          return null;
        }
        if (!Number.isFinite(topN) || topN < 1 || topN > assets.length) {
          setValidationError(t("backtest:validation.invalid_top_n", { max: assets.length }));
          return null;
        }
        const defSym = momentumDefensiveSymbol.trim()
          ? momentumDefensiveSymbol.trim().toUpperCase()
          : null;
        strategy = {
          type: "momentumRotation",
          lookbackDays: lookback,
          topN,
          rebalanceFrequency: momentumFrequency,
          weighting: momentumWeighting,
          absoluteMomentum,
          defensiveSymbol: defSym,
          defensiveExchangeMic: defSym ? momentumDefensiveExchangeMic : null,
          defensiveInstrumentType: defSym ? momentumDefensiveInstrumentType : null,
        };
        break;
      }
    }

    setValidationError(null);
    const cleanBenchmark = benchmarkSymbol.trim() ? benchmarkSymbol.trim().toUpperCase() : null;
    return {
      assets: assets.map((a) => ({
        symbol: a.symbol.trim().toUpperCase(),
        weight: a.weight,
        exchangeMic: a.exchangeMic ?? null,
        instrumentType: a.instrumentType ?? null,
      })),
      startDate,
      endDate,
      initialCapital: capital,
      contribution: dca > 0 ? { amount: dca, frequency: contributionFrequency } : null,
      dividendMode,
      feeBps: fee,
      benchmarkSymbol: cleanBenchmark,
      benchmarkExchangeMic: cleanBenchmark ? benchmarkExchangeMic : null,
      benchmarkInstrumentType: cleanBenchmark ? benchmarkInstrumentType : null,
      strategy,
    };
  };

  const handleRun = (e: React.FormEvent) => {
    e.preventDefault();
    const request = buildRequest();
    if (!request) return;
    setSubmittedRequest(request);
  };

  return (
    <div className="space-y-6 pb-8">
      {/* Header & Presets */}
      <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
        <div>
          <div className="flex flex-wrap items-center gap-2">
            <h2 className="text-xl font-semibold tracking-tight">{t("backtest:title")}</h2>
            <Badge variant="secondary">{t("backtest:badges.us_usd")}</Badge>
            <Badge variant="outline">{t("backtest:badges.in_session")}</Badge>
          </div>
          <p className="text-muted-foreground mt-1 text-sm">{t("backtest:subtitle")}</p>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={handleImportCurrentHoldings}
          className="shrink-0"
        >
          <Icons.Holdings className="me-1.5 size-4" />
          {t("backtest:actions.import_holdings")}
        </Button>
      </div>

      {/* Quick Presets */}
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-muted-foreground text-xs font-medium">
          {t("backtest:presets.label")}
        </span>
        {BACKTEST_PRESETS.map((preset) => (
          <Button
            key={preset.id}
            type="button"
            variant="outline"
            size="sm"
            className="h-7 text-xs"
            onClick={() => applyPreset(preset)}
            title={t(preset.descriptionKey)}
          >
            {t(preset.labelKey)}
          </Button>
        ))}
      </div>

      {importNotice && (
        <AlertFeedback variant="warning" title={t("backtest:notices.holdings_title")}>
          {importNotice}
        </AlertFeedback>
      )}

      <div className="grid gap-6 xl:grid-cols-12">
        {/* Configuration Panel */}
        <form onSubmit={handleRun} noValidate className="space-y-4 xl:col-span-4">
          {/* Universe & Weights */}
          <Card>
            <CardHeader className="pb-3">
              <div className="flex items-center justify-between gap-2">
                <CardTitle className="text-base">{t("backtest:sections.universe")}</CardTitle>
                {requiresWeights && assets.length > 0 && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    className="h-7 px-2 text-xs"
                    onClick={handleEqualizeWeights}
                  >
                    {t("backtest:actions.equalize")}
                  </Button>
                )}
              </div>
              <CardDescription>{t("backtest:sections.universe_description")}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <div className="flex items-center gap-2">
                <div className="flex-1">
                  <TickerSearchInput
                    value={tickerInput}
                    placeholder={t("backtest:placeholders.add_ticker")}
                    defaultCurrency="USD"
                    hideCustomCreate
                    onSelectResult={(symbol, searchResult) => handleAddTicker(symbol, searchResult)}
                    aria-label={t("backtest:placeholders.add_ticker")}
                  />
                </div>
              </div>

              <div className="space-y-2">
                {assets.map((asset) => (
                  <div
                    key={asset.symbol}
                    className="bg-muted/30 flex items-center justify-between gap-2 rounded-md border px-2.5 py-1.5"
                  >
                    <span className="font-mono text-sm font-semibold">{asset.symbol}</span>
                    <div className="flex items-center gap-2">
                      {requiresWeights && (
                        <div className="flex items-center gap-1">
                          <Input
                            type="number"
                            min={0}
                            max={100}
                            step="any"
                            value={asset.weight}
                            onChange={(e) => handleWeightChange(asset.symbol, e.target.value)}
                            className="h-8 w-20 text-end tabular-nums"
                            aria-label={`${asset.symbol} ${t("backtest:fields.weight")}`}
                          />
                          <span className="text-muted-foreground text-xs">%</span>
                        </div>
                      )}
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon"
                        className="text-muted-foreground hover:text-destructive size-7"
                        onClick={() => handleRemoveTicker(asset.symbol)}
                        aria-label={t("backtest:actions.remove_ticker", { symbol: asset.symbol })}
                      >
                        <Icons.Trash2 className="size-3.5" />
                      </Button>
                    </div>
                  </div>
                ))}
              </div>

              {requiresWeights && (
                <div className="flex items-center justify-between text-xs">
                  <span className="text-muted-foreground">{t("backtest:fields.total_weight")}</span>
                  <span
                    className={
                      isWeightSumValid
                        ? "text-success font-medium tabular-nums"
                        : "text-destructive font-medium tabular-nums"
                    }
                  >
                    {weightSum.toFixed(1)}%
                  </span>
                </div>
              )}
            </CardContent>
          </Card>

          {/* Strategy Configuration */}
          <Card>
            <CardHeader className="pb-3">
              <CardTitle className="text-base">{t("backtest:sections.strategy")}</CardTitle>
              <CardDescription>{t("backtest:sections.strategy_description")}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <div className="space-y-1.5">
                <Label htmlFor="backtest-strategy">{t("backtest:fields.strategy")}</Label>
                <Select
                  value={strategyKind}
                  onValueChange={(val) => {
                    if (val) setStrategyKind(val);
                  }}
                >
                  <SelectTrigger id="backtest-strategy">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="buyAndHold">
                      {t("backtest:strategies.buy_and_hold")}
                    </SelectItem>
                    <SelectItem value="periodicRebalance">
                      {t("backtest:strategies.periodic_rebalance")}
                    </SelectItem>
                    <SelectItem value="driftBandRebalance">
                      {t("backtest:strategies.drift_band")}
                    </SelectItem>
                    <SelectItem value="smaTrendFilter">
                      {t("backtest:strategies.sma_trend")}
                    </SelectItem>
                    <SelectItem value="momentumRotation">
                      {t("backtest:strategies.momentum_rotation")}
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>

              {strategyKind === "periodicRebalance" && (
                <div className="space-y-1.5">
                  <Label htmlFor="rebalance-freq">{t("backtest:fields.rebalance_frequency")}</Label>
                  <Select
                    value={rebalanceFrequency}
                    onValueChange={(val) => {
                      if (val) setRebalanceFrequency(val);
                    }}
                  >
                    <SelectTrigger id="rebalance-freq">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="monthly">{t("backtest:frequencies.monthly")}</SelectItem>
                      <SelectItem value="quarterly">
                        {t("backtest:frequencies.quarterly")}
                      </SelectItem>
                      <SelectItem value="semiannually">
                        {t("backtest:frequencies.semiannually")}
                      </SelectItem>
                      <SelectItem value="annually">{t("backtest:frequencies.annually")}</SelectItem>
                    </SelectContent>
                  </Select>
                </div>
              )}

              {strategyKind === "driftBandRebalance" && (
                <div className="space-y-1.5">
                  <Label htmlFor="drift-threshold">{t("backtest:fields.drift_threshold")}</Label>
                  <Input
                    id="drift-threshold"
                    type="number"
                    min={0.5}
                    max={50}
                    step={0.5}
                    value={driftThresholdPct}
                    onChange={(e) => setDriftThresholdPct(e.target.value)}
                  />
                  <p className="text-muted-foreground text-xs">
                    {t("backtest:hints.drift_threshold")}
                  </p>
                </div>
              )}

              {strategyKind === "smaTrendFilter" && (
                <div className="space-y-3">
                  <div className="grid grid-cols-2 gap-3">
                    <div className="space-y-1.5">
                      <Label htmlFor="sma-days">{t("backtest:fields.sma_days")}</Label>
                      <Input
                        id="sma-days"
                        type="number"
                        min={5}
                        max={400}
                        value={smaDays}
                        onChange={(e) => setSmaDays(e.target.value)}
                      />
                    </div>
                    <div className="space-y-1.5">
                      <Label htmlFor="sma-check">{t("backtest:fields.signal_frequency")}</Label>
                      <Select
                        value={smaCheckFrequency}
                        onValueChange={(val) => {
                          if (val) setSmaCheckFrequency(val);
                        }}
                      >
                        <SelectTrigger id="sma-check">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="monthly">
                            {t("backtest:frequencies.monthly")}
                          </SelectItem>
                          <SelectItem value="daily">{t("backtest:frequencies.daily")}</SelectItem>
                        </SelectContent>
                      </Select>
                    </div>
                  </div>
                  <div className="space-y-1.5">
                    <Label htmlFor="sma-defensive">{t("backtest:fields.defensive_ticker")}</Label>
                    <Input
                      id="sma-defensive"
                      value={smaDefensiveSymbol}
                      onChange={(e) => {
                        setSmaDefensiveSymbol(e.target.value.toUpperCase());
                        setSmaDefensiveExchangeMic(null);
                        setSmaDefensiveInstrumentType(null);
                      }}
                      placeholder={t("backtest:placeholders.defensive_ticker")}
                    />
                  </div>
                </div>
              )}

              {strategyKind === "momentumRotation" && (
                <div className="space-y-3">
                  <div className="grid grid-cols-2 gap-3">
                    <div className="space-y-1.5">
                      <Label htmlFor="mom-lookback">{t("backtest:fields.lookback_days")}</Label>
                      <Input
                        id="mom-lookback"
                        type="number"
                        min={5}
                        max={400}
                        value={momentumLookbackDays}
                        onChange={(e) => setMomentumLookbackDays(e.target.value)}
                      />
                    </div>
                    <div className="space-y-1.5">
                      <Label htmlFor="mom-topn">{t("backtest:fields.top_n")}</Label>
                      <Input
                        id="mom-topn"
                        type="number"
                        min={1}
                        max={Math.max(1, assets.length)}
                        value={momentumTopN}
                        onChange={(e) => setMomentumTopN(e.target.value)}
                      />
                    </div>
                  </div>

                  <div className="grid grid-cols-2 gap-3">
                    <div className="space-y-1.5">
                      <Label htmlFor="mom-freq">{t("backtest:fields.rebalance_frequency")}</Label>
                      <Select
                        value={momentumFrequency}
                        onValueChange={(val) => {
                          if (val) setMomentumFrequency(val);
                        }}
                      >
                        <SelectTrigger id="mom-freq">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="monthly">
                            {t("backtest:frequencies.monthly")}
                          </SelectItem>
                          <SelectItem value="quarterly">
                            {t("backtest:frequencies.quarterly")}
                          </SelectItem>
                          <SelectItem value="semiannually">
                            {t("backtest:frequencies.semiannually")}
                          </SelectItem>
                          <SelectItem value="annually">
                            {t("backtest:frequencies.annually")}
                          </SelectItem>
                        </SelectContent>
                      </Select>
                    </div>
                    <div className="space-y-1.5">
                      <Label htmlFor="mom-weight">{t("backtest:fields.weighting")}</Label>
                      <Select
                        value={momentumWeighting}
                        onValueChange={(val) => {
                          if (val) setMomentumWeighting(val);
                        }}
                      >
                        <SelectTrigger id="mom-weight">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="equal">{t("backtest:weighting.equal")}</SelectItem>
                          <SelectItem value="inverseVolatility">
                            {t("backtest:weighting.inverse_volatility")}
                          </SelectItem>
                        </SelectContent>
                      </Select>
                    </div>
                  </div>

                  <div className="flex items-center justify-between gap-2 rounded-md border px-3 py-2">
                    <div>
                      <Label htmlFor="abs-momentum" className="text-sm">
                        {t("backtest:fields.absolute_momentum")}
                      </Label>
                      <p className="text-muted-foreground text-xs">
                        {t("backtest:hints.absolute_momentum")}
                      </p>
                    </div>
                    <Switch
                      id="abs-momentum"
                      checked={absoluteMomentum}
                      onCheckedChange={setAbsoluteMomentum}
                    />
                  </div>

                  {absoluteMomentum && (
                    <div className="space-y-1.5">
                      <Label htmlFor="mom-defensive">{t("backtest:fields.defensive_ticker")}</Label>
                      <Input
                        id="mom-defensive"
                        value={momentumDefensiveSymbol}
                        onChange={(e) => {
                          setMomentumDefensiveSymbol(e.target.value.toUpperCase());
                          setMomentumDefensiveExchangeMic(null);
                          setMomentumDefensiveInstrumentType(null);
                        }}
                        placeholder={t("backtest:placeholders.defensive_ticker")}
                      />
                    </div>
                  )}
                </div>
              )}
            </CardContent>
          </Card>

          {/* Period, Cash Flows & Execution Settings */}
          <Card>
            <CardHeader className="pb-3">
              <CardTitle className="text-base">{t("backtest:sections.parameters")}</CardTitle>
            </CardHeader>
            <CardContent className="space-y-3">
              <div className="flex flex-wrap gap-1.5">
                {[1, 3, 5, 10].map((yr) => (
                  <Button
                    key={yr}
                    type="button"
                    variant="outline"
                    size="sm"
                    className="h-7 px-2.5 text-xs"
                    onClick={() => applyDateHorizon(yr)}
                  >
                    {t("backtest:horizons.years", { count: yr })}
                  </Button>
                ))}
              </div>

              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1.5">
                  <Label htmlFor="bt-start">{t("backtest:fields.start_date")}</Label>
                  <Input
                    id="bt-start"
                    type="date"
                    value={startDate}
                    onChange={(e) => setStartDate(e.target.value)}
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="bt-end">{t("backtest:fields.end_date")}</Label>
                  <Input
                    id="bt-end"
                    type="date"
                    value={endDate}
                    onChange={(e) => setEndDate(e.target.value)}
                  />
                </div>
              </div>

              <Separator />

              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1.5">
                  <Label htmlFor="bt-capital">{t("backtest:fields.initial_capital")}</Label>
                  <Input
                    id="bt-capital"
                    type="number"
                    min={1}
                    step="any"
                    value={initialCapital}
                    onChange={(e) => setInitialCapital(e.target.value)}
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="bt-benchmark">{t("backtest:fields.benchmark")}</Label>
                  <Input
                    id="bt-benchmark"
                    value={benchmarkSymbol}
                    onChange={(e) => {
                      setBenchmarkSymbol(e.target.value.toUpperCase());
                      setBenchmarkExchangeMic(null);
                      setBenchmarkInstrumentType(null);
                    }}
                    placeholder="SPY"
                  />
                </div>
              </div>

              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1.5">
                  <Label htmlFor="bt-dca">{t("backtest:fields.dca_amount")}</Label>
                  <Input
                    id="bt-dca"
                    type="number"
                    min={0}
                    step="any"
                    value={contributionAmount}
                    onChange={(e) => setContributionAmount(e.target.value)}
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="bt-dca-freq">{t("backtest:fields.dca_frequency")}</Label>
                  <Select
                    value={contributionFrequency}
                    onValueChange={(val) => {
                      if (val) setContributionFrequency(val);
                    }}
                  >
                    <SelectTrigger id="bt-dca-freq">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="biweekly">{t("backtest:frequencies.biweekly")}</SelectItem>
                      <SelectItem value="monthly">{t("backtest:frequencies.monthly")}</SelectItem>
                      <SelectItem value="quarterly">
                        {t("backtest:frequencies.quarterly")}
                      </SelectItem>
                      <SelectItem value="annually">{t("backtest:frequencies.annually")}</SelectItem>
                    </SelectContent>
                  </Select>
                </div>
              </div>

              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1.5">
                  <Label htmlFor="bt-dividends">{t("backtest:fields.dividend_mode")}</Label>
                  <Select
                    value={dividendMode}
                    onValueChange={(val) => {
                      if (val) setDividendMode(val);
                    }}
                  >
                    <SelectTrigger id="bt-dividends">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="reinvest">{t("backtest:dividends.reinvest")}</SelectItem>
                      <SelectItem value="cash">{t("backtest:dividends.cash")}</SelectItem>
                      <SelectItem value="exclude">{t("backtest:dividends.exclude")}</SelectItem>
                    </SelectContent>
                  </Select>
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="bt-fee">{t("backtest:fields.fee_bps")}</Label>
                  <Input
                    id="bt-fee"
                    type="number"
                    min={0}
                    max={500}
                    step="any"
                    value={feeBps}
                    onChange={(e) => setFeeBps(e.target.value)}
                  />
                </div>
              </div>

              {validationError && (
                <AlertFeedback variant="error" title={t("backtest:validation.title")}>
                  {validationError}
                </AlertFeedback>
              )}

              <Button type="submit" className="w-full" disabled={isFetching}>
                {isFetching ? (
                  <>
                    <Icons.Spinner className="me-2 size-4 animate-spin" />
                    {t("backtest:actions.running")}
                  </>
                ) : (
                  <>
                    <Icons.PlayCircle className="me-2 size-4" />
                    {t("backtest:actions.run")}
                  </>
                )}
              </Button>
            </CardContent>
          </Card>
        </form>

        {/* Results Panel */}
        <div className="space-y-4 xl:col-span-8">
          {isError && (
            <AlertFeedback variant="error" title={t("backtest:errors.run_failed")}>
              {error?.message || t("backtest:errors.generic")}
            </AlertFeedback>
          )}

          {isFetching && !result && (
            <Card>
              <CardHeader>
                <Skeleton className="h-6 w-48" />
                <Skeleton className="h-4 w-72" />
              </CardHeader>
              <CardContent className="space-y-4">
                <div className="grid gap-3 sm:grid-cols-4">
                  <Skeleton className="h-24 w-full" />
                  <Skeleton className="h-24 w-full" />
                  <Skeleton className="h-24 w-full" />
                  <Skeleton className="h-24 w-full" />
                </div>
                <Skeleton className="h-75 w-full" />
              </CardContent>
            </Card>
          )}

          {!result && !isFetching && !isError && (
            <Card className="border-dashed">
              <CardContent className="flex flex-col items-center justify-center py-16 text-center">
                <Icons.ClockCounterClockwise className="text-muted-foreground mb-3 size-10" />
                <h3 className="text-base font-semibold">{t("backtest:empty.title")}</h3>
                <p className="text-muted-foreground mt-1 max-w-md text-sm">
                  {t("backtest:empty.description")}
                </p>
              </CardContent>
            </Card>
          )}

          {result && (
            <div className="space-y-4">
              {result.warnings.length > 0 && (
                <AlertFeedback variant="warning" title={t("backtest:notices.warnings_title")}>
                  <ul className="list-disc space-y-1 ps-4">
                    {result.warnings.map((w, idx) => (
                      <li key={`${w.code}-${idx}`}>{formatBacktestWarning(w, t, formatDate)}</li>
                    ))}
                  </ul>
                </AlertFeedback>
              )}

              <BacktestCharts result={result} />
              <BacktestMetricsTable result={result} />

              <p className="text-muted-foreground text-xs">{t("backtest:disclaimer")}</p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
