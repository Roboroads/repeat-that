import { expect, test } from "@playwright/test";
import { UA } from "./helpers";

// The build with every platform and a placeholder demo video (see playwright.config.ts).

const BASE = "https://github.com/Roboroads/repeat-that/releases/download/v0.2.0";
const FILE = {
  windows: `${BASE}/Repeat-That_0.2.0_x64-setup.exe`,
  macos: `${BASE}/Repeat-That_0.2.0_aarch64.dmg`,
  linux: `${BASE}/Repeat-That_0.2.0_amd64.AppImage`,
};

test("has a card for each platform of the release, in the order of the data", async ({ page }) => {
  await page.goto("/");
  const cards = page.locator("#downloads [data-platform]");
  await expect(cards).toHaveCount(4);
  expect(await cards.evaluateAll((els) => els.map((e) => e.getAttribute("data-platform")))).toEqual(["windows", "linux", "flatpak", "macos"]);
});

for (const [os, label] of [
  ["windows", "Windows"],
  ["macos", "macOS"],
  ["linux", "Linux"],
] as const) {
  test.describe(`${label} visitor`, () => {
    test.use({ userAgent: UA[os] });

    test("gets their card marked and a hero button that points at its file", async ({ page }) => {
      await page.goto("/");
      const card = page.locator(`[data-platform="${os}"]`);
      await expect(card).toHaveAttribute("data-detected", "true");
      await expect(card.locator("[data-badge]")).toBeVisible();
      await expect(page.locator('[data-detected="true"]')).toHaveCount(1);
      const button = page.locator("[data-hero-button]");
      await expect(button).toHaveText(`Download for ${label}`);
      await expect(button).toHaveAttribute("href", FILE[os]);
    });
  });
}

test.describe("a visitor on a system without a download", () => {
  test.use({ userAgent: UA.android });

  test("sees no card marked, and the hero button goes to the downloads", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("[data-detected]")).toHaveCount(0);
    await expect(page.locator("[data-hero-button]")).toHaveAttribute("href", "#downloads");
  });
});

test.describe("without JavaScript", () => {
  test.use({ javaScriptEnabled: false, userAgent: UA.windows });

  test("every card is shown, none is marked, and the hero button jumps to the downloads", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("#downloads [data-platform]")).toHaveCount(4);
    await expect(page.locator("[data-detected]")).toHaveCount(0);
    await expect(page.locator("[data-badge]").first()).toBeHidden();
    await expect(page.locator("[data-hero-button]")).toHaveAttribute("href", "#downloads");
  });
});

test("the Windows card has the installer, its hash, the sums, the SmartScreen note and the install steps", async ({ page }) => {
  await page.goto("/");
  const card = page.locator('[data-platform="windows"]');
  await expect(card.getByRole("link", { name: "Repeat-That_0.2.0_x64-setup.exe" })).toHaveAttribute("href", FILE.windows);
  await expect(card.locator("code")).toContainText("977a78fe093b2522df24f902fb933ea31e80d0d841ada33ad5de671061de7508");
  await expect(card.getByText("9.6 MB")).toBeVisible();
  await expect(card.getByRole("link", { name: "SHA256SUMS for v0.2.0" })).toHaveAttribute("href", `${BASE}/SHA256SUMS`);
  await expect(card.getByRole("link", { name: "How to verify" })).toHaveAttribute("href", "/verify/");
  await expect(card).toContainText("SmartScreen");
  await expect(card.getByRole("link", { name: "Install steps" })).toHaveAttribute("href", "/install/windows/");
});

test("the macOS card says it is untested, links the report form and quotes the requirements from the README", async ({ page }) => {
  await page.goto("/");
  const card = page.locator('[data-platform="macos"]');
  await expect(card.locator('[data-notice="untested"]')).toContainText("not tested on a real Mac");
  await expect(card.getByRole("link", { name: "file a macOS report" })).toHaveAttribute(
    "href",
    "https://github.com/Roboroads/repeat-that/issues/new?template=macos.yml",
  );
  await expect(card).toContainText("macOS 14.2 or newer. Apple Silicon (M1 or later). Intel Macs aren't supported.");
});

test("the Linux card lists the AppImage, the .deb and the .rpm in that order", async ({ page }) => {
  await page.goto("/");
  const names = await page.locator('[data-platform="linux"] .file-link').allInnerTexts();
  expect(names).toEqual(["Repeat-That_0.2.0_amd64.AppImage", "Repeat-That_0.2.0_amd64.deb", "Repeat-That-0.2.0-1.x86_64.rpm"]);
  await expect(page.locator('[data-platform="linux"]').getByRole("link", { name: "Hotkeys on Linux" })).toHaveAttribute("href", "/linux/");
});

test("the requirements section is the one from the README", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Minimum system requirements" })).toBeVisible();
  const cards = page.locator("#requirements [data-requirements]");
  expect(await cards.evaluateAll((els) => els.map((e) => e.getAttribute("data-requirements")))).toEqual(["windows", "macos", "linux"]);
  await expect(cards.nth(2)).toContainText("Debian 13, Ubuntu 24.04 or Fedora 40");
  await expect(cards.nth(2)).toContainText("PipeWire with WirePlumber");
  // The paragraph that points at the install guides sits under the section, with links for the site.
  await expect(page.locator("#requirements .req-footer").getByRole("link", { name: "docs/install-windows.md" })).toHaveAttribute(
    "href",
    "/install/windows/",
  );
});

test("the video is there but loads nothing until play is pressed", async ({ page }) => {
  // The captions file is a few hundred bytes and the browser fetches it for a default track, the video itself must wait.
  const videoRequests: string[] = [];
  page.on("request", (r) => {
    if (r.url().endsWith("/media/demo.mp4")) videoRequests.push(r.url());
  });
  await page.goto("/", { waitUntil: "networkidle" });

  const video = page.locator("#demo video");
  await expect(page.locator("#demo").getByRole("heading", { name: "Watch the introduction" })).toBeVisible();
  await expect(video).toHaveAttribute("preload", "none");
  await expect(video.locator("track")).toHaveAttribute("src", "/media/demo.vtt");
  await expect(page.locator("#demo figcaption")).toHaveText("A short introduction to Repeat That. The app never records to disk.");
  expect(videoRequests, "requests for the video before play").toEqual([]);

  const asked = page.waitForRequest((r) => r.url().endsWith("/media/demo.mp4"));
  // Headless Chromium ignores a click on the video, so play is pressed from the keyboard, which is a real way to do it.
  await video.focus();
  await page.keyboard.press("Space");
  await asked;
});

test("the download links can be reached with the keyboard and show where the focus is", async ({ page }) => {
  await page.goto("/");
  const link = page.locator('[data-platform="windows"] .file-link');
  for (let i = 0; i < 80; i++) {
    await page.keyboard.press("Tab");
    if (await link.evaluate((el) => el === document.activeElement)) break;
  }
  await expect(link).toBeFocused();
  const outline = await link.evaluate((el) => {
    const style = getComputedStyle(el);
    return { style: style.outlineStyle, width: parseFloat(style.outlineWidth) };
  });
  expect(outline.style).not.toBe("none");
  expect(outline.width).toBeGreaterThanOrEqual(2);
});
