import { defineConfig } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { VARIANTS } from "./tests/e2e/variants.mjs";

// The browser tests run against three builds of the site, see tests/e2e/variants.mjs. tests/e2e/start.mjs builds them
// one after the other and serves each on its own port.
const root = import.meta.dirname;

// A placeholder where the demo would be. It is a few bytes, never real audio or video.
const mediaDir = resolve(root, ".e2e/media");
mkdirSync(mediaDir, { recursive: true });
writeFileSync(resolve(mediaDir, "demo.mp4"), Buffer.from("00000018667479706d703432000000006d70343269736f6d", "hex"));
writeFileSync(resolve(mediaDir, "demo.vtt"), "WEBVTT\n\n00:00.000 --> 00:02.000\nPlaceholder caption.\n");

export default defineConfig({
  testDir: "tests/e2e",
  outputDir: ".e2e/results",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: "list",
  use: { browserName: "chromium" },
  projects: VARIANTS.map((v) => ({
    name: v.name,
    testMatch: v.match,
    use: { baseURL: `http://127.0.0.1:${v.port}` },
  })),
  webServer: {
    command: "node tests/e2e/start.mjs",
    url: `http://127.0.0.1:${VARIANTS[0].port}/`,
    timeout: 240_000,
    reuseExistingServer: false,
  },
});
