// The IPC contract with the Rust side (app/src-tauri/src/ipc.rs). Names and shapes are fixed: both sides code
// against the same contract, so a change here needs the same change there.
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export type Sensitivity = 'low' | 'normal' | 'high';
/** An accelerator such as "Ctrl+Alt+Space"; `null` means unbound. */
export type Accel = string | null;

export interface Settings {
  version: 1;
  target: { exePath: string | null; name: string } | null;
  /** 10..120, default 30 */
  bufferSeconds: number;
  sensitivity: Sensitivity;
  /** `null` = system default */
  outputDevice: string | null;
  /** 0.75..1.5, default 1.0 */
  replaySpeed: number;
  /** 1.25..2.0, default 1.5 */
  catchUpSpeed: number;
  /** level 0.05..1.0, default 0.2 */
  duck: { enabled: boolean; level: number };
  hotkeys: { replay: Accel; stop: Accel; catchUp: Accel };
  autostart: boolean;
  firstRunDone: boolean;
}

export type HotkeyName = keyof Settings['hotkeys'];
export type HotkeyState = 'ok' | 'inUse' | 'invalid' | 'unbound';

export type CaptureState = 'noTarget' | 'waiting' | 'capturing' | 'paused' | 'error' | 'unsupported';
export type Playback = 'idle' | 'replaying' | 'catchingUp';

export interface Status {
  capture: { state: CaptureState; message: string | null };
  targetName: string | null;
  playback: Playback;
  behindMs: number | null;
  bufferedMs: number;
  output: { deviceName: string | null; fellBack: boolean; error: string | null };
  hotkeys: Record<HotkeyName, HotkeyState>;
  vadFallback: boolean;
}

export interface AppEntry {
  pid: number;
  name: string;
  exePath: string | null;
  /** A `data:` URL, or `null` for the generic icon. */
  icon: string | null;
}

export interface OutputDevice {
  id: string;
  name: string;
  isDefault: boolean;
}

export interface CommandError {
  kind: 'invalid' | 'conflict' | 'backend';
  message: string;
}

/**
 * What `update_settings` takes. Nested objects (`duck`, `hotkeys`) are always sent whole, so the call works whether
 * the backend merges shallowly or deeply.
 */
export type SettingsPatch = Partial<Omit<Settings, 'version'>>;

/** A rejected command, normalised to the contract's `CommandError`. */
export class ApiError extends Error implements CommandError {
  readonly kind: CommandError['kind'];

  constructor(kind: CommandError['kind'], message: string) {
    super(message);
    this.name = 'ApiError';
    this.kind = kind;
  }
}

const KINDS: ReadonlySet<string> = new Set(['invalid', 'conflict', 'backend']);

/** Anything a rejected `invoke` can carry: a `CommandError` object, a string, an `Error`. */
export function toApiError(raw: unknown): ApiError {
  if (raw instanceof ApiError) return raw;
  if (typeof raw === 'string' && raw !== '') return new ApiError('backend', raw);
  if (raw instanceof Error) return new ApiError('backend', raw.message || 'Something went wrong');
  if (typeof raw === 'object' && raw !== null) {
    const { kind, message } = raw as { kind?: unknown; message?: unknown };
    if (typeof message === 'string' && message !== '') {
      return new ApiError(typeof kind === 'string' && KINDS.has(kind) ? (kind as CommandError['kind']) : 'backend', message);
    }
  }
  return new ApiError('backend', 'Something went wrong');
}

async function call<T = void>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (raw) {
    throw toApiError(raw);
  }
}

export const getSettings = () => call<Settings>('get_settings');
export const updateSettings = (patch: SettingsPatch) => call<Settings>('update_settings', { patch });
export const getStatus = () => call<Status>('get_status');
export const listApps = () => call<AppEntry[]>('list_apps');
export const listOutputDevices = () => call<OutputDevice[]>('list_output_devices');
/** Sets the target and wipes the buffer. The icon stays home: it can be tens of kB and the backend has it. */
export const selectApp = (app: AppEntry) => call('select_app', { app: { ...app, icon: null } });
export const setPaused = (paused: boolean) => call('set_paused', { paused });
export const replay = () => call('replay');
export const stop = () => call('stop');
export const catchUp = () => call('catch_up');
export const suspendHotkeys = (suspended: boolean) => call('suspend_hotkeys', { suspended });

export type Unlisten = () => void;

/** Stops listening. Never throws: when the window is already going away there is nothing left to undo. */
function quietly(unlisten: () => unknown): Unlisten {
  return () => {
    void (async () => {
      try {
        await unlisten();
      } catch {
        // see above
      }
    })();
  };
}

export const onStatus = async (handler: (status: Status) => void): Promise<Unlisten> =>
  quietly(await listen<Status>('status', (event) => handler(event.payload)));

export const onSettingsChanged = async (handler: (settings: Settings) => void): Promise<Unlisten> =>
  quietly(await listen<Settings>('settings-changed', (event) => handler(event.payload)));
