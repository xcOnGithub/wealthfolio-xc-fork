import { test, expect } from "@playwright/test";

test("invitation-only signup, email sign-in, recovery navigation, and logout", async ({
  page,
}, testInfo) => {
  await page.goto("/auth/login");
  await expect(page.getByLabel("Email address")).toBeVisible();
  await page.getByRole("link", { name: "Recover account" }).click();
  await page.getByLabel("Email address").fill("not-registered@example.invalid");
  await page.getByRole("button", { name: "Send recovery code" }).click();
  await expect(page.getByRole("status")).toContainText("If this email is registered");
  // This unregistered address never triggers any outbound email request.
  await page.getByRole("link", { name: "I have a recovery code" }).click();
  await expect(page.getByLabel("Recovery code")).toBeVisible();
  await page.getByRole("link", { name: "Sign in", exact: true }).click();
  await page.getByRole("link", { name: "Create account" }).click();
  await expect(page.getByLabel("Invitation code")).toBeVisible();
  // The ?dir override is development-only. Production follows the cached UI language.
  await page.evaluate(() => localStorage.setItem("wealthfolio-language", "ar"));
  await page.goto("/auth/signup?dir=rtl");
  await expect(page.getByLabel("رمز الدعوة")).toBeVisible();
  await expect(page.locator("html")).toHaveAttribute("dir", "rtl");
  await page.screenshot({ path: testInfo.outputPath("signup-rtl.png") });
  await page.evaluate(() => localStorage.setItem("wealthfolio-language", "en"));
  await page.goto("/auth/signup");
  await page.getByLabel("Email address").fill("owner@example.invalid");
  await page.getByLabel("Invitation code").fill("e2e-only-invitation");
  await page.getByLabel("Password", { exact: true }).fill("owner test password for browser");
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page.getByRole("status")).toContainText("Account created");
  await page.getByLabel("Email address").fill("owner@example.invalid");
  await page.getByLabel("Password", { exact: true }).fill("owner test password for browser");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  // Profile and settings initialization precede the authenticated onboarding route.
  await expect(page.getByTestId("onboarding-page")).toBeVisible({ timeout: 15000 });
  await expect(page).toHaveURL(/\/onboarding$/);
  await expect(page.getByRole("button", { name: "Sign in", exact: true })).toHaveCount(0);
  const me = await page.request.get("/api/v1/auth/me");
  expect(await me.json()).toMatchObject({ authenticated: true });
  const repeat = await page.request.post("/api/v1/auth/signup", {
    data: {
      email: "other@example.invalid",
      password: "other long test password",
      invitationCode: "e2e-only-invitation",
    },
  });
  expect(repeat.status()).toBe(400);
  await page.request.post("/api/v1/auth/logout");
  await page.goto("/auth/login");
  await expect(page.getByLabel("Email address")).toBeVisible();
});
