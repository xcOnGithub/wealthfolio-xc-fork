import { invoke } from "./platform";

export interface PlaidMapping {
  id: string;
  accountId: string;
  status: string;
  lastSuccess?: string;
  lastError?: string;
}
export interface PlaidConnection {
  id: string;
  institution: string;
  product: "transactions" | "investments";
  status: string;
  accounts: PlaidMapping[];
  canRemove?: boolean;
}
export interface PlaidExternalAccount {
  id: string;
  name: string;
  mask?: string;
  type: string;
  currency: string;
}
export interface PlaidAccountsResponse {
  accounts: PlaidExternalAccount[];
  totalAccounts?: number;
  accountTypes?: string[];
  accountClassifications?: { type: string; subtype: string | null }[];
}
export interface PlaidStatus {
  enabled: boolean;
  institutions: string[];
  connections: PlaidConnection[];
}
export interface PlaidRecord {
  transaction_id: string;
  date: string;
  amount: string | number;
  name: string;
  pending: boolean;
}
export interface PlaidPreview {
  mappingId: string;
  digest: string;
  initial: boolean;
  currency: string;
  suggestedOpening?: string;
  records: PlaidRecord[];
  reviewRequired: string[];
  removedRecords?: {
    activityId: string;
    accountId: string;
    name: string;
    date: string;
    amount: string | number;
    currency: string;
  }[];
  decisions: Record<string, { kind: string; group?: string }>;
}
export type PlaidAction =
  | { operation: "status" | "sync" }
  | { operation: "start"; institution: string; reconnect?: string; duplicate?: boolean }
  | { operation: "finish"; flowId: string }
  | { operation: "accounts" | "disconnect" | "remove_connection"; connectionId: string }
  | {
      operation: "map";
      connectionId: string;
      externalId: string;
      accountId: string;
      merrillConfirmed: boolean;
      cutoverDate?: string;
    }
  | { operation: "preview"; mappingId: string; refresh?: boolean }
  | { operation: "decide"; mappingId: string; recordId: string; kind: string }
  | {
      operation: "pair";
      mappingId: string;
      recordId: string;
      otherMappingId: string;
      otherRecordId: string;
    }
  | { operation: "approve"; mappingId: string; digest: string; openingBalance?: string }
  | { operation: "preview_holdings"; mappingId: string }
  | { operation: "approve_holdings"; mappingId: string; digest: string; allowOpaque: boolean }
  | { operation: "history"; mappingId: string; offset?: number };

export function plaidAction<T = PlaidStatus>(action: PlaidAction): Promise<T> {
  return invoke<T>("plaid_action", { ...action });
}

export interface PlaidHoldingsPreview {
  mappingId: string;
  digest: string;
  currency: string;
  holdings: {
    security_id: string;
    quantity: number;
    institution_value: number;
    cost_basis: number | null;
    institution_price_as_of?: string;
  }[];
  securities: { security_id: string; name?: string; ticker_symbol?: string; type?: string }[];
  transactionCount: number;
  fetchedAt: string;
}
