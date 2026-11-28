import { describe, expect, it } from "vitest";
import {
  applyDefaultUnitPrice,
  CASH_EQUIVALENT_DEFAULT_UNIT_PRICE,
  DEFAULT_UNIT_PRICE_KEY,
  explicitDefaultUnitPrice,
  MAX_DEFAULT_UNIT_PRICE,
} from "./asset-default-unit-price";

describe("asset-default-unit-price", () => {
  it("reads positive numbers and numeric strings up to MAX_DEFAULT_UNIT_PRICE", () => {
    expect(explicitDefaultUnitPrice({ [DEFAULT_UNIT_PRICE_KEY]: 1 })).toBe(1);
    expect(explicitDefaultUnitPrice({ [DEFAULT_UNIT_PRICE_KEY]: "1.25" })).toBe(1.25);
    expect(explicitDefaultUnitPrice({ [DEFAULT_UNIT_PRICE_KEY]: MAX_DEFAULT_UNIT_PRICE })).toBe(
      MAX_DEFAULT_UNIT_PRICE,
    );
    expect(CASH_EQUIVALENT_DEFAULT_UNIT_PRICE).toBe(1);
  });

  it("ignores missing, zero, negative, non-numeric, or out-of-range values", () => {
    expect(explicitDefaultUnitPrice(null)).toBeNull();
    expect(explicitDefaultUnitPrice({})).toBeNull();
    expect(explicitDefaultUnitPrice({ [DEFAULT_UNIT_PRICE_KEY]: 0 })).toBeNull();
    expect(explicitDefaultUnitPrice({ [DEFAULT_UNIT_PRICE_KEY]: -1 })).toBeNull();
    expect(explicitDefaultUnitPrice({ [DEFAULT_UNIT_PRICE_KEY]: "abc" })).toBeNull();
    expect(explicitDefaultUnitPrice({ [DEFAULT_UNIT_PRICE_KEY]: 1e21 })).toBeNull();
    expect(explicitDefaultUnitPrice({ [DEFAULT_UNIT_PRICE_KEY]: "1e21" })).toBeNull();
  });

  it("writes positive values within range and removes the key when cleared or out of range without mutating input", () => {
    const original = { contractMultiplier: 10 };
    const withPrice = applyDefaultUnitPrice(original, 1);
    expect(withPrice).toEqual({ contractMultiplier: 10, defaultUnitPrice: 1 });
    expect(original).toEqual({ contractMultiplier: 10 });

    const atMax = applyDefaultUnitPrice(original, MAX_DEFAULT_UNIT_PRICE);
    expect(atMax).toEqual({ contractMultiplier: 10, defaultUnitPrice: MAX_DEFAULT_UNIT_PRICE });

    const overMax = applyDefaultUnitPrice(withPrice, 1e21);
    expect(overMax).toEqual({ contractMultiplier: 10 });
    expect(overMax).not.toHaveProperty(DEFAULT_UNIT_PRICE_KEY);

    const cleared = applyDefaultUnitPrice(withPrice, null);
    expect(cleared).toEqual({ contractMultiplier: 10 });
    expect(cleared).not.toHaveProperty(DEFAULT_UNIT_PRICE_KEY);
  });
});
