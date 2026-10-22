// What the update specs know about the update banner and the Updates block of General. A debug build gets a pretend
// update source from `REPEAT_THAT_MOCK_UPDATE` (see src-tauri/src/updater/source.rs): `available` offers 99.0.0, and
// the app has one source for its whole run, so each answer of it has a spec file of its own (SPEC_ENV in wdio.conf.ts).
import { $, browser } from '@wdio/globals';
import { invoke } from './app.js';

/** The version every mock but `none` and `fail-check` announces. */
export const MOCK_VERSION = '99.0.0';

/** `get_update_status`, as far as the specs look at it. */
export interface BackendUpdateStatus {
  mode: 'install' | 'notify' | 'external' | 'off';
  modeReason: string | null;
  currentVersion: string;
  state: 'idle' | 'checking' | 'upToDate' | 'available' | 'downloading' | 'installing' | 'error';
  available: { version: string; date: string | null; notes: string; releaseUrl: string } | null;
  progress: number | null;
  /** In state `error`. */
  error: string | null;
  errorKind: 'check' | 'install' | null;
  lastCheckedAt: number | null;
  manual: boolean;
}

/** The backend's own view of the updater, for what the page does not show. */
export const backendUpdateStatus = () => invoke<BackendUpdateStatus>('get_update_status');

/**
 * Fails with a clear message when the app wasn't started the way the spec needs. Without it every test would fail
 * without saying why. `mock` is the value of `REPEAT_THAT_MOCK_UPDATE`, or `null` for an app with no update source.
 */
export async function requireUpdateSource(mock: string | null): Promise<void> {
  const { mode } = await backendUpdateStatus();
  // A mock makes the app act as an install that can replace itself; no mock in a debug build is no updater at all.
  if (mode === (mock === null ? 'off' : 'install')) return;
  throw new Error(
    mock === null
      ? `the app has an updater (mode "${mode}"), but this spec needs none: REPEAT_THAT_MOCK_UPDATE must not be set (SPEC_ENV in e2e/wdio.conf.ts)`
      : `the app wasn't started with REPEAT_THAT_MOCK_UPDATE=${mock} (SPEC_ENV in e2e/wdio.conf.ts), its update mode is "${mode}"`,
  );
}

/** "Check for updates automatically?", the question a build that checks asks until it is answered. */
export const consentQuestion = () => $('aside*=Check for updates automatically?');

/** The banner for the mock's version, in any of its forms: available, downloading, installing or failed. */
export const offerBanner = () => $(`aside*=${MOCK_VERSION}`);

/** What the banner for the mock's version shows at one moment. */
export interface OfferSnapshot {
  /** Everything the banner says; empty without a banner. */
  text: string;
  /** The value of its progress bar, 0 to 1; `null` without a bar. */
  progress: number | null;
}

/** The banner for the mock's version, read in one round trip: for what changes faster than separate commands follow. */
export async function offerSnapshot(): Promise<OfferSnapshot> {
  return browser.execute((version: string) => {
    const banner = [...document.querySelectorAll('aside')].find((aside) => aside.textContent?.includes(version));
    const bar = banner?.querySelector('progress');
    return { text: banner?.textContent ?? '', progress: bar ? bar.value : null };
  }, MOCK_VERSION);
}

/** The "Check for updates automatically" switch in General. */
export const updateSwitch = () =>
  $('//input[@role="switch"][@aria-labelledby=//label[normalize-space()="Check for updates automatically"]/@id]');

/** "Version 0.1.0 · last checked never", under the switch. */
export const versionLine = () => $('p*=last checked');

export const checkNowButton = () => $('button=Check now');

/** What the last check came to, next to "Check now": "You have the latest version.", the error, ... */
export const checkResult = () => $('div[role="status"]');
