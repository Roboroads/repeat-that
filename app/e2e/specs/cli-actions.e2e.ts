import { browser, expect } from '@wdio/globals';
import {
  backendStatus,
  bufferedSeconds,
  launchAgain,
  pickFakeApp,
  playbackLine,
  waitForBufferedSpeech,
  waitForPage,
  windowCount,
} from '../support/app.js';

// What a key bound to `repeat-that --replay` does on a desktop with no hotkeys of its own: it starts the app again, and
// the app that is already running takes the action. The fake app plays the speech fixture over and over, so there is
// always something to replay.
describe('command-line actions', () => {
  before(async () => {
    await waitForPage();
    await pickFakeApp();
    await waitForBufferedSpeech(12_000);
  });

  it('--replay replays in the running app, and the second app hands over and exits', async () => {
    await expect(playbackLine()).not.toBeExisting();
    const windows = await windowCount();

    await launchAgain(['--replay']);

    await expect(playbackLine()).toHaveText(expect.stringContaining('Replaying'));
    expect((await backendStatus()).playback).toBe('replaying');
    // No second window, and the one there is stays the same page.
    expect(await windowCount()).toBe(windows);
    await expect(playbackLine()).toBeDisplayed();
  });

  // Pressing Replay again during a replay steps back one utterance, so the replay is further from live afterwards. The
  // first replay may be over by now, so this one starts its own.
  it('a second --replay during the replay is one more press: it steps back', async () => {
    await launchAgain(['--stop']);
    await launchAgain(['--replay']);
    await expect(playbackLine()).toHaveText(expect.stringContaining('Replaying'));
    const before = (await backendStatus()).behindMs ?? 0;

    await launchAgain(['--replay']);

    await browser.waitUntil(async () => ((await backendStatus()).behindMs ?? 0) > before, {
      timeout: 5_000,
      timeoutMsg: `the replay was not stepped back (it was ${before} ms behind live)`,
    });
    expect((await backendStatus()).playback).toBe('replaying');
  });

  it('--stop ends the replay', async () => {
    await launchAgain(['--stop']);

    await playbackLine().waitForExist({ reverse: true, timeout: 5_000, timeoutMsg: 'the replay went on after --stop' });
    expect((await backendStatus()).playback).toBe('idle');
  });

  it('--catch-up starts a catch-up, which ends by itself at live', async () => {
    await browser.waitUntil(async () => (await bufferedSeconds()) >= 3, {
      timeout: 20_000,
      timeoutMsg: 'less than 3 s of speech was buffered after 20 s',
    });

    await launchAgain(['--catch-up']);

    await expect(playbackLine()).toHaveText(expect.stringContaining('Catching up'));
    await playbackLine().waitForExist({ reverse: true, timeout: 60_000, timeoutMsg: 'catch-up never reached live' });
    expect((await backendStatus()).playback).toBe('idle');
  });

  it('flags it does not know are ignored, also next to --hidden', async () => {
    await launchAgain(['--frobnicate', '--hidden', '--replay']);

    await expect(playbackLine()).toHaveText(expect.stringContaining('Replaying'));
    await launchAgain(['--stop']);
  });

  it('a launch without an action only hands over and exits', async () => {
    const before = await backendStatus();

    await launchAgain([]);

    expect((await backendStatus()).playback).toBe(before.playback);
  });
});
