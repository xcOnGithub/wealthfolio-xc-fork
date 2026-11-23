import type { Holding } from "@/lib/types";
import { describe, expect, it } from "vitest";
import { extractUsdHoldingsWeights } from "./presets";

function mockHolding(
  overrides: Partial<Holding> & {
    symbol?: string;
    currency?: string;
    quoteMode?: "MARKET" | "MANUAL";
    exchangeMic?: string;
    instrumentType?: string;
    value?: number;
  },
): Holding {
  const {
    symbol = "VTI",
    currency = "USD",
    quoteMode = "MARKET",
    exchangeMic = "XNAS",
    instrumentType = "EQUITY",
    value = 1000,
    ...rest
  } = overrides;
  return {
    id: `h-${symbol}`,
    holdingType: "security",
    isClosed: false,
    accountId: "acc-1",
    instrument: {
      id: `SEC:${symbol}:${exchangeMic}`,
      symbol,
      name: symbol,
      currency,
      quoteMode,
      exchangeMic,
      instrumentType,
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
  it("filters to open, market-priced US-listed USD securities and normalizes weights to 100%", () => {
    const holdings: Holding[] = [
      mockHolding({
        symbol: "VTI",
        currency: "USD",
        exchangeMic: "ARCX",
        instrumentType: "ETF",
        value: 6000,
      }),
      mockHolding({
        symbol: "BND",
        currency: "USD",
        exchangeMic: "XNAS",
        instrumentType: "ETF",
        value: 4000,
      }),
      // Should be excluded: non-USD, non-US USD listing, USD crypto, manual, closed, and cash
      mockHolding({ symbol: "SHOP.TO", currency: "CAD", exchangeMic: "XTSE", value: 5000 }),
      mockHolding({
        symbol: "VWRA.L",
        currency: "USD",
        exchangeMic: "XLON",
        instrumentType: "ETF",
        value: 4000,
      }),
      mockHolding({
        symbol: "BTC-USD",
        currency: "USD",
        exchangeMic: "XNAS",
        instrumentType: "CRYPTO",
        value: 3000,
      }),
      mockHolding({ symbol: "PRIVATE", currency: "USD", quoteMode: "MANUAL", value: 2000 }),
      mockHolding({ symbol: "CLOSED", currency: "USD", isClosed: true, value: 3000 }),
      mockHolding({ symbol: "USD", holdingType: "cash", currency: "USD", value: 1000 }),
    ];

    const weights = extractUsdHoldingsWeights(holdings);
    expect(weights).toEqual([
      { symbol: "VTI", weight: 60, exchangeMic: "ARCX", instrumentType: "ETF" },
      { symbol: "BND", weight: 40, exchangeMic: "XNAS", instrumentType: "ETF" },
    ]);
  });

  it("merges duplicate tickers across accounts and caps at 10 assets", () => {
    const holdings: Holding[] = [
      mockHolding({ id: "1", symbol: "VTI", exchangeMic: "ARCX", value: 2000 }),
      mockHolding({ id: "2", symbol: "VTI", exchangeMic: "ARCX", value: 3000 }),
      mockHolding({ id: "3", symbol: "QQQ", exchangeMic: "XNAS", value: 5000 }),
    ];

    const weights = extractUsdHoldingsWeights(holdings);
    expect(weights).toEqual([
      { symbol: "VTI", weight: 50, exchangeMic: "ARCX", instrumentType: "EQUITY" },
      { symbol: "QQQ", weight: 50, exchangeMic: "XNAS", instrumentType: "EQUITY" },
    ]);
  });

  it("returns empty array when no eligible USD holdings exist", () => {
    const weights = extractUsdHoldingsWeights([
      mockHolding({ symbol: "SHOP.TO", currency: "CAD", exchangeMic: "XTSE", value: 5000 }),
    ]);
    expect(weights).toEqual([]);
  });
});
