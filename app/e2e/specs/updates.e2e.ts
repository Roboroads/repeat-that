import { $, browser, expect } from '@wdio/globals';
import { backendSettings, waitForPage } from '../support/app.js';
import {
  type OfferSnapshot,
  MOCK_VERSION,
  backendUpdateStatus,
  checkNowButton,
  checkResult,
  consentQuestion,
  offerBanner,
  offerSnapshot,
  requireUpdateSource,
  updateSwitch,
  versionLine,
} from '../support/updates.js';

// A debug build with a pretend update source that offers 99.0.0 (REPEAT_THAT_MOCK_UPDATE=available, see SPEC_ENV in
// wdio.conf.ts), driven from the first launch to the install. The tests follow each other, like the steps a user takes.
// The mock's download takes about 2 s, and its install ends in "installing" without restarting the app.
describe('updates, with a version on offer', () => {
  before(async () => {
    await waitForPage();
    await requireUpdateSource('available');
  });

  it('asks whether to check for updates, under the first-run banner, and has not contacted anyone yet', async () => {
    await expect(consentQuestion()).toBeDisplayed();
    await expect(consentQuestion()).toHaveText(expect.stringContaining('GitHub sees your IP address'));
    await expect(consentQuestion().$('button=Check automatically')).toBeEnabled();
    await expect(consentQuestion().$('button=Not now')).toBeEnabled();
    const firstRun = $('aside*=Pick the app you want to catch up on');
    await expect(firstRun).toBeDisplayed();
    expect(await firstRun.getLocation('y')).toBeLessThan(await consentQuestion().getLocation('y'));

    expect(await backendUpdateStatus()).toMatchObject({ mode: 'install', state: 'idle', lastCheckedAt: null });
    expect((await backendSettings()).checkForUpdates).toBeNull();
    await expect(offerBanner()).not.toBeExisting();
    await expect(updateSwitch()).not.toBeChecked();
    await expect(versionLine()).toHaveText(expect.stringContaining('last checked never'));
  });

  it('stops asking once it is answered, and remembers the answer across a reload', async () => {
    await consentQuestion().$('button=Check automatically').click();
    await consentQuestion().waitForExist({ reverse: true, timeoutMsg: 'the question stayed after the answer' });
    await expect(updateSwitch()).toBeChecked();
    await browser.waitUntil(async () => (await backendSettings()).checkForUpdates === true, {
      timeoutMsg: 'the backend never got the answer',
    });

    await browser.refresh();
    await waitForPage();
    // The switch only exists in a build that checks, so this also tells that the page has loaded the update status.
    await expect(updateSwitch()).toBeChecked();
    await expect(consentQuestion()).not.toBeExisting();
  });

  // The app checks 5 s after the switch was turned on. Nobody clicks "Check now" here.
  it('finds the update by itself and offers it', async () => {
    await offerBanner().waitForDisplayed({ timeout: 20_000, timeoutMsg: 'no automatic check found the update' });
    await expect(offerBanner()).toHaveText(expect.stringContaining(`Version ${MOCK_VERSION} is available`));
    await expect(offerBanner().$('button=Install and restart')).toBeEnabled();
    await expect(offerBanner().$('button=Later')).toBeEnabled();
    const status = await backendUpdateStatus();
    expect(status).toMatchObject({ state: 'available', manual: false, available: { version: MOCK_VERSION } });
    // "You have 0.1.0." says which version this is.
    await expect(offerBanner()).toHaveText(expect.stringContaining(`You have ${status.currentVersion}.`));
  });

  it('checks again on "Check now", and keeps the offer', async () => {
    const before = (await backendUpdateStatus()).lastCheckedAt;
    expect(before).not.toBeNull();
    await checkNowButton().click();
    await browser.waitUntil(
      async () => {
        const status = await backendUpdateStatus();
        return status.manual && status.state === 'available' && (status.lastCheckedAt ?? 0) > (before ?? 0);
      },
      { timeoutMsg: 'the check the button started never finished' },
    );
    await expect(checkResult()).toHaveText(`Version ${MOCK_VERSION} is available.`);
    await expect(checkNowButton()).toBeEnabled();
    await expect(offerBanner()).toBeDisplayed();
    await expect(versionLine()).not.toHaveText(expect.stringContaining('never'));
  });

  it('shows the release notes as plain text under "What\'s new", and links to the release', async () => {
    const details = offerBanner().$('details');
    expect(await details.getProperty('open')).toBe(false);
    await expect(offerBanner().$('pre')).not.toBeDisplayed();
    await details.$('summary').click();
    expect(await details.getProperty('open')).toBe(true);
    await expect(offerBanner().$('pre')).toBeDisplayed();
    await expect(offerBanner().$('pre')).toHaveText(expect.stringContaining('A sample of release notes.'));

    await expect(offerBanner().$('a=Release notes on GitHub')).toHaveAttribute(
      'href',
      `https://github.com/Roboroads/repeat-that/releases/tag/v${MOCK_VERSION}`,
    );
  });

  it('downloads with a progress bar, then installs and stays running', async () => {
    await offerBanner().$('button=Install and restart').click();

    // The download takes about 2 s, which is too short to look at one thing at a time: this reads the banner in one
    // round trip, over and over, until it says "Installing".
    const seen: OfferSnapshot[] = [];
    await browser.waitUntil(
      async () => {
        const snapshot = await offerSnapshot();
        seen.push(snapshot);
        return snapshot.text.includes(`Installing version ${MOCK_VERSION}`);
      },
      { interval: 50, timeoutMsg: 'the banner never said "Installing"' },
    );
    expect(seen.some((s) => s.text.includes(`Downloading version ${MOCK_VERSION}`) && s.progress !== null)).toBe(true);
    // Part of the way: neither at the start nor done.
    expect(seen.some((s) => s.progress !== null && s.progress > 0 && s.progress < 1)).toBe(true);

    await expect(offerBanner()).toHaveText(expect.stringContaining("Repeat That restarts when it's done."));
    await expect(offerBanner().$('button=Install and restart')).not.toBeExisting();
    await expect(offerBanner().$('progress')).not.toBeExisting();
    await expect(checkNowButton()).toBeDisabled();

    // The mock has nothing to restart into: the app, and the session driving it, are still there.
    await browser.pause(1_500);
    expect(await backendUpdateStatus()).toMatchObject({ state: 'installing', errorMessage: null });
    await expect(offerBanner()).toHaveText(expect.stringContaining(`Installing version ${MOCK_VERSION}`));
  });
});
