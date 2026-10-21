import { browser, expect } from '@wdio/globals';
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { pickFakeApp, playbackLine, replayButton, waitForBufferedSpeech, waitForPage } from '../support/app.js';

/** Every file under `dir`, relative to it. */
function filesUnder(dir: string, base = dir): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    return entry.isDirectory() ? filesUnder(full, base) : [path.relative(base, full)];
  });
}

// The first bytes of the audio formats a recording could end up in.
const AUDIO_MAGIC = ['RIFF', 'OggS', 'fLaC', 'ID3', 'FORM'];

// The app runs with its own config, data and cache folders (see wdio.conf.ts), so everything it wrote is in one place.
describe('privacy', () => {
  const home = () => process.env.E2E_APP_HOME as string;

  before(async () => {
    await waitForPage();
    await pickFakeApp();
    await waitForBufferedSpeech(12_000);
    // Capture and a replay have both run: this is when a recording would exist if the app made one.
    await replayButton().click();
    await playbackLine().waitForExist();
    await browser.pause(1_000);
  });

  it('keeps the chosen app, and nothing else of the call, in the settings file', async () => {
    const file = path.join(home(), 'config', 'me.roboroads.repeat-that', 'settings.json');
    const settings = JSON.parse(readFileSync(file, 'utf8'));
    expect(settings.settings.target.name).toBe('speech_8s');
    expect(filesUnder(path.join(home(), 'config'))).toEqual([path.join('me.roboroads.repeat-that', 'settings.json')]);
  });

  it('writes no audio file anywhere', async () => {
    const audio = filesUnder(home()).filter((file) => {
      // The D-Bus socket is not a regular file and can't be read.
      if (file === 'bus') return false;
      const head = readFileSync(path.join(home(), file)).subarray(0, 4).toString('latin1');
      return AUDIO_MAGIC.some((magic) => head.startsWith(magic)) || /\.(wav|mp3|ogg|opus|flac|pcm|raw)$/i.test(file);
    });
    expect(audio).toEqual([]);
  });
});
