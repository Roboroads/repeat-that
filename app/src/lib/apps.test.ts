import { describe, expect, it } from 'vitest';
import type { AppEntry } from './api';
import { appRows, matchesTarget } from './apps';

function app(name: string, exePath: string | null = `C:\\Apps\\${name}.exe`, pid = 1): AppEntry {
  return { pid, name, exePath, icon: null };
}

describe('matchesTarget', () => {
  it('matches by exe path regardless of case', () => {
    const target = { exePath: 'c:\\apps\\discord.exe', name: 'Discord' };
    expect(matchesTarget(target, app('Discord'))).toBe(true);
  });

  it('does not match another exe with the same name when both have paths', () => {
    const target = { exePath: 'C:\\Other\\Discord.exe', name: 'Discord' };
    expect(matchesTarget(target, app('Discord'))).toBe(false);
  });

  it('matches by name when the target has no path', () => {
    expect(matchesTarget({ exePath: null, name: 'speech_8s' }, app('Speech_8s', null))).toBe(true);
  });

  it('never matches without a target', () => {
    expect(matchesTarget(null, app('Discord'))).toBe(false);
  });
});

describe('appRows', () => {
  it('lists the apps in order, the target selected', () => {
    const rows = appRows([app('Discord', undefined, 10), app('Teams', undefined, 11)], {
      exePath: 'C:\\Apps\\Teams.exe',
      name: 'Teams',
    });

    expect(rows.map((r) => [r.name, r.selected, r.waiting])).toEqual([
      ['Discord', false, false],
      ['Teams', true, false],
    ]);
  });

  it('pins a target that is not running on top, marked waiting', () => {
    const rows = appRows([app('Teams')], { exePath: 'C:\\Apps\\Discord.exe', name: 'Discord' });

    expect(rows.map((r) => [r.name, r.selected, r.waiting])).toEqual([
      ['Discord', true, true],
      ['Teams', false, false],
    ]);
    expect(rows[0].app).toBeNull();
  });

  it('keeps only the first entry of an exe that has several sessions', () => {
    const rows = appRows([app('Chrome', 'C:\\chrome.exe', 1), app('Chrome', 'C:\\CHROME.exe', 2)], null);

    expect(rows).toHaveLength(1);
    expect(rows[0].app?.pid).toBe(1);
  });

  it('is empty with no apps and no target', () => {
    expect(appRows([], null)).toEqual([]);
  });

  it('takes the icon of the running target for the pinned row when it shows up in the list', () => {
    const withIcon: AppEntry = { ...app('Discord'), icon: 'data:image/png;base64,AAAA' };
    const rows = appRows([withIcon], { exePath: withIcon.exePath, name: 'Discord' });

    expect(rows).toHaveLength(1);
    expect(rows[0].icon).toBe('data:image/png;base64,AAAA');
  });
});
