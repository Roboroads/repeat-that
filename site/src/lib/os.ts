export type Os = "windows" | "macos" | "linux";

/**
 * Which desktop system a visitor is on, or null when we have no download for it (Android, ChromeOS, iOS) or can't tell.
 *
 * `platformHint` is `navigator.userAgentData.platform` where the browser has it. It wins over the user agent string,
 * which many browsers freeze or make up. An iPad that reports itself as a Mac comes out as macOS, because its string is
 * the same as Safari on a Mac.
 */
export function detectOs(userAgent: string, platformHint?: string | null): Os | null {
  const hint = platformHint?.trim().toLowerCase();
  if (hint) {
    if (hint === "windows") return "windows";
    if (hint === "macos") return "macos";
    if (hint === "linux") return "linux";
    return null;
  }

  // Android and ChromeOS user agents also say Linux or X11, and an iPhone's or iPad's says Mac OS X, so they go first.
  // Whole words only, "Microsoft" has cros in it.
  if (/\b(android|cros|iphone|ipad|ipod)\b/i.test(userAgent)) return null;
  if (/windows/i.test(userAgent)) return "windows";
  if (/macintosh|mac os x/i.test(userAgent)) return "macos";
  if (/linux|x11/i.test(userAgent)) return "linux";
  return null;
}
