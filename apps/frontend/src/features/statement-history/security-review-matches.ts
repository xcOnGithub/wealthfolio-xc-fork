import { sha256 } from "@noble/hashes/sha256";
import { bytesToHex, utf8ToBytes } from "@noble/hashes/utils";
import type { StoredDailyStatement, StoredStatement } from "@/adapters/shared/statements";
import type { StatementActivity } from "./merrill-activity";
import type { MerrillHistoryDraft } from "./merrill-history";
import { normalizeSecurityLabel } from "./merrill-format";

function securityKey(label: string): string | undefined {
  const normalized = normalizeSecurityLabel(label);
  // Exact label matches preserve share classes and punctuation. Version the
  // fingerprint independently of statement storage and omit the source text.
  return normalized
    ? bytesToHex(sha256(utf8ToBytes(`merrill-security-v1:${normalized}`)))
    : undefined;
}

export function activitySecurityLabel(activity: StatementActivity): string | undefined {
  if (
    activity.section !== "sweep" &&
    activity.section !== "pending" &&
    ["BUY", "SELL", "TRANSFER_IN", "TRANSFER_OUT"].includes(activity.action)
  ) {
    return activity.security || activity.description;
  }
}

export function activitySecurityKey(activity: StatementActivity): string | undefined {
  const label = activitySecurityLabel(activity);
  return label ? securityKey(label) : undefined;
}

type Matches = Map<string, { assetId: string | null; order: number }>;

function remember(matches: Matches, key: string, assetId: string | null, order: number) {
  const previous = matches.get(key);
  if (!previous || order > previous.order) {
    matches.set(key, { assetId, order });
  } else if (order === previous.order && assetId !== previous.assetId) {
    // A single review batch can use identical statement labels for different
    // securities. Only an unambiguous match is safe to preselect.
    matches.set(key, { assetId: null, order });
  }
}

export function reviewedSecurityMatches(
  daily: StoredDailyStatement[],
  monthly: StoredStatement[],
  availableAssetIds: ReadonlySet<string>,
) {
  const snapshots = new Map(monthly.map(({ snapshot }) => [snapshot.date, snapshot]));
  const latest: Matches = new Map();
  const byDate = new Map<string, Matches>();
  for (const review of daily) {
    const snapshot = snapshots.get(review.interval.through);
    if (!snapshot || snapshot.currency !== review.interval.currency) continue;
    const matches: Matches = new Map();
    const order = review.reviewOrder ?? 0;
    if (review.closingAssetIds.length === snapshot.positions.length) {
      snapshot.positions.forEach(({ label }, index) => {
        const key = securityKey(label);
        if (key) remember(matches, key, review.closingAssetIds[index], order);
      });
    }
    for (const event of review.interval.events) {
      const key = review.activitySecurityKeys?.[event.sourceId];
      if (key && event.assetId) remember(matches, key, event.assetId, order);
    }
    byDate.set(snapshot.date, matches);
    for (const [key, match] of matches) remember(latest, key, match.assetId, match.order);
  }

  return (draft: MerrillHistoryDraft): Record<string, string> => {
    const links: Record<string, string> = {};
    const savedPeriod = byDate.get(draft.through);
    const selectedAsset = (key: string | undefined) => {
      if (!key) return undefined;
      const assetId = (savedPeriod?.get(key) ?? latest.get(key))?.assetId;
      return assetId && availableAssetIds.has(assetId) ? assetId : undefined;
    };
    draft.closing.positions.forEach(({ label }, index) => {
      const assetId = selectedAsset(securityKey(label));
      if (assetId) links[`holding:${index}`] = assetId;
    });
    for (const event of draft.evidence.activities) {
      const eventKey = activitySecurityKey(event);
      const holdingLabel = draft.activityHoldingLabels[event.id];
      if (!eventKey || holdingLabel === null) continue;
      const fromEvent = selectedAsset(eventKey);
      const fromHolding = holdingLabel ? selectedAsset(securityKey(holdingLabel)) : undefined;
      const assetId =
        fromEvent && fromHolding && fromEvent !== fromHolding ? "" : (fromEvent ?? fromHolding);
      if (assetId !== undefined) links[`activity:${event.id}`] = assetId;
    }
    return links;
  };
}

/** Join repeated rows only on an exact label or a validated statement alias. */
export function securityReviewGroups(draft: MerrillHistoryDraft): Record<string, string> {
  const groups: Record<string, string> = {};
  draft.closing.positions.forEach(({ label }, index) => {
    const key = securityKey(label);
    if (key) groups[`holding:${index}`] = key;
  });
  for (const event of draft.evidence.activities) {
    const eventKey = activitySecurityKey(event);
    const holdingLabel = draft.activityHoldingLabels[event.id];
    if (!eventKey || holdingLabel === null) continue;
    const key = holdingLabel ? securityKey(holdingLabel) : eventKey;
    if (key) groups[`activity:${event.id}`] = key;
  }
  return groups;
}

interface ReviewLinks {
  groups: Record<string, string>;
  savedLinks: Record<string, string>;
  edits: Record<string, string>;
}

export function resolveReviewLinks(reviews: ReviewLinks[]): Record<string, string>[] {
  const savedSelections: Matches = new Map();
  const selections: Matches = new Map();
  for (const review of reviews) {
    for (const [row, assetId] of Object.entries(review.savedLinks)) {
      const group = review.groups[row];
      if (group && assetId) remember(savedSelections, group, assetId, 0);
    }
    for (const [row, assetId] of Object.entries(review.edits)) {
      const group = review.groups[row];
      if (group && assetId) remember(selections, group, assetId, 0);
    }
  }

  return reviews.map(({ groups, savedLinks, edits }) => {
    const links = { ...savedLinks };
    for (const [row, group] of Object.entries(groups)) {
      const selection = selections.get(group);
      if (selection) links[row] = selection.assetId ?? "";
      else if (links[row] === undefined) {
        const saved = savedSelections.get(group)?.assetId;
        if (saved) links[row] = saved;
      }
    }
    return { ...links, ...edits };
  });
}
