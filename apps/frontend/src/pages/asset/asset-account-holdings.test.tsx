import { render, screen, within } from "@/test/render";
import { expect, it, vi } from "vitest";
import type { Holding } from "@/lib/types";
import { AssetAccountHoldings } from "./asset-account-holdings";

const state = vi.hoisted(() => ({ mobile: false, holdings: [] as Holding[] }));
vi.mock("@/adapters", () => ({
  getAssetHoldings: vi.fn(),
  getSnapshotByDate: vi.fn(),
  getSnapshots: vi.fn(),
  saveManualHoldings: vi.fn(),
  deleteSnapshot: vi.fn(),
}));
vi.mock("@/hooks/use-platform", () => ({ useIsMobileViewport: () => state.mobile }));
vi.mock("@/hooks/use-accounts", () => ({
  useAccounts: () => ({
    accounts: [{ id: "a", name: "Synthetic brokerage", trackingMode: "HOLDINGS" }],
  }),
}));
vi.mock("@/hooks/use-balance-privacy", () => ({
  useBalancePrivacy: () => ({ isBalanceHidden: false }),
}));
vi.mock("@tanstack/react-query", () => ({
  useQuery: () => ({ data: state.holdings, isLoading: false }),
  useQueries: () => [],
  useQueryClient: () => ({ invalidateQueries: vi.fn() }),
}));
vi.mock("@/pages/holdings/components/holdings-edit-mode", () => ({ HoldingsEditMode: () => null }));

it.each([false, true])(
  "shows unknown gain as unavailable and keeps actual zero on mobile=%s",
  (mobile) => {
    state.mobile = mobile;
    const holding: Holding = {
      id: "h",
      accountId: "a",
      holdingType: "security",
      quantity: 1,
      localCurrency: "USD",
      baseCurrency: "USD",
      marketValue: { local: 100, base: 100 },
      costBasis: null,
      totalGain: null,
      totalGainPct: null,
      weight: 1,
      asOfDate: "2026-11-01",
    };
    state.holdings = [holding];
    const { rerender } = render(<AssetAccountHoldings assetId="stock" baseCurrency="USD" />);
    expect(screen.getAllByText("—").length).toBeGreaterThan(0);
    if (!mobile) {
      const cells = within(screen.getAllByRole("row")[1]).getAllByRole("cell");
      expect(cells[3]).toHaveTextContent("—");
      expect(cells[4]).not.toHaveTextContent("0.00");
    }
    state.holdings = [
      {
        ...holding,
        costBasis: { local: 0, base: 0 },
        totalGain: { local: 0, base: 0 },
        totalGainPct: 0,
      },
    ];
    rerender(<AssetAccountHoldings assetId="stock" baseCurrency="USD" />);
    expect(screen.queryByText("—")).not.toBeInTheDocument();
    if (!mobile)
      expect(within(screen.getAllByRole("row")[1]).getAllByRole("cell")[3]).toHaveTextContent(
        "0.00",
      );
  },
);
