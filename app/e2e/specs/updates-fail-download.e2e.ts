import { browser, expect } from '@wdio/globals';
import { waitForPage } from '../support/app.js';
import {
  MOCK_VERSION,
  backendUpdateStatus,
  checkNowButton,
  checkResult,
  consentQuestion,
  offerBanner,
  requireUpdateSource,
} from '../support/updates.js';

// A version on offer whose download breaks half way (REPEAT_THAT_MOCK_UPDATE=fail-download, see SPEC_ENV in
// wdio.conf.ts). The question about automatic checks is left alone: "Check now" is the user asking, and works anyway.
describe('updates, when the download fails', () => {
  before(async () => {
    await waitForPage();
    await requireUpdateSource('fail-download');
  });

  it('offers the update to a user who asks, although the question was never answered', async () => {
    await expect(consentQuestion()).toBeDisplayed();
    await checkNowButton().click();
    await offerBanner().waitForDisplayed({ timeoutMsg: 'the check never found the update' });
    await expect(offerBanner()).toHaveText(expect.stringContaining(`Version ${MOCK_VERSION} is available`));
    await expect(consentQuestion()).toBeDisplayed();
    expect(await backendUpdateStatus()).toMatchObject({ state: 'available', manual: true });
  });

  it("says that it couldn't install, with the reason and a way to the download page", async () => {
    await offerBanner().$('button=Install and restart').click();
    await browser.waitUntil(async () => (await backendUpdateStatus()).state === 'error', {
      timeoutMsg: 'the download never failed',
    });
    await expect(offerBanner()).toHaveText(expect.stringContaining(`Couldn't install version ${MOCK_VERSION}`));
    await expect(offerBanner()).toHaveText(expect.stringContaining("Couldn't install the update: the download failed."));
    await expect(offerBanner().$('button=Download page')).toBeEnabled();
    await expect(offerBanner().$('button=Later')).toBeEnabled();
    await expect(offerBanner().$('button=Install and restart')).not.toBeExisting();
    await expect(offerBanner().$('progress')).not.toBeExisting();
    expect(await backendUpdateStatus()).toMatchObject({
      state: 'error',
      errorKind: 'install',
      errorMessage: "Couldn't install the update: the download failed.",
      available: { version: MOCK_VERSION },
    });
  });

  it('says so under "Check now" as well, and offers the update again after a new check', async () => {
    await expect(checkResult()).toHaveText(expect.stringContaining(`Couldn't install version ${MOCK_VERSION}.`));
    await expect(checkResult()).toHaveText(expect.stringContaining('the download failed'));
    await expect(checkNowButton()).toBeEnabled();

    await checkNowButton().click();
    await expect(offerBanner()).toHaveText(expect.stringContaining(`Version ${MOCK_VERSION} is available`));
    await expect(offerBanner().$('button=Install and restart')).toBeEnabled();
    await expect(checkResult()).toHaveText(`Version ${MOCK_VERSION} is available.`);
  });
});
