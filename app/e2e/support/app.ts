// What the specs know about the settings page. Selectors follow the roles and labels a user (or a screen reader) sees,
// not the CSS, so a restyle does not break them.
import { $, browser } from '@wdio/globals';

/** The fake app the debug build on Linux plays: the speech fixture, named after its file. */
export const FAKE_APP = 'speech_8s';

/** Waits for the settings page to have loaded its settings and status (the heading and the status bar). */
export async function waitForPage(): Promise<void> {
  await $('h1=Repeat That').waitForDisplayed();
  await statusBar().waitForDisplayed();
}

export const statusBar = () => $('section[aria-label="Status"]');
/** "Capturing speech_8s", "Paused", "No app selected", ... */
export const headline = () => $('section[aria-label="Status"] .headline');
/** "Replaying · 3 s behind live"; only exists while a replay plays. */
export const playbackLine = () => $('section[aria-label="Status"] .playback');
// A text selector (`button=Replay`) can't follow a CSS one, so these look inside the status bar by chaining.
export const replayButton = () => statusBar().$('button=Replay');
export const stopButton = () => statusBar().$('button=Stop');
export const pauseButton = () => statusBar().$('button=Pause capture');

/** The app list when it is open: one radio button per running app. */
export const appRadio = (name: string) => $(`input[type="radio"][aria-label^="${name}"]`);
/** The app list when it is collapsed: one button holding the chosen app. */
export const chosenApp = (name: string) => $(`button[aria-label^="${name}, change app"]`);

/** What the status bar's second line says it has buffered, in seconds; 0 when it says nothing. */
export async function bufferedSeconds(): Promise<number> {
  const text = await $('section[aria-label="Status"] .lines').getText();
  const match = /(\d+) s of speech buffered/.exec(text);
  return match ? Number(match[1]) : 0;
}

/** Picks the fake app and waits until it is being captured. */
export async function pickFakeApp(): Promise<void> {
  const radio = appRadio(FAKE_APP);
  await radio.waitForDisplayed({ timeoutMsg: `${FAKE_APP} is not in the app list` });
  await radio.click();
  await chosenApp(FAKE_APP).waitForDisplayed();
  await browser.waitUntil(async () => (await headline().getText()) === `Capturing ${FAKE_APP}`, {
    timeoutMsg: `the status never said "Capturing ${FAKE_APP}"`,
  });
}

/** The speech buffer has some speech in it, which a fresh capture takes a few seconds to fill. */
export async function waitForBufferedSpeech(timeout = 12_000): Promise<void> {
  await browser.waitUntil(async () => (await bufferedSeconds()) > 0, {
    timeout,
    timeoutMsg: `no speech was buffered within ${timeout / 1000} s`,
  });
}

/** The backend's own view, for what the page does not show: the status as `get_status` answers it. */
export interface BackendStatus {
  capture: { state: string };
  playback: string;
  bufferedMs: number;
}

export async function backendStatus(): Promise<BackendStatus> {
  return browser.execute(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string) => Promise<unknown> } })
      .__TAURI_INTERNALS__;
    return internals.invoke('get_status') as Promise<BackendStatus>;
  });
}
