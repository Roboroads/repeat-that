import { $, browser, expect } from '@wdio/globals';
import { FAKE_APP, appRadio, chosenApp, headline, pickFakeApp, waitForPage } from '../support/app.js';

// A fresh app: no settings file yet, so this is the first launch.
describe('first run', () => {
  before(waitForPage);

  it('opens the settings window with the right title', async () => {
    await expect(browser).toHaveTitle('Repeat That');
    await expect($('h1')).toHaveText('Repeat That');
  });

  it('asks for an app and has none selected', async () => {
    await expect($('aside.banner')).toBeDisplayed();
    await expect($('aside.banner')).toHaveText(expect.stringContaining('Pick the app you want to catch up on'));
    await expect(headline()).toHaveText('No app selected');
  });

  // The private D-Bus of the test has no StatusNotifierWatcher, so this is a desktop without a tray: the window is the
  // only way in, and the General section says so and has the Quit the tray menu would have.
  it('explains that there is no tray, and offers to quit', async () => {
    await expect($('p*=No tray on this desktop')).toBeExisting();
    await expect($('p*=Open Repeat That from your app menu')).toBeExisting();
    await expect($('button=Quit Repeat That')).toBeExisting();
  });

  it('keeps the plain recorder on X11, where the hotkeys work through the plugin', async () => {
    await expect($('button[aria-label^="Replay hotkey"]')).toBeDisplayed();
    await expect($('p*=Global hotkeys aren')).not.toBeExisting();
    await expect($('button=Change in system settings')).not.toBeExisting();
  });

  it('lists the fake app that the debug build plays', async () => {
    await expect(appRadio(FAKE_APP)).toBeDisplayed();
    await expect(appRadio(FAKE_APP)).not.toBeSelected();
  });

  it('drops the banner and folds the list into the chosen app once one is picked', async () => {
    await pickFakeApp();
    await expect($('aside.banner')).not.toBeExisting();
    await expect(chosenApp(FAKE_APP)).toBeDisplayed();
    // The other rows are gone: only the chosen app is left, with a way back to the list.
    await expect(appRadio(FAKE_APP)).not.toBeExisting();
    await expect(chosenApp(FAKE_APP)).toHaveText(expect.stringContaining('Change'));
  });

  it('opens the list again on a click and closes it on the chosen app', async () => {
    await chosenApp(FAKE_APP).click();
    await expect(appRadio(FAKE_APP)).toBeDisplayed();
    await expect(appRadio(FAKE_APP)).toBeSelected();
    await appRadio(FAKE_APP).click();
    await expect(chosenApp(FAKE_APP)).toBeDisplayed();
    // Picking the app that is already chosen changes nothing.
    await expect(headline()).toHaveText(`Capturing ${FAKE_APP}`);
  });
});
