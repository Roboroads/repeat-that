import { browser, expect } from '@wdio/globals';
import { closeSync, openSync, readFileSync, readSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { pickFakeApp, playbackLine, replayButton, waitForBufferedSpeech, waitForPage } from '../support/app.js';

/**
 * The regular files under `dir`, relative to it. Sockets, pipes and symlinks are left out: they hold no data of their
 * own, and reading a socket or a FIFO would fail or block.
 */
function filesUnder(dir: string, base = dir): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) return filesUnder(full, base);
    return entry.isFile() ? [path.relative(base, full)] : [];
  });
}

/** The first four bytes of a file, as text. */
function head(file: string): string {
  const fd = openSync(file, 'r');
  try {
    const bytes = Buffer.alloc(4);
    return bytes.subarray(0, readSync(fd, bytes, 0, 4, 0)).toString('latin1');
  } finally {
    closeSync(fd);
  }
}

// The first bytes of the audio formats a recording could end up in.
const AUDIO_MAGIC = ['RIFF', 'OggS', 'fLaC', 'ID3', 'FORM'];
const AUDIO_NAME = /\.(wav|mp3|ogg|opus|flac|pcm|raw|aiff?|m4a)$/i;

// The app runs with its home, temp, runtime, config, data and cache folders all inside one scratch folder (see
// wdio.conf.ts), so a recording written to any of the places a program writes to by default ends up in it.
describe('privacy', () => {
  const root = () => process.env.E2E_APP_HOME as string;

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
    const file = path.join(root(), 'config', 'me.roboroads.repeat-that', 'settings.json');
    const settings = JSON.parse(readFileSync(file, 'utf8'));
    expect(settings.settings.target.name).toBe('speech_8s');
    expect(filesUnder(path.join(root(), 'config'))).toEqual([path.join('me.roboroads.repeat-that', 'settings.json')]);
  });

  it('writes no audio file to its home, temp, config, data or cache folder', async () => {
    const files = filesUnder(root());
    expect(files.length).toBeGreaterThan(0); // the scan did find the app's files
    const looksLikeAudio = (file: string) =>
      AUDIO_NAME.test(file) || AUDIO_MAGIC.some((magic) => head(path.join(root(), file)).startsWith(magic));
    expect(files.filter(looksLikeAudio)).toEqual([]);
  });
});
