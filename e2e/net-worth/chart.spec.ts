import { expect, test } from "@playwright/test";

for (const width of [320, 1280]) {
  for (const values of ["100,150,120", "-100,-80,-90", "-100,50,100", "-100,-100,-100", "0,0,0"]) {
    test(`history fills downward at ${width}px for ${values}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 1100 });
      await page.goto(`/e2e/net-worth/?chart=${values}&locale=en-US&language=en`);
      const chart = page.getByRole("region", { name: "Net worth history" });
      const curve = chart.locator(".recharts-area-curve");
      await expect(curve).toBeAttached();
      // The line takes the theme's net-worth colour, which must resolve to a real colour.
      await expect(curve).toHaveAttribute("stroke", "var(--net-worth)");
      // Recharts can replace this path during its initial reveal.
      await expect
        .poll(() =>
          curve.evaluate((path) => (path.isConnected ? getComputedStyle(path).stroke : "")),
        )
        .toMatch(/^(rgb|oklab|oklch|color)\(/);
      const area = chart.locator(".recharts-area-area");
      // The fill closes along the bottom of the plot, including all-negative ranges.
      await expect(area).toHaveAttribute("d", /[,L]0,280Z$/);
      const zeroLine = chart.locator(".recharts-reference-line");
      if (values === "-100,50,100") {
        await expect(zeroLine.locator("line")).toHaveAttribute("stroke-dasharray", "4 4");
        await expect(chart.getByText("$0.00", { exact: true })).toBeVisible();
      } else {
        await expect(zeroLine).toHaveCount(0);
      }
      const path = await curve.getAttribute("d");
      expect(path).not.toMatch(/NaN|Infinity/);
      // Recharts animates the reveal using an SVG clipping rectangle.
      await expect
        .poll(async () =>
          chart
            .locator('clipPath[id^="animationClipPath"] rect')
            .evaluateAll((rects) =>
              rects.every(
                (rect) =>
                  Number(rect.getAttribute("width")) >=
                  (rect.ownerSVGElement?.clientWidth ?? Infinity) - 1,
              ),
            ),
        )
        .toBe(true);
      await page.screenshot({ path: test.info().outputPath("chart.png") });
    });
  }
}

for (const direction of ["ltr", "rtl"]) {
  test(`monthly statement history discloses source and incomplete coverage (${direction})`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto("/e2e/net-worth/?chart=100,100,100&statements=1&locale=en-US&language=en");
    await page.evaluate((dir) => {
      document.documentElement.dir = dir;
    }, direction);
    const chart = page.getByRole("region", { name: "Net worth history" });
    const curve = chart.locator(".recharts-area-curve");
    await expect(curve).toBeAttached();
    const box = await chart.boundingBox();
    expect(box).not.toBeNull();
    await page.mouse.move(box!.x + box!.width / 2, box!.y + box!.height / 2);
    await expect(chart.getByText(/Monthly statement value/)).toBeVisible();
    await expect(chart.getByText(/Partial history/)).toBeVisible();
    expect(await curve.getAttribute("d")).not.toContain("C");
  });
}
