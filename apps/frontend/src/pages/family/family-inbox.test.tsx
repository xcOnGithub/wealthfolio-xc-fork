import { beforeEach, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { fireEvent, render, screen, within } from "@/test/render";
import type { FamilyItem, FamilySnapshot } from "@/features/family/types";
import FamilyPage from "./family-page";

function renderPage() {
  const client = new QueryClient();
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <FamilyPage />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

const mocks = vi.hoisted(() => ({
  refreshMutate: vi.fn(),
  actionMutate: vi.fn(),
  bulkActionMutate: vi.fn(),
  settingsMutateAsync: vi.fn(),
  retryMutate: vi.fn(),
  snapshot: null as FamilySnapshot | null,
}));

vi.mock("@/hooks/use-balance-privacy", () => ({
  useBalancePrivacy: () => ({ isBalanceHidden: false }),
}));
vi.mock("@/lib/settings-provider", () => ({
  useSettingsContext: () => ({ settings: { baseCurrency: "USD", timezone: "UTC" } }),
}));
vi.mock("@/features/profiles/session", () => ({
  selectedProfileId: () => "test-profile",
}));
vi.mock("@/adapters", () => ({
  previewFamilyDigest: vi.fn(),
}));
vi.mock("@/features/family/use-family", () => ({
  useFamily: () => ({
    data: mocks.snapshot,
    isError: false,
    refresh: { mutate: mocks.refreshMutate, isPending: false, isError: false },
    settings: { mutateAsync: mocks.settingsMutateAsync, isPending: false, isSuccess: false },
    action: { mutate: mocks.actionMutate, isPending: false, isError: false },
    bulkAction: { mutate: mocks.bulkActionMutate, isPending: false, isError: false },
    retry: { mutate: mocks.retryMutate, isPending: false, isError: false },
  }),
}));

function makeItem(overrides: Partial<FamilyItem> & { id: string; title: string }): FamilyItem {
  return {
    key: `key:${overrides.id}`,
    source: "transactions",
    kind: "uncategorized",
    severity: "info",
    detail: `Detail for ${overrides.title}`,
    href: "/activities",
    amount: 42,
    currency: "USD",
    notify: false,
    status: "open",
    active: true,
    firstSeen: "2026-11-01T17:00:00Z",
    updatedAt: "2026-11-01T17:00:00Z",
    snoozedUntil: null,
    note: "",
    ...overrides,
  };
}

function makeSnapshot(items: FamilyItem[]): FamilySnapshot {
  return {
    settings: {
      enabled: true,
      largeChargeAmount: 1000,
      unusualMultiplier: 3,
      unusualMinimum: 100,
      budgetThreshold: 0.8,
      consentNoticeDays: 14,
      staleValuationDays: 90,
      billNoticeDays: 30,
      emailAlerts: false,
      emailDigest: false,
      webhookAlerts: false,
      webhookDigest: false,
      emailFrom: "",
      recipients: [],
      bills: [],
    },
    configRevision: 1,
    items,
    digests: {},
    deliveries: [],
    lastEvaluated: "2026-11-01T17:00:00Z",
    warnings: [],
    resendConfigured: false,
    webhookConfigured: false,
    webhookTokenConfigured: false,
  };
}

beforeEach(() => {
  mocks.refreshMutate.mockReset();
  mocks.actionMutate.mockReset();
  mocks.bulkActionMutate.mockReset();
  mocks.snapshot = makeSnapshot([
    makeItem({ id: "item-1", title: "Costco", firstSeen: "2026-11-03T10:00:00Z", status: "open" }),
    makeItem({
      id: "item-2",
      title: "Walmart",
      firstSeen: "2026-11-02T10:00:00Z",
      status: "acknowledged",
    }),
    makeItem({ id: "item-3", title: "Target", firstSeen: "2026-11-01T10:00:00Z", status: "open" }),
    makeItem({
      id: "item-4",
      title: "Old active",
      firstSeen: "2026-10-20T10:00:00Z",
      status: "resolved",
      active: true,
    }),
    makeItem({
      id: "item-5",
      title: "Old inactive",
      firstSeen: "2026-10-19T10:00:00Z",
      status: "resolved",
      active: false,
    }),
  ]);
});

it("selects multiple items, sends eligible IDs for bulk mark read and dismiss, and clears selection on success", () => {
  mocks.bulkActionMutate.mockImplementation((_input, options?: { onSuccess?: () => void }) => {
    options?.onSuccess?.();
  });

  renderPage();

  const toolbar = screen.getByRole("region", { name: "Bulk actions" });
  expect(within(toolbar).queryByRole("button", { name: "Dismiss" })).not.toBeInTheDocument();

  fireEvent.click(screen.getByRole("checkbox", { name: "Select Costco" }));
  fireEvent.click(screen.getByRole("checkbox", { name: "Select Walmart" }));

  expect(within(toolbar).getByText("2 selected")).toBeVisible();

  // Only "Costco" is open ("Walmart" is already acknowledged), so Mark read targets only item-1.
  fireEvent.click(within(toolbar).getByRole("button", { name: "Mark read" }));
  expect(mocks.bulkActionMutate).toHaveBeenCalledWith(
    { ids: ["item-1"], action: "acknowledge" },
    expect.any(Object),
  );
  // Selection cleared after onSuccess.
  expect(within(toolbar).queryByRole("button", { name: "Dismiss" })).not.toBeInTheDocument();

  // Select all visible attention items (item-1, item-2, item-3) and dismiss.
  fireEvent.click(within(toolbar).getByRole("checkbox", { name: "Select all" }));
  expect(within(toolbar).getByText("3 selected")).toBeVisible();

  fireEvent.click(within(toolbar).getByRole("button", { name: "Dismiss" }));
  expect(mocks.bulkActionMutate).toHaveBeenLastCalledWith(
    { ids: ["item-1", "item-2", "item-3"], action: "resolve" },
    expect.any(Object),
  );
});

it("supports shift-click range selection and bulk snooze", () => {
  renderPage();

  fireEvent.click(screen.getByRole("checkbox", { name: "Select Costco" }));
  fireEvent.click(screen.getByRole("checkbox", { name: "Select Target" }), { shiftKey: true });

  const toolbar = screen.getByRole("region", { name: "Bulk actions" });
  expect(within(toolbar).getByText("3 selected")).toBeVisible();

  fireEvent.click(within(toolbar).getByRole("button", { name: "Snooze 7 days" }));
  expect(mocks.bulkActionMutate).toHaveBeenCalledWith(
    expect.objectContaining({
      ids: ["item-1", "item-2", "item-3"],
      action: "snooze",
      snoozedUntil: expect.any(String),
    }),
    expect.any(Object),
  );
});

it("clears selection on filter change and only bulk-reopens active resolved items", () => {
  renderPage();

  fireEvent.click(screen.getByRole("checkbox", { name: "Select Costco" }));
  const toolbar = screen.getByRole("region", { name: "Bulk actions" });
  expect(within(toolbar).getByText("1 selected")).toBeVisible();

  fireEvent.change(screen.getByRole("combobox", { name: "Filter inbox" }), {
    target: { value: "resolved" },
  });
  expect(within(toolbar).queryByText("1 selected")).not.toBeInTheDocument();

  fireEvent.click(within(toolbar).getByRole("checkbox", { name: "Select all" }));
  expect(within(toolbar).getByText("2 selected")).toBeVisible();

  fireEvent.click(within(toolbar).getByRole("button", { name: "Reopen" }));
  expect(mocks.bulkActionMutate).toHaveBeenCalledWith(
    { ids: ["item-4"], action: "reopen" },
    expect.any(Object),
  );
});
