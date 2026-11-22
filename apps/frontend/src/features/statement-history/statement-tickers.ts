import { normalizeSecurityLabel } from "./merrill-format";

export function printedTicker(value: string): string | undefined {
  const ticker = value.trim();
  // Accept only ticker-shaped contents from a broker's explicit symbol field.
  if (/^[A-Z]{1,6}(?:[.-][A-Z])?$/.test(ticker) && ticker !== "CASH") return ticker;
}

export function rememberTicker(tickers: Map<string, string | null>, label: string, ticker: string) {
  const key = normalizeSecurityLabel(label);
  const existing = tickers.get(key);
  tickers.set(key, existing === undefined || existing === ticker ? ticker : null);
}
