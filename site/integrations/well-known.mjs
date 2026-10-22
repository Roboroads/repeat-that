// @ts-check
import { mkdir, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const DAY_MS = 24 * 60 * 60 * 1000;
const VALID_DAYS = 180;

/**
 * The text of /.well-known/security.txt (RFC 9116). `Expires` is 180 days after `now`, so the daily rebuild keeps it
 * fresh and a site that stops being rebuilt says so after six months.
 * @param {Date} now
 */
export function securityTxt(now) {
  const expires = new Date(now.getTime() + VALID_DAYS * DAY_MS).toISOString().replace(/\.\d{3}Z$/, "Z");
  return [
    "Contact: https://github.com/Roboroads/repeat-that/security/advisories/new",
    `Expires: ${expires}`,
    "Preferred-Languages: en",
    "Canonical: https://repeat-that.roboroads.me/.well-known/security.txt",
    "Policy: https://github.com/Roboroads/repeat-that/security/policy",
    "",
  ].join("\n");
}

/**
 * Writes `.well-known/security.txt` into the build. Everything else in `public/.well-known/` (for example a
 * verification token) is copied by Astro as it is.
 * @returns {import("astro").AstroIntegration}
 */
export default function wellKnown() {
  return {
    name: "well-known",
    hooks: {
      "astro:build:done": async ({ dir, logger }) => {
        const folder = join(fileURLToPath(dir), ".well-known");
        await mkdir(folder, { recursive: true });
        await writeFile(join(folder, "security.txt"), securityTxt(new Date()));
        // The placeholder only keeps the folder in git.
        await rm(join(folder, ".gitkeep"), { force: true });
        logger.info("wrote .well-known/security.txt");
      },
    },
  };
}
