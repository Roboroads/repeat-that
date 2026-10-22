import { expect, test } from "@playwright/test";
import { aDocPage, crawl, expectNoSeriousA11yIssues } from "./helpers";

// These run against every build of the site.

test("every page loads from this site only, and every link on the site resolves", async ({ page, request, baseURL }) => {
  const { pages } = await crawl(page, request, baseURL!, async () => {
    await expect(page.locator("h1").first()).toBeVisible();
  });
  // The home page and the docs that were there before the guide.
  for (const path of ["/", "/install/windows/", "/install/macos/", "/linux/", "/macos/", "/verify/", "/privacy/", "/security/"]) {
    expect(pages).toContain(path);
  }
});

test("doc pages take their title from the first heading", async ({ page }) => {
  await page.goto("/verify/");
  const heading = await page.locator("h1").first().innerText();
  await expect(page).toHaveTitle(`${heading} - Repeat That`);
});

test("links in a doc go to the site, or to GitHub for maintainer docs", async ({ page }) => {
  await page.goto("/install/windows/");
  const hrefs = await page.$$eval("article a[href]", (links) => links.map((a) => a.getAttribute("href")!));
  expect(hrefs.length).toBeGreaterThan(0);
  expect(hrefs).toContain("/verify/");
  expect(hrefs).toContain("/privacy/");
  for (const href of hrefs) {
    expect(href, "no link to a .md file is left").not.toMatch(/^(\.{0,2}\/)?[\w./-]+\.md(#.*)?$/);
  }
});

test("security.txt is served with a future Expires and the canonical line", async ({ request }) => {
  const response = await request.get("/.well-known/security.txt");
  expect(response.status()).toBe(200);
  const text = await response.text();
  const expires = /^Expires: (.+)$/m.exec(text)?.[1];
  expect(expires).toBeTruthy();
  expect(new Date(expires!).getTime()).toBeGreaterThan(Date.now() + 150 * 24 * 3600 * 1000);
  expect(text).toContain("Canonical: https://repeat-that.roboroads.me/.well-known/security.txt");
  expect(text).toMatch(/^Contact: https:\/\/github\.com\/Roboroads\/repeat-that\/security\/advisories\/new$/m);
});

test("the head has the canonical url, the preview image and the referrer policy", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator('link[rel="canonical"]')).toHaveAttribute("href", "https://repeat-that.roboroads.me/");
  await expect(page.locator('meta[property="og:image"]')).toHaveAttribute("content", "https://repeat-that.roboroads.me/og.png");
  await expect(page.locator('meta[name="twitter:card"]')).toHaveAttribute("content", "summary_large_image");
  await expect(page.locator('meta[name="referrer"]')).toHaveAttribute("content", "strict-origin-when-cross-origin");
});

test("og.png is a 1280 x 640 PNG", async ({ request }) => {
  const response = await request.get("/og.png");
  expect(response.status()).toBe(200);
  const body = await response.body();
  expect(body.subarray(1, 4).toString()).toBe("PNG");
  expect(body.readUInt32BE(16)).toBe(1280);
  expect(body.readUInt32BE(20)).toBe(640);
});

test("the pages don't scroll sideways on a phone", async ({ page, request }) => {
  await page.setViewportSize({ width: 360, height: 740 });
  for (const path of ["/", await aDocPage(request)]) {
    await page.goto(path);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(overflow, path).toBeLessThanOrEqual(0);
  }
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`${scheme} mode`, () => {
    test.use({ colorScheme: scheme });

    test("the home page has no serious accessibility issues", async ({ page }) => {
      await page.goto("/");
      await expectNoSeriousA11yIssues(page);
    });

    test("a doc page has no serious accessibility issues", async ({ page, request }) => {
      await page.goto(await aDocPage(request));
      await expectNoSeriousA11yIssues(page);
    });
  });
}
