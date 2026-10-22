import { expect, test } from "@playwright/test";
import { UA } from "./helpers";

// A repository without a release.

test("shows the No release yet card with a link to the Releases page, and no downloads", async ({ page }) => {
  await page.goto("/");
  const card = page.locator("#downloads [data-empty]");
  await expect(card).toContainText("No release yet");
  await expect(card.getByRole("link", { name: "Releases on GitHub" })).toHaveAttribute(
    "href",
    "https://github.com/Roboroads/repeat-that/releases",
  );
  await expect(page.locator("#downloads [data-platform]")).toHaveCount(0);
  await expect(page.locator("#downloads .file-link")).toHaveCount(0);
});

test.describe("a Linux visitor", () => {
  test.use({ userAgent: UA.linux });

  test("still gets a hero button that goes to the downloads section", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("[data-hero-button]")).toHaveText("Download");
    await expect(page.locator("[data-hero-button]")).toHaveAttribute("href", "#downloads");
  });
});
