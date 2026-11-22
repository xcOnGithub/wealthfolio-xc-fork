import { test, expect } from "./fixtures";
import {
  api,
  createActivity,
  restoreSpendingSettings,
  seedSpendingAccounts,
  uniqueName,
  updateSettings,
} from "./api";
import { BASE_URL } from "./helpers";
import type { FamilySnapshot } from "../apps/frontend/src/features/family/types";

for (const dir of ["ltr", "rtl"]) {
  test(`family inbox decisions, monthly preview and delivery opt-in persist in ${dir}`, async ({
    page,
  }) => {
    const previousAppSettings = await updateSettings(page, { baseCurrency: "USD" });
    const { bank, previous } = await seedSpendingAccounts(page, `Family ${dir}`);
    const family = async (path: string, data: unknown) => {
      const res = await page.request.post(`${BASE_URL}/api/v1/family/${path}`, {
        headers: { Origin: BASE_URL },
        data,
      });
      expect(res.ok(), await res.text()).toBe(true);
      return res.json() as Promise<FamilySnapshot>;
    };
    const initial = await api<FamilySnapshot>(page, "GET", "/family");
    const merchant = uniqueName(`Family synthetic charge ${dir}`);
    try {
      const charge = await createActivity(page, {
        accountId: bank.id,
        activityType: "WITHDRAWAL",
        amount: 2400,
        currency: "USD",
        activityDate: new Date().toISOString(),
        notes: merchant,
      });
      const billName = uniqueName(`Annual family bill ${dir}`);
      const dueDate = new Date(Date.now() + 7 * 86400000).toISOString().slice(0, 10);
      await family("settings", {
        settings: {
          ...initial.settings,
          enabled: true,
          largeChargeAmount: 1000,
          bills: [
            {
              id: crypto.randomUUID(),
              name: billName,
              dueDate,
              amount: 1200,
              currency: "USD",
              annual: true,
            },
          ],
          emailAlerts: false,
          emailDigest: false,
          webhookAlerts: false,
          webhookDigest: false,
        },
      });
      const snapshot = await family("refresh", {});
      const item = snapshot.items.find((i) => i.key === `transaction:${charge.id}`);
      expect(item).toBeDefined();
      expect(snapshot.items.some((i) => i.key === `charge:${charge.id}`)).toBe(true);
      expect(snapshot.items.some((i) => i.title.includes(billName))).toBe(true);
      expect(snapshot.deliveries.filter((d) => d.status === "pending")).toHaveLength(0);
      await page.goto(`${BASE_URL}/family?dir=${dir}`);
      const card = page
        .locator('[data-slot="card"]')
        .filter({ has: page.getByRole("heading", { name: "What was this charge?", exact: true }) })
        .filter({ hasText: merchant });
      await expect(card).toBeVisible();
      await card.getByText("Family note", { exact: true }).click();
      await card.getByRole("textbox", { name: "Family note" }).fill("Discuss at our next meeting");
      await card.getByRole("button", { name: "Save note" }).click();
      await expect
        .poll(
          async () =>
            (await api<FamilySnapshot>(page, "GET", "/family")).items.find((i) => i.id === item!.id)
              ?.note,
        )
        .toBe("Discuss at our next meeting");
      await card.getByRole("button", { name: "Mark read" }).click();
      await expect(card.getByText("Read", { exact: true })).toBeVisible();
      await page.reload();
      await expect(card.getByText("Read", { exact: true })).toBeVisible();
      await card.getByRole("button", { name: "Snooze 7 days" }).click();
      await expect(card).toHaveCount(0);
      await page.getByRole("combobox", { name: "Filter inbox" }).selectOption("snoozed");
      await expect(card).toBeVisible();
      await card.getByRole("button", { name: "Dismiss", exact: true }).click();
      await page.getByRole("combobox", { name: "Filter inbox" }).selectOption("resolved");
      await expect(card).toBeVisible();
      await page.getByRole("button", { name: "Monthly meeting", exact: true }).click();
      await page.getByRole("button", { name: "Preview meeting", exact: true }).click();
      await expect(
        page.getByText("Three questions for your money meeting", { exact: true }),
      ).toBeVisible();
      await expect(page.getByText("Savings rate", { exact: true })).toBeVisible();
      await page.getByRole("button", { name: "Alerts & delivery", exact: true }).click();
      for (const label of [
        "Email new alerts",
        "Email monthly digest",
        "Webhook new alerts",
        "Webhook monthly digest",
      ])
        await expect(page.getByRole("checkbox", { name: label, exact: true })).not.toBeChecked();
      await page.getByLabel("Large charge amount", { exact: true }).fill("1500");
      await page.getByRole("button", { name: "Save settings", exact: true }).click();
      await expect(page.getByText("Settings saved.", { exact: true })).toBeVisible();
      await page.reload();
      await expect(page.getByLabel("Large charge amount", { exact: true })).toHaveValue("1500");
      await expect(page.locator("html")).toHaveAttribute("dir", dir);
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1),
      ).toBe(true);
      const forbidden = await page.request.post(`${BASE_URL}/api/v1/family/settings`, {
        headers: { Origin: "https://unrelated.example" },
        data: { settings: initial.settings },
      });
      expect(forbidden.status()).toBe(403);
    } finally {
      await restoreSpendingSettings(page, previous);
      await api(page, "PUT", "/settings", previousAppSettings);
      await family("settings", { settings: initial.settings });
    }
  });
}
