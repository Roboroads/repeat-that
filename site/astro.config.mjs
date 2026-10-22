// @ts-check
import { defineConfig } from "astro/config";
import media from "./integrations/media.mjs";
import wellKnown from "./integrations/well-known.mjs";

// The site is served from the root of its own domain, so `base` stays "/". GitHub Pages ignores the CNAME file when it
// deploys from Actions, the domain is set under Settings > Pages.
export default defineConfig({
  site: "https://repeat-that.roboroads.me",
  base: "/",
  // SITE_OUT_DIR lets the browser tests build several variants of the site side by side.
  outDir: process.env.SITE_OUT_DIR ?? "./dist",
  trailingSlash: "always",
  build: { format: "directory" },
  // Compressing the HTML drops the space between a word and a link on the next line.
  compressHTML: false,
  devToolbar: { enabled: false },
  integrations: [wellKnown(), media()],
  vite: {
    server: {
      // The docs and the app icon live outside site/ and are read from there, not copied.
      fs: { allow: [".."] },
    },
  },
});
