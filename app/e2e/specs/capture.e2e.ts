import { browser, expect } from '@wdio/globals';
import {
  FAKE_APP,
  backendStatus,
  bufferedSeconds,
  headline,
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

  it('replays what was buffered, then goes back to idle', async () => {
    await expect(playbackLine()).not.toBeExisting();
    await replayButton().click();
    await expect(playbackLine()).toHaveText(expect.stringContaining('Replaying'));
    expect((await backendStatus()).playback).toBe('replaying');
    // The replay runs in real time and plays on into the speech that comes in meanwhile, so it takes a while.
    await playbackLine().waitForExist({ reverse: true, timeout: 45_000, timeoutMsg: 'the replay never ended' });
    expect((await backendStatus()).playback).toBe('idle');
  });

  it('stops a replay at once', async () => {
    await replayButton().click();
    await expect(playbackLine()).toBeExisting();
    await stopButton().click();
    await playbackLine().waitForExist({ reverse: true, timeout: 5_000, timeoutMsg: 'the replay went on after Stop' });
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
