import type { UpdateMode } from './api';

/** When the last update check happened, for the line under the update switch: "never", "14:32" or "21 Oct, 14:32". */
export function formatChecked(at: number | null, now: number = Date.now(), locale?: string): string {
  if (at === null) return 'never';
  const when = new Date(at);
  const time = when.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' });
  if (when.toDateString() === new Date(now).toDateString()) return time;
  return `${when.toLocaleDateString(locale, { day: 'numeric', month: 'short' })}, ${time}`;
}

/** "Version 0.1.0 · last checked 14:32". */
export function versionLine(version: string, lastCheckedAt: number | null, now?: number, locale?: string): string {
  const checked = formatChecked(lastCheckedAt, now, locale);
  return version === '' ? `Last checked ${checked}` : `Version ${version} · last checked ${checked}`;
}

/** Whether a build in `mode` looks for new versions itself (and so asks, and has a switch and a button). */
export function checksForUpdates(mode: UpdateMode): boolean {
  return mode === 'install' || mode === 'notify';
}

/** The release page comes from the backend; the window only opens it when it is a web address. */
export function isLink(url: string): boolean {
  return url.startsWith('https://');
}
