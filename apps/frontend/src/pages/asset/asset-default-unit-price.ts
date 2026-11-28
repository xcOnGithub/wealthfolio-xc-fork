/**
 * Fallback / par unit price metadata, mirroring the Rust resolver in
 * `crates/core/src/assets/assets_model.rs`.
 *
 * Stored under `metadata.defaultUnitPrice` as a positive number. Used when no
 * explicit quote exists on or before a valuation date and as a fallback unit
 * cost for holdings snapshots with unknown cost basis. Explicit quotes and
 * explicit cost basis always take precedence.
 */

export const DEFAULT_UNIT_PRICE_KEY = "defaultUnitPrice";
export const CASH_EQUIVALENT_DEFAULT_UNIT_PRICE = 1;
/** Mirrors `wealthfolio_portfolio_engine::model::MAX_MAGNITUDE` (10^20). */
export const MAX_DEFAULT_UNIT_PRICE = 1e20;

type Metadata = Record<string, unknown>;

function asObject(value: unknown): Metadata | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  return value as Metadata;
}

const DECIMAL_STRING_RE = /^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?$/;

function toPositiveNumber(value: unknown): number | null {
  if (typeof value === "number") {
    return Number.isFinite(value) && value > 0 && value <= MAX_DEFAULT_UNIT_PRICE ? value : null;
  }
  if (typeof value === "string") {
    const trimmed = value.trim();
    if (!DECIMAL_STRING_RE.test(trimmed)) return null;
    const parsed = Number(trimmed);
    return Number.isFinite(parsed) && parsed > 0 && parsed <= MAX_DEFAULT_UNIT_PRICE
      ? parsed
      : null;
  }
  return null;
}

/**
 * The positive fallback unit price explicitly stored on the asset within the
 * engine-representable valuation range, or null when unset or invalid.
 */
export function explicitDefaultUnitPrice(metadata: unknown): number | null {
  const meta = asObject(metadata);
  if (!meta) return null;
  return toPositiveNumber(meta[DEFAULT_UNIT_PRICE_KEY]);
}

/**
 * Returns metadata with `defaultUnitPrice` applied or cleared, without
 * mutating the input.
 */
export function applyDefaultUnitPrice(
  metadata: Metadata,
  price: number | null | undefined,
): Metadata {
  const next: Metadata = { ...metadata };
  if (price != null && Number.isFinite(price) && price > 0 && price <= MAX_DEFAULT_UNIT_PRICE) {
    next[DEFAULT_UNIT_PRICE_KEY] = price;
  } else {
    delete next[DEFAULT_UNIT_PRICE_KEY];
  }
  return next;
}
