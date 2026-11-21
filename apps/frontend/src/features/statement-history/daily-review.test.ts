import { expect, it } from "vitest";
import { dailyStatementWrite } from "./daily-review";
import { dailyDraft } from "./test-fixtures/daily-draft";

it("requires explicit links for closing positions and transactions, never infers identity from quantity", () => {
  expect(() => dailyStatementWrite(dailyDraft(), {}, null, null)).toThrow("security");
  expect(() => dailyStatementWrite(dailyDraft(), { "holding:0": "demo" }, null, null)).toThrow(
    "security",
  );
});
it("preserves final net cash, unknown basis and source occurrence without sending private descriptions", () => {
  const write = dailyStatementWrite(
    dailyDraft(),
    { "holding:0": "demo", "activity:row-1": "demo" },
    "daily-old",
    "monthly-old",
  );
  expect(write.interval.events[0]).toMatchObject({
    cash: "-10",
    sourceId: "row-1",
    sourcePage: 2,
    sourceLine: 10,
  });
  expect(write.interval.closingPositions[0].costBasis).toBeNull();
  expect(write.expectedRevision).toBe("daily-old");
  expect(write.expectedSnapshotRevision).toBe("monthly-old");
  expect(JSON.stringify(write)).not.toContain("Private transaction memo");
  expect(JSON.stringify(write)).not.toContain("fee");
});
it("never uses an in-kind transfer value as cash or carries internal sweeps into events", () => {
  const draft = dailyDraft();
  draft.evidence.activities[0] = {
    ...draft.evidence.activities[0],
    action: "TRANSFER_IN",
    cash: null,
    transferValue: "1000",
  };
  draft.evidence.activities.push({
    ...draft.evidence.activities[0],
    id: "sweep",
    section: "sweep",
    action: "CASH",
    cash: "-1000",
  });
  const write = dailyStatementWrite(
    draft,
    { "holding:0": "demo", "activity:row-1": "demo" },
    null,
    null,
  );
  expect(write.interval.events).toHaveLength(1);
  expect(write.interval.events[0].cash).toBe("0");
});
it("keeps unknown-basis fractional quantities and blocks unresolved or pending activity", () => {
  const draft = dailyDraft();
  draft.closing.positions[0].quantity = "2.0001";
  expect(
    dailyStatementWrite(draft, { "holding:0": "demo", "activity:row-1": "demo" }, null, null)
      .interval.closingPositions[0].quantity,
  ).toBe("2.0001");
  for (const code of ["unsupportedActivity", "unsettledTrades", "cashMismatch"] as const) {
    draft.issues = [{ code }];
    expect(() =>
      dailyStatementWrite(draft, { "holding:0": "demo", "activity:row-1": "demo" }, null, null),
    ).toThrow("incomplete");
  }
});

it("groups explicitly mapped rows for the same security and retains its unknown-basis fraction", () => {
  const draft = dailyDraft();
  draft.closing.positions = [
    { label: "DEMO", quantity: "1.9999", marketValue: "23.9988", costBasis: "10" },
    { label: "DEMO", quantity: "0.0001", marketValue: "0.0012", costBasis: null },
  ];
  const write = dailyStatementWrite(
    draft,
    { "holding:0": "demo", "holding:1": "demo", "activity:row-1": "demo" },
    null,
    null,
  );
  expect(write.closingAssetIds).toEqual(["demo", "demo"]);
  expect(write.interval.closingPositions).toEqual([
    { assetId: "demo", quantity: "2", marketValue: "24", costBasis: null },
  ]);
  expect(write.snapshot.positions).toHaveLength(2);
});
