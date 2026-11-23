export interface FamilyBill {
  id: string;
  name: string;
  dueDate: string;
  amount: number;
  currency: string;
  annual: boolean;
}
export interface FamilySettings {
  enabled: boolean;
  largeChargeAmount: number;
  unusualMultiplier: number;
  unusualMinimum: number;
  budgetThreshold: number;
  consentNoticeDays: number;
  staleValuationDays: number;
  billNoticeDays: number;
  emailAlerts: boolean;
  emailDigest: boolean;
  webhookAlerts: boolean;
  webhookDigest: boolean;
  emailFrom: string;
  recipients: string[];
  bills: FamilyBill[];
}
export interface FamilyItem {
  id: string;
  key: string;
  source: string;
  kind: string;
  severity: string;
  title: string;
  detail: string;
  href: string;
  amount: number | null;
  currency: string | null;
  notify: boolean;
  status: "open" | "acknowledged" | "snoozed" | "resolved";
  active: boolean;
  firstSeen: string;
  updatedAt: string;
  snoozedUntil: string | null;
  note: string;
}
export interface FamilyGoal {
  title: string;
  progress: number | null;
  current: number | null;
  target: number | null;
  currency: string | null;
}
export interface MonthlyFamilyDigest {
  month: string;
  generatedAt: string;
  currency: string;
  netWorth: number | null;
  netWorthChange: number | null;
  income: number | null;
  spending: number | null;
  savingsRate: number | null;
  movers: { name: string; current: number; previous: number; change: number }[];
  goals: FamilyGoal[];
  upcomingBills: FamilyBill[];
  warnings: string[];
  text: string;
}
export interface FamilyDelivery {
  id: string;
  channel: "email" | "webhook";
  configRevision: number;
  notification: { subject: string; text: string; digestMonth: string | null; itemIds: string[] };
  status: "pending" | "sending" | "delivered" | "failed" | "cancelled";
  createdAt: string;
  updatedAt: string;
  error: string | null;
}
export interface FamilySnapshot {
  settings: FamilySettings;
  configRevision: number;
  items: FamilyItem[];
  digests: Record<string, MonthlyFamilyDigest>;
  deliveries: FamilyDelivery[];
  lastEvaluated: string | null;
  warnings: string[];
  resendConfigured: boolean;
  webhookConfigured: boolean;
  webhookTokenConfigured: boolean;
}
export interface SaveFamilySettings {
  settings: FamilySettings;
  resendApiKey?: string;
  webhookToken?: string;
  webhookUrl?: string;
}
export interface FamilyItemAction {
  id: string;
  action: "acknowledge" | "resolve" | "reopen" | "snooze" | "note";
  snoozedUntil?: string;
  note?: string;
}
export interface FamilyTask {
  accountId: string;
  title: string;
  detail: string;
  resolved: boolean;
}
