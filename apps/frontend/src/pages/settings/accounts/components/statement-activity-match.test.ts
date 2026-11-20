import { describe, it, expect } from "vitest";
import { extractMerrillStatementV2 } from "./merrill-statement-v2";
import { merrillActivityFixture } from "./test-fixtures/merrill-statement-v2";
import { proposeDisposedBasis } from "./statement-activity-match";
import {
  calculateTransferBasis,
  updateInvestmentReviewCosts,
  type InvestmentReviewCosts,
} from "./transfer-basis";
import { transferPreview, transferStatement } from "./test-fixtures/transfer-basis";

function setup() {
  const statement = {
    ...transferStatement(),
    asOf: "2025-05-12",
    documentId: "doc",
    activity: extractMerrillStatementV2(merrillActivityFixture()).evidence,
  };
  const record = {
    ...transferPreview().records[0],
    id: "sell",
    type: "SELL",
    date: "2025-05-06",
    quantity: "1",
    cash: "15",
    fee: "0",
    requiresTransferCost: false,
  };
  return {
    statement,
    record,
    security: "DEMO",
    rowIds: [statement.activity.dispositions[0].id],
    verifiedDates: false,
  };
}
describe("reviewed statement sale evidence", () => {
  it("uses reported basis rather than sale proceeds or transfer value", () => {
    expect(proposeDisposedBasis(setup())).toMatchObject({ value: "9", rows: [{ page: 2 }] });
  });
  it("combines explicit lots only when quantity and proceeds reconcile", () => {
    const x = setup(),
      row = x.statement.activity.dispositions[0];
    x.statement.activity.dispositions.push({
      ...row,
      id: "second",
      quantity: "2",
      basis: "17",
      proceeds: "30",
    });
    x.rowIds.push("second");
    x.record.quantity = "3";
    x.record.cash = "44";
    x.record.fee = "1";
    expect(proposeDisposedBasis(x).value).toBe("26");
  });
  it("requires explicit date verification for trade/settlement differences", () => {
    const x = setup();
    x.record.date = "2025-05-05";
    expect(() => proposeDisposedBasis(x)).toThrow("date");
    x.verifiedDates = true;
    expect(proposeDisposedBasis(x).value).toBe("9");
  });
  it.each([
    "missing",
    "identity",
    "quantity",
    "proceeds",
    "duplicate",
    "transfer",
    "negative",
  ] as const)("rejects %s evidence", (kind) => {
    const x = setup();
    if (kind === "missing") x.statement.activity.dispositions[0].basis = null;
    if (kind === "identity") x.statement.activity.dispositions[0].security = "OTHER";
    if (kind === "quantity") x.record.quantity = "2";
    if (kind === "proceeds") x.record.cash = "999";
    if (kind === "duplicate") x.rowIds.push(x.rowIds[0]);
    if (kind === "transfer") x.record.type = "TRANSFER_OUT";
    if (kind === "negative") x.statement.activity.dispositions[0].basis = "-1";
    expect(() => proposeDisposedBasis(x)).toThrow();
  });
  it("requires explicit identity review for a security without an ending holding", () => {
    const x = setup();
    x.statement.activity.dispositions[0].security = undefined;
    expect(proposeDisposedBasis(x).value).toBe("9");
    x.security = "";
    expect(() => proposeDisposedBasis(x)).toThrow();
  });
  it("fills empty disposed costs, preserves manual values and invalidates dependent transfer costs atomically", () => {
    const x = setup(),
      proposal = proposeDisposedBasis(x),
      source = { documentId: "doc", filename: "synthetic.pdf", rows: proposal.rows };
    const initial: InvestmentReviewCosts = {
      openingCosts: {},
      transferCosts: {},
      disposedCosts: {},
      transferEvidence: {},
    };
    const fill = { fillDisposed: { id: "sell", value: proposal.value, source } };
    let costs = updateInvestmentReviewCosts(initial, fill);
    expect(costs.disposedCosts.sell).toBe("9");
    costs = updateInvestmentReviewCosts(costs, {
      fill: {
        id: "incoming",
        unitCost: "25",
        dependencies: { openingCosts: {}, transferCosts: {}, disposedCosts: { sell: "9" } },
      },
    });
    costs = updateInvestmentReviewCosts(costs, {
      invalidateDisposed: { documentId: "doc", rowIds: x.rowIds },
    });
    expect(costs.disposedCosts.sell).toBe("");
    expect(costs.transferCosts.incoming).toBe("");
    costs = updateInvestmentReviewCosts(updateInvestmentReviewCosts(initial, fill), {
      disposedCosts: { sell: "9" },
    });
    costs = updateInvestmentReviewCosts(costs, {
      invalidateDisposed: { documentId: "doc", rowIds: x.rowIds },
    });
    expect(costs.disposedCosts.sell).toBe("9");
    expect(costs.disposedSources?.sell).toBeUndefined();
    expect(
      updateInvestmentReviewCosts(costs, { fillDisposed: { id: "sell", value: "99", source } })
        .disposedCosts.sell,
    ).toBe("9");
  });
  it("rejects a source row reused by a second sale, including reopening the same document", () => {
    const x = setup(),
      p = proposeDisposedBasis(x),
      source = { documentId: "same-hash", filename: "a.pdf", rows: p.rows };
    const costs = updateInvestmentReviewCosts(
      { openingCosts: {}, transferCosts: {}, disposedCosts: {}, transferEvidence: {} },
      { fillDisposed: { id: "sale1", value: p.value, source } },
    );
    const next = updateInvestmentReviewCosts(costs, {
      fillDisposed: { id: "sale2", value: p.value, source: { ...source, filename: "renamed.pdf" } },
    });
    expect(next.disposedCosts.sale1).toBe("9");
    expect(next.disposedCosts.sale2).toBeUndefined();
  });
  it("keeps unrelated document evidence intact", () => {
    const x = setup(),
      p = proposeDisposedBasis(x);
    const costs = updateInvestmentReviewCosts(
      { openingCosts: {}, transferCosts: {}, disposedCosts: {}, transferEvidence: {} },
      {
        fillDisposed: {
          id: "sell",
          value: p.value,
          source: { documentId: "doc", filename: "a.pdf", rows: p.rows },
        },
      },
    );
    expect(
      updateInvestmentReviewCosts(costs, {
        invalidateDisposed: { documentId: "other", rowIds: x.rowIds },
      }).disposedCosts.sell,
    ).toBe("9");
  });
  it("blocks unsupported statement basis changes but leaves pending trades informational", () => {
    const statement = transferStatement(),
      preview = transferPreview();
    statement.activity = setup().statement.activity;
    statement.activity.activities = [
      { ...statement.activity.activities[0], action: "UNSUPPORTED", security: "DEMO" },
    ];
    const x = {
      preview,
      statement,
      candidate: statement.candidates[0],
      transferId: "incoming",
      openingCosts: {},
      transferCosts: {},
      disposedCosts: {},
    };
    expect(() => calculateTransferBasis(x)).toThrow("action");
    statement.activity.activities[0].section = "pending";
    expect(calculateTransferBasis(x).unitCost).toBe("25");
  });
});
