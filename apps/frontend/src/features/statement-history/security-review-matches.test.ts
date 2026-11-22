import { expect, it } from "vitest";
import { dailyStatementWrite } from "./daily-review";
import { reviewedSecurityMatches } from "./security-review-matches";
import { dailyDraft } from "./test-fixtures/daily-draft";

function reviewedStatement(date: string, holding: string, trade: string, reviewOrder?: number) {
  const draft = dailyDraft();
  draft.closing.date = draft.through = date;
  draft.evidence.activities[0].security = undefined;
  draft.evidence.activities[0].description = "Former holding";
  const { activitySecurityKeys, closingAssetIds, interval, snapshot } = dailyStatementWrite(
    draft,
    { "holding:0": holding, "activity:row-1": trade },
    null,
    null,
  );
  return {
    monthly: { snapshot, revision: "monthly" },
    daily: {
      interval,
      closingAssetIds,
      activitySecurityKeys: reviewOrder === undefined ? undefined : activitySecurityKeys,
      reviewOrder,
      revision: "daily",
      version: 1,
    },
  };
}

it("reuses legacy closing links and fingerprints trades even when their row changes", () => {
  const legacy = reviewedStatement("2025-03-31", "demo", "sold");
  const saved = reviewedStatement("2025-04-30", "demo", "sold", 1);
  const draft = dailyDraft();
  draft.closing.positions[0].label = " demo ";
  draft.evidence.activities[0] = {
    ...draft.evidence.activities[0],
    security: undefined,
    description: "FORMER    holding",
    id: "other-row",
  };
  const assets = new Set(["demo", "sold"]);
  expect(reviewedSecurityMatches([legacy.daily], [legacy.monthly], assets)(draft)).toEqual({
    "holding:0": "demo",
  });
  expect(reviewedSecurityMatches([saved.daily], [saved.monthly], assets)(draft)).toEqual({
    "holding:0": "demo",
    "activity:other-row": "sold",
  });
  expect(saved.daily.activitySecurityKeys?.["row-1"]).toMatch(/^[0-9a-f]{64}$/);
});

it("uses the last confirmed correction for new periods and keeps a saved period's own links", () => {
  const later = reviewedStatement("2025-04-30", "demo", "old", 1);
  const corrected = reviewedStatement("2025-03-31", "demo", "corrected", 2);
  const draft = dailyDraft();
  draft.evidence.activities[0].security = undefined;
  draft.evidence.activities[0].description = "Former holding";
  const match = reviewedSecurityMatches(
    [later.daily, corrected.daily],
    [later.monthly, corrected.monthly],
    new Set(["demo", "old", "corrected"]),
  );
  expect(match(draft)["activity:row-1"]).toBe("corrected");
  expect(match({ ...draft, through: "2025-04-30" })["activity:row-1"]).toBe("old");
});

it("leaves conflicting, changed, and unavailable labels for manual review", () => {
  const first = reviewedStatement("2025-03-31", "demo", "sold", 1);
  const second = reviewedStatement("2025-04-30", "demo", "other", 1);
  const draft = dailyDraft();
  draft.evidence.activities = [
    { ...draft.evidence.activities[0], security: undefined, description: "Former holding" },
    { ...draft.evidence.activities[0], id: "changed", security: "DEMO CLASS B" },
  ];
  expect(
    reviewedSecurityMatches(
      [first.daily, second.daily],
      [first.monthly, second.monthly],
      new Set(["sold", "other"]),
    )(draft),
  ).toEqual({});
});
