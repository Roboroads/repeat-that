import { describe, expect, it } from 'vitest';
import { formatChecked, isLink, versionLine } from './updates';

// Local time, so the expected text does not depend on the machine's time zone.
const at = (day: number, hour: number, minute: number) => new Date(2026, 9, day, hour, minute).getTime();

describe('formatChecked', () => {
  it('says never before the first check of this run', () => {
    expect(formatChecked(null)).toBe('never');
  });

  it('shows the time of a check made today', () => {
    expect(formatChecked(at(22, 14, 32), at(22, 18, 0), 'en-GB')).toBe('14:32');
  });

  it('adds the date once the check is from another day', () => {
    expect(formatChecked(at(21, 14, 32), at(22, 9, 0), 'en-GB')).toBe('21 Oct, 14:32');
  });
});

describe('versionLine', () => {
  it('names the version and when it was last checked', () => {
    expect(versionLine('0.1.0', at(22, 14, 32), at(22, 15, 0), 'en-GB')).toBe('Version 0.1.0 · last checked 14:32');
    expect(versionLine('0.1.0', null)).toBe('Version 0.1.0 · last checked never');
  });

  it('starts with the check when the version is not known', () => {
    expect(versionLine('', null)).toBe('Last checked never');
  });
});

describe('isLink', () => {
  it('accepts an https address only', () => {
    expect(isLink('https://github.com/Roboroads/repeat-that/releases/tag/v0.2.0')).toBe(true);
    expect(isLink('http://github.com/Roboroads/repeat-that')).toBe(false);
    expect(isLink('javascript:alert(1)')).toBe(false);
    expect(isLink('')).toBe(false);
  });
});
