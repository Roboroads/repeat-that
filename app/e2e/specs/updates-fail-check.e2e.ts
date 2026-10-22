import { browser, expect } from '@wdio/globals';
import { waitForPage } from '../support/app.js';
import {
  backendUpdateStatus,
  checkNowButton,
  checkResult,
  offerBanner,
  requireUpdateSource,
} from '../support/updates.js';

// A source that can't be reached (REPEAT_THAT_MOCK_UPDATE=fail-check, see SPEC_ENV in wdio.conf.ts).
describe('updates, when the check fails', () => {
  before(async () => {
    await waitForPage();
    await requireUpdateSource('fail-check');
  });

  it('says that it could not check, and why, next to "Check now", and shows no banner', async () => {
    await checkNowButton().click();
    // Absence can only be checked once the check is over: before that there is nothing yet that could show a banner.
    await browser.waitUntil(async () => (await backendUpdateStatus()).state === 'error', {
      timeoutMsg: 'the check never failed',
    });
    await expect(checkResult()).toHaveText(expect.stringContaining("Couldn't check for updates."));
    await expect(checkResult()).toHaveText(
      expect.stringContaining("Couldn't check for updates: the update server can't be reached."),
    );
    await expect(offerBanner()).not.toBeExisting();
    expect(await backendUpdateStatus()).toMatchObject({
      state: 'error',
      errorKind: 'check',
      error: "Couldn't check for updates: the update server can't be reached.",
      manual: true,
      available: null,
    });
    // The user can try again.
    await expect(checkNowButton()).toBeEnabled();
  });
});
