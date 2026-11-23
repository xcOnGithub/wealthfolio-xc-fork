import type { Holding } from "@/lib/types";
import { describe, expect, it } from "vitest";
import { extractUsdHoldingsWeights } from "./presets";

function mockHolding(
  overrides: Partial<Holding> & {
    symbol?: string;
    currency?: string;
    quoteMode?: "MARKET" | "MANUAL";
    value?: number;
  },
): Holding {
  const {
    symbol = "VTI",
    currency = "USD",
    quoteMode = "MARKET",
    value = 1000,
    ...rest
  } = overrides;
  return {
    id: `h-${symbol}`,
    holdingType: "security",
    isClosed: false,
    accountId: "acc-1",
    instrument: {
      id: `SEC:${symbol}:XNAS`,
      symbol,
      name: symbol,
      currency,
      quoteMode,
    },
    quantity: 10,
    localCurrency: currency,
    baseCurrency: "USD",
    marketValue: { local: value, base: value },
    weight: null,
    asOfDate: "2026-01-01",
    ...rest,
  };
}

describe("extractUsdHoldingsWeights", () => {
  it("filters to open, market-priced USD securities and normalizes weights to 100%", () => {
    const holdings: Holding[] = [
      mockHolding({ symbol: "VTI", currency: "USD", value: 6000 }),
      mockHolding({ symbol: "BND", currency: "USD", value: 4000 }),
      // Should be excluded: non-USD, manual, closed, and cash
      mockHolding({ symbol: "SHOP.TO", currency: "CAD", value: 5000 }),
      mockHolding({ symbol: "PRIVATE", currency: "USD", quoteMode: "MANUAL", value: 2000 }),
      mockHolding({ symbol: "CLOSED", currency: "USD", isClosed: true, value: 3000 }),
      mockHolding({ symbol: "USD", holdingType: "cash", currency: "USD", value: 1000 }),
    ];

    const weights = extractUsdHoldingsWeights(holdings);
    expect(weights).toEqual([
      { symbol: "VTI", weight: 60 },
      { symbol: "BND", weight: 40 },
    ]);
  });

  it("merges duplicate tickers across accounts and caps at 10 assets", () => {
    const holdings: Holding[] = [
      mockHolding({ id: "1", symbol: "VTI", value: 2000 }),
      mockHolding({ id: "2", symbol: "VTI", value: 3000 }),
      mockHolding({ id: "3", symbol: "QQQ", value: 5000 }),
    ];

    const weights = extractUsdHoldingsWeights(holdings);
    expect(weights).toEqual([
      { symbol: "VTI", weight: 50 },
      { symbol: "QQQ", weight: 50 },
    ]);
  });

  it("returns empty array when no eligible USD holdings exist", () => {
    const weights = extractUsdHoldingsWeights([
      mockHolding({ symbol: "SHOP.TO", currency: "CAD", value: 5000 }),
    ]);
    expect(weights).toEqual([]);
  });
});
