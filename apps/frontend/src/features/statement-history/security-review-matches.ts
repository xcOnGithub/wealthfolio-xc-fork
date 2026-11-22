import { sha256 } from "@noble/hashes/sha256";
import { bytesToHex, utf8ToBytes } from "@noble/hashes/utils";
import type { StoredDailyStatement, StoredStatement } from "@/adapters/shared/statements";
import type { StatementActivity } from "./merrill-activity";
import type { MerrillHistoryDraft } from "./merrill-history";

function securityKey(label: string): string | undefined {
  const normalized = label.normalize("NFC").trim().replace(/\s+/g, " ").toLowerCase();
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
    const preselect = (row: string, key: string | undefined) => {
      if (!key) return;
      const assetId = (savedPeriod?.get(key) ?? latest.get(key))?.assetId;
      if (assetId && availableAssetIds.has(assetId)) links[row] = assetId;
    };
    draft.closing.positions.forEach(({ label }, index) =>
      preselect(`holding:${index}`, securityKey(label)),
    );
    for (const event of draft.evidence.activities) {
      preselect(`activity:${event.id}`, activitySecurityKey(event));
    }
    return links;
  };
}
