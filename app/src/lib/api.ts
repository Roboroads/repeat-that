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
  /** level 0.05..1.0 (0..1.0 on macOS, where 0 mutes the app), default 0.2 */
  duck: { enabled: boolean; level: number };
  hotkeys: { replay: Accel; stop: Accel; catchUp: Accel };
  autostart: boolean;
  firstRunDone: boolean;
  /** Check for updates by itself. `null`: not asked yet, which counts as off. */
  checkForUpdates: boolean | null;
}

export type HotkeyName = keyof Settings['hotkeys'];
export type HotkeyState = 'ok' | 'inUse' | 'invalid' | 'unbound';
/**
 * How the global hotkeys reach the app on this desktop. `plugin`: the app registers the combinations itself (Windows,
 * macOS, X11). `portal`: the desktop owns the keys and says which it assigned (Wayland, through the XDG GlobalShortcuts
 * portal). `none`: no hotkeys here; the command-line actions are what the user binds.
 */
export type HotkeyBackend = 'plugin' | 'portal' | 'none';

/**
 * `error` (a failed start) and `notAllowed` (the OS doesn't let the app capture audio, macOS) are tried again every
 * 2 s; `unsupported` is final. `notAllowed` keeps the buffer, and its message says how to allow it.
 */
export type CaptureState = 'noTarget' | 'waiting' | 'capturing' | 'paused' | 'error' | 'notAllowed' | 'unsupported';
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
  /** Why the settings can't be saved, as a sentence for the user; `null` when they can. */
  settingsProblem: string | null;
  hotkeyBackend: HotkeyBackend;
  /** The key the desktop assigned to each action, as it describes it; only filled under `portal`. */
  hotkeyTriggers: Record<HotkeyName, string | null>;
  /** The shell command that runs each action in the running app; only filled under `none`. */
  hotkeyCommands: Record<HotkeyName, string> | null;
  /** Whether a tray host is there to show the tray icon. Always `true` outside Linux. */
  trayAvailable: boolean;
}

/**
 * What `configure_shortcuts` did: the desktop shows its shortcut settings (or its dialog to pick the keys), it can't be
 * asked to, or it is waiting for the answer to a dialog it has open and was not asked again.
 */
export type ConfigureOutcome = 'opened' | 'unsupported' | 'busy';

/**
 * What `capture_permission_help` answers: which of the other two permission commands work here. Both `false` outside
 * macOS.
 */
export interface PermissionHelp {
  /** `open_capture_permission_settings` opens the settings page with the switch (macOS 15 and later). */
  canOpenSettings: boolean;
  /** `ask_for_capture_permission` makes the OS ask once more. */
  canAskAgain: boolean;
}

/**
 * What this build does about updates. `install`: downloads and installs them itself. `notify`: only says that one is
 * out, and links to the download page. `external`: something else updates it (Flathub). `off`: this build has no
 * updater.
 */
export type UpdateMode = 'install' | 'notify' | 'external' | 'off';
export type UpdateState = 'idle' | 'checking' | 'upToDate' | 'available' | 'downloading' | 'installing' | 'error';

/** Which step failed in state `error`: looking for a version, or downloading and installing the one found. */
export type UpdateErrorKind = 'check' | 'install';

export interface UpdateStatus {
  mode: UpdateMode;
  /** A sentence for `notify`, `external` and `off`; `null` for `install`. */
  modeReason: string | null;
  currentVersion: string;
  state: UpdateState;
  /** The newer version. Plain text notes, and a release page on this project's GitHub. */
  available: { version: string; date: string | null; notes: string; releaseUrl: string } | null;
  /** 0..1 while downloading; `null` when the size is unknown. */
  progress: number | null;
  /** In state `error`. */
  error: string | null;
  /**
   * In state `error`: which step failed, `null` otherwise. After `install`, `available` is still filled in: the
   * release page link in it is what the window offers. After `check`, `available` may or may not be filled in, and the
   * window does not use it.
   */
  errorKind: UpdateErrorKind | null;
  /** Milliseconds since the epoch; this run only. */
  lastCheckedAt: number | null;
  /** The last check was asked for by the user. */
  manual: boolean;
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
/** Asks the desktop to show where the keys are changed (Wayland, where it owns them). */
export const configureShortcuts = () => call<ConfigureOutcome>('configure_shortcuts');
/** Quits the app, for a desktop with no tray menu to do it from. */
export const quitApp = () => call('quit_app');
export const capturePermissionHelp = () => call<PermissionHelp>('capture_permission_help');
/** Makes the OS ask for the permission again; capture starts by itself once the user allows it. */
export const askForCapturePermission = () => call('ask_for_capture_permission');
/** Opens the OS settings where the permission is given. */
export const openCapturePermissionSettings = () => call('open_capture_permission_settings');

/** Checks for a newer version now, whatever the setting says, and answers with the result. */
export const checkForUpdates = () => call<UpdateStatus>('check_for_updates');
/**
 * Downloads and installs the version found, then restarts the app. Refused unless one was found and the mode is
 * `install`. The progress comes as `update-status` events.
 */
export const installUpdate = () => call('install_update');
/** Fails on a backend that has no updater commands; the caller treats that as mode `off`. */
export const getUpdateStatus = () => call<UpdateStatus>('get_update_status');

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

export const onUpdateStatus = async (handler: (status: UpdateStatus) => void): Promise<Unlisten> =>
  quietly(await listen<UpdateStatus>('update-status', (event) => handler(event.payload)));
