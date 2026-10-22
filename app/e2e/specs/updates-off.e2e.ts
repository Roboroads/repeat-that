import { $, expect } from '@wdio/globals';
import { waitForPage } from '../support/app.js';
import {
  backendUpdateStatus,
  checkNowButton,
  consentQuestion,
  offerBanner,
  requireUpdateSource,
  updateSwitch,
} from '../support/updates.js';

// A debug build without REPEAT_THAT_MOCK_UPDATE: no update source and no key, so the app never looks for updates. It
// is the app of every other spec too, which is why the question and the banner must stay out of their way.
describe('updates, in a build that has none', () => {
  before(async () => {
    await waitForPage();
    await requireUpdateSource(null);
  });

  it('does not ask, and has no banner, switch or button', async () => {
    await expect(consentQuestion()).not.toBeExisting();
    await expect(offerBanner()).not.toBeExisting();
    await expect(updateSwitch()).not.toBeExisting();
    await expect(checkNowButton()).not.toBeExisting();
    expect(await backendUpdateStatus()).toMatchObject({ mode: 'off', state: 'idle', lastCheckedAt: null });
  });

  it("says in General that the build doesn't check for updates", async () => {
    await expect($('h3=Updates')).toBeDisplayed();
    await expect($("p=This build doesn't check for updates.")).toBeDisplayed();
  });
});
