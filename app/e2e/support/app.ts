// What the specs know about the settings page. Selectors follow the roles and labels a user (or a screen reader) sees,
// not the CSS, so a restyle does not break them.
import { $, browser } from '@wdio/globals';
import { spawn } from 'node:child_process';
import path from 'node:path';

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

/** Calls a command of the backend the way the page does, and returns what it answers. */
export async function invoke<T = void>(command: string): Promise<T> {
  return browser.execute((name: string) => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string) => Promise<unknown> } })
      .__TAURI_INTERNALS__;
    return internals.invoke(name);
  }, command) as Promise<T>;
}

/** The backend's own view of the engine, for what the page does not show. */
export interface BackendStatus {
  capture: { state: string };
  playback: string;
  /** How far the replay is from live; `null` while idle. */
  behindMs: number | null;
  bufferedMs: number;
}

export const backendStatus = () => invoke<BackendStatus>('get_status');

/** The settings as the backend has them, which is what a reload shows. */
export const backendSettings = () =>
  invoke<{ bufferSeconds: number; checkForUpdates: boolean | null }>('get_settings');

/** The built app, where `wdio.conf.ts` starts it from. */
function applicationPath(): string {
  const repoDir = path.resolve(import.meta.dirname, '..', '..', '..');
  const targetDir = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(repoDir, 'target');
  return path.join(targetDir, 'debug', 'repeat-that');
}

export interface LaunchResult {
  /** Everything the second app printed. */
  output: string;
}

/**
 * Starts the app again with `args`, in the session of the app that is running: the same folders and the same D-Bus, which
 * is how the single-instance check finds the running one. That is what a key bound to `repeat-that --replay` does. The
 * second app is meant to hand its arguments over and exit with 0 by itself, so this waits for that, and fails (with
 * what the app printed) if it exits with anything else or takes longer than `timeoutMs`.
 */
export async function launchAgain(args: string[], timeoutMs = 20_000): Promise<LaunchResult> {
  const root = process.env.E2E_APP_HOME as string;
  const folder = (name: string) => path.join(root, name);
  const env: NodeJS.ProcessEnv = {
    ...process.env,
    HOME: folder('home'),
    TMPDIR: folder('tmp'),
    XDG_RUNTIME_DIR: folder('run'),
    XDG_CONFIG_HOME: folder('config'),
    XDG_DATA_HOME: folder('data'),
    XDG_CACHE_HOME: folder('cache'),
    DBUS_SESSION_BUS_ADDRESS: `unix:path=${folder('bus')}`,
    LIBGL_ALWAYS_SOFTWARE: '1',
    REPEAT_THAT_NULL_OUTPUT: '1',
  };
  const child = spawn(applicationPath(), args, { env, stdio: ['ignore', 'pipe', 'pipe'] });
  let output = '';
  child.stdout.on('data', (chunk: Buffer) => (output += chunk.toString()));
  child.stderr.on('data', (chunk: Buffer) => (output += chunk.toString()));
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => child.kill('SIGKILL'), timeoutMs);
    child.once('exit', (code, signal) => {
      clearTimeout(timer);
      if (code === 0) resolve({ output });
      else reject(new Error(`the second app ended with ${code ?? signal} after \`${args.join(' ')}\`:\n${output}`));
    });
  });
}

/** How many windows the webdriver session sees. The app has the one. */
export async function windowCount(): Promise<number> {
  return (await browser.getWindowHandles()).length;
}
