import { expect, test } from "@playwright/test";
import { BASE_URL, completeOnboardingIfNeeded } from "./helpers";

test("currency names and search update on a live language change without changing values", async ({
  page,
}, testInfo) => {
  test.setTimeout(120_000);
  await completeOnboardingIfNeeded(page);
  const api = `${BASE_URL}/api/v1`;
  const original = await (await page.request.get(`${api}/settings`)).json();
  try {
    expect((await page.request.put(`${api}/settings`, { data: { language: "en" } })).ok()).toBe(
      true,
    );
    await page.goto(`${BASE_URL}/settings/general?dir=rtl`);
    await page.getByRole("button", { name: "Customize currency formats", exact: true }).click();
    const currency = page.getByTestId("currency-format-new-currency");
    await expect(currency).toBeVisible();
    await currency.click();
    await page.getByPlaceholder("Search currency...").fill("CNY");
    await page.getByRole("option", { name: /Chinese\/Yuan renminbi \(CNY\)/ }).click();
    await expect(currency).toContainText("CNY");
    const timeOrigin = await page.evaluate(() => performance.timeOrigin);

    await page.getByTestId("language-select").click();
    await page.getByTestId("language-option-zh").click();
    await expect(currency).toContainText("人民币");
    await expect(currency).toContainText("CNY");
    expect(await page.evaluate(() => performance.timeOrigin)).toBe(timeOrigin);
    await expect(page.locator("html")).toHaveAttribute("dir", "rtl");
    await page.screenshot({
      path: testInfo.outputPath("currency-selected-zh-rtl.png"),
      fullPage: true,
    });

    await currency.click();
    const search = page.getByPlaceholder("搜索货币...");
    for (const query of ["美元", "United States", "USD"]) {
      await search.fill(query);
      await expect(page.getByRole("option", { name: /^美元 \(USD\)$/ })).toBeVisible();
    }
    await search.fill("not-a-currency-42");
    await expect(page.getByText("未找到货币。", { exact: true })).toBeVisible();
    await page.keyboard.press("Escape");

    await page.getByTestId("language-select").click();
    await page.getByTestId("language-option-en").click();
    await expect(currency).toContainText("Chinese/Yuan renminbi");
    expect(await page.evaluate(() => performance.timeOrigin)).toBe(timeOrigin);
    const current = await (await page.request.get(`${api}/settings`)).json();
    expect(current.baseCurrency).toBe(original.baseCurrency);
    expect(current.currencyFormattingRegions).toEqual(original.currencyFormattingRegions);
  } finally {
    expect(
      (
        await page.request.put(`${api}/settings`, { data: { language: original.language || "en" } })
      ).ok(),
    ).toBe(true);
  }
});

test("Chinese liability options preserve raw metadata and custom names in LTR and RTL", async ({
  page,
}, testInfo) => {
  test.setTimeout(180_000);
  await completeOnboardingIfNeeded(page);
  const api = `${BASE_URL}/api/v1`;
  const original = await (await page.request.get(`${api}/settings`)).json();
  try {
    expect((await page.request.put(`${api}/settings`, { data: { language: "zh" } })).ok()).toBe(
      true,
    );
    for (const direction of ["ltr", "rtl"]) {
      await page.goto(`${BASE_URL}/holdings?tab=assets&dir=${direction}`);
      await expect(page.locator("html")).toHaveAttribute("dir", direction);
      await page.locator('button.h-9.w-9[aria-haspopup="dialog"]').click();
      await page.getByRole("dialog").getByRole("button", { name: "添加资产", exact: true }).click();
      const sheet = page.getByRole("dialog");
      await sheet.getByRole("button", { name: /负债.*贷款与债务/ }).click();
      await sheet.getByRole("button", { name: "继续", exact: true }).click();
      const type = sheet.getByRole("combobox", { name: "负债类型", exact: true });
      await expect(type).toContainText("抵押贷款");
      await type.click();
      for (const label of ["抵押贷款", "汽车贷款", "助学贷款", "个人贷款", "其他"]) {
        await expect(page.getByRole("option", { name: label, exact: true })).toBeVisible();
      }
      await page.screenshot({
        path: testInfo.outputPath(`liability-options-zh-${direction}.png`),
        fullPage: true,
      });
      await page.getByRole("option", { name: "汽车贷款", exact: true }).click();
      const name = `Untranslated custom loan ${direction}`;
      await sheet.getByPlaceholder("住房抵押贷款、汽车贷款……").fill(name);
      await sheet
        .getByRole("combobox")
        .filter({ hasText: /\([A-Z]{3}\)/ })
        .click();
      await page.getByPlaceholder("搜索货币...").fill("人民币");
      await page.getByRole("option", { name: /^人民币 \(CNY\)$/ }).click();
      await sheet.getByRole("switch", { name: "根据还款估算余额" }).click();
      await sheet
        .getByText("原始金额", { exact: true })
        .locator("..")
        .getByPlaceholder("0.00")
        .fill("100");
      await sheet.locator('input[type="date"]').first().fill("2022-06-05");
      await sheet.getByRole("button", { name: "添加负债", exact: true }).click();
      await expect(sheet).toHaveCount(0, { timeout: 15_000 });
      const holdings = await (await page.request.get(`${api}/alternative-holdings`)).json();
      const created = holdings.find((holding: { name: string }) => holding.name === name);
      expect(created).toMatchObject({ name, currency: "CNY", metadata: { sub_type: "auto_loan" } });
    }
  } finally {
    expect(
      (
        await page.request.put(`${api}/settings`, { data: { language: original.language || "en" } })
      ).ok(),
    ).toBe(true);
  }
});
