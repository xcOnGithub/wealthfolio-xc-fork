# Backtest Lab Design (v1)

- **Date:** 2026-11-22
- **Status:** Implemented (v1) — PR #44 (`feat/backtest-experiment-tab`)
- **Primary paths:**
  - Core engine & service: `crates/core/src/portfolio/backtest/`
  - Tauri command: `apps/tauri/src/commands/backtest.rs`
  - Web API route: `apps/server/src/api/backtest.rs`
  - Frontend adapter & types: `apps/frontend/src/adapters/shared/backtest.ts`,
    `apps/frontend/src/lib/types/backtest.ts`
  - Frontend UI: `apps/frontend/src/features/backtest/`
  - E2E coverage: `e2e/backtest.spec.ts`

## 1. Motivation & Goals

Wealthfolio already provides historical performance analysis, holdings
breakdowns, and market-data integrations. Users exploring asset-allocation
adjustments often want to answer practical "what-if" questions before changing
their portfolio:

- How would a 60/40 or three-fund portfolio have behaved under quarterly vs.
  drift-band rebalancing?
- What is the impact of reinvesting dividends (DRIP) versus accumulating them as
  cash?
- How does adding recurring dollar-cost averaging (DCA) change total return and
  ending value?
- How do simple, transparent tactical overlays (such as a 200-day SMA trend
  filter or monthly dual-momentum rotation) compare against Buy & Hold and a
  benchmark like `SPY`?

**Design goals for v1:**

1. **Portfolio-level experimentation, not quant scripting:** Focus on five
   well-understood allocation, rebalancing, and rule-based strategies that
   everyday investors actually evaluate.
2. **One-click connection to real holdings:** Let users seed an experiment from
   their current open, market-priced USD holdings or start from common US
   presets.
3. **Total-return realism without disturbing portfolio accounting:** Incorporate
   cash dividend events via existing market-data provider capabilities without
   altering stored `quotes` rows or portfolio valuation logic.
4. **Zero persistence blast radius:** Run stateless, in-session experiments with
   no database migrations and no writes to real assets, quotes, or activities.
5. **Dual-runtime & localization parity:** Work identically in Tauri desktop and
   self-hosted Web builds, across all 11 supported locales and right-to-left
   (`rtl`) layouts.

## 2. Scope & Explicit Non-Goals

### In Scope (v1)

- **Market & currency:** US-listed, USD-denominated securities (1–10 tickers per
  experiment, plus optional benchmark and optional defensive ticker).
- **Market data:** On-demand historical daily close quotes and cash dividend
  events fetched through existing configured providers (including tickers not
  currently held in the user's database).
- **Strategies:**
  1. Buy & Hold
  2. Periodic Rebalance (monthly, quarterly, semiannually, annually)
  3. Drift-Band Rebalance (rebalance when any asset or cash drifts by $\ge \pm
     X\%$ from target)
  4. SMA Trend Filter ($N$-day moving average with daily or monthly signal
     checks, rotating risk-off slices to a defensive ticker or USD cash)
  5. Momentum Rotation (top-$N$ by $L$-day trailing return on a fixed cadence,
     equal or inverse-volatility weighting, optional absolute-momentum filter to
     a defensive ticker or USD cash)
- **Cash flows & costs:** Initial capital, optional recurring DCA (biweekly,
  monthly, quarterly, annually), dividend mode (`reinvest`, `cash`, `exclude`),
  and optional trading fee drag in basis points (0–500 bps).
- **Persistence:** In-session only (React state + React Query 5-minute cache
  keyed by request).

### Deferred (v2+)

- Non-USD / multi-currency backtests with daily FX conversion and cross-market
  holiday alignment.
- Persisting and syncing named experiments in SQLite.
- Tax-lot and capital-gains drag modeling.
- Intraday bars, bid-ask spread modeling, or custom formula/scripting languages.

## 3. Architecture & Data Flow

```mermaid
flowchart LR
    UI["BacktestPage<br/>(Insights > Backtest tab)"] --> Adapter["adapters/shared/backtest.ts<br/>runBacktest()"]
    Adapter -->|Tauri IPC| TauriCmd["apps/tauri/src/commands/backtest.rs<br/>run_backtest"]
    Adapter -->|HTTP POST| AxumApi["apps/server/src/api/backtest.rs<br/>POST /api/v1/backtest/run"]
    TauriCmd --> Service["crates/core/src/portfolio/backtest/service.rs<br/>run_backtest()"]
    AxumApi --> Service
    Service -->|async concurrent fetch| QS["QuoteServiceTrait<br/>search + quotes + dividends + splits"]
    Service -->|off async workers| Blocking["portfolio::coordinator::blocking()"]
    Blocking --> Engine["crates/core/src/portfolio/backtest/engine.rs<br/>simulate()"]
    Engine --> UI
```

### Key Architectural Boundaries & Invariants

- **No persistence side effects:** `run_backtest` never writes to SQLite
  (`assets`, `quotes`, `activities`, or `projection_state`) and introduces zero
  database migrations.
- **Provider settings preserved:** All listing, quote, dividend, and split
  fetches go through `QuoteServiceTrait`, respecting enabled/disabled providers
  and priority ordering (disabled providers receive zero requests).
- **Async worker safety:** After async network fetches complete, the pure
  simulation loop runs inside `crates/core/src/portfolio/coordinator::blocking`,
  matching Wealthfolio's rule for history-sized portfolio computations.
- **US-listed USD security gate:** Every requested ticker (universe, defensive,
  and benchmark) carries optional `exchangeMic` and `instrumentType` listing
  metadata from search/holdings and is verified server-side against enabled
  provider metadata and ISO 10383 US exchange MICs (`is_us_exchange_mic`),
  rejecting non-USD tickers, non-US USD listings (e.g. `XLON`), and non-equity
  USD instruments (e.g. `BTC-USD`).
- **Failure isolation:**
  - If a provider does not support dividends or dividend lookup fails for a
    ticker, the backtest degrades gracefully to price return for that ticker and
    surfaces a structured warning in `response.warnings`.
  - If an optional benchmark fails US/USD verification, fails to fetch, ends
    early, starts late, has consecutive missing sessions, or lacks split
    normalization, the benchmark series is omitted with a structured warning
    instead of failing the user's experiment.
  - Invalid required tickers fail fast with `ValidationError::InvalidInput`
    (`400 Bad Request` on Web).

## 4. Simulation Semantics

### 4.1 Calendar Alignment, Gap Limits & Indicator Warmup

- Required symbols (universe assets plus optional defensive asset) are aligned
  onto the union of observed trading days within their common overlapping range
  `[max(first_date), min(last_date)]`.
- Isolated single-session missing closes (`consecutive_missing == 1`) are
  forward-filled from the prior valid close with an `isolatedGapsForwardFilled`
  warning. Two or more consecutive missing trading days fail validation for
  required assets (or omit an optional benchmark with `benchmarkDataGap`).
  History is never fabricated before a security's first available quote or after
  its last quote (benchmarks ending before `effective_end_date` are omitted with
  `benchmarkEndsEarly`).
- For indicator strategies (`SmaTrendFilter` and `MomentumRotation`),
  `service.rs` automatically fetches additional lookback history prior to
  `start_date` (`warmup_trading_days * 7/5 + 45` calendar days) so moving
  averages and trailing returns are warmed up on day 1 of the requested window.
- If available history cannot cover `start_date` plus warmup,
  `effective_start_date` is shifted forward to the earliest valid trading day
  and an `effectiveStartAdjusted` warning is returned.

### 4.2 Zero-Lookahead Daily Execution Loop

On each trading day $t$ in `[effective_start_date, effective_end_date]`:

1. **Dividends (`t > 0`):** For each held position with an ex-dividend event on
   day $t$, credit `shares[i] * dividend_per_share`:
   - `reinvest`: buy additional shares of asset $i$ at day $t$'s close (DRIP).
   - `cash`: add to USD cash (deployed on the next rebalance; held as cash in
     Buy & Hold).
   - `exclude`: ignore dividend events.
2. **Mark-to-market & TWR update:** Value existing positions and cash at day
   $t$'s close _before_ external contributions and trades. Update the daily
   Time-Weighted Return (TWR) index from day $t - 1$'s post-trade value to day
   $t$'s pre-flow value so DCA inflows never distort return or risk metrics.
3. **Scheduled DCA contribution (`t > 0`):** On scheduled contribution dates,
   add the configured USD amount to cash and cumulative invested capital.
4. **Signal evaluation & trading:** Evaluate strategy rules strictly using
   prices available on or before day $t$:
   - If a rebalance/rotation triggers (including day-0 initial investment),
     compute gross traded notional $\sum_i |\text{target}_i -
     \text{current}_i|$, deduct `gross_traded * (fee_bps / 10_000)`, update
     shares and cash at day $t$'s close, and record turnover and fee drag on day
     $t$'s TWR (for both day 0 and subsequent trade days).
   - If no rebalance triggers on a DCA day, deploy only the new contribution
     into the currently active target weights without selling existing holdings,
     charging `fee_bps` on the deployed DCA buy notional and reflecting that fee
     drag in day $t$'s TWR.

### 4.3 Split Normalization & Explicit Dividend Events

In `crates/core/src/quotes/client.rs`, stored `Quote.adjclose` defaults to
`close`, and provider close semantics differ:

- `YAHOO`, `MARKETDATA_APP`, and `FINNHUB` return split-adjusted daily closes.
- `ALPHA_VANTAGE` (`TIME_SERIES_DAILY`) returns as-traded (unadjusted) closes.

To prevent stock splits from distorting returns on unadjusted sources while
preserving configured provider settings:

- `service.rs` inspects `Quote.data_source`. For any unadjusted or mixed quote
  source, it calls `QuoteServiceTrait::fetch_splits_for_symbol` through today
  and applies backward split adjustment (`apply_splits_backward`) so pre-split
  closes align with today's share basis (matching split-adjusted cash
  dividends).
- If no enabled provider can supply split data for an unadjusted source, the
  request fails fast for required assets (or omits an optional benchmark with
  `benchmarkSplitAdjustmentUnavailable`) rather than computing corrupted
  split-drop returns.
- Cash dividends are fetched via `QuoteServiceTrait::fetch_dividends` rather
  than mutating global `quotes.adjclose`, enabling explicit user control over
  `reinvest`, `cash`, and `exclude` dividend modes.

### 4.4 Strategy Definitions

- **Buy & Hold:** Invest initial capital into target weights on day 0; never
  sell to rebalance. Subsequent DCA inflows buy into the initial target weights.
- **Periodic Rebalance:** Rebalance back to target weights on the first trading
  day of each new calendar period (month, quarter, half-year, or year).
- **Drift-Band Rebalance:** Check daily weights; whenever any asset's weight or
  cash share deviates from target by at least `threshold_pct` percentage points,
  rebalance the full portfolio to target weights.
- **SMA Trend Filter:** On day 0 and each check date (`daily` or `monthly`),
  compare each asset's close against its trailing `sma_days` simple moving
  average. Assets above SMA keep their target slice; assets at or below SMA
  redirect their slice to `defensive_symbol` (or USD cash). Under `daily`
  checks, trades execute only when a trend signal flips to avoid daily
  micro-rebalancing noise.
- **Momentum Rotation:** On day 0 and each rebalance boundary, rank universe
  assets by trailing `lookback_days` total return (using dividend-reinvested
  virtual series when dividends are enabled) and select the top `top_n` assets.
  Allocate `equal` or `inverseVolatility` (63-day realized volatility) weights.
  When `absolute_momentum` is enabled, any selected leader with trailing return
  $\le 0\%$ redirects its slice to `defensive_symbol` (or USD cash).

### 4.5 Metrics

All risk and annualized return metrics are derived from the daily **TWR** series
so recurring DCA inflows do not inflate volatility or mask drawdowns:

- **Final Value, Total Invested, Net Profit, Total Return:** Money-on-money
  outcome `(final_value - total_invested) / total_invested`.
- **TWR & CAGR:** Cumulative TWR and annualized TWR over elapsed calendar years
  (`365.25` days/year).
- **Volatility, Sharpe, Sortino:** Annualized ($\times \sqrt{252}$) daily TWR
  standard deviation, Sharpe ratio (0% risk-free rate), and Sortino ratio (0%
  target return).
- **Max Drawdown:** Worst peak-to-trough decline on the TWR index, including
  peak, trough, and recovery dates.
- **Annual Returns, Best/Worst Year, Dividends, Fees, Rebalances, Annualized
  Turnover:** One-way turnover (`0.5 * gross_traded / equity`) annualized across
  the effective window.

## 5. UI & Localization Design

- **Location:** Added as the `backtest` tab in `PortfolioInsightsPage`
  (`apps/frontend/src/pages/insights/portfolio-insights.tsx`) between
  `Performance` and `Income`.
- **Configuration workflow:**
  - Preset bar (60/40 Classic, Three-Fund Drift, All-Weather, 200-Day SMA Trend,
    Dual Momentum) + **Import USD Holdings** button (`extractUsdHoldingsWeights`
    filters open, market-priced, US-listed USD holdings and normalizes up to top
    10 weights to 100%, preserving `exchangeMic` and `instrumentType`).
  - Asset builder powered by `TickerSearchInput` (`hideCustomCreate`), capturing
    listing metadata (`exchangeMic`, `quoteType`, `currency`), weight inputs,
    and one-click **Equalize**.
  - Strategy selector dynamically rendering only the parameters relevant to the
    active strategy.
  - Client-side localized validation (`noValidate` on `<form>` with
    `AlertFeedback`) backed by identical server-side validation.
- **Results dashboard:**
  - Growth chart (`USD Value` / `TWR %` toggle, overlaying Strategy, Buy & Hold
    baseline, Benchmark, and dashed `Total Invested` line when DCA is active),
    Drawdown underwater chart, and Calendar-Year Returns bar chart.
  - Headline KPI cards, full comparison table, ending allocation table,
    market-data coverage table, simulation warnings banner, and methodology
    disclaimer.
- **Localization & RTL:** The backend returns structured
  `BacktestSeriesDescriptor`, `BacktestWarning` enums, and `isCash` flags rather
  than English strings; `apps/frontend/src/features/backtest/formatters.ts`
  renders all series labels, cash labels, and warnings through the `backtest`
  i18n namespace across all 11 supported locales (`en`, `ar`, `de`, `es`, `fr`,
  `it`, `ja`, `ko`, `pt`, `zh`, `zh-Hant`), with CLDR plural forms, logical CSS
  utilities (`ms-`, `me-`, `ps-`, `text-end`), and formatting via
  `@wealthfolio/ui` hooks.

## 6. Verification & Limitations

- **Rust unit tests (`crates/core/src/portfolio/backtest/`):** Verify exact Buy
  & Hold return/drawdown, DRIP vs. Cash vs. Exclude dividends, drift-band
  rebalancing, DCA TWR isolation, initial-purchase and DCA-only fee TWR drag,
  single-session gap limit, benchmark late-start / early-end omission,
  lookahead-free SMA exits, momentum rotation + absolute momentum, Alpha Vantage
  split normalization and missing-split rejection, non-US USD listing (`XLON`)
  and USD crypto (`BTC-USD`) rejection, and dividend/benchmark failure
  isolation.
- **Adapter & i18n checks:** `adapter-command-parity.test.ts` (Tauri/Web
  parity), `i18n:check`, and locale tests (`arabic.test.ts`,
  `traditional-chinese.test.ts`, `pluralization.test.ts`).
- **Playwright E2E (`e2e/backtest.spec.ts`):** Runs against the real built web
  app and Rust `wealthfolio-server` with synthetic fixtures, exercising API
  strategies, non-USD (`SHOP.TO`) and USD crypto (`BTC-USD`) rejection, full UI
  workflow with localized labels/warnings, and `?dir=rtl` desktop/mobile layout
  with zero horizontal overflow.
- **Known v1 limitation:** Automated tests use stubbed/fixture market data
  rather than live Yahoo/Finnhub/Alpha Vantage HTTP endpoints.

## 7. Future Directions (v2+)

1. **Saved experiments:** Local SQLite persistence for naming, saving, and
   comparing multiple strategy runs side by side.
2. **Multi-currency support:** Convert non-USD tickers into base currency via
   existing `FxService` historical rates.
3. **Allocation Target integration:** One-click "Backtest this target" entry
   point from Allocation Targets.
4. **Tax & inflation options:** Optional dividend withholding tax, capital-gains
   drag on rebalancing, and CPI-adjusted real returns.
