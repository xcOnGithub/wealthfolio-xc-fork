import { beforeEach, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@/test/render";
import { MemoryRouter } from "react-router-dom";
import { DigestCard, ItemCard } from "./family-page";
import type { FamilyItem, MonthlyFamilyDigest } from "@/features/family/types";
const privacy = vi.hoisted(() => ({ isBalanceHidden: false }));
vi.mock("@/hooks/use-balance-privacy", () => ({ useBalancePrivacy: () => privacy }));
vi.mock("@/adapters", () => ({ previewFamilyDigest: vi.fn() }));
const digest: MonthlyFamilyDigest = {
  month: "2026-10",
  generatedAt: "2026-11-01T17:00:00Z",
  currency: "USD",
  netWorth: null,
  netWorthChange: null,
  income: null,
  spending: 0,
  savingsRate: null,
  movers: [],
  goals: [],
  upcomingBills: [],
  warnings: ["Synthetic incomplete data"],
  text: "",
};
beforeEach(() => {
  privacy.isBalanceHidden = false;
});
it("shows unavailable comparisons separately from genuine zero spending", () => {
  render(<DigestCard digest={digest} />);
  expect(screen.getAllByText("—")).toHaveLength(3);
  expect(screen.getByText(/0\.00/)).toBeVisible();
  expect(screen.getByText("Synthetic incomplete data")).toBeVisible();
});
it("respects the existing hide-balances preference", () => {
  privacy.isBalanceHidden = true;
  render(
    <DigestCard
      digest={{ ...digest, netWorthChange: 12345, income: 67890, spending: 999, savingsRate: 0.25 }}
    />,
  );
  expect(screen.getAllByText("••••")).toHaveLength(4);
  expect(screen.queryByText(/12,345|67,890|999\.00|25%/)).not.toBeInTheDocument();
});

it.each([true, false])(
  "reopens a dismissed item only while its condition is active (%s)",
  (active) => {
    const action = vi.fn();
    const item: FamilyItem = {
      id: "synthetic-item",
      key: "synthetic:key",
      source: "charges",
      kind: "large_charge",
      severity: "warning",
      title: "Synthetic charge",
      detail: "Synthetic only",
      href: "/activities",
      amount: 1200,
      currency: "USD",
      notify: true,
      status: "resolved",
      active,
      firstSeen: "2026-11-01T17:00:00Z",
      updatedAt: "2026-11-01T17:00:00Z",
      snoozedUntil: null,
      note: "",
    };
    render(
      <MemoryRouter>
        <ItemCard item={item} busy={false} onAction={action} />
      </MemoryRouter>,
    );
    if (active) {
      fireEvent.click(screen.getByRole("button", { name: "Reopen" }));
      expect(action).toHaveBeenCalledWith({ id: "synthetic-item", action: "reopen" });
    } else {
      expect(screen.queryByRole("button", { name: "Reopen" })).not.toBeInTheDocument();
    }
  },
);
