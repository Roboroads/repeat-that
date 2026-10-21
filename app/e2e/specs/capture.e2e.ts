import { browser, expect } from '@wdio/globals';
import {
  FAKE_APP,
  backendStatus,
  bufferedSeconds,
  headline,
  invoke,
  pauseButton,
  pickFakeApp,
  playbackLine,
  replayButton,
  stopButton,
  waitForBufferedSpeech,
  waitForPage,
} from '../support/app.js';

// The fake app plays the speech fixture over and over, so there is always something to capture and replay.
describe('capturing and replaying', () => {
  before(async () => {
    await waitForPage();
    await pickFakeApp();
  });

  it('buffers speech from the chosen app', async () => {
    await waitForBufferedSpeech(12_000);
    expect(await bufferedSeconds()).toBeGreaterThan(0);
    expect((await backendStatus()).bufferedMs).toBeGreaterThan(0);
  });

  // A replay at normal speed plays on into the speech that keeps coming in, so when it ends on its own depends on the
  // fixture and the machine. Stopping it is what makes this one deterministic.
  it('replays what was buffered, until Stop ends it at once', async () => {
    await expect(playbackLine()).not.toBeExisting();
    await replayButton().click();
    await expect(playbackLine()).toHaveText(expect.stringContaining('Replaying'));
    expect((await backendStatus()).playback).toBe('replaying');

    await stopButton().click();
    await playbackLine().waitForExist({ reverse: true, timeout: 5_000, timeoutMsg: 'the replay went on after Stop' });
    expect((await backendStatus()).playback).toBe('idle');
  });

  // A catch-up replay is the one that ends by itself: it stops as soon as it has reached live. It has no button, only a
  // hotkey and a tray item, so the test calls the command they both end up in.
  it('catches up with live and ends by itself', async () => {
    await browser.waitUntil(async () => (await bufferedSeconds()) >= 3, {
      timeout: 20_000,
      timeoutMsg: 'less than 3 s of speech was buffered after 20 s',
    });
    await invoke('catch_up');
    await expect(playbackLine()).toHaveText(expect.stringContaining('Catching up'));
    await playbackLine().waitForExist({ reverse: true, timeout: 60_000, timeoutMsg: 'catch-up never reached live' });
    expect((await backendStatus()).playback).toBe('idle');
  });

  it('pauses capture and throws away the buffer', async () => {
    await pauseButton().click();
    await expect(headline()).toHaveText('Paused');
    await expect(pauseButton()).toHaveAttribute('aria-pressed', 'true');
    const status = await backendStatus();
    expect(status.capture.state).toBe('paused');
    expect(status.bufferedMs).toBe(0);
  });

  it('resumes capturing and buffers again', async () => {
    await pauseButton().click();
    await browser.waitUntil(async () => (await headline().getText()) === `Capturing ${FAKE_APP}`, {
      timeoutMsg: 'capture did not resume',
    });
    await expect(pauseButton()).toHaveAttribute('aria-pressed', 'false');
    await waitForBufferedSpeech(12_000);
  });
});
