import { ContributionLimitEditModal } from "@/pages/settings/contribution-limits/components/contribution-limit-edit-modal";
import type {
  ContributionLimit,
  RealizedGainRow,
  TaxExportFormat,
  TaxTreatment,
  WashSaleSeverity,
} from "@/lib/types";
import { resolveDisplayTimezone } from "@/lib/utils";
import { today } from "@internationalized/date";
import {
  Alert,
  AlertDescription,
  AlertTitle,
  Badge,
  Button,
  buttonVariants,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
  Icons,
  Progress,
  ResponsiveSelect,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
  useAmountFormatting,
  useDateFormatting,
  useLocalizationSettings,
  type ResponsiveSelectOption,
} from "@wealthfolio/ui";
import { toast } from "@wealthfolio/ui/components/ui/use-toast";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import { downloadTaxPacket, useTaxYearReport } from "./use-tax-year-report";

const ALL_MEMBERS_VALUE = "__all__";

function toNum(value: number | string | null | undefined): number {
  if (typeof value === "number") return Number.isFinite(value) ? value : 0;
  if (typeof value === "string") {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : 0;
  }
  return 0;
}

type DisposalFilter = "all" | "short_term" | "long_term" | "wash_sale";

export function TaxCenterPage() {
  const { t } = useTranslation(["tax", "common", "settings"]);
  const { timezone } = useLocalizationSettings();
  const displayTimezone = resolveDisplayTimezone(timezone);
  const currentYear = today(displayTimezone).year;

  const [taxYear, setTaxYear] = useState<number>(currentYear);
  const [selectedMember, setSelectedMember] = useState<string>(ALL_MEMBERS_VALUE);
  const [disposalFilter, setDisposalFilter] = useState<DisposalFilter>("all");
  const [isExporting, setIsExporting] = useState(false);
  const [limitModalOpen, setLimitModalOpen] = useState(false);
  const [editingLimit, setEditingLimit] = useState<ContributionLimit | null>(null);

  const amountFormatting = useAmountFormatting();
  const dateFormatting = useDateFormatting();

  const memberFilter = selectedMember === ALL_MEMBERS_VALUE ? null : selectedMember;
  const { data: report, isLoading, error } = useTaxYearReport(taxYear, memberFilter);

  const yearOptions: ResponsiveSelectOption[] = useMemo(() => {
    const years: ResponsiveSelectOption[] = [];
    for (let y = currentYear; y >= currentYear - 5; y--) {
      years.push({ value: String(y), label: String(y) });
    }
    return years;
  }, [currentYear]);

  const memberOptions: ResponsiveSelectOption[] = useMemo(() => {
    const options: ResponsiveSelectOption[] = [
      { value: ALL_MEMBERS_VALUE, label: t("tax:filters.all_family") },
    ];
    for (const member of report?.availableMembers ?? []) {
      options.push({ value: member, label: member });
    }
    return options;
  }, [report?.availableMembers, t]);

  const handleExport = async (format: TaxExportFormat) => {
    setIsExporting(true);
    try {
      const filename = await downloadTaxPacket(taxYear, format, memberFilter);
      toast({
        title: t("tax:export.success_title"),
        description: t("tax:export.success_description", { filename }),
        variant: "success",
      });
    } catch {
      toast({
        title: t("tax:export.error_title"),
        description: t("tax:export.error_description"),
        variant: "destructive",
      });
    } finally {
      setIsExporting(false);
    }
  };

  const formatTreatmentLabel = (treatment: TaxTreatment | null): string => {
    if (!treatment) return t("tax:treatment.unclassified");
    return t(`tax:treatment.${treatment}`);
  };

  const formatWashSeverity = (severity: WashSaleSeverity): string => {
    return t(`tax:wash_sales.severity.${severity}`);
  };

  const filteredDisposals: RealizedGainRow[] = useMemo(() => {
    const rows = report?.realizedGains.disposals ?? [];
    switch (disposalFilter) {
      case "short_term":
        return rows.filter((r) => r.term === "SHORT_TERM");
      case "long_term":
        return rows.filter((r) => r.term === "LONG_TERM");
      case "wash_sale":
        return rows.filter((r) => r.washSaleRisk !== null);
      default:
        return rows;
    }
  }, [disposalFilter, report?.realizedGains.disposals]);

  const openAddLimitModal = (prefillMember?: string | null) => {
    setEditingLimit(
      prefillMember
        ? ({
            id: "",
            groupName: "",
            contributionYear: taxYear,
            limitAmount: 0,
            accountIds: "",
            isRecurring: true,
            memberName: prefillMember,
            limitCategory: null,
          } as ContributionLimit)
        : null,
    );
    setLimitModalOpen(true);
  };

  if (isLoading) {
    return (
      <div className="space-y-4">
        <div className="grid gap-4 md:grid-cols-4">
          <Skeleton className="h-28 w-full" />
          <Skeleton className="h-28 w-full" />
          <Skeleton className="h-28 w-full" />
          <Skeleton className="h-28 w-full" />
        </div>
        <Skeleton className="h-80 w-full" />
      </div>
    );
  }

  if (error || !report) {
    return (
      <Alert variant="destructive">
        <Icons.AlertTriangle className="h-4 w-4" />
        <AlertTitle>{t("tax:errors.load_title")}</AlertTitle>
        <AlertDescription>{error?.message ?? t("tax:errors.load_description")}</AlertDescription>
      </Alert>
    );
  }

  const ccy = report.baseCurrency;
  const fmt = (v: number | string) => amountFormatting.formatAmount(toNum(v), ccy);
  const netRealized = toNum(report.realizedGains.totalNetRealizedBase);
  const netShort = toNum(report.realizedGains.netShortTermBase);
  const netLong = toNum(report.realizedGains.netLongTermBase);
  const taxableGrossIncome =
    toNum(report.income.taxable.grossDividendsBase) +
    toNum(report.income.taxable.grossInterestBase) +
    toNum(report.income.taxable.grossOtherIncomeBase);
  const taxableWithholding = toNum(report.income.taxable.incomeWithholdingTaxBase);
  const standaloneTaxesPaid = toNum(report.income.taxable.standaloneTaxPaidBase);
  const iraWashCount = report.washSales.filter(
    (w) => w.severity === "IRA_PERMANENT_DISALLOWANCE",
  ).length;
  const totalContribRemaining = toNum(report.contributionLimits.totalRemainingBase);
  const totalContribLimit = toNum(report.contributionLimits.totalLimitBase);
  const totalAssetLoc = toNum(report.assetLocation.totals.totalBase);

  return (
    <div className="space-y-6">
      {/* Header & Controls */}
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <h2 className="text-xl font-semibold tracking-tight">{t("tax:title")}</h2>
          <p className="text-muted-foreground text-sm">{t("tax:subtitle")}</p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <div className="w-28">
            <ResponsiveSelect
              value={String(taxYear)}
              onValueChange={(value) => setTaxYear(Number(value))}
              options={yearOptions}
              placeholder={t("tax:filters.year_label")}
              sheetTitle={t("tax:filters.year_label")}
              triggerClassName="h-9"
            />
          </div>
          <div className="w-44">
            <ResponsiveSelect
              value={selectedMember}
              onValueChange={setSelectedMember}
              options={memberOptions}
              placeholder={t("tax:filters.member_label")}
              sheetTitle={t("tax:filters.member_label")}
              triggerClassName="h-9"
            />
          </div>
          <DropdownMenu>
            <DropdownMenuTrigger
              render={<Button variant="outline" size="sm" disabled={isExporting} className="h-9" />}
            >
              <Icons.Download className="me-2 h-4 w-4" />
              {t("tax:export.button")}
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onClick={() => handleExport("realized_gains_csv")}>
                <Icons.FileText className="me-2 h-4 w-4" />
                {t("tax:export.realized_gains_csv")}
              </DropdownMenuItem>
              <DropdownMenuItem onClick={() => handleExport("income_csv")}>
                <Icons.Receipt className="me-2 h-4 w-4" />
                {t("tax:export.income_csv")}
              </DropdownMenuItem>
              <DropdownMenuItem onClick={() => handleExport("contribution_limits_csv")}>
                <Icons.Users className="me-2 h-4 w-4" />
                {t("tax:export.contribution_limits_csv")}
              </DropdownMenuItem>
              <DropdownMenuItem onClick={() => handleExport("full_json")}>
                <Icons.Download className="me-2 h-4 w-4" />
                {t("tax:export.full_json")}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>

      {/* Contextual Warnings */}
      {report.warnings.unclassifiedAccounts.length > 0 && (
        <Alert>
          <Icons.AlertTriangle className="h-4 w-4" />
          <AlertTitle>
            {t("tax:warnings.unclassified_title", {
              count: report.warnings.unclassifiedAccounts.length,
            })}
          </AlertTitle>
          <AlertDescription className="flex flex-wrap items-center justify-between gap-2">
            <span>{t("tax:warnings.unclassified_description")}</span>
            <Link
              to="/settings/accounts"
              className={buttonVariants({ variant: "outline", size: "sm" })}
            >
              {t("tax:warnings.classify_accounts_cta")}
            </Link>
          </AlertDescription>
        </Alert>
      )}

      {(report.warnings.staleAccountIds.length > 0 ||
        report.warnings.unconvertedCurrencies.length > 0 ||
        report.warnings.holdingsModeTaxableAccounts.length > 0) && (
        <Alert>
          <Icons.Info className="h-4 w-4" />
          <AlertTitle>{t("tax:warnings.data_notices_title")}</AlertTitle>
          <AlertDescription className="space-y-1 text-xs">
            {report.warnings.staleAccountIds.length > 0 && (
              <p>
                {t("tax:warnings.stale_accounts", {
                  count: report.warnings.staleAccountIds.length,
                })}
              </p>
            )}
            {report.warnings.unconvertedCurrencies.length > 0 && (
              <p>
                {t("tax:warnings.unconverted_currencies", {
                  currencies: report.warnings.unconvertedCurrencies.join(", "),
                })}
              </p>
            )}
            {report.warnings.holdingsModeTaxableAccounts.length > 0 && (
              <p>
                {t("tax:warnings.holdings_mode_accounts", {
                  count: report.warnings.holdingsModeTaxableAccounts.length,
                })}
              </p>
            )}
          </AlertDescription>
        </Alert>
      )}

      {/* KPI Strip */}
      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <Card>
          <CardHeader className="pb-2">
            <CardDescription>{t("tax:kpi.net_realized")}</CardDescription>
            <CardTitle
              className={`text-2xl ${
                netRealized > 0 ? "text-gain" : netRealized < 0 ? "text-loss" : ""
              }`}
            >
              {fmt(netRealized)}
            </CardTitle>
          </CardHeader>
          <CardContent className="text-muted-foreground space-y-0.5 text-xs">
            <div className="flex justify-between">
              <span>{t("tax:term.SHORT_TERM")}</span>
              <span>{fmt(netShort)}</span>
            </div>
            <div className="flex justify-between">
              <span>{t("tax:term.LONG_TERM")}</span>
              <span>{fmt(netLong)}</span>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-2">
            <CardDescription>{t("tax:kpi.taxable_income")}</CardDescription>
            <CardTitle className="text-2xl">{fmt(taxableGrossIncome)}</CardTitle>
          </CardHeader>
          <CardContent className="text-muted-foreground space-y-0.5 text-xs">
            <div className="flex justify-between">
              <span>{t("tax:kpi.income_withheld")}</span>
              <span>{fmt(taxableWithholding)}</span>
            </div>
            {standaloneTaxesPaid > 0 && (
              <div className="flex justify-between">
                <span>{t("tax:kpi.standalone_tax_paid")}</span>
                <span>{fmt(standaloneTaxesPaid)}</span>
              </div>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-2">
            <CardDescription>{t("tax:kpi.wash_sale_alerts")}</CardDescription>
            <CardTitle className={`text-2xl ${report.washSales.length > 0 ? "text-warning" : ""}`}>
              {report.washSales.length}
            </CardTitle>
          </CardHeader>
          <CardContent className="text-muted-foreground text-xs">
            {iraWashCount > 0 ? (
              <span className="text-destructive font-medium">
                {t("tax:kpi.ira_wash_count", { count: iraWashCount })}
              </span>
            ) : (
              <span>{t("tax:kpi.wash_sale_hint")}</span>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-2">
            <CardDescription>{t("tax:kpi.contribution_room")}</CardDescription>
            <CardTitle className="text-2xl">{fmt(totalContribRemaining)}</CardTitle>
          </CardHeader>
          <CardContent className="text-muted-foreground space-y-0.5 text-xs">
            <div className="flex justify-between">
              <span>{t("tax:kpi.total_limit")}</span>
              <span>{fmt(totalContribLimit)}</span>
            </div>
            {report.contributionLimits.overLimitCount > 0 && (
              <span className="text-destructive font-medium">
                {t("tax:kpi.over_limit_count", {
                  count: report.contributionLimits.overLimitCount,
                })}
              </span>
            )}
          </CardContent>
        </Card>
      </div>

      {/* Main Section Tabs */}
      <Tabs defaultValue="gains" className="space-y-4">
        <TabsList className="flex flex-wrap">
          <TabsTrigger value="gains">{t("tax:tabs.gains_and_wash_sales")}</TabsTrigger>
          <TabsTrigger value="limits">{t("tax:tabs.contribution_limits")}</TabsTrigger>
          <TabsTrigger value="tlh">{t("tax:tabs.tlh")}</TabsTrigger>
          <TabsTrigger value="income_location">{t("tax:tabs.income_and_location")}</TabsTrigger>
        </TabsList>

        {/* TAB 1: Realized Gains & Wash Sales */}
        <TabsContent value="gains" className="space-y-4">
          {report.washSales.length > 0 && (
            <Card>
              <CardHeader>
                <CardTitle className="flex items-center gap-2 text-base">
                  <Icons.ShieldAlert className="text-warning h-4 w-4" />
                  {t("tax:wash_sales.title")}
                </CardTitle>
                <CardDescription>{t("tax:wash_sales.description")}</CardDescription>
              </CardHeader>
              <CardContent className="space-y-3">
                {report.washSales.map((alert) => (
                  <div
                    key={`${alert.disposalId}-${alert.replacementActivityId}`}
                    className="border-border/70 flex flex-col gap-2 rounded-lg border p-3 text-sm sm:flex-row sm:items-center sm:justify-between"
                  >
                    <div className="space-y-1">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-semibold">
                          <bdi>{alert.symbol}</bdi>
                        </span>
                        <Badge
                          variant={
                            alert.severity === "IRA_PERMANENT_DISALLOWANCE"
                              ? "destructive"
                              : "secondary"
                          }
                        >
                          {formatWashSeverity(alert.severity)}
                        </Badge>
                        {alert.confidence === "NEEDS_REVIEW" && (
                          <Badge variant="outline">{t("tax:wash_sales.needs_review")}</Badge>
                        )}
                      </div>
                      <p className="text-muted-foreground text-xs">
                        {t("tax:wash_sales.detail_line", {
                          disposalAccount: alert.disposalAccountName,
                          disposalDate: dateFormatting.formatCalendarDate(alert.disposalDate),
                          replacementAccount: alert.replacementAccountName,
                          replacementDate: dateFormatting.formatCalendarDate(alert.replacementDate),
                        })}
                      </p>
                      {alert.note && (
                        <p className="text-muted-foreground text-xs italic">{alert.note}</p>
                      )}
                    </div>
                    <div className="text-end">
                      <div className="text-destructive font-semibold">
                        {fmt(alert.estimatedDisallowedLossBase)}
                      </div>
                      <div className="text-muted-foreground text-xs">
                        {t("tax:wash_sales.est_disallowed")}
                      </div>
                    </div>
                  </div>
                ))}
              </CardContent>
            </Card>
          )}

          <Card>
            <CardHeader className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
              <div>
                <CardTitle className="text-base">{t("tax:gains.title")}</CardTitle>
                <CardDescription>{t("tax:gains.description")}</CardDescription>
              </div>
              <div className="flex flex-wrap gap-1.5">
                {(["all", "short_term", "long_term", "wash_sale"] as DisposalFilter[]).map(
                  (filterKey) => (
                    <Button
                      key={filterKey}
                      size="sm"
                      variant={disposalFilter === filterKey ? "default" : "outline"}
                      onClick={() => setDisposalFilter(filterKey)}
                    >
                      {t(`tax:gains.filter_${filterKey}`)}
                    </Button>
                  ),
                )}
              </div>
            </CardHeader>
            <CardContent>
              {filteredDisposals.length === 0 ? (
                <p className="text-muted-foreground py-6 text-center text-sm">
                  {t("tax:gains.empty")}
                </p>
              ) : (
                <div className="overflow-x-auto">
                  <Table>
                    <TableHeader>
                      <TableRow>
                        <TableHead>{t("tax:table.symbol")}</TableHead>
                        <TableHead>{t("tax:table.account")}</TableHead>
                        <TableHead>{t("tax:table.treatment")}</TableHead>
                        <TableHead>{t("tax:table.acquired")}</TableHead>
                        <TableHead>{t("tax:table.sold")}</TableHead>
                        <TableHead>{t("tax:table.term")}</TableHead>
                        <TableHead className="text-end">{t("tax:table.proceeds")}</TableHead>
                        <TableHead className="text-end">{t("tax:table.cost_basis")}</TableHead>
                        <TableHead className="text-end">{t("tax:table.gain_loss")}</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {filteredDisposals.map((row) => {
                        const pnl = toNum(row.realizedPnlBase);
                        return (
                          <TableRow key={row.disposalId}>
                            <TableCell className="font-medium">
                              <div className="flex items-center gap-1.5">
                                <bdi>{row.symbol}</bdi>
                                {row.isShortSale && (
                                  <Badge variant="outline" className="text-2xs">
                                    {t("tax:gains.short_badge")}
                                  </Badge>
                                )}
                                {row.washSaleRisk && (
                                  <Badge variant="destructive" className="text-2xs">
                                    {t("tax:gains.wash_badge")}
                                  </Badge>
                                )}
                              </div>
                            </TableCell>
                            <TableCell>
                              <div>
                                <bdi>{row.accountName}</bdi>
                              </div>
                              {row.ownerName && (
                                <div className="text-muted-foreground text-xs">
                                  <bdi>{row.ownerName}</bdi>
                                </div>
                              )}
                            </TableCell>
                            <TableCell>
                              <Badge variant="secondary">
                                {formatTreatmentLabel(row.taxTreatment)}
                              </Badge>
                            </TableCell>
                            <TableCell>
                              {row.openDate ? dateFormatting.formatCalendarDate(row.openDate) : "—"}
                            </TableCell>
                            <TableCell>
                              {dateFormatting.formatCalendarDate(row.disposalDate)}
                            </TableCell>
                            <TableCell>{t(`tax:term.${row.term}`)}</TableCell>
                            <TableCell className="text-end">{fmt(row.proceedsBase)}</TableCell>
                            <TableCell className="text-end">{fmt(row.costBasisBase)}</TableCell>
                            <TableCell
                              className={`text-end font-medium ${
                                pnl > 0 ? "text-gain" : pnl < 0 ? "text-loss" : ""
                              }`}
                            >
                              {fmt(pnl)}
                            </TableCell>
                          </TableRow>
                        );
                      })}
                    </TableBody>
                  </Table>
                </div>
              )}
            </CardContent>
          </Card>
        </TabsContent>

        {/* TAB 2: Per-Member Contribution Limits */}
        <TabsContent value="limits" className="space-y-4">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <p className="text-muted-foreground text-sm">{t("tax:limits.section_description")}</p>
            <div className="flex items-center gap-2">
              <Link
                to="/settings/contribution-limits"
                className={buttonVariants({ variant: "outline", size: "sm" })}
              >
                {t("tax:limits.manage_all")}
              </Link>
              <Button size="sm" onClick={() => openAddLimitModal(memberFilter)}>
                <Icons.Plus className="me-1.5 h-4 w-4" />
                {t("tax:limits.add_limit")}
              </Button>
            </div>
          </div>

          {report.contributionLimits.byMember.length === 0 ? (
            <Card>
              <CardContent className="text-muted-foreground py-8 text-center text-sm">
                {t("tax:limits.empty", { year: taxYear })}
              </CardContent>
            </Card>
          ) : (
            <div className="space-y-4">
              {report.contributionLimits.byMember.map((group) => (
                <Card key={group.memberName ?? "__household__"}>
                  <CardHeader className="flex flex-row items-center justify-between pb-3">
                    <div>
                      <CardTitle className="text-base">
                        <bdi>{group.memberName ?? t("tax:limits.household_unassigned")}</bdi>
                      </CardTitle>
                      <CardDescription>
                        {t("tax:limits.member_summary", {
                          contributed: fmt(group.totalContributedBase),
                          limit: fmt(group.totalLimitBase),
                          remaining: fmt(group.totalRemainingBase),
                        })}
                      </CardDescription>
                    </div>
                  </CardHeader>
                  <CardContent className="space-y-3">
                    {group.limits.map((limit) => {
                      const pct = Math.min(100, Math.max(0, toNum(limit.progressPct)));
                      const over = toNum(limit.overContributionAmount);
                      return (
                        <div
                          key={limit.id}
                          className="border-border/70 space-y-2 rounded-lg border p-3"
                        >
                          <div className="flex flex-wrap items-center justify-between gap-2">
                            <div className="flex flex-wrap items-center gap-2">
                              <span className="font-medium">
                                <bdi>{limit.groupName}</bdi>
                              </span>
                              {limit.limitCategory && (
                                <Badge variant="secondary">
                                  {t(`tax:categories.${limit.limitCategory}`, {
                                    defaultValue: limit.limitCategory,
                                  })}
                                </Badge>
                              )}
                              {limit.isProjected && (
                                <Badge variant="outline">{t("tax:limits.projected_badge")}</Badge>
                              )}
                              {over > 0 && (
                                <Badge variant="destructive">
                                  {t("tax:limits.over_limit_badge", { amount: fmt(over) })}
                                </Badge>
                              )}
                              {limit.useItOrLoseItWarning && (
                                <Badge variant="destructive">
                                  {t("tax:limits.fsa_warning", { days: limit.daysRemaining })}
                                </Badge>
                              )}
                            </div>
                            <div className="text-end text-sm font-medium">
                              {fmt(limit.contributedBase)} / {fmt(limit.limitAmount)}
                            </div>
                          </div>
                          <Progress value={pct} className="h-2" />
                          <div className="text-muted-foreground flex flex-wrap items-center justify-between gap-2 text-xs">
                            <span>
                              {t("tax:limits.deadline", {
                                date: dateFormatting.formatCalendarDate(limit.deadlineDate),
                              })}
                              {limit.isPastDeadline
                                ? ` · ${t("tax:limits.past_deadline")}`
                                : ` · ${t("tax:limits.days_remaining", { count: limit.daysRemaining })}`}
                            </span>
                            <span>
                              {t("tax:limits.remaining_label", {
                                amount: fmt(limit.remainingAmount),
                              })}
                            </span>
                          </div>
                        </div>
                      );
                    })}
                  </CardContent>
                </Card>
              ))}
            </div>
          )}
        </TabsContent>

        {/* TAB 3: Tax-Loss Harvesting */}
        <TabsContent value="tlh" className="space-y-4">
          <Card>
            <CardHeader>
              <CardTitle className="flex items-center gap-2 text-base">
                <Icons.TrendingDown className="h-4 w-4" />
                {t("tax:tlh.title")}
              </CardTitle>
              <CardDescription>{t("tax:tlh.description")}</CardDescription>
            </CardHeader>
            <CardContent>
              {report.tlhCandidates.length === 0 ? (
                <p className="text-muted-foreground py-6 text-center text-sm">
                  {t("tax:tlh.empty")}
                </p>
              ) : (
                <div className="overflow-x-auto">
                  <Table>
                    <TableHeader>
                      <TableRow>
                        <TableHead>{t("tax:table.symbol")}</TableHead>
                        <TableHead>{t("tax:table.account")}</TableHead>
                        <TableHead>{t("tax:table.acquired")}</TableHead>
                        <TableHead>{t("tax:table.term")}</TableHead>
                        <TableHead className="text-end">{t("tax:table.cost_basis")}</TableHead>
                        <TableHead className="text-end">{t("tax:table.market_value")}</TableHead>
                        <TableHead className="text-end">{t("tax:tlh.unrealized_loss")}</TableHead>
                        <TableHead>{t("tax:tlh.wash_cooldown")}</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {report.tlhCandidates.map((cand) => (
                        <TableRow key={cand.lotId}>
                          <TableCell className="font-medium">
                            <bdi>{cand.symbol}</bdi>
                          </TableCell>
                          <TableCell>
                            <div>
                              <bdi>{cand.accountName}</bdi>
                            </div>
                            {cand.ownerName && (
                              <div className="text-muted-foreground text-xs">
                                <bdi>{cand.ownerName}</bdi>
                              </div>
                            )}
                          </TableCell>
                          <TableCell>{dateFormatting.formatCalendarDate(cand.openDate)}</TableCell>
                          <TableCell>
                            <div>{t(`tax:term.${cand.currentTerm}`)}</div>
                            {cand.daysUntilLongTerm !== null && (
                              <div className="text-muted-foreground text-xs">
                                {t("tax:tlh.days_to_long_term", {
                                  count: cand.daysUntilLongTerm,
                                })}
                              </div>
                            )}
                          </TableCell>
                          <TableCell className="text-end">{fmt(cand.costBasisBase)}</TableCell>
                          <TableCell className="text-end">{fmt(cand.marketValueBase)}</TableCell>
                          <TableCell className="text-destructive text-end font-semibold">
                            -{fmt(cand.unrealizedLossBase)}
                          </TableCell>
                          <TableCell>
                            {cand.washSaleCooldownUntil ? (
                              <div className="space-y-0.5">
                                <Badge variant="destructive">
                                  <Icons.Clock className="me-1 h-3 w-3" />
                                  {t("tax:tlh.wait_until", {
                                    date: dateFormatting.formatCalendarDate(
                                      cand.washSaleCooldownUntil,
                                    ),
                                  })}
                                </Badge>
                                {cand.cooldownTriggerAccountName && (
                                  <div className="text-muted-foreground text-xs">
                                    <bdi>{cand.cooldownTriggerAccountName}</bdi>
                                  </div>
                                )}
                              </div>
                            ) : (
                              <Badge variant="outline">{t("tax:tlh.clear")}</Badge>
                            )}
                          </TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </div>
              )}
            </CardContent>
          </Card>
        </TabsContent>

        {/* TAB 4: Income, Asset Location & Readiness */}
        <TabsContent value="income_location" className="space-y-4">
          {/* Asset Location Matrix */}
          <Card>
            <CardHeader>
              <CardTitle className="flex items-center gap-2 text-base">
                <Icons.PieChart className="h-4 w-4" />
                {t("tax:asset_location.title")}
              </CardTitle>
              <CardDescription>{t("tax:asset_location.description")}</CardDescription>
            </CardHeader>
            <CardContent>
              {report.assetLocation.byCategory.length === 0 ? (
                <p className="text-muted-foreground py-6 text-center text-sm">
                  {t("tax:asset_location.empty")}
                </p>
              ) : (
                <div className="overflow-x-auto">
                  <Table>
                    <TableHeader>
                      <TableRow>
                        <TableHead>{t("tax:asset_location.category")}</TableHead>
                        <TableHead className="text-end">{t("tax:treatment.TAXABLE")}</TableHead>
                        <TableHead className="text-end">
                          {t("tax:treatment.TAX_DEFERRED")}
                        </TableHead>
                        <TableHead className="text-end">{t("tax:treatment.TAX_FREE")}</TableHead>
                        <TableHead className="text-end">
                          {t("tax:treatment.unclassified")}
                        </TableHead>
                        <TableHead className="text-end">{t("tax:asset_location.total")}</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {report.assetLocation.byCategory.map((row) => (
                        <TableRow key={row.categoryName}>
                          <TableCell className="font-medium">{row.categoryName}</TableCell>
                          <TableCell className="text-end">{fmt(row.totals.taxableBase)}</TableCell>
                          <TableCell className="text-end">
                            {fmt(row.totals.taxDeferredBase)}
                          </TableCell>
                          <TableCell className="text-end">{fmt(row.totals.taxFreeBase)}</TableCell>
                          <TableCell className="text-end">
                            {fmt(row.totals.unclassifiedBase)}
                          </TableCell>
                          <TableCell className="text-end font-semibold">
                            {fmt(row.totals.totalBase)}
                          </TableCell>
                        </TableRow>
                      ))}
                      <TableRow className="bg-muted/40 font-semibold">
                        <TableCell>{t("tax:asset_location.total")}</TableCell>
                        <TableCell className="text-end">
                          {fmt(report.assetLocation.totals.taxableBase)}
                        </TableCell>
                        <TableCell className="text-end">
                          {fmt(report.assetLocation.totals.taxDeferredBase)}
                        </TableCell>
                        <TableCell className="text-end">
                          {fmt(report.assetLocation.totals.taxFreeBase)}
                        </TableCell>
                        <TableCell className="text-end">
                          {fmt(report.assetLocation.totals.unclassifiedBase)}
                        </TableCell>
                        <TableCell className="text-end">{fmt(totalAssetLoc)}</TableCell>
                      </TableRow>
                    </TableBody>
                  </Table>
                </div>
              )}
            </CardContent>
          </Card>

          {/* Investment Income by Account */}
          <Card>
            <CardHeader>
              <CardTitle className="text-base">{t("tax:income.title")}</CardTitle>
              <CardDescription>{t("tax:income.description")}</CardDescription>
            </CardHeader>
            <CardContent>
              {report.income.byAccount.length === 0 ? (
                <p className="text-muted-foreground py-6 text-center text-sm">
                  {t("tax:income.empty")}
                </p>
              ) : (
                <div className="overflow-x-auto">
                  <Table>
                    <TableHeader>
                      <TableRow>
                        <TableHead>{t("tax:table.account")}</TableHead>
                        <TableHead>{t("tax:table.treatment")}</TableHead>
                        <TableHead className="text-end">{t("tax:income.dividends")}</TableHead>
                        <TableHead className="text-end">{t("tax:income.interest")}</TableHead>
                        <TableHead className="text-end">{t("tax:income.roc")}</TableHead>
                        <TableHead className="text-end">{t("tax:income.withheld")}</TableHead>
                        <TableHead className="text-end">{t("tax:income.net_income")}</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {report.income.byAccount.map((row) => (
                        <TableRow key={row.accountId}>
                          <TableCell className="font-medium">
                            <div>
                              <bdi>{row.accountName}</bdi>
                            </div>
                            {row.ownerName && (
                              <div className="text-muted-foreground text-xs">
                                <bdi>{row.ownerName}</bdi>
                              </div>
                            )}
                          </TableCell>
                          <TableCell>
                            <Badge variant="secondary">
                              {formatTreatmentLabel(row.taxTreatment)}
                            </Badge>
                          </TableCell>
                          <TableCell className="text-end">
                            {fmt(row.totals.grossDividendsBase)}
                          </TableCell>
                          <TableCell className="text-end">
                            {fmt(row.totals.grossInterestBase)}
                          </TableCell>
                          <TableCell className="text-end">
                            {fmt(row.totals.returnOfCapitalBase)}
                          </TableCell>
                          <TableCell className="text-end">
                            {fmt(row.totals.incomeWithholdingTaxBase)}
                          </TableCell>
                          <TableCell className="text-end font-semibold">
                            {fmt(row.totals.netInvestmentIncomeBase)}
                          </TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </div>
              )}
            </CardContent>
          </Card>

          {/* Year-End Readiness & Statement Coverage */}
          <Card>
            <CardHeader>
              <CardTitle className="text-base">{t("tax:readiness.title")}</CardTitle>
              <CardDescription>{t("tax:readiness.description")}</CardDescription>
            </CardHeader>
            <CardContent>
              {report.readiness.length === 0 ? (
                <p className="text-muted-foreground py-6 text-center text-sm">
                  {t("tax:readiness.empty")}
                </p>
              ) : (
                <div className="overflow-x-auto">
                  <Table>
                    <TableHeader>
                      <TableRow>
                        <TableHead>{t("tax:table.account")}</TableHead>
                        <TableHead>{t("tax:table.treatment")}</TableHead>
                        <TableHead>{t("tax:readiness.statements")}</TableHead>
                        <TableHead>{t("tax:readiness.needs_review")}</TableHead>
                        <TableHead>{t("tax:readiness.status")}</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {report.readiness.map((acc) => {
                        const ready =
                          acc.taxTreatment !== null && acc.needsReviewCount === 0 && !acc.isStale;
                        return (
                          <TableRow key={acc.accountId}>
                            <TableCell className="font-medium">
                              <div>
                                <bdi>{acc.accountName}</bdi>
                              </div>
                              {acc.ownerName && (
                                <div className="text-muted-foreground text-xs">
                                  <bdi>{acc.ownerName}</bdi>
                                </div>
                              )}
                            </TableCell>
                            <TableCell>
                              <Badge variant={acc.taxTreatment ? "secondary" : "outline"}>
                                {formatTreatmentLabel(acc.taxTreatment)}
                              </Badge>
                            </TableCell>
                            <TableCell>
                              {acc.accountType === "SECURITIES"
                                ? t("tax:readiness.statement_count", {
                                    count: acc.statementsInYear,
                                  })
                                : "—"}
                            </TableCell>
                            <TableCell>
                              {acc.needsReviewCount > 0 ? (
                                <Badge variant="destructive">{acc.needsReviewCount}</Badge>
                              ) : (
                                "0"
                              )}
                            </TableCell>
                            <TableCell>
                              {ready ? (
                                <span className="text-success inline-flex items-center gap-1 text-xs font-medium">
                                  <Icons.CheckCircle className="h-3.5 w-3.5" />
                                  {t("tax:readiness.ready")}
                                </span>
                              ) : (
                                <span className="text-warning inline-flex items-center gap-1 text-xs font-medium">
                                  <Icons.AlertTriangle className="h-3.5 w-3.5" />
                                  {t("tax:readiness.action_needed")}
                                </span>
                              )}
                            </TableCell>
                          </TableRow>
                        );
                      })}
                    </TableBody>
                  </Table>
                </div>
              )}
            </CardContent>
          </Card>
        </TabsContent>
      </Tabs>

      {/* Non-tax-advice disclaimer footer */}
      <p className="text-muted-foreground border-border/60 border-t pt-4 text-xs">
        {t("tax:disclaimer")}
      </p>

      <ContributionLimitEditModal
        limit={editingLimit}
        open={limitModalOpen}
        onClose={() => setLimitModalOpen(false)}
      />
    </div>
  );
}

export default TaxCenterPage;
