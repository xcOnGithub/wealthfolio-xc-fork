import { expect, test } from "@playwright/test";
import { BASE_URL, completeOnboardingIfNeeded, gotoAppPath } from "./helpers";

const api = `${BASE_URL}/api/v1`;

/** Entirely invented statement, using a built-in monospace font for exact columns. */
function monthlyPdf(month: string, day: string, quantity: number, basis: number) {
  const text: string[] = [];
  const put = (value: string, x: number, y: number) =>
    text.push(`BT /F1 10 Tf 1 0 0 1 ${x} ${y} Tm (${value}) Tj ET`);
  const right = (value: string, x: number, y: number) => put(value, x - value.length * 6, y);
  const value = quantity * 100 + 10;
  put("Merrill", 36, 780);
  put("Account Number: SYNTHETIC-01", 36, 760);
  put(`${month} 01, 2024 - ${month} ${day}, 2024`, 400, 720);
  right(`${month} ${day}`, 600, 600);
  right("December 31", 700, 600);
  put("Cash/Money Accounts", 36, 580);
  right("10.00", 600, 580);
  put("TOTAL ASSETS", 36, 560);
  right(`${value}.00`, 600, 560);
  put("TOTAL LIABILITIES", 36, 540);
  right("0.00", 600, 540);
  put("NET PORTFOLIO VALUE", 36, 520);
  right(`${value}.00`, 600, 520);
  right("100.00", 700, 520);
  put("CMA ASSETS", 36, 450);
  put(`${month} 01, 2024 - ${month} ${day}, 2024`, 400, 450);
  put("EQUITIES", 36, 400);
  right("Total", 400, 380);
  put("Description", 36, 370);
  put("Symbol", 180, 370);
  right("Quantity", 300, 370);
  right("Cost Basis", 400, 370);
  right("Market Price", 500, 370);
  right("Market Value", 600, 370);
  put("Synthetic equity", 36, 350);
  put("DEMO", 180, 350);
  right(String(quantity), 300, 350);
  right(`${basis}.00`, 400, 350);
  right("100.00", 500, 350);
  right(`${quantity * 100}.00`, 600, 350);
  put("TOTAL", 36, 330);
  right(`${basis}.00`, 400, 330);
  right(`${quantity * 100}.00`, 600, 330);
  const stream = text.join("\n");
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 842 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>",
    "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>",
    `<< /Length ${Buffer.byteLength(stream)} >>\nstream\n${stream}\nendstream`,
  ];
  let pdf = "%PDF-1.4\n";
  const offsets = [0];
  for (const [index, object] of objects.entries()) {
    offsets.push(Buffer.byteLength(pdf));
    pdf += `${index + 1} 0 obj\n${object}\nendobj\n`;
  }
  const xref = Buffer.byteLength(pdf);
  pdf += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  for (const offset of offsets.slice(1)) pdf += `${String(offset).padStart(10, "0")} 00000 n \n`;
  pdf += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF`;
  return Buffer.from(pdf);
}

for (const direction of ["ltr", "rtl"]) {
  test(`monthly PDFs persist reviewed account history without ledger activities (${direction})`, async ({
    page,
  }) => {
    test.setTimeout(120_000);
    await completeOnboardingIfNeeded(page, { keyboard: true });
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
      const input = page.getByLabel(/Merrill monthly statements/);
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
      await expect(save).toBeDisabled();
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
      ).toContainEqual({ accountId, statementDate: "2024-01-31" });
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
    }
  });
}
