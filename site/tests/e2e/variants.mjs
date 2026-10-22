// @ts-check
// The builds of the site that the browser tests run against, shared by playwright.config.ts and start.mjs.
//   all      every platform, plus a placeholder demo video and captions
//   windows  the release the site has today (v0.1.0, Windows only), no video
//   none     no release at all
// Each build goes to .e2e/dist-<name> through SITE_OUT_DIR, so the real dist/ is left alone.

/** @type {{ name: string, port: number, release: string, media: string, match: string[] }[]} */
export const VARIANTS = [
  { name: "all", port: 4331, release: "tests/fixtures/release-all.json", media: ".e2e/media", match: ["all.spec.ts", "common.spec.ts"] },
  { name: "windows", port: 4332, release: "tests/fixtures/release-windows.json", media: ".e2e/no-media", match: ["windows.spec.ts", "common.spec.ts"] },
  { name: "none", port: 4333, release: "tests/fixtures/release-none.json", media: ".e2e/no-media", match: ["none.spec.ts", "common.spec.ts"] },
];

/**
 * The environment of the build of a variant.
 * @param {{ name: string, release: string, media: string }} variant
 */
export function variantEnv(variant) {
  return {
    ASTRO_TELEMETRY_DISABLED: "1",
    SITE_OUT_DIR: `.e2e/dist-${variant.name}`,
    SITE_RELEASE_JSON: variant.release,
    // The README's requirements section as it will be, so the tests don't depend on the README of the branch they run on.
    SITE_README: "tests/fixtures/readme-requirements.md",
    SITE_MEDIA_DIR: variant.media,
  };
}
