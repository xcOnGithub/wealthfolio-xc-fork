import { expect, it } from "vitest";
import { dailyStatementWrite } from "./daily-review";
import {
  resolveReviewLinks,
  reviewedSecurityMatches,
  securityReviewGroups,
} from "./security-review-matches";
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

it("prefills an event from older confirmed holdings even without saved event fingerprints", () => {
  const saved = reviewedStatement("2025-04-30", "demo", "sold");
  saved.monthly.snapshot.positions[0].label = "Synthetic equity DEMO";
  const draft = dailyDraft();
  draft.closing.positions[0].label = "Synthetic equity DEMO";
  draft.evidence.activities[0].security = undefined;
  draft.evidence.activities[0].description = "Synthetic equity";
  draft.activityHoldingLabels["row-1"] = "Synthetic equity DEMO";
  const preselect = reviewedSecurityMatches([saved.daily], [saved.monthly], new Set(["demo"]));
  expect(preselect(draft)).toEqual({ "holding:0": "demo", "activity:row-1": "demo" });
  draft.activityHoldingLabels["row-1"] = null;
  expect(preselect(draft)).toEqual({ "holding:0": "demo" });
});

it("reuses a review edit for repeated rows and keeps individual dropdown overrides", () => {
  const draft = dailyDraft();
  draft.evidence.activities.push({ ...draft.evidence.activities[0], id: "row-2" });
  const groups = securityReviewGroups(draft);
  const review = { groups, savedLinks: { "holding:0": "old" }, edits: { "holding:0": "new" } };
  expect(resolveReviewLinks([review, { groups, savedLinks: {}, edits: {} }])).toEqual([
    { "holding:0": "new", "activity:row-1": "new", "activity:row-2": "new" },
    { "holding:0": "new", "activity:row-1": "new", "activity:row-2": "new" },
  ]);
  expect(
    resolveReviewLinks([{ ...review, edits: { ...review.edits, "activity:row-1": "override" } }]),
  ).toEqual([{ "holding:0": "new", "activity:row-1": "override", "activity:row-2": "" }]);
  expect(
    resolveReviewLinks([{ ...review, edits: { ...review.edits, "activity:row-1": "" } }]),
  ).toEqual([{ "holding:0": "new", "activity:row-1": "", "activity:row-2": "new" }]);
});

it("reuses an agreed saved selection while leaving a conflicting event blank", () => {
  const saved = reviewedStatement("2025-04-30", "demo", "other", 1);
  saved.monthly.snapshot.positions[0].label = "Synthetic equity DEMO";
  const draft = dailyDraft();
  draft.closing.positions[0].label = "Synthetic equity DEMO";
  draft.evidence.activities[0].security = undefined;
  draft.evidence.activities[0].description = "Former holding";
  draft.evidence.activities.push({
    ...draft.evidence.activities[0],
    id: "row-2",
    description: "DEMO",
  });
  draft.activityHoldingLabels = {
    "row-1": "Synthetic equity DEMO",
    "row-2": "Synthetic equity DEMO",
  };
  const preselect = reviewedSecurityMatches(
    [saved.daily],
    [saved.monthly],
    new Set(["demo", "other"]),
  );
  expect(
    resolveReviewLinks([
      {
        groups: securityReviewGroups(draft),
        savedLinks: preselect(draft),
        edits: {},
      },
    ]),
  ).toEqual([{ "holding:0": "demo", "activity:row-1": "", "activity:row-2": "demo" }]);
});
