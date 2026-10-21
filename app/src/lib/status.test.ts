import { describe, expect, it } from 'vitest';
import type { Status } from './api';
import { describeStatus, formatSeconds, outputWarnings } from './status';

function status(over: Partial<Status> = {}): Status {
  return {
    capture: { state: 'capturing', message: null },
    targetName: 'Discord',
    playback: 'idle',
    behindMs: null,
    bufferedMs: 24_400,
    output: { deviceName: 'Speakers', fellBack: false, error: null },
    hotkeys: { replay: 'ok', stop: 'ok', catchUp: 'ok' },
    vadFallback: false,
    settingsProblem: null,
    ...over,
  };
}

describe('formatSeconds', () => {
  it('rounds to whole seconds', () => {
    expect(formatSeconds(24_400)).toBe('24 s');
    expect(formatSeconds(24_600)).toBe('25 s');
    expect(formatSeconds(0)).toBe('0 s');
  });
});

describe('describeStatus', () => {
  it('says what is captured and how much speech is buffered', () => {
    expect(describeStatus(status(), null)).toMatchObject({
      headline: 'Capturing Discord',
      detail: '24 s of speech buffered',
      tone: 'ok',
    });
  });

  it('says nothing is selected', () => {
    const s = status({ capture: { state: 'noTarget', message: null }, targetName: null, bufferedMs: 0 });
    expect(describeStatus(s, null)).toMatchObject({ headline: 'No app selected', detail: null, tone: 'idle' });
  });

  it('waits for the target and mentions the speech kept meanwhile', () => {
    const s = status({ capture: { state: 'waiting', message: null } });
    expect(describeStatus(s, null)).toMatchObject({
      headline: 'Waiting for Discord',
      detail: '24 s of speech buffered',
      tone: 'idle',
    });
  });

  it('falls back to the stored target name when the status has none', () => {
    const s = status({ capture: { state: 'waiting', message: null }, targetName: null, bufferedMs: 0 });
    expect(describeStatus(s, { exePath: null, name: 'Teams' }).headline).toBe('Waiting for Teams');
  });

  it('says paused', () => {
    const s = status({ capture: { state: 'paused', message: null }, bufferedMs: 0 });
    expect(describeStatus(s, null)).toMatchObject({ headline: 'Paused', detail: null, tone: 'idle' });
  });

  it('shows the capture error message', () => {
    const s = status({ capture: { state: 'error', message: 'Access denied' }, bufferedMs: 0 });
    expect(describeStatus(s, null)).toMatchObject({ headline: "Can't capture Discord", detail: 'Access denied', tone: 'warn' });
  });

  it('shows why capturing is unsupported', () => {
    const s = status({
      capture: { state: 'unsupported', message: "Capturing apps isn't supported on this OS yet" },
      bufferedMs: 0,
    });
    expect(describeStatus(s, null)).toMatchObject({
      headline: "Capturing apps isn't supported on this OS yet",
      tone: 'warn',
    });
  });

  it('shows the replay with how far behind live it is', () => {
    const s = status({ playback: 'replaying', behindMs: 3_200 });
    expect(describeStatus(s, null).playback).toBe('Replaying · 3 s behind live');
  });

  it('shows catching up, and leaves the lag out when it is gone', () => {
    expect(describeStatus(status({ playback: 'catchingUp', behindMs: 1_400 }), null).playback).toBe(
      'Catching up · 1 s behind live',
    );
    expect(describeStatus(status({ playback: 'catchingUp', behindMs: 200 }), null).playback).toBe('Catching up');
    expect(describeStatus(status({ playback: 'replaying', behindMs: null }), null).playback).toBe('Replaying');
  });

  it('has no playback line while idle', () => {
    expect(describeStatus(status(), null).playback).toBeNull();
  });

  it('describes the first load, before any status arrived', () => {
    expect(describeStatus(null, null)).toMatchObject({ headline: 'Starting…', tone: 'idle' });
  });
});

describe('outputWarnings', () => {
  it('is empty for a healthy status', () => {
    expect(outputWarnings(status())).toEqual([]);
  });

  it('warns that a chosen device was replaced by the default', () => {
    const s = status({ output: { deviceName: 'Speakers', fellBack: true, error: null } });
    expect(outputWarnings(s)).toEqual([
      "The chosen output device isn't available, so replays use the system default (Speakers).",
    ]);
  });

  it('warns that there is no output at all', () => {
    const s = status({ output: { deviceName: null, fellBack: false, error: 'No audio output device found' } });
    expect(outputWarnings(s)).toEqual(['Replays have nowhere to play: No audio output device found']);
  });

  it('warns about the energy fallback of the speech detector', () => {
    expect(outputWarnings(status({ vadFallback: true }))).toEqual([
      'The speech detection model did not load. A simpler detector is used, so some speech may be missed.',
    ]);
  });

  it('passes on why the settings cannot be saved', () => {
    const problem = "The settings file couldn't be set up, so changes are lost when the app closes: read-only";
    expect(outputWarnings(status({ settingsProblem: problem }))).toEqual([problem]);
  });
});
