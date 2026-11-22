import type { Asset } from "@/lib/types";
import { invoke } from "./platform";

export interface StatementPosition {
  label: string;
  quantity: string;
  marketValue: string | null;
  costBasis: string | null;
}
export interface StatementSnapshot {
  date: string;
  currency: string;
  totalValue: string;
  cashValue: string | null;
  positions: StatementPosition[];
  positionsComplete: boolean;
  format: string;
  sourcePage: number;
}
export interface StoredStatement {
  snapshot: StatementSnapshot;
  revision: string;
}
export interface StatementWrite {
  snapshot: StatementSnapshot;
  expectedRevision: string | null;
}
export interface StatementValue {
  source?: "statementClosing" | "monthlyCarryForward" | "statementDaily";
  quality?: StatementPriceQuality;
  cashValue: string | null;
  date: string;
  statementDate: string;
  value: string;
  currency: string;
}
export const listStatements = (accountId: string) =>
  invoke<StoredStatement[]>("list_statements", { accountId });
export const saveStatements = (accountId: string, writes: StatementWrite[], confirmed: boolean) =>
  invoke<void>("save_statements", { accountId, writes, confirmed });
export const deleteStatement = (accountId: string, date: string, revision: string) =>
  invoke<void>("delete_statement", { accountId, date, revision });
export const getStatementValues = (accountId: string, startDate: string, endDate: string) =>
  invoke<StatementValue[]>("get_statement_values", { accountId, startDate, endDate });

export type StatementPriceQuality =
  | "COMPLETE"
  | "STALE"
  | "PARTIAL_UNPRICED"
  | "UNAVAILABLE"
  | "CLOSING_PRICE_MISMATCH";
export interface StatementEvent {
  sourceId: string;
  date: string;
  kind: "BUY" | "SELL" | "TRANSFER_IN" | "TRANSFER_OUT" | "CASH";
  assetId: string | null;
  quantity: string | null;
  cash: string;
  sourcePage: number;
  sourceLine: number;
}
export interface StatementActivityRow extends StatementEvent {
  statementDate: string;
  currency: string;
  assetLabel: string | null;
}
export interface StatementActivityPage {
  rows: StatementActivityRow[];
  nextOffset: number | null;
}
export const listStatementActivities = (accountId: string, offset: number) =>
  invoke<StatementActivityPage>("list_statement_activities", { accountId, offset });
export interface StatementInterval {
  datePolicy: "settlement";
  from: string;
  through: string;
  currency: string;
  openingCash: string;
  closingCash: string;
  closingValue: string;
  closingPositionsComplete: boolean;
  activityComplete: boolean;
  closingPositions: {
    assetId: string;
    quantity: string;
    marketValue: string;
    costBasis: string | null;
  }[];
  events: StatementEvent[];
}
export interface DailyStatementWrite {
  closingAssetIds: string[];
  activitySecurityKeys?: Record<string, string>;
  snapshot: StatementSnapshot;
  interval: StatementInterval;
  expectedRevision: string | null;
  expectedSnapshotRevision: string | null;
}
export interface StoredDailyStatement {
  closingAssetIds: string[];
  activitySecurityKeys?: Record<string, string>;
  reviewOrder?: number;
  priced?: DailyStatementPreview["priced"] | null;
  liveHandoff?: "unverified" | "match" | "mismatch";
  version: number;
  interval: StatementInterval;
  revision: string;
}
export interface DailyStatementPreview {
  reconstructed: {
    from: string;
    through: string;
    currency: string;
    openingCash: string;
    openingPositions: Record<string, string>;
    days: { date: string; cash: string; positions: Record<string, string> }[];
  };
  priced: {
    from: string;
    through: string;
    currency: string;
    closingPricesMatch: boolean | null;
    returnsEligible: false;
    days: {
      date: string;
      cash: string;
      value: string | null;
      quality: StatementPriceQuality;
      staleAssets: string[];
    }[];
  };
  liveHandoff: "unverified" | "match" | "mismatch";
}
export const registerStatementSecurities = (accountId: string, tickers: string[]) =>
  invoke<Record<string, Asset>>("register_statement_securities", { accountId, tickers });

export const listDailyStatements = (accountId: string) =>
  invoke<StoredDailyStatement[]>("list_daily_statements", { accountId });
export const previewDailyStatements = (accountId: string, writes: DailyStatementWrite[]) =>
  invoke<DailyStatementPreview[]>("preview_daily_statements", {
    accountId,
    writes,
    confirmed: false,
  });
export const saveDailyStatements = (
  accountId: string,
  writes: DailyStatementWrite[],
  confirmed: boolean,
) => invoke<void>("save_daily_statements", { accountId, writes, confirmed });
export const refreshStatementQuotes = (accountId: string, writes: DailyStatementWrite[]) =>
  invoke<number>("refresh_statement_quotes", { accountId, writes, confirmed: false });
