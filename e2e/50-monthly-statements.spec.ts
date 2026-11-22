import { expect, test } from "@playwright/test";
import { BASE_URL, gotoAppPath } from "./helpers";

const api = `${BASE_URL}/api/v1`;

import { monthlyPdf, prepareStatementProfile } from "./statement-fixtures";

for (const direction of ["ltr", "rtl"]) {
  test(`monthly PDFs persist reviewed account history without ledger activities (${direction})`, async ({
    page,
  }) => {
    test.setTimeout(120_000);
    const cleanupProfile = await prepareStatementProfile(page, `Monthly statement ${direction}`);
    const created = await page.request.post(`${api}/accounts`, {
      data: {
        name: `Statement ${direction}`,
        accountType: "SECURITIES",
        currency: "USD",
        isDefault: false,
        isActive: true,
        trackingMode: "TRANSACTIONS",
      },
    });
    expect(created.ok()).toBeTruthy();
    const accountId = (await created.json()).id as string;
    try {
      await gotoAppPath(page, `/accounts/${accountId}?dir=${direction}`);
      await page.getByText("Import historical statements", { exact: true }).first().click();
      const input = page.getByLabel(/Monthly brokerage statements/);
      await expect(input).toBeEnabled();
      await input.setInputFiles([
        {
          name: "synthetic-january.pdf",
          mimeType: "application/pdf",
          buffer: monthlyPdf("January", "31", 2, 150),
        },
        {
          name: "synthetic-february.pdf",
          mimeType: "application/pdf",
          buffer: monthlyPdf("February", "29", 3, 250),
        },
      ]);
      const save = page.getByRole("button", { name: "Save", exact: true });
      await expect(save).toBeDisabled({ timeout: 30_000 });
      const before = await page.request.get(`${api}/statement-history?accountId=${accountId}`);
      expect(await before.json()).toEqual([]);
      await page.getByRole("checkbox", { name: /I verified the dates/ }).check();
      const saved = page.waitForResponse(
        (r) => r.url().endsWith("/api/v1/statement-history") && r.request().method() === "POST",
      );
      await save.click();
      expect((await saved).ok()).toBeTruthy();
      await page.reload();
      await page.getByText("Import historical statements", { exact: true }).first().click();
      await expect(page.getByRole("button", { name: "Delete", exact: true })).toHaveCount(2);
      // No current holdings: the range control must still expose old statements.
      const allHistory = page.waitForResponse((response) => {
        const url = new URL(response.url());
        return (
          url.pathname.endsWith("/statement-history/values") &&
          url.searchParams.get("startDate") === "1970-01-01"
        );
      });
      await page.getByRole("button", { name: "ALL", exact: true }).click();
      const allHistoryResponse = await allHistory;
      expect(allHistoryResponse.ok()).toBeTruthy();
      expect(
        (await allHistoryResponse.json()).some(
          (row: { date: string }) => row.date === "2024-01-31",
        ),
      ).toBeTruthy();
      await expect(page.locator(".recharts-wrapper").first()).toBeVisible();
      await page.getByRole("button", { name: "3M", exact: true }).click();
      await expect(page.getByRole("button", { name: "ALL", exact: true })).toBeVisible();
      const records = await (
        await page.request.get(`${api}/statement-history?accountId=${accountId}`)
      ).json();
      expect(
        records.map((r: { snapshot: { totalValue: string } }) => r.snapshot.totalValue),
      ).toEqual(["210", "310"]);
      expect(records[0].snapshot.positions[0].costBasis).toBe("150");
      expect(JSON.stringify(records)).not.toContain("SYNTHETIC-01");
      const valuesResponse = await page.request.get(
        `${api}/statement-history/values?accountId=${accountId}&startDate=2024-01-01&endDate=2024-03-01`,
      );
      expect(valuesResponse.ok()).toBeTruthy();
      const values = await valuesResponse.json();
      for (const [date, value] of [
        ["2024-01-31", 210],
        ["2024-02-15", 210],
        ["2024-02-29", 310],
        ["2024-03-01", 310],
      ]) {
        const point = values.find((row: { date: string }) => row.date === date);
        expect(Number(point.value)).toBe(value);
        expect(Number(point.cashValue)).toBe(10);
        expect(point.currency).toBe("USD");
      }
      // Other E2E specs may own accounts in this profile. Check our source
      // contribution rather than assuming the global net worth starts at zero.
      const historyResponse = await page.request.get(
        `${api}/net-worth/history?startDate=2024-01-01&endDate=2024-03-01`,
      );
      expect(historyResponse.ok()).toBeTruthy();
      const history = await historyResponse.json();
      expect(
        history.find((row: { date: string }) => row.date === "2024-02-15").statementSources,
      ).toContainEqual(expect.objectContaining({ accountId, statementDate: "2024-01-31" }));
      const activities = await page.request.post(`${api}/activities/search`, {
        data: { accountIdFilter: [accountId], page: 1, pageSize: 10 },
      });
      expect(activities.ok()).toBeTruthy();
      const activityData = await activities.json();
      expect(activityData.meta.totalRowCount).toBe(0);
      await page.getByRole("button", { name: "Delete", exact: true }).first().click();
      await page.getByRole("button", { name: "Confirm", exact: true }).click();
      await expect(page.getByRole("button", { name: "Delete", exact: true })).toHaveCount(1);
    } finally {
      expect((await page.request.delete(`${api}/accounts/${accountId}`)).ok()).toBeTruthy();
      await cleanupProfile();
    }
  });
}
