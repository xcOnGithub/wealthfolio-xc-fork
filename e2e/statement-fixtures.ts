import { expect, type Page } from "@playwright/test";
import { BASE_URL } from "./helpers";

/** A dedicated profile keeps these financial assertions independent of other suites. */
export async function prepareStatementProfile(page: Page, name: string) {
  const api = `${BASE_URL}/api/v1`;
  const created = await page.request.post(`${api}/profiles/create_profile`, {
    data: { name, avatarId: "clay-bot-animated" },
  });
  expect(created.ok()).toBeTruthy();
  const profile = await created.json();
  const opened = await page.request.post(`${api}/profiles/unlock_profile`, {
    data: { profileId: profile.id },
  });
  expect(opened.ok()).toBeTruthy();
  const session = await opened.json();
  // Match the production profile admission boundary for both browser and API requests.
  await page.context().setExtraHTTPHeaders({ "x-wf-profile-scope": session.scopeId });
  const saved = await page.request.put(`${api}/settings`, {
    data: {
      onboardingCompleted: true,
      theme: "light",
      language: "en",
      timezone: "UTC",
      baseCurrency: "USD",
      syncEnabled: false,
      autoUpdateCheckEnabled: false,
    },
  });
  expect(saved.ok()).toBeTruthy();
  return async () => {
    const deleted = await page.request.post(`${api}/profiles/delete_profile`, {
      data: { profileId: profile.id, confirmation: name },
    });
    expect(deleted.ok()).toBeTruthy();
  };
}

/** Entirely invented statement, using a built-in monospace font for exact columns. */
export function monthlyPdf(
  month: string,
  day: string,
  quantity: number,
  basis: number | null,
  daily?: {
    year: number;
    price: number;
    closingCash: number;
    openingCash: number;
    fromDay: string;
  },
) {
  const text: string[] = [];
  const put = (value: string, x: number, y: number) =>
    text.push(`BT /F1 10 Tf 1 0 0 1 ${x} ${y} Tm (${value}) Tj ET`);
  const right = (value: string, x: number, y: number) => put(value, x - value.length * 6, y);
  const price = daily?.price ?? 100;
  const cash = daily?.closingCash ?? 10;
  const value = quantity * price + cash;
  const year = daily?.year ?? 2024;
  const fromDay = daily?.fromDay ?? "01";
  put("Merrill", 36, 780);
  put("Account Number: SYNTHETIC-01", 36, 760);
  put(`${month} ${fromDay}, ${year} - ${month} ${day}, ${year}`, 400, 720);
  right(`${month} ${Number(day)}`, 600, 600);
  right("December 31", 700, 600);
  put("Cash/Money Accounts", 36, 580);
  right(`${cash}.00`, 600, 580);
  put("TOTAL ASSETS", 36, 560);
  right(`${value}.00`, 600, 560);
  put("TOTAL LIABILITIES", 36, 540);
  right("0.00", 600, 540);
  put("NET PORTFOLIO VALUE", 36, 520);
  right(`${value}.00`, 600, 520);
  right("100.00", 700, 520);
  put("CMA ASSETS", 36, 450);
  put(`${month} ${fromDay}, ${year} - ${month} ${day}, ${year}`, 400, 450);
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
  right(basis === null ? "N/A" : `${basis}.00`, 400, 350);
  right(`${price}.00`, 500, 350);
  right(`${quantity * price}.00`, 600, 350);
  put("TOTAL", 36, 330);
  right(basis === null ? "N/A" : `${basis}.00`, 400, 330);
  right(`${quantity * price}.00`, 600, 330);
  if (daily) {
    put("This Statement", 240, 480);
    put("Opening Cash/Money Accounts", 36, 460);
    right(`${daily.openingCash}.00`, 324, 460);
    put("YOUR CMA TRANSACTIONS", 36, 300);
    put("SECURITY TRANSACTIONS", 37, 280);
    put("Settlement", 36, 260);
    put("Date", 36, 250);
    put("Description", 84, 250);
    put("Transaction Type", 220, 250);
    right("Quantity", 430, 250);
    right("Amount", 510, 250);
    right("Trading Fees", 590, 250);
    right("Credit", 673, 250);
    put("05/02", 37, 230);
    put("Synthetic equity", 84, 230);
    put("Purchase", 220, 230);
    right("1", 430, 230);
    right("10.00", 510, 230);
    right("-10.00", 673, 230);
    put("TOTAL", 37, 210);
    right("-10.00", 673, 210);
  }
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
