import { describe, expect, it } from 'vitest';
import { isMac, isWindows } from './platform';

describe('platform', () => {
  it.each(['Win32', 'Windows', 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)'])('%s is Windows', (platform) => {
    expect(isWindows(platform)).toBe(true);
    expect(isMac(platform)).toBe(false);
  });

  it.each(['MacIntel', 'macOS', 'Darwin x86_64', 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)'])(
    '%s is a Mac, even when the text has "win" in it',
    (platform) => {
      expect(isMac(platform)).toBe(true);
      expect(isWindows(platform)).toBe(false);
    },
  );

  it.each(['Linux x86_64', 'X11; Linux', ''])('"%s" is neither', (platform) => {
    expect(isWindows(platform)).toBe(false);
    expect(isMac(platform)).toBe(false);
  });
});
