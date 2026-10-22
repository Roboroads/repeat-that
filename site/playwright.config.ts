import { defineConfig } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

// The browser tests build the site three times and serve each build on its own port:
//   all      every platform, plus a placeholder demo video and captions
//   windows  the release the site has today (v0.1.0, Windows only), no video
//   none     no release at all
// Each build goes to .e2e/dist-<name> through SITE_OUT_DIR, so the real dist/ is left alone.
const root = import.meta.dirname;
const e2eDir = resolve(root, ".e2e");

// A placeholder where the demo would be. It is a few bytes, never real audio or video.
const mediaDir = resolve(e2eDir, "media");
mkdirSync(mediaDir, { recursive: true });
writeFileSync(resolve(mediaDir, "demo.mp4"), Buffer.from("00000018667479706d703432000000006d70343269736f6d", "hex"));
writeFileSync(resolve(mediaDir, "demo.vtt"), "WEBVTT\n\n00:00.000 --> 00:02.000\nPlaceholder caption.\n");

const variants = [
  { name: "all", port: 4331, release: "tests/fixtures/release-all.json", media: ".e2e/media", match: ["all.spec.ts", "common.spec.ts"] },
  { name: "windows", port: 4332, release: "tests/fixtures/release-windows.json", media: ".e2e/no-media", match: ["windows.spec.ts", "common.spec.ts"] },
  { name: "none", port: 4333, release: "tests/fixtures/release-none.json", media: ".e2e/no-media", match: ["none.spec.ts", "common.spec.ts"] },
];

export default defineConfig({
  testDir: "tests/e2e",
  outputDir: ".e2e/results",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: "list",
  use: { browserName: "chromium" },
  projects: variants.map((v) => ({
    name: v.name,
    testMatch: v.match,
    use: { baseURL: `http://127.0.0.1:${v.port}` },
  })),
  webServer: variants.map((v) => ({
    command: `npx astro build && node tests/e2e/serve.mjs .e2e/dist-${v.name} ${v.port}`,
    url: `http://127.0.0.1:${v.port}/`,
    timeout: 240_000,
    reuseExistingServer: false,
    env: {
      ASTRO_TELEMETRY_DISABLED: "1",
      SITE_OUT_DIR: `.e2e/dist-${v.name}`,
      SITE_RELEASE_JSON: v.release,
      // The README's requirements section as it will be, so the tests don't depend on the README of the branch they run on.
      SITE_README: "tests/fixtures/readme-requirements.md",
      SITE_MEDIA_DIR: v.media,
    },
  })),
});
