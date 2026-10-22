import { $, browser, expect } from '@wdio/globals';
import {
  FAKE_APP,
  appRadio,
  backendStatus,
  chosenApp,
  headline,
  invoke,
  pauseButton,
  playbackLine,
  replayButton,
  statusBar,
  stopButton,
  waitForBufferedSpeech,
  waitForPage,
} from '../support/app.js';

// What macOS does without the permission to capture audio, on Linux: wdio.conf.ts starts this app with
// REPEAT_THAT_MOCK_PERMISSION=denied, so captures are refused until "Ask again", which allows them at once. The help
// says what macOS 15 would offer: both buttons.
const NOT_ALLOWED = "Repeat That isn't allowed to capture audio";
const help = () => $('[role="group"][aria-label="Allow audio capture"]');

describe('capture that is not allowed', () => {
  before(async () => {
    await waitForPage();
    // Without the variable the app has nothing to allow here, and every test below would fail without saying why.
    const offered = await invoke<{ canOpenSettings: boolean; canAskAgain: boolean }>('capture_permission_help');
    if (!offered.canAskAgain) {
      throw new Error("the app wasn't started with REPEAT_THAT_MOCK_PERMISSION=denied (SPEC_ENV in e2e/wdio.conf.ts)");
    }
    const radio = appRadio(FAKE_APP);
    await radio.waitForDisplayed({ timeoutMsg: `${FAKE_APP} is not in the app list` });
    await radio.click();
    await chosenApp(FAKE_APP).waitForDisplayed();
  });

  it('says so, how to allow it, and has both buttons', async () => {
    await expect(headline()).toHaveText(NOT_ALLOWED);
    await expect(statusBar()).toHaveText(expect.stringContaining('Screen & System Audio Recording'));
    await help().waitForDisplayed({ timeoutMsg: 'no help to allow capture' });
    await expect(help().$('button=Open System Settings')).toBeEnabled();
    await expect(help().$('button=Ask again')).toBeEnabled();
    expect((await backendStatus()).capture.state).toBe('notAllowed');
    expect(await invoke('capture_permission_help')).toEqual({ canOpenSettings: true, canAskAgain: true });
  });

  it('keeps Replay and Pause, since the buffer is kept', async () => {
    await expect(replayButton()).toBeEnabled();
    await expect(pauseButton()).toBeEnabled();
  });

  // The mock has no settings page: the button works and changes nothing.
  it('opens the settings without an error, and stays not allowed', async () => {
    await help().$('button=Open System Settings').click();
    await expect(help().$('button=Open System Settings')).toBeEnabled();
    await expect(help().$('[role="alert"]')).not.toBeExisting();
    await expect(headline()).toHaveText(NOT_ALLOWED);
  });

  it('captures once Ask again allowed it', async () => {
    await help().$('button=Ask again').click();
    // The engine tries again every 2 s.
    await browser.waitUntil(async () => (await headline().getText()) === `Capturing ${FAKE_APP}`, {
      timeout: 10_000,
      timeoutMsg: `the status never said "Capturing ${FAKE_APP}" after Ask again`,
    });
    await help().waitForExist({ reverse: true, timeoutMsg: 'the help stayed after capture started' });
  });

  it('buffers and replays what it captures from then on', async () => {
    await waitForBufferedSpeech(12_000);
    await replayButton().click();
    await expect(playbackLine()).toHaveText(expect.stringContaining('Replaying'));
    expect((await backendStatus()).playback).toBe('replaying');
    await stopButton().click();
    await playbackLine().waitForExist({ reverse: true, timeout: 5_000, timeoutMsg: 'the replay went on after Stop' });
  });
});
