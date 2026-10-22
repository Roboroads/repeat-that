import type { Settings, Status } from './api';

export type Tone = 'ok' | 'idle' | 'warn';

export interface StatusText {
  headline: string;
  /** Second line: buffered speech or the error message. */
  detail: string | null;
  /** Replay line, only while playing. */
  playback: string | null;
  tone: Tone;
}

export function formatSeconds(ms: number): string {
  return `${Math.round(ms / 1000)} s`;
}

function buffered(ms: number): string | null {
  return ms >= 500 ? `${formatSeconds(ms)} of speech buffered` : null;
}

function playbackLine(status: Status): string | null {
  if (status.playback === 'idle') return null;
  const base = status.playback === 'replaying' ? 'Replaying' : 'Catching up';
  const behind = status.behindMs !== null && Math.round(status.behindMs / 1000) >= 1 ? status.behindMs : null;
  return behind === null ? base : `${base} · ${formatSeconds(behind)} behind live`;
}

/** What the status bar says for a `Status`; `target` is the stored one, for a name before the first status. */
export function describeStatus(status: Status | null, target: Settings['target']): StatusText {
  if (status === null) return { headline: 'Starting…', detail: null, playback: null, tone: 'idle' };

  const name = status.targetName ?? target?.name ?? 'the app';
  const { state, message } = status.capture;
  const playback = playbackLine(status);

  switch (state) {
    case 'noTarget':
      return { headline: 'No app selected', detail: null, playback, tone: 'idle' };
    case 'waiting':
      return { headline: `Waiting for ${name}`, detail: buffered(status.bufferedMs), playback, tone: 'idle' };
    case 'capturing':
      return { headline: `Capturing ${name}`, detail: buffered(status.bufferedMs), playback, tone: 'ok' };
    case 'paused':
      return { headline: 'Paused', detail: null, playback, tone: 'idle' };
    case 'error':
      return { headline: `Can't capture ${name}`, detail: message, playback, tone: 'warn' };
    case 'notAllowed':
      // The buffer is kept, so a replay may still play; the message says how to allow capture.
      return { headline: "Repeat That isn't allowed to capture audio", detail: message, playback, tone: 'warn' };
    case 'unsupported':
      return {
        headline: message ?? "Capturing apps isn't supported on this OS yet",
        detail: null,
        playback,
        tone: 'warn',
      };
  }
}

/** Problems with the output, the speech detector or the settings file, one sentence each. */
export function outputWarnings(status: Status): string[] {
  const warnings: string[] = [];
  if (status.output.error !== null) {
    warnings.push(`Replays have nowhere to play: ${status.output.error}`);
  } else if (status.output.fellBack) {
    const device = status.output.deviceName === null ? '' : ` (${status.output.deviceName})`;
    warnings.push(`The chosen output device isn't available, so replays use the system default${device}.`);
  }
  if (status.vadFallback) {
    warnings.push('The speech detection model did not load. A simpler detector is used, so some speech may be missed.');
  }
  // Already a sentence from the backend.
  if (status.settingsProblem !== null) warnings.push(status.settingsProblem);
  return warnings;
}
