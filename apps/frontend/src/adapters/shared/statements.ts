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
