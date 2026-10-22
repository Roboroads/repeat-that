import { browser, expect } from '@wdio/globals';
import { backendSettings, waitForPage } from '../support/app.js';
import {
  backendUpdateStatus,
  checkNowButton,
  checkResult,
  consentQuestion,
  offerBanner,
  requireUpdateSource,
  updateSwitch,
  versionLine,
} from '../support/updates.js';

// A source with nothing newer than this version (REPEAT_THAT_MOCK_UPDATE=none, see SPEC_ENV in wdio.conf.ts), and a
// user who answers "Not now" to the question.
describe('updates, with nothing newer', () => {
  before(async () => {
    await waitForPage();
    await requireUpdateSource('none');
  });

  it('keeps the switch off and remembers it across a reload after "Not now"', async () => {
    await expect(consentQuestion()).toBeDisplayed();
    await consentQuestion().$('button=Not now').click();
    await consentQuestion().waitForExist({ reverse: true, timeoutMsg: 'the question stayed after the answer' });
    await browser.waitUntil(async () => (await backendSettings()).checkForUpdates === false, {
      timeoutMsg: 'the backend never got the answer',
    });
    await expect(updateSwitch()).not.toBeChecked();

    await browser.refresh();
    await waitForPage();
    await expect(updateSwitch()).toBeDisplayed(); // only a build that checks has it, so the update status is loaded
    await expect(updateSwitch()).not.toBeChecked();
    await expect(consentQuestion()).not.toBeExisting();
    // No answer, no check: it was said not to contact anyone.
    expect(await backendUpdateStatus()).toMatchObject({ state: 'idle', lastCheckedAt: null });
  });

  it('says "You have the latest version." when asked, and shows no banner', async () => {
    await expect(checkResult()).toHaveText('');
    await checkNowButton().click();
    // Absence can only be checked once the check is over: before that there is nothing yet that could show a banner.
    await browser.waitUntil(async () => (await backendUpdateStatus()).state === 'upToDate', {
      timeoutMsg: 'the check never finished',
    });
    await expect(checkResult()).toHaveText('You have the latest version.');
    await expect(offerBanner()).not.toBeExisting();
    await expect(consentQuestion()).not.toBeExisting();
    await expect(versionLine()).not.toHaveText(expect.stringContaining('never'));
    expect(await backendUpdateStatus()).toMatchObject({ state: 'upToDate', manual: true, available: null });
    await expect(checkNowButton()).toBeEnabled();
  });
});
