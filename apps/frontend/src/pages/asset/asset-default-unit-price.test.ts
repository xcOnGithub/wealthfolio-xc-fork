import { describe, expect, it } from "vitest";
import {
  applyDefaultUnitPrice,
  CASH_EQUIVALENT_DEFAULT_UNIT_PRICE,
  DEFAULT_UNIT_PRICE_KEY,
  explicitDefaultUnitPrice,
} from "./asset-default-unit-price";

describe("asset-default-unit-price", () => {
  it("reads positive numbers and numeric strings", () => {
    expect(explicitDefaultUnitPrice({ [DEFAULT_UNIT_PRICE_KEY]: 1 })).toBe(1);
    expect(explicitDefaultUnitPrice({ [DEFAULT_UNIT_PRICE_KEY]: "1.25" })).toBe(1.25);
    expect(CASH_EQUIVALENT_DEFAULT_UNIT_PRICE).toBe(1);
  });

  it("ignores missing, zero, negative, or non-numeric values", () => {
    expect(explicitDefaultUnitPrice(null)).toBeNull();
    expect(explicitDefaultUnitPrice({})).toBeNull();
    expect(explicitDefaultUnitPrice({ [DEFAULT_UNIT_PRICE_KEY]: 0 })).toBeNull();
    expect(explicitDefaultUnitPrice({ [DEFAULT_UNIT_PRICE_KEY]: -1 })).toBeNull();
    expect(explicitDefaultUnitPrice({ [DEFAULT_UNIT_PRICE_KEY]: "abc" })).toBeNull();
  });

  it("writes positive values and removes the key when cleared without mutating input", () => {
    const original = { contractMultiplier: 10 };
    const withPrice = applyDefaultUnitPrice(original, 1);
    expect(withPrice).toEqual({ contractMultiplier: 10, defaultUnitPrice: 1 });
    expect(original).toEqual({ contractMultiplier: 10 });

    const cleared = applyDefaultUnitPrice(withPrice, null);
    expect(cleared).toEqual({ contractMultiplier: 10 });
    expect(cleared).not.toHaveProperty(DEFAULT_UNIT_PRICE_KEY);
  });
});
