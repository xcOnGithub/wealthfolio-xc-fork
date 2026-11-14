import { cleanup, fireEvent, render, screen } from "@/test/render";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { PlaidInvestmentPreview } from "@/adapters/shared/plaid";
import messages from "@/i18n/locales/en/connect.json";
import { PlaidInvestmentReview } from "./plaid-investment-review";

afterEach(() => {
  cleanup();
  document.documentElement.dir = "ltr";
});
const preview = (): PlaidInvestmentPreview => ({
  mappingId: "mapping",
  digest: "digest",
  initial: true,
  startDate: "2026-01-01",
  currency: "USD",
  openingCash: "100",
  currentCash: "90",
  openingPositions: [
    { securityId: "security", name: "Synthetic equity", quantity: "2", unitCost: null },
  ],
  records: [],
  issues: [],
  sourceTransactionCount: 0,
});
describe("reviewed brokerage conversion", () => {
  it.each(["ltr", "rtl"])(
    "requires known opening basis and explicit approval in %s",
    (direction) => {
      document.documentElement.dir = direction;
      const approve = vi.fn();
      render(
        <PlaidInvestmentReview
          preview={preview()}
          busy={false}
          onApprove={approve}
          onClose={vi.fn()}
        />,
      );
      const confirm = screen.getByRole("button", { name: "Confirm" });
      const cost = screen.getByRole("textbox");
      expect(confirm).toBeDisabled();
      fireEvent.click(screen.getByRole("checkbox"));
      for (const value of ["", "-1", "Infinity", "unknown"]) {
        fireEvent.change(cost, { target: { value } });
        expect(confirm).toBeDisabled();
      }
      fireEvent.change(cost, { target: { value: "12.50" } });
      expect(confirm).toBeEnabled();
      fireEvent.click(confirm);
      expect(approve).toHaveBeenCalledExactlyOnceWith({ security: "12.50" });
      expect(screen.getByText(messages.plaid.investments.help)).toBeVisible();
      expect(screen.getByText(/Available history starts Jan 1, 2026/)).toBeVisible();
    },
  );
  it.each(["unsupportedInstrument", "reconciliation"])("cannot skip a %s blocker", (reason) => {
    const p = preview();
    p.openingPositions = [];
    p.issues = [{ recordId: "s", name: "Synthetic option", reason }];
    render(
      <PlaidInvestmentReview preview={p} busy={false} onApprove={vi.fn()} onClose={vi.fn()} />,
    );
    fireEvent.click(screen.getByRole("checkbox"));
    expect(screen.getByRole("button", { name: "Confirm" })).toBeDisabled();
    expect(screen.getByRole("alert")).toHaveTextContent(
      messages.plaid.investments.issues[reason as keyof typeof messages.plaid.investments.issues],
    );
  });
  it("keeps the committed baseline fixed and exposes all source records through pagination", () => {
    const p = preview();
    p.initial = false;
    p.openingPositions[0].unitCost = "5";
    p.records = Array.from({ length: 51 }, (_, index) => ({
      id: String(index),
      date: "2026-01-02",
      type: "BUY",
      name: `Synthetic trade ${index}`,
      securityId: "security",
      quantity: "1",
      price: "10",
      cash: "-10",
      fee: "0",
    }));
    render(
      <PlaidInvestmentReview preview={p} busy={false} onApprove={vi.fn()} onClose={vi.fn()} />,
    );
    expect(screen.getByRole("textbox")).toBeDisabled();
    expect(screen.getByText("Synthetic trade 0")).toBeVisible();
    expect(screen.queryByText("Synthetic trade 50")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByText("Synthetic trade 50")).toBeVisible();
    expect(screen.getByRole("button", { name: "Next" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Previous" }));
    expect(screen.getByText("Synthetic trade 0")).toBeVisible();
    fireEvent.click(screen.getByRole("checkbox"));
    expect(screen.getByRole("button", { name: "Confirm" })).toBeEnabled();
  });
  it("prevents repeated approval while saving", () => {
    const p = preview();
    p.openingPositions = [];
    render(<PlaidInvestmentReview preview={p} busy onApprove={vi.fn()} onClose={vi.fn()} />);
    expect(screen.getByRole("checkbox")).toBeDisabled();
    expect(screen.getByRole("button", { name: "Confirm" })).toBeDisabled();
  });
});

it.each(["ltr", "rtl"])("requires explicit incoming-transfer basis in %s", (direction) => {
  document.documentElement.dir = direction;
  const p = preview();
  p.openingPositions = [];
  p.records = [
    {
      id: "investment:transfer",
      date: "2026-01-02",
      type: "TRANSFER_IN",
      securityId: "security",
      name: "Synthetic incoming",
      quantity: "3",
      price: "0",
      cash: "0",
      fee: "0",
      requiresTransferCost: true,
      transferUnitCost: null,
    },
  ];
  const approve = vi.fn();
  render(<PlaidInvestmentReview preview={p} busy={false} onApprove={approve} onClose={vi.fn()} />);
  const cost = screen.getByRole("textbox");
  const confirm = screen.getByRole("button", { name: "Confirm" });
  fireEvent.click(screen.getByRole("checkbox"));
  expect(confirm).toBeDisabled();
  fireEvent.change(cost, { target: { value: "4.50" } });
  expect(confirm).toBeDisabled();
  fireEvent.click(screen.getByRole("checkbox"));
  expect(confirm).toBeEnabled();
  fireEvent.click(confirm);
  expect(approve).toHaveBeenCalledExactlyOnceWith({}, { "investment:transfer": "4.50" });
});
it("retains reviewed transfer cost while permitting review of a new transfer", () => {
  const p = preview();
  p.initial = false;
  p.openingPositions = [];
  p.records = [
    {
      id: "investment:old",
      date: "2026-01-02",
      type: "TRANSFER_IN",
      securityId: "security",
      name: "Reviewed incoming",
      quantity: "3",
      price: "0",
      cash: "0",
      fee: "0",
      requiresTransferCost: true,
      transferUnitCost: "4",
    },
    {
      id: "investment:new",
      date: "2026-01-03",
      type: "TRANSFER_IN",
      securityId: "security",
      name: "New incoming",
      quantity: "1",
      price: "0",
      cash: "0",
      fee: "0",
      requiresTransferCost: true,
      transferUnitCost: null,
    },
  ];
  render(<PlaidInvestmentReview preview={p} busy={false} onApprove={vi.fn()} onClose={vi.fn()} />);
  const fields = screen.getAllByRole("textbox");
  expect(fields[0]).toBeDisabled();
  expect(fields[0]).toHaveValue("4");
  expect(fields[1]).toBeEnabled();
});
