import { expect, test } from "./fixtures";
import { api, createAccount, createActivity } from "./api";
import { completeOnboardingIfNeeded, gotoAppPath } from "./helpers";
import { PHONE } from "./phone";

test.use(PHONE);

for (const direction of ["ltr", "rtl"] as const) {
  test(`mobile holding performance remains below its value (${direction})`, async ({ page }) => {
    test.setTimeout(90_000);
    await completeOnboardingIfNeeded(page);
    const symbol = `ALIGN${direction.toUpperCase()}${Date.now().toString(36).toUpperCase()}`;
    const account = await createAccount(page, { name: `Value alignment ${symbol}` });
    await createActivity(page, {
      accountId: account.id,
      activityType: "DEPOSIT",
      activityDate: "2026-01-02T12:00:00Z",
      currency: "USD",
      amount: 100,
    });
    const buy = await createActivity(page, {
      accountId: account.id,
      activityType: "BUY",
      activityDate: "2026-01-02T13:00:00Z",
      currency: "USD",
      quantity: 2,
      unitPrice: 50,
      asset: { symbol, name: `Alignment ${symbol}`, quoteCcy: "USD" },
    });
    await expect
      .poll(
        async () => {
          const scope = await api<{
            holdings: { instrument?: { id: string }; marketValue: { local: number | null } }[];
          }>(page, "POST", "/holdings/list/query", {
            filter: { type: "account", accountId: account.id },
          });
          return scope.holdings.find((holding) => holding.instrument?.id === buy.assetId)
            ?.marketValue.local;
        },
        { timeout: 60_000 },
      )
      .toBe(100);

    await gotoAppPath(page, `/holdings?dir=${direction}`);
    await expect(page.locator("html")).toHaveAttribute("dir", direction);
    const row = page.getByRole("link").filter({ hasText: symbol });
    await expect(row).toBeVisible();
    // RTL formatting wraps amounts in bidi isolates; match the amount within them.
    const value = row.getByText(/\$100\.00/);
    const valueColumn = value.locator("..");
    const performance = valueColumn.getByText(/\$0\.00/);
    await expect(performance).toBeVisible();
    const valueBounds = (await value.boundingBox())!;
    const performanceBounds = (await performance.boundingBox())!;
    expect(performanceBounds.y).toBeGreaterThanOrEqual(valueBounds.y + valueBounds.height - 1);
    await row.screenshot({ path: test.info().outputPath(`holding-${direction}.png`) });
  });
}
