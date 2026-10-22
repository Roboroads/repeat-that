import { expect, test } from "@playwright/test";
import { UA } from "./helpers";

// The release the site has today: v0.1.0, Windows only, and no demo video.

test("has one card, for Windows, with the hash from the release", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator("#downloads [data-platform]")).toHaveCount(1);
  const card = page.locator('[data-platform="windows"]');
  await expect(card.locator("code")).toHaveText("fdc279452d7692574c3a9dc6888243eaa33a464777a3076036d707fb1eaef29d");
  await expect(card.getByRole("link", { name: "SHA256SUMS for v0.1.0" })).toHaveAttribute(
    "href",
    "https://github.com/Roboroads/repeat-that/releases/download/v0.1.0/SHA256SUMS",
  );
  await expect(page.getByText("Version 0.1.0, released 21 October 2026.")).toBeVisible();
  await expect(page.getByRole("link", { name: "All releases" })).toHaveAttribute("href", "https://github.com/Roboroads/repeat-that/releases");
});

test("has no video section", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator("#demo")).toHaveCount(0);
  await expect(page.locator("video")).toHaveCount(0);
});

test.describe("a Linux visitor", () => {
  test.use({ userAgent: UA.linux });

  test("is told Linux isn't available yet, and the hero button goes to the Linux page", async ({ page }) => {
    await page.goto("/");
    const button = page.locator("[data-hero-button]");
    await expect(button).toHaveText("Not available for Linux yet");
    await expect(button).toHaveAttribute("href", "/linux/");
    await expect(page.locator("[data-detected]")).toHaveCount(0);
  });
});

test.describe("a macOS visitor", () => {
  test.use({ userAgent: UA.macos });

  test("is told macOS isn't available yet when the release has no .dmg", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("[data-hero-button]")).toHaveText("Not available for macOS yet");
    await expect(page.locator("[data-hero-button]")).toHaveAttribute("href", "/macos/");
  });
});

test.describe("a Windows visitor", () => {
  test.use({ userAgent: UA.windows });

  test("gets the Windows card marked and the installer behind the hero button", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator('[data-platform="windows"]')).toHaveAttribute("data-detected", "true");
    await expect(page.locator("[data-hero-button]")).toHaveText("Download for Windows");
    await expect(page.locator("[data-hero-button]")).toHaveAttribute(
      "href",
      "https://github.com/Roboroads/repeat-that/releases/download/v0.1.0/Repeat-That_0.1.0_x64-setup.exe",
    );
  });
});

test("the download links can be reached with the keyboard and show where the focus is", async ({ page }) => {
  await page.goto("/");
  const link = page.locator('[data-platform="windows"] .file-link');
  for (let i = 0; i < 80; i++) {
    await page.keyboard.press("Tab");
    if (await link.evaluate((el) => el === document.activeElement)) break;
  }
  await expect(link).toBeFocused();
  const outlineWidth = await link.evaluate((el) => parseFloat(getComputedStyle(el).outlineWidth));
  expect(outlineWidth).toBeGreaterThanOrEqual(2);
});
