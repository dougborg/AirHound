import { test, expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
for (const size of [
  { name: "desktop", width: 1440, height: 1000 },
  { name: "mobile", width: 390, height: 844 },
]) {
  test(`${size.name}: homepage and accessible navigation`, async ({ page }) => {
    await page.setViewportSize(size);
    await page.goto("/");
    await expect(page.getByRole("heading", { level: 1 })).toContainText(
      "wireless world",
    );
    await expect(
      page.getByRole("link", { name: "Choose your setup" }),
    ).toHaveAttribute("href", "/start/");
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBeTruthy();
    const results = await new AxeBuilder({ page })
      .withTags(["wcag2a", "wcag2aa", "wcag21aa", "wcag22aa"])
      .analyze();
    expect(results.violations).toEqual([]);
    await page.screenshot({
      path: `.impeccable/review/${size.name}.png`,
      fullPage: true,
    });
  });
}
test("receiver and signature filters have real empty and reset states", async ({
  page,
}) => {
  await page.goto("/hardware/");
  await page.getByLabel("Platform", { exact: true }).selectOption("Research");
  await expect(page.locator("[data-receiver]:visible")).toHaveCount(4);
  await page.goto("/signatures/");
  await page
    .getByLabel("Search label or rule ID")
    .fill("no-such-signature-xyz");
  await expect(page.getByText("No rules match these filters.")).toBeVisible();
  await page.getByLabel("Search label or rule ID").fill("");
  expect(
    await page.locator("[data-signature]:visible").count(),
  ).toBeGreaterThan(0);
});
test("docs local search finds imported component guidance", async ({
  page,
}) => {
  await page.goto("/reference/docs/");
  await expect(
    page.getByText("Development documentation", { exact: false }).first(),
  ).toBeVisible();
  await page
    .getByRole("button", { name: /Search/ })
    .first()
    .click();
  await page.getByRole("textbox", { name: "Search", exact: true }).fill("gpsd");
  await expect(page.locator(".pagefind-ui__result").first()).toBeVisible();
});
test("key content pages pass automated accessibility checks", async ({
  page,
}) => {
  for (const path of ["/downloads/", "/hardware/", "/signatures/", "/start/"]) {
    await page.goto(path);
    const results = await new AxeBuilder({ page })
      .withTags(["wcag2a", "wcag2aa", "wcag21aa", "wcag22aa"])
      .analyze();
    expect(results.violations, path).toEqual([]);
  }
});
test("essential content works without JavaScript", async ({ browser }) => {
  const context = await browser.newContext({ javaScriptEnabled: false });
  const page = await context.newPage();
  await page.goto("http://127.0.0.1:4321/hardware/");
  await expect(page.getByText("CatSniffer 3.2 / Sniffle")).toBeVisible();
  await page.goto("http://127.0.0.1:4321/downloads/");
  await expect(
    page.getByRole("link", { name: "Release notes & all assets" }).first(),
  ).toBeVisible();
  await context.close();
});
