import { expect, test } from "./fixtures";
import { BASE_URL, completeOnboardingIfNeeded } from "./helpers";

test.describe("Backtest Lab", () => {
  test("executes allocation, tactical strategies, and USD validation via real backend API", async ({
    page,
  }) => {
    test.setTimeout(120000);
    await completeOnboardingIfNeeded(page);

    // 1. Periodic Rebalance with DCA + benchmark over fixture USD equities
    const periodicRes = await page.request.post(`${BASE_URL}/api/v1/backtest/run`, {
      data: {
        assets: [
          { symbol: "AAPL", weight: 60 },
          { symbol: "MSFT", weight: 40 },
        ],
        startDate: "2025-01-02",
        endDate: "2026-05-01",
        initialCapital: 10000,
        contribution: { amount: 500, frequency: "monthly" },
        dividendMode: "reinvest",
        feeBps: 5,
        benchmarkSymbol: "VOO",
        strategy: { type: "periodicRebalance", frequency: "quarterly" },
      },
    });
    expect(periodicRes.ok(), await periodicRes.text()).toBe(true);
    const periodic = await periodicRes.json();
    expect(periodic.currency).toBe("USD");
    expect(periodic.tradingDays).toBeGreaterThan(200);
    expect(periodic.strategy.descriptor).toEqual({
      kind: "periodicRebalance",
      frequency: "quarterly",
    });
    expect(periodic.strategy.series.length).toBe(periodic.tradingDays);
    expect(periodic.strategy.metrics.totalInvested).toBeGreaterThan(10000);
    expect(periodic.buyAndHold).not.toBeNull();
    expect(periodic.benchmark?.descriptor).toEqual({ kind: "benchmark", symbol: "VOO" });
    // Fixture provider does not supply dividends; verify graceful isolation structured warning
    expect(periodic.warnings.some((w: { code: string }) => w.code === "dividendsUnavailable")).toBe(
      true,
    );

    // 2. SMA Trend Filter with warmup
    const smaRes = await page.request.post(`${BASE_URL}/api/v1/backtest/run`, {
      data: {
        assets: [
          { symbol: "AAPL", weight: 50 },
          { symbol: "MSFT", weight: 50 },
        ],
        startDate: "2025-06-01",
        endDate: "2026-05-01",
        initialCapital: 10000,
        dividendMode: "exclude",
        feeBps: 0,
        benchmarkSymbol: "VOO",
        strategy: {
          type: "smaTrendFilter",
          smaDays: 50,
          checkFrequency: "monthly",
          defensiveSymbol: "VOO",
        },
      },
    });
    expect(smaRes.ok(), await smaRes.text()).toBe(true);
    const sma = await smaRes.json();
    expect(sma.strategy.descriptor).toEqual({ kind: "smaTrendFilter", smaDays: 50 });
    expect(sma.strategy.series.length).toBeGreaterThan(100);

    // 3. Momentum Rotation with inverse volatility + absolute momentum
    const momRes = await page.request.post(`${BASE_URL}/api/v1/backtest/run`, {
      data: {
        assets: [
          { symbol: "AAPL", weight: 34 },
          { symbol: "MSFT", weight: 33 },
          { symbol: "NVDA", weight: 33 },
        ],
        startDate: "2025-06-01",
        endDate: "2026-05-01",
        initialCapital: 10000,
        dividendMode: "exclude",
        feeBps: 0,
        benchmarkSymbol: "VOO",
        strategy: {
          type: "momentumRotation",
          lookbackDays: 60,
          topN: 2,
          rebalanceFrequency: "monthly",
          weighting: "inverseVolatility",
          absoluteMomentum: true,
          defensiveSymbol: "VOO",
        },
      },
    });
    expect(momRes.ok(), await momRes.text()).toBe(true);
    const mom = await momRes.json();
    expect(mom.strategy.descriptor).toEqual({
      kind: "momentumRotation",
      topN: 2,
      lookbackDays: 60,
    });
    expect(mom.strategy.endingHoldings.length).toBeGreaterThan(0);

    // 4. Non-USD ticker rejection (CAD security SHOP.TO)
    const cadRes = await page.request.post(`${BASE_URL}/api/v1/backtest/run`, {
      data: {
        assets: [{ symbol: "SHOP.TO", weight: 100 }],
        startDate: "2025-01-02",
        endDate: "2026-05-01",
        initialCapital: 10000,
        dividendMode: "exclude",
        feeBps: 0,
        strategy: { type: "buyAndHold" },
      },
    });
    expect(cadRes.status()).toBe(400);

    // 5. USD crypto rejection (outside US-listed equity/ETF scope)
    const cryptoRes = await page.request.post(`${BASE_URL}/api/v1/backtest/run`, {
      data: {
        assets: [{ symbol: "BTC-USD", weight: 100 }],
        startDate: "2025-01-02",
        endDate: "2026-05-01",
        initialCapital: 10000,
        dividendMode: "exclude",
        feeBps: 0,
        strategy: { type: "buyAndHold" },
      },
    });
    expect(cryptoRes.status()).toBe(400);
  });

  test("renders Backtest tab, runs a simulation through the UI, and works cleanly in RTL", async ({
    page,
  }) => {
    test.setTimeout(120000);
    await completeOnboardingIfNeeded(page);
    await page.setViewportSize({ width: 1280, height: 800 });

    await page.goto(`${BASE_URL}/insights?tab=backtest`);
    await expect(page.getByRole("heading", { name: "Backtest Lab" })).toBeVisible();

    // Remove default VTI and BND (not in synthetic E2E fixtures)
    await page.getByRole("button", { name: "Remove VTI" }).click();
    await page.getByRole("button", { name: "Remove BND" }).click();

    // Add AAPL and MSFT using the real TickerSearch combobox
    const addTickerTrigger = page.getByRole("combobox", {
      name: /search and add us ticker/i,
    });
    for (const ticker of ["AAPL", "MSFT"]) {
      await addTickerTrigger.click();
      const searchBox = page.getByPlaceholder(/search/i).last();
      await searchBox.fill(ticker);
      await page
        .getByRole("option", { name: new RegExp(`\\b${ticker}\\b`) })
        .first()
        .click();
    }

    await page.getByRole("button", { name: "Equalize" }).click();
    await expect(page.getByText("100.0%")).toBeVisible();

    // Configure date range and fixture benchmark VOO
    await page.getByLabel("Start date").fill("2025-01-02");
    await page.getByLabel("End date").fill("2026-05-01");
    await page.getByLabel("Benchmark").fill("VOO");

    // Submit and verify full result dashboard
    await page.getByRole("button", { name: "Run Backtest" }).click();

    await expect(page.getByText("Portfolio Growth")).toBeVisible({ timeout: 30000 });
    await expect(page.getByText("Rebalance (Quarterly)").first()).toBeVisible();
    await expect(page.getByText("Drawdown", { exact: true })).toBeVisible();
    await expect(page.getByText("Calendar-Year Returns")).toBeVisible();
    await expect(page.getByText("Performance & Risk Comparison")).toBeVisible();
    await expect(page.getByText("Ending Allocation")).toBeVisible();
    await expect(page.getByText("Market Data Coverage")).toBeVisible();
    await expect(page.getByText("Simulation notes")).toBeVisible();
    await expect(
      page.getByText(/Cash dividend history was unavailable for/i).first(),
    ).toBeVisible();

    // Toggle growth chart mode to TWR %
    await page.getByRole("button", { name: "TWR %" }).click();

    // Verify no horizontal overflow in LTR
    const ltrOverflow = await page.evaluate(
      () => document.documentElement.scrollWidth - window.innerWidth,
    );
    expect(ltrOverflow).toBeLessThanOrEqual(2);

    // Verify RTL layout on desktop and mobile viewports
    for (const viewport of [
      { width: 1280, height: 800 },
      { width: 390, height: 844 },
    ]) {
      await page.setViewportSize(viewport);
      await page.goto(`${BASE_URL}/insights?tab=backtest&dir=rtl`);
      await expect(page.getByRole("heading", { name: "Backtest Lab" })).toBeVisible();
      const dir = await page.evaluate(() => document.documentElement.dir);
      expect(dir).toBe("rtl");
      const rtlOverflow = await page.evaluate(
        () => document.documentElement.scrollWidth - window.innerWidth,
      );
      expect(rtlOverflow).toBeLessThanOrEqual(2);
    }
  });
});
