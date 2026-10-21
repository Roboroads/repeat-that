import { $, browser, expect } from '@wdio/globals';
import { waitForPage } from '../support/app.js';

describe('settings', () => {
  before(waitForPage);

  it('keeps the buffer length across a reload', async () => {
    const slider = $('#buffer');
    await expect(slider).toHaveValue('30'); // the default
    // A click lands the thumb somewhere along the track: wherever it is, it is not the default.
    await slider.click();
    const chosen = await slider.getValue();
    expect(chosen).not.toBe('30');
    await expect($('output[for="buffer"]')).toHaveText(`${chosen} s`);

    await browser.refresh();
    await waitForPage();
    await expect($('#buffer')).toHaveValue(chosen);
    await expect($('output[for="buffer"]')).toHaveText(`${chosen} s`);
  });

  it('refuses a hotkey that another action already uses, and keeps the old one', async () => {
    const stop = $('button[aria-label^="Stop hotkey"]');
    await expect(stop).toHaveAttribute('aria-label', 'Stop hotkey: Ctrl+Alt+Backspace');
    await stop.click();
    await expect(stop).toHaveAttribute('aria-label', 'Stop hotkey: press a combination');

    // Replay's default combination.
    await browser.keys(['Control', 'Alt', 'Space']);

    const problem = $('[role="alert"]');
    await expect(problem).toBeDisplayed();
    await expect(problem).toHaveText(expect.stringContaining("can't use the same hotkey"));
    await expect(stop).toHaveAttribute('aria-label', 'Stop hotkey: Ctrl+Alt+Backspace');
    await expect($('button[aria-label^="Replay hotkey"]')).toHaveAttribute(
      'aria-label',
      'Replay hotkey: Ctrl+Alt+Space',
    );
  });
});
