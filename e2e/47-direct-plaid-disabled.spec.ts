import { expect, test } from "@playwright/test";
import { completeOnboardingIfNeeded, gotoAppPath } from "./helpers";

test.describe.configure({ mode: "serial" });
for (const direction of ["ltr", "rtl"] as const) {
  test(`direct Plaid stays disabled without changing account setup (${direction})`, async ({
    page,
  }) => {
    test.setTimeout(120_000);
    await completeOnboardingIfNeeded(page, { keyboard: true });
    const externalRequests: string[] = [];
    page.on("request", (request) => {
      if (new URL(request.url()).hostname.endsWith("plaid.com"))
        externalRequests.push(request.url());
    });
    const status = page.waitForResponse(
      (response) =>
        response.url().endsWith("/api/v1/plaid/action") && response.request().method() === "POST",
    );
    await gotoAppPath(page, `/settings/accounts?dir=${direction}`);
    const response = await status;
    expect(response.ok()).toBeTruthy();
    expect(await response.json()).toEqual({ enabled: false, institutions: [], connections: [] });
    await expect(page.getByTestId("direct-plaid-panel")).not.toBeVisible();
    const add = page.getByRole("button", { name: /Add account/i }).first();
    await expect(add).toBeVisible();
    await add.click();
    await expect(page.getByRole("dialog")).toBeVisible();
    expect(externalRequests).toEqual([]);
  });
}
