// @ts-check
import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** The files the site knows how to show, as they are named on the `site-media` prerelease. */
export const MEDIA_FILES = {
  video: "demo.mp4",
  poster: "demo-poster.jpg",
  captions: "demo.vtt",
};

/** `public/media` unless `SITE_MEDIA_DIR` (relative to the site folder) names another folder. */
export function mediaDirectory(env = process.env, root = process.cwd()) {
  return resolve(root, env.SITE_MEDIA_DIR ?? "public/media");
}

/**
 * Copies the known media files into `media/` of the build. Files in `public/media` are already copied by Astro, so this
 * is for a media folder somewhere else (the browser tests use one).
 * @param {string} sourceDir
 * @param {string} outDir
 */
export function copyMedia(sourceDir, outDir) {
  for (const name of Object.values(MEDIA_FILES)) {
    const source = join(sourceDir, name);
    if (!existsSync(source)) continue;
    mkdirSync(join(outDir, "media"), { recursive: true });
    copyFileSync(source, join(outDir, "media", name));
  }
}

/** @returns {import("astro").AstroIntegration} */
export default function media() {
  return {
    name: "media",
    hooks: {
      "astro:build:done": ({ dir, logger }) => {
        const source = mediaDirectory();
        if (source === resolve(process.cwd(), "public/media")) return;
        copyMedia(source, fileURLToPath(dir));
        logger.info(`copied the demo media from ${source}`);
      },
    },
  };
}
