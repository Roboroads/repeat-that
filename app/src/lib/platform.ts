// Which OS the page runs on, from `navigator.platform` (or the user agent where that is empty). Only for wording:
// what the Super key and the autostart switch are called.

export function currentPlatform(): string {
  return typeof navigator === 'undefined' ? '' : navigator.platform || navigator.userAgent;
}

// "Darwin" contains "win", so a Mac has to be ruled out first.
export function isMac(platform: string = currentPlatform()): boolean {
  return /mac|darwin/i.test(platform);
}

export function isWindows(platform: string = currentPlatform()): boolean {
  return !isMac(platform) && /win/i.test(platform);
}
