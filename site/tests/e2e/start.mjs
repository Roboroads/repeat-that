// @ts-check
// What Playwright starts before the browser tests: builds every variant of the site one after the other (Astro keeps
// its caches and generated types in this folder, so two builds at once could trip over each other), then serves each
// build on its own port. The first variant is served last, it is the port Playwright waits for.
import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { createStaticServer } from "./serve.mjs";
import { VARIANTS, variantEnv } from "./variants.mjs";

const astro = resolve("node_modules/astro/bin/astro.mjs");

for (const variant of VARIANTS) {
  const build = spawnSync(process.execPath, [astro, "build"], { env: { ...process.env, ...variantEnv(variant) }, stdio: "inherit" });
  if (build.status !== 0) {
    console.error(`the build of the "${variant.name}" variant failed`);
    process.exit(build.status ?? 1);
  }
}

/** @param {{ name: string, port: number }} variant */
const serve = (variant) =>
  new Promise((done) => {
    createStaticServer(`.e2e/dist-${variant.name}`).listen(variant.port, "127.0.0.1", () => {
      console.log(`serving ${variant.name} on http://127.0.0.1:${variant.port}/`);
      done(undefined);
    });
  });

for (const variant of [...VARIANTS].reverse()) await serve(variant);
