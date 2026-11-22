import { expect, test } from "@playwright/test";
import { BASE_URL, gotoAppPath } from "./helpers";
import { monthlyPdf, prepareStatementProfile } from "./statement-fixtures";
const api = `${BASE_URL}/api/v1`;
for (const direction of ["ltr", "rtl"]) {
  test(`statement PDF reconstructs persisted daily net worth independently of Plaid (${direction})`, async ({
    page,
  }) => {
    test.setTimeout(180_000);
    const cleanupProfile = await prepareStatementProfile(page, `Daily statement ${direction}`);
    const created = await page.request.post(`${api}/accounts`, {
      data: {
        name: `Daily statement ${direction}`,
        accountType: "SECURITIES",
        currency: "USD",
        isDefault: false,
        isActive: true,
        trackingMode: "TRANSACTIONS",
      },
    });
    expect(created.ok()).toBeTruthy();
    const accountId = (await created.json()).id as string;
    let assetId: string | undefined;
    try {
      const historyUrl = `${api}/net-worth/history?startDate=2025-05-02&endDate=2025-05-05`;
      const before = await (await page.request.get(historyUrl)).json();
      let plaidRequests = 0;
      let quoteRefreshes = 0;
      page.on("request", (r) => {
        if (r.url().includes("/plaid/")) plaidRequests++;
        if (r.url().endsWith("/statement-history/daily/quotes")) quoteRefreshes++;
      });
      await gotoAppPath(page, `/accounts/${accountId}?dir=${direction}`);
      await expect(page.locator("html")).toHaveAttribute("dir", direction);
      await page.getByText("Import historical statements", { exact: true }).first().click();
      await page
        .getByRole("combobox", { name: "History import mode", exact: true })
        .selectOption("daily");
      const input = page.getByLabel(/Monthly brokerage statements/);
      await expect(input).toBeEnabled();
      await input.setInputFiles({
        name: "synthetic-daily.pdf",
        mimeType: "application/pdf",
        buffer: monthlyPdf("May", "05", 2, null, {
          year: 2025,
          price: 12,
          closingCash: 90,
          openingCash: 100,
          fromDay: "02",
        }),
      });
      const holding = page.getByRole("combobox", {
        name: "Synthetic equity DEMO · 2",
        exact: true,
      });
      await expect(holding).not.toHaveValue("");
      assetId = await holding.inputValue();
      const catalog = await page.request.get(`${api}/assets`);
      expect(catalog.ok()).toBeTruthy();
      expect(
        (await catalog.json()).filter(
          (asset: { instrumentSymbol?: string }) => asset.instrumentSymbol === "DEMO",
        ),
      ).toEqual([expect.objectContaining({ id: assetId, quoteCcy: "USD", quoteMode: "MARKET" })]);
      for (const [date, close] of [
        ["2025-05-02", 10],
        ["2025-05-05", 12],
      ] as const) {
        const response = await page.request.put(`${api}/market-data/quotes/${assetId}`, {
          data: {
            id: "",
            assetId,
            timestamp: `${date}T16:00:00Z`,
            createdAt: new Date().toISOString(),
            open: close,
            high: close,
            low: close,
            close,
            adjclose: 999,
            volume: 0,
            currency: "USD",
            dataSource: "MANUAL",
          },
        });
        expect(response.ok()).toBeTruthy();
      }
      await page
        .getByText("Review dated events and transaction asset links", { exact: true })
        .click();
      await expect(
        page.getByRole("combobox", { name: "Synthetic equity", exact: true }),
      ).toHaveValue(assetId!);
      await page.getByRole("button", { name: "Preview daily history", exact: true }).click();
      const save = page.getByRole("button", { name: "Save", exact: true });
      await expect(save).toBeDisabled({ timeout: 30_000 });
      await page.getByRole("checkbox", { name: /I verified the account, USD currency/ }).check();
      const saved = page.waitForResponse(
        (r) => r.url().endsWith("/statement-history/daily") && r.request().method() === "POST",
      );
      await save.click();
      expect((await saved).ok()).toBeTruthy();
      const response = await page.request.get(
        `${api}/statement-history/values?accountId=${accountId}&startDate=2025-05-02&endDate=2025-05-05`,
      );
      expect(response.ok()).toBeTruthy();
      const values = await response.json();
      expect(values.map((r: { value: string }) => Number(r.value))).toEqual([110, 110, 110, 114]);
      expect(values.map((r: { source: string }) => r.source)).toEqual([
        "statementDaily",
        "statementDaily",
        "statementDaily",
        "statementClosing",
      ]);
      const net = await (await page.request.get(historyUrl)).json();
      expect(net.map((point: { date: string }) => point.date)).toEqual(
        values.map((v: { date: string }) => v.date),
      );
      for (const point of net) {
        const previous = before.find((p: { date: string }) => p.date === point.date);
        expect(Number(point.netWorth) - Number(previous?.netWorth ?? 0)).toBe(
          Number(values.find((v: { date: string }) => v.date === point.date).value),
        );
        expect(
          point.statementSources.filter((s: { accountId: string }) => s.accountId === accountId),
        ).toHaveLength(1);
      }
      const activities = await page.request.post(`${api}/activities/search`, {
        data: { accountIdFilter: [accountId], page: 1, pageSize: 10 },
      });
      expect((await activities.json()).meta.totalRowCount).toBe(0);
      await page.getByRole("button", { name: "Activities", exact: true }).click();
      const statementActivity = page.getByRole("region", { name: "Merrill statement activity" });
      const buy = statementActivity.getByRole("row").filter({ hasText: "Buy" });
      await expect(buy).toContainText("DEMO");
      await expect(buy).toContainText(/-\$10\.00/);
      await expect(buy.getByRole("button")).toHaveCount(0);
      await expect(page.getByText("No activities", { exact: true })).toHaveCount(0);
      expect(plaidRequests).toBe(0);
      expect(quoteRefreshes).toBe(0);
      await page.reload();
      await page.getByText("Import historical statements", { exact: true }).first().click();
      await page
        .getByRole("combobox", { name: "History import mode", exact: true })
        .selectOption("daily");
      await expect(page.getByText(/Priced daily observations: 4 of 4/)).toBeVisible();
      await page.getByRole("button", { name: "ALL", exact: true }).click();
      await expect(page.locator(".recharts-wrapper").first()).toBeVisible();
      await input.setInputFiles({
        name: "synthetic-daily-later.pdf",
        mimeType: "application/pdf",
        buffer: monthlyPdf("June", "05", 3, null, {
          year: 2025,
          price: 12,
          closingCash: 80,
          openingCash: 90,
          fromDay: "02",
          tradeDate: "06/02",
        }),
      });
      await expect(
        page.getByRole("combobox", { name: "Synthetic equity DEMO · 3", exact: true }),
      ).toHaveValue(assetId!);
      await page
        .getByText("Review dated events and transaction asset links", { exact: true })
        .click();
      await expect(
        page.getByRole("combobox", { name: "Synthetic equity", exact: true }),
      ).toHaveValue(assetId!);
      await page
        .getByRole("combobox", { name: "History import mode", exact: true })
        .selectOption("monthly");
      await page.getByRole("button", { name: "Delete", exact: true }).click();
      await page.getByRole("button", { name: "Confirm", exact: true }).click();
      await expect
        .poll(
          async () =>
            await (
              await page.request.get(`${api}/statement-history/daily?accountId=${accountId}`)
            ).json(),
        )
        .toEqual([]);
      await expect(statementActivity).toHaveCount(0);
    } catch (error) {
      await test.info().attach("statement-review", {
        body: await page.locator("main").ariaSnapshot(),
        contentType: "text/plain",
      });
      throw error;
    } finally {
      await page.request.delete(`${api}/accounts/${accountId}`);
      if (assetId) await page.request.delete(`${api}/assets/${assetId}`);
      await cleanupProfile();
    }
  });
}
