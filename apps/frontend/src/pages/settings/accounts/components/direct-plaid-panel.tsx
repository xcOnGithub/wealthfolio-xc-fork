import { isWeb, plaidAction } from "@/adapters";
import type {
  PlaidAction,
  PlaidConnection,
  PlaidMapping,
  PlaidPreview,
  PlaidHoldingsPreview,
  PlaidExternalAccount,
  PlaidAccountsResponse,
} from "@/adapters/shared/plaid";
import type { Account } from "@/lib/types";
import { QueryKeys } from "@/lib/query-keys";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button, Card, CardContent, PrivacyAmount, useDateFormatting } from "@wealthfolio/ui";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link, useSearchParams } from "react-router-dom";

const names: Record<string, string> = {
  chase: "Chase",
  boa: "Bank of America",
  usbank: "U.S. Bank",
  amex: "American Express",
  merrill_edge: "Merrill Edge",
  schwab: "Charles Schwab",
  robinhood: "Robinhood",
};
interface Props {
  accounts: Account[];
  onCreateAccount: () => void;
}

export function DirectPlaidPanel({ accounts, onCreateAccount }: Props) {
  const { t } = useTranslation();
  const dates = useDateFormatting();
  const client = useQueryClient();
  const [params, setParams] = useSearchParams();
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [institution, setInstitution] = useState("");
  const [duplicate, setDuplicate] = useState(false);
  const [selected, setSelected] = useState<PlaidConnection>();
  const [external, setExternal] = useState<PlaidExternalAccount[]>([]);
  const [totalAccounts, setTotalAccounts] = useState<number>();
  const [accountTypes, setAccountTypes] = useState<string[]>([]);
  const [externalId, setExternalId] = useState("");
  const [localId, setLocalId] = useState("");
  const [cutoverDate, setCutoverDate] = useState("");
  const [merrillConfirmed, setMerrillConfirmed] = useState(false);
  const [preview, setPreview] = useState<PlaidPreview>();
  const [holdingsPreview, setHoldingsPreview] = useState<PlaidHoldingsPreview>();
  const [allowOpaque, setAllowOpaque] = useState(false);
  const [opening, setOpening] = useState("");
  const [pairRecord, setPairRecord] = useState("");
  const [other, setOther] = useState<PlaidPreview>();
  const [history, setHistory] = useState<
    {
      date: string;
      name: string;
      amount: string | number;
      type: string;
      iso_currency_code?: string;
    }[]
  >([]);
  const [confirmDisconnect, setConfirmDisconnect] = useState<string>();
  const [confirmRemove, setConfirmRemove] = useState<string>();
  const handledFlow = useRef<string | undefined>(undefined);
  const status = useQuery({
    queryKey: ["direct-plaid"],
    queryFn: () => plaidAction({ operation: "status" }),
    enabled: isWeb,
    retry: false,
  });
  const mappings =
    status.data?.connections
      .filter((c) => c.product === "transactions")
      .flatMap((c) => c.accounts) ?? [];
  const refresh = async () => {
    await client.invalidateQueries({ queryKey: ["direct-plaid"] });
    await client.invalidateQueries({ queryKey: [QueryKeys.ACCOUNTS] });
  };
  const run = async <T,>(action: PlaidAction, next?: (value: T) => void | Promise<void>) => {
    setBusy(true);
    setMessage("");
    try {
      const value = await plaidAction<T>(action);
      await next?.(value);
      await refresh();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : t("connect:plaid.failed"));
    } finally {
      setBusy(false);
    }
  };
  const finish = (flow: string) =>
    run({ operation: "finish", flowId: flow }, () => {
      const next = new URLSearchParams(params);
      next.delete("plaidFlow");
      setParams(next, { replace: true });
    });
  const openAccounts = (connection: PlaidConnection) =>
    run<PlaidAccountsResponse>({ operation: "accounts", connectionId: connection.id }, (value) => {
      setSelected(connection);
      setExternal(value.accounts);
      setTotalAccounts(value.totalAccounts);
      setAccountTypes(value.accountTypes ?? []);
      setExternalId("");
      setLocalId("");
      setMerrillConfirmed(false);
    });
  useEffect(() => {
    const flow = params.get("plaidFlow");
    if (isWeb && status.data?.enabled && flow && handledFlow.current !== flow) {
      handledFlow.current = flow;
      void finish(flow);
    }
    // Each flow is consumed once; failed/unfinished flows have an explicit retry.
  }, [params, status.data?.enabled]);
  if (!isWeb || status.data?.enabled === false || status.isLoading) return null;
  const openLink = (value: { url: string }) => {
    const url = new URL(value.url);
    if (
      url.protocol !== "https:" ||
      url.hostname !== "secure.plaid.com" ||
      !url.pathname.startsWith("/hl/")
    )
      throw new Error(t("connect:plaid.failed"));
    window.location.assign(url.href);
  };
  const openPreview = (mapping: PlaidMapping) =>
    run<PlaidPreview>({ operation: "preview", mappingId: mapping.id }, (value) => {
      setPreview(value);
      setOpening(value.suggestedOpening ?? "");
      setOther(undefined);
      setPairRecord("");
    });
  const chosenExternal = external.find((a) => a.id === externalId);
  const manualAccounts = accounts.filter(
    (a) =>
      !a.provider &&
      !a.isArchived &&
      a.currency === chosenExternal?.currency &&
      a.trackingMode === (selected?.product === "investments" ? "HOLDINGS" : "TRANSACTIONS") &&
      a.accountType ===
        (selected?.product === "investments"
          ? "SECURITIES"
          : chosenExternal?.type === "credit"
            ? "CREDIT_CARD"
            : "CASH"),
  );
  const selectClass = "rounded-md border border-input bg-background px-3 py-2 text-sm";
  return (
    <Card data-testid="direct-plaid-panel">
      <CardContent className="space-y-4 pt-6">
        <h2 className="text-lg font-semibold">{t("connect:plaid.title")}</h2>
        <p className="text-muted-foreground text-sm">{t("connect:plaid.description")}</p>
        {message && (
          <p role="alert" className="text-destructive">
            {message}
          </p>
        )}
        {status.isError && (
          <Button variant="outline" onClick={() => void status.refetch()}>
            {t("common:retry")}
          </Button>
        )}
        {params.get("plaidFlow") && (
          <Button disabled={busy} onClick={() => void finish(params.get("plaidFlow")!)}>
            {t("common:retry")}
          </Button>
        )}
        <div className="flex flex-wrap items-center gap-2">
          <select
            aria-label={t("connect:plaid.institution")}
            className={selectClass}
            value={institution}
            onChange={(e) => setInstitution(e.target.value)}
          >
            <option value="">{t("common:select")}</option>
            {status.data?.institutions.map((key) => (
              <option key={key} value={key}>
                {names[key] ?? key}
              </option>
            ))}
          </select>
          <Button
            disabled={busy || !institution}
            onClick={() => void run({ operation: "start", institution, duplicate }, openLink)}
          >
            {t("connect:plaid.connect")}
          </Button>
          <Button variant="outline" disabled={busy} onClick={() => void run({ operation: "sync" })}>
            {t("connect:plaid.sync")}
          </Button>
          <Button variant="outline" onClick={onCreateAccount}>
            {t("connect:plaid.newAccount")}
          </Button>
        </div>
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={duplicate}
            onChange={(e) => setDuplicate(e.target.checked)}
          />
          {t("connect:plaid.anotherLogin")}
        </label>
        <p className="text-muted-foreground text-xs">{t("connect:plaid.limits")}</p>
        {status.data?.connections.map((c) => (
          <section key={c.id} className="space-y-2 rounded-md border p-3">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <strong>{names[c.institution] ?? c.institution}</strong>
              <span>{t(`connect:plaid.states.${c.status}`, { defaultValue: c.status })}</span>
            </div>
            <div className="flex flex-wrap gap-2">
              {["linking", "exchanging", "validating"].includes(c.status) && (
                <Button
                  variant="outline"
                  size="sm"
                  disabled={busy}
                  onClick={() => void finish(c.id)}
                >
                  {t("common:retry")}
                </Button>
              )}
              <Button
                variant="outline"
                size="sm"
                disabled={busy || c.status !== "connected"}
                onClick={() => void openAccounts(c)}
              >
                {t("connect:plaid.map")}
              </Button>
              <Button
                variant="outline"
                size="sm"
                disabled={busy || !["connected", "attention"].includes(c.status)}
                onClick={() =>
                  void run(
                    { operation: "start", institution: c.institution, reconnect: c.id },
                    openLink,
                  )
                }
              >
                {t("connect:plaid.reconnect")}
              </Button>
              <Button
                variant="outline"
                size="sm"
                disabled={busy || c.status === "disconnected"}
                onClick={() => setConfirmDisconnect(c.id)}
              >
                {t("connect:plaid.disconnect")}
              </Button>
              {c.status === "disconnected" && c.canRemove && (
                <Button
                  variant="outline"
                  size="sm"
                  disabled={busy}
                  onClick={() => setConfirmRemove(c.id)}
                >
                  {t("connect:plaid.removeConnection")}
                </Button>
              )}
            </div>
            {confirmRemove === c.id && (
              <div className="space-y-2">
                <p>{t("connect:plaid.removeConnectionConfirm")}</p>
                <Button
                  variant="destructive"
                  disabled={busy}
                  onClick={() =>
                    void run({ operation: "remove_connection", connectionId: c.id }, () => {
                      setConfirmRemove(undefined);
                      if (selected?.id === c.id) setSelected(undefined);
                    })
                  }
                >
                  {t("common:confirm")}
                </Button>
                <Button variant="ghost" onClick={() => setConfirmRemove(undefined)}>
                  {t("common:cancel")}
                </Button>
              </div>
            )}
            {confirmDisconnect === c.id && (
              <div className="space-y-2">
                <p>{t("connect:plaid.disconnectConfirm")}</p>
                <Button
                  disabled={busy}
                  onClick={() =>
                    void run({ operation: "disconnect", connectionId: c.id }, () =>
                      setConfirmDisconnect(undefined),
                    )
                  }
                >
                  {t("common:confirm")}
                </Button>
                <Button variant="ghost" onClick={() => setConfirmDisconnect(undefined)}>
                  {t("common:cancel")}
                </Button>
              </div>
            )}
            {c.accounts.map((m) => (
              <div key={m.id} className="flex flex-wrap items-center gap-2 border-t pt-2 text-sm">
                <span>{accounts.find((a) => a.id === m.accountId)?.name}</span>
                <span>{t(`connect:plaid.states.${m.status}`, { defaultValue: m.status })}</span>
                <span>
                  {m.lastSuccess ? dates.formatDate(m.lastSuccess) : t("connect:plaid.never")}
                </span>
                {m.lastError && <span role="status">{m.lastError}</span>}
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() =>
                    c.product === "transactions"
                      ? void openPreview(m)
                      : void run<PlaidHoldingsPreview>(
                          { operation: "preview_holdings", mappingId: m.id },
                          (v) => {
                            setHoldingsPreview(v);
                            setAllowOpaque(false);
                          },
                        )
                  }
                >
                  {t(
                    c.product === "transactions"
                      ? "connect:plaid.review"
                      : "connect:plaid.importHoldings",
                  )}
                </Button>
                {c.product === "investments" && (
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={busy}
                    onClick={() =>
                      void run<{ records: typeof history }>(
                        { operation: "history", mappingId: m.id },
                        (v) => setHistory(v.records),
                      )
                    }
                  >
                    {t("connect:plaid.history")}
                  </Button>
                )}
              </div>
            ))}
          </section>
        ))}
        {selected && (
          <div className="space-y-3 rounded-md border p-3">
            <p>{t("connect:plaid.mappingHelp")}</p>
            {external.length === 0 && (
              <div role="status" className="space-y-2">
                <p>
                  {t(
                    totalAccounts === 0
                      ? "connect:plaid.noProviderAccounts"
                      : "connect:plaid.noEligibleAccounts",
                  )}
                </p>
                {totalAccounts !== undefined && (
                  <p>{t("connect:plaid.providerAccountCount", { total: totalAccounts })}</p>
                )}
                {accountTypes.length > 0 && (
                  <p>
                    {t("connect:plaid.providerAccountTypes", { types: accountTypes.join(", ") })}
                  </p>
                )}
                <Button
                  variant="outline"
                  disabled={busy}
                  onClick={() => void openAccounts(selected)}
                >
                  {t("common:retry")}
                </Button>
              </div>
            )}
            <p className="text-sm">{t("connect:plaid.externalAccount")}</p>
            <select
              className={selectClass}
              aria-label={t("connect:plaid.externalAccount")}
              value={externalId}
              disabled={external.length === 0 || busy}
              onChange={(e) => {
                setExternalId(e.target.value);
                setLocalId("");
              }}
            >
              <option value="">{t("common:select")}</option>
              {external.map((a) => (
                <option value={a.id} key={a.id}>
                  {a.name}
                  {a.mask ? ` •${a.mask}` : ""}
                </option>
              ))}
            </select>
            <p className="text-sm">{t("connect:plaid.localAccount")}</p>
            <select
              className={selectClass}
              aria-label={t("connect:plaid.localAccount")}
              value={localId}
              disabled={!externalId || busy}
              onChange={(e) => setLocalId(e.target.value)}
            >
              <option value="">{t("common:select")}</option>
              {manualAccounts.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.name}
                </option>
              ))}
            </select>
            {externalId && manualAccounts.length === 0 && (
              <p role="status" className="text-sm">
                {t("connect:plaid.noLocalAccounts")}
              </p>
            )}
            {selected.product === "transactions" && (
              <label className="grid gap-2">
                {t("connect:plaid.cutover")}
                <input
                  className={selectClass}
                  type="date"
                  value={cutoverDate}
                  onChange={(e) => setCutoverDate(e.target.value)}
                />
                <span className="text-muted-foreground text-sm">
                  {t("connect:plaid.cutoverHelp")}
                </span>
              </label>
            )}
            {selected.institution === "merrill_edge" && (
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={merrillConfirmed}
                  onChange={(e) => setMerrillConfirmed(e.target.checked)}
                />
                {t("connect:plaid.merrillConfirm")}
              </label>
            )}
            <Button
              disabled={
                busy ||
                !externalId ||
                !localId ||
                (selected.institution === "merrill_edge" && !merrillConfirmed)
              }
              onClick={() =>
                void run(
                  {
                    operation: "map",
                    connectionId: selected.id,
                    externalId,
                    accountId: localId,
                    merrillConfirmed,
                    cutoverDate: cutoverDate || undefined,
                  },
                  () => setSelected(undefined),
                )
              }
            >
              {t("common:save")}
            </Button>
          </div>
        )}
        {preview && (
          <div className="space-y-3 rounded-md border p-3">
            <h3 className="font-semibold">{t("connect:plaid.review")}</h3>
            <p>{t("connect:plaid.reviewHelp")}</p>
            {!!preview.removedRecords?.length && (
              <div role="status" className="space-y-2 rounded-md border p-3">
                <p>{t("connect:plaid.removedHelp")}</p>
                {preview.removedRecords.map((record) => (
                  <div key={record.activityId} className="flex flex-wrap items-center gap-2">
                    <Link
                      className="text-primary underline"
                      to={`/accounts/${encodeURIComponent(record.accountId)}?tab=activities`}
                    >
                      {record.name} · {dates.formatDate(record.date)}
                    </Link>
                    <PrivacyAmount value={Number(record.amount)} currency={record.currency} />
                    <span className="text-muted-foreground text-xs">{record.activityId}</span>
                  </div>
                ))}
              </div>
            )}
            {preview.initial && (
              <label className="grid gap-2">
                {t("connect:plaid.opening")}
                <input
                  className={selectClass}
                  inputMode="decimal"
                  value={opening}
                  onChange={(e) => setOpening(e.target.value)}
                />
              </label>
            )}
            <div className="max-h-96 overflow-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr>
                    <th className="text-start">{t("common:date")}</th>
                    <th className="text-start">{t("common:description")}</th>
                    <th className="text-end">{t("common:amount")}</th>
                    <th className="text-start">{t("connect:plaid.classification")}</th>
                  </tr>
                </thead>
                <tbody>
                  {preview.records
                    .filter((r) => !r.pending)
                    .map((r) => (
                      <tr key={r.transaction_id}>
                        <td>{dates.formatDate(r.date)}</td>
                        <td>{r.name}</td>
                        <td className="text-end">
                          <PrivacyAmount value={Number(r.amount)} currency={preview.currency} />
                        </td>
                        <td>
                          <select
                            className={selectClass}
                            aria-label={`${t("connect:plaid.classification")} ${r.name}`}
                            value={
                              preview.decisions[r.transaction_id]?.kind ??
                              (preview.reviewRequired.includes(r.transaction_id) ? "" : "automatic")
                            }
                            disabled={busy}
                            onChange={(e) => {
                              const kind = e.target.value;
                              if (kind === "pair") {
                                setPairRecord(r.transaction_id);
                                return;
                              }
                              void run(
                                {
                                  operation: "decide",
                                  mappingId: preview.mappingId,
                                  recordId: r.transaction_id,
                                  kind,
                                },
                                async () => {
                                  const next = await plaidAction<PlaidPreview>({
                                    operation: "preview",
                                    mappingId: preview.mappingId,
                                  });
                                  setPreview(next);
                                },
                              );
                            }}
                          >
                            <option value="">{t("common:select")}</option>
                            {preview.decisions[r.transaction_id]?.group && (
                              <option value="internal" disabled>
                                {t("connect:plaid.paired")}
                              </option>
                            )}
                            <option value="automatic" disabled>
                              {t("connect:plaid.automatic")}
                            </option>
                            {["income", "expense", "refund", "external"].map((k) => (
                              <option key={k} value={k}>
                                {t(`connect:plaid.${k}`)}
                              </option>
                            ))}
                            <option
                              value="pair"
                              disabled={!!preview.decisions[r.transaction_id]?.group}
                            >
                              {t("connect:plaid.pair")}
                            </option>
                          </select>
                        </td>
                      </tr>
                    ))}
                </tbody>
              </table>
            </div>
            {pairRecord && (
              <div className="space-y-2">
                <p>{t("connect:plaid.pairHelp")}</p>
                <select
                  className={selectClass}
                  aria-label={t("connect:plaid.localAccount")}
                  value={other?.mappingId ?? ""}
                  onChange={(e) => {
                    const mappingId = e.target.value;
                    if (mappingId)
                      void run<PlaidPreview>({ operation: "preview", mappingId }, setOther);
                  }}
                >
                  <option value="">{t("common:select")}</option>
                  {mappings
                    .filter((m) => m.id !== preview.mappingId)
                    .map((m) => (
                      <option key={m.id} value={m.id}>
                        {accounts.find((a) => a.id === m.accountId)?.name}
                      </option>
                    ))}
                </select>
                {other?.records
                  .filter((r) => !r.pending)
                  .map((r) => (
                    <Button
                      variant="outline"
                      key={r.transaction_id}
                      disabled={busy}
                      onClick={() =>
                        void run(
                          {
                            operation: "pair",
                            mappingId: preview.mappingId,
                            recordId: pairRecord,
                            otherMappingId: other.mappingId,
                            otherRecordId: r.transaction_id,
                          },
                          async () => {
                            setPreview(
                              await plaidAction<PlaidPreview>({
                                operation: "preview",
                                mappingId: preview.mappingId,
                              }),
                            );
                            setPairRecord("");
                            setOther(undefined);
                          },
                        )
                      }
                    >
                      {r.name} · {dates.formatDate(r.date)} ·{" "}
                      <PrivacyAmount value={Number(r.amount)} currency={other.currency} />
                    </Button>
                  ))}
              </div>
            )}
            <div className="flex flex-wrap gap-2">
              <Button
                disabled={
                  busy || preview.reviewRequired.length > 0 || (preview.initial && !opening.trim())
                }
                onClick={() =>
                  void run(
                    {
                      operation: "approve",
                      mappingId: preview.mappingId,
                      digest: preview.digest,
                      openingBalance: preview.initial ? opening : undefined,
                    },
                    () => setPreview(undefined),
                  )
                }
              >
                {t("common:confirm")}
              </Button>
              <Button
                variant="outline"
                disabled={busy}
                onClick={() =>
                  void run<PlaidPreview>(
                    { operation: "preview", mappingId: preview.mappingId, refresh: true },
                    (value) => {
                      setPreview(value);
                      setOpening(value.suggestedOpening ?? "");
                    },
                  )
                }
              >
                {t("common:refresh")}
              </Button>
              <Button variant="ghost" onClick={() => setPreview(undefined)}>
                {t("common:close")}
              </Button>
            </div>
          </div>
        )}
        {holdingsPreview && (
          <section className="space-y-3 rounded-md border p-3">
            <h3 className="font-semibold">{t("connect:plaid.importHoldings")}</h3>
            <p>{t("connect:plaid.historyHelp")}</p>
            <p>{dates.formatDate(holdingsPreview.fetchedAt)}</p>
            <ul>
              {holdingsPreview.holdings.map((h) => {
                const security = holdingsPreview.securities.find(
                  (v) => v.security_id === h.security_id,
                );
                return (
                  <li key={h.security_id}>
                    {security?.name ?? security?.ticker_symbol ?? h.security_id} ·{" "}
                    <PrivacyAmount
                      value={Number(h.institution_value)}
                      currency={holdingsPreview.currency}
                    />
                    {h.cost_basis === null ? ` · ${t("connect:plaid.unknownBasis")}` : ""}
                    {h.institution_price_as_of
                      ? ` · ${dates.formatDate(h.institution_price_as_of)}`
                      : ""}
                  </li>
                );
              })}
            </ul>
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={allowOpaque}
                onChange={(e) => setAllowOpaque(e.target.checked)}
              />
              {t("connect:plaid.opaqueConfirm")}
            </label>
            <Button
              disabled={busy}
              onClick={() =>
                void run(
                  {
                    operation: "approve_holdings",
                    mappingId: holdingsPreview.mappingId,
                    digest: holdingsPreview.digest,
                    allowOpaque,
                  },
                  () => setHoldingsPreview(undefined),
                )
              }
            >
              {t("common:confirm")}
            </Button>
            <Button variant="ghost" onClick={() => setHoldingsPreview(undefined)}>
              {t("common:close")}
            </Button>
          </section>
        )}
        {history.length > 0 && (
          <section>
            <p>{t("connect:plaid.historyHelp")}</p>
            <ul>
              {history.map((r, i) => (
                <li key={i}>
                  {dates.formatDate(r.date)} · {r.name} · {r.type} ·{" "}
                  <PrivacyAmount value={Number(r.amount)} currency={r.iso_currency_code ?? "USD"} />
                </li>
              ))}
            </ul>
            <Button variant="ghost" onClick={() => setHistory([])}>
              {t("common:close")}
            </Button>
          </section>
        )}
      </CardContent>
    </Card>
  );
}
