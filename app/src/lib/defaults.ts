import type { Settings } from './api';
import { currentPlatform, isMac } from './platform';

/** Ranges the backend clamps to (contract §3.6); the sliders and the dev mock use the same ones. */
export const LIMITS = {
  bufferSeconds: { min: 10, max: 120, step: 5 },
  replaySpeed: { min: 0.75, max: 1.5, step: 0.05 },
  catchUpSpeed: { min: 1.25, max: 2.0, step: 0.05 },
} as const;

/**
 * The duck level's range on `platform`. macOS can mute the app during a replay (0), because it captures the app before
 * it is muted. Windows and Linux stop at 5%, because their capture can't hear a muted app.
 */
export function duckLevelLimits(platform: string = currentPlatform()) {
  return { min: isMac(platform) ? 0 : 0.05, max: 1.0, step: 0.05 } as const;
}

export const DEFAULT_HOTKEYS: Settings['hotkeys'] = {
  replay: 'Ctrl+Alt+Space',
  stop: 'Ctrl+Alt+Backspace',
  catchUp: 'Ctrl+Alt+Enter',
};

/** The backend's defaults; used by the dev mock and the tests. */
export function defaultSettings(): Settings {
  return {
    version: 1,
    target: null,
    bufferSeconds: 30,
    sensitivity: 'normal',
    outputDevice: null,
    replaySpeed: 1.0,
    catchUpSpeed: 1.5,
    duck: { enabled: true, level: 0.2 },
    hotkeys: { ...DEFAULT_HOTKEYS },
    autostart: false,
    firstRunDone: false,
  };
}

export function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}
