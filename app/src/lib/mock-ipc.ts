// A stand-in for the Rust backend so the settings page can be clicked through in a plain browser (`npm run dev`) and
// tested without Tauri. It follows the IPC contract in api.ts and the validation rules of §3.6 loosely: it clamps,
// rejects a hotkey used twice, and simulates capture, buffer and playback over time. Dev and tests only: main.ts
// imports it behind `import.meta.env.DEV`, so it is not in the production bundle.
import { emit } from '@tauri-apps/api/event';
import { mockIPC } from '@tauri-apps/api/mocks';
import type {
  AppEntry,
  CommandError,
  ConfigureOutcome,
  HotkeyName,
  HotkeyState,
  OutputDevice,
  PermissionHelp,
  Settings,
  Status,
} from './api';
import { matchesTarget } from './apps';
import { clamp, defaultSettings, duckLevelLimits, LIMITS } from './defaults';

export const SCENARIOS = [
  'first-run',
  'configured',
  'waiting',
  'problems',
  'empty',
  'portal',
  'no-hotkeys',
  'no-tray',
  'not-allowed',
] as const;
export type Scenario = (typeof SCENARIOS)[number];

export interface MockBackendInit {
  settings?: Partial<Settings>;
  apps?: AppEntry[];
  devices?: OutputDevice[];
  /** Fields forced onto every status, for states the simulation does not reach (errors, warnings). */
  statusOverrides?: Partial<Status>;
  /** What `configure_shortcuts` answers: the desktop shows its shortcut settings, or can't be asked to. */
  configureOutcome?: ConfigureOutcome;
  /**
   * Whether the OS lets the app capture audio, as macOS asks. `denied` makes a running target `notAllowed` until
   * `ask_for_capture_permission`, which allows it at once. Allowed by default.
   */
  permission?: 'allowed' | 'denied';
  /** What `capture_permission_help` answers; nothing to allow by default, as outside macOS. */
  permissionHelp?: PermissionHelp;
  /** Where `status` and `settings-changed` go. */
  emit?: (event: string, payload: unknown) => void;
}

const HOTKEY_NAMES: HotkeyName[] = ['replay', 'stop', 'catchUp'];
const HOTKEY_LABELS: Record<HotkeyName, string> = { replay: 'Replay', stop: 'Stop', catchUp: 'Catch up' };
/** What the pretend macOS says while capture isn't allowed: the sentence macOS 15 gets. */
export const MOCK_HOW_TO_ALLOW =
  'Turn on Repeat That in System Settings > Privacy & Security > Screen & System Audio Recording.';
/** What the permission commands say where there is nothing to allow, as on Linux. */
const NOTHING_TO_ALLOW = "This OS doesn't ask for permission to capture audio";
/** Pretend another app owns this one, to show the "In use by another app" state. */
const TAKEN_BY_ANOTHER_APP = 'Ctrl+Shift+Escape';
const ACCEL_SHAPE = /^((Ctrl|Alt|Shift|Super)\+)*[A-Za-z0-9]+$/;

function fail(kind: CommandError['kind'], message: string): never {
  throw { kind, message } satisfies CommandError;
}

function iconFor(letter: string, color: string): string {
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect width="32" height="32" rx="7" fill="${color}"/>` +
    `<text x="16" y="22.5" font-family="sans-serif" font-size="18" font-weight="700" text-anchor="middle" fill="#fff">${letter}</text></svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}

export function mockApps(): AppEntry[] {
  return [
    { pid: 4120, name: 'Discord', exePath: 'C:\\Users\\you\\AppData\\Local\\Discord\\app-1.0.9\\Discord.exe', icon: iconFor('D', '#5865f2') },
    { pid: 8844, name: 'Microsoft Teams', exePath: 'C:\\Program Files\\WindowsApps\\MSTeams\\ms-teams.exe', icon: iconFor('T', '#6264a7') },
    { pid: 2390, name: 'Firefox', exePath: 'C:\\Program Files\\Mozilla Firefox\\firefox.exe', icon: iconFor('F', '#e66000') },
    { pid: 5521, name: 'speech_8s', exePath: null, icon: null },
  ];
}

export function mockDevices(): OutputDevice[] {
  return [
    { id: 'speakers', name: 'Speakers (Realtek Audio)', isDefault: true },
    { id: 'headphones', name: 'Headphones (USB Audio)', isDefault: false },
    { id: 'hdmi', name: 'Monitor (HDMI)', isDefault: false },
  ];
}

export function scenarioInit(scenario: Scenario): MockBackendInit {
  const discord = mockApps()[0];
  const target = { exePath: discord.exePath, name: discord.name };
  switch (scenario) {
    case 'first-run':
      return { apps: mockApps() };
    case 'configured':
      return { apps: mockApps(), settings: { target, firstRunDone: true } };
    case 'waiting':
      return { apps: mockApps().slice(1), settings: { target, firstRunDone: true } };
    case 'empty':
      return { apps: [] };
    case 'portal':
      // A Wayland desktop that owns the keys: two got one, one did not.
      return {
        apps: mockApps(),
        settings: { target, firstRunDone: true },
        statusOverrides: {
          hotkeyBackend: 'portal',
          hotkeyTriggers: { replay: 'Ctrl+Alt+Space', stop: null, catchUp: 'Meta+Shift+C' },
        },
      };
    case 'no-hotkeys':
      return {
        apps: mockApps(),
        settings: { target, firstRunDone: true },
        statusOverrides: {
          hotkeyBackend: 'none',
          hotkeyCommands: {
            replay: 'repeat-that --replay',
            stop: 'repeat-that --stop',
            catchUp: 'repeat-that --catch-up',
          },
        },
      };
    case 'no-tray':
      return { apps: mockApps(), settings: { target, firstRunDone: true }, statusOverrides: { trayAvailable: false } };
    case 'not-allowed':
      // macOS 15 without the permission to capture audio.
      return {
        apps: mockApps(),
        settings: { target, firstRunDone: true },
        permission: 'denied',
        permissionHelp: { canOpenSettings: true, canAskAgain: true },
      };
    case 'problems':
      return {
        apps: mockApps(),
        settings: {
          target,
          firstRunDone: true,
          outputDevice: 'usb-dac-1234',
          hotkeys: { replay: TAKEN_BY_ANOTHER_APP, stop: null, catchUp: 'Ctrl+Alt+Enter' },
        },
        statusOverrides: {
          output: { deviceName: 'Speakers (Realtek Audio)', fellBack: true, error: null },
          vadFallback: true,
          settingsProblem: "The settings file couldn't be set up, so changes are lost when the app closes: access denied",
        },
      };
  }
}

export class MockBackend {
  settings: Settings;
  apps: AppEntry[];
  devices: OutputDevice[];
  statusOverrides: Partial<Status>;
  configureOutcome: ConfigureOutcome;
  permission: 'allowed' | 'denied';
  permissionHelp: PermissionHelp;
  /** Every command received, oldest first. */
  readonly calls: Array<{ cmd: string; args: unknown }> = [];
  /** Replaces a command, to make it fail or hang in a test. */
  readonly overrides: Record<string, (args: unknown) => unknown> = {};

  private readonly send: (event: string, payload: unknown) => void;
  private paused = false;
  private bufferedMs = 0;
  private playback: Status['playback'] = 'idle';
  private behindMs = 0;

  constructor(init: MockBackendInit = {}) {
    this.settings = { ...defaultSettings(), ...init.settings };
    this.apps = init.apps ?? mockApps();
    this.devices = init.devices ?? mockDevices();
    this.statusOverrides = init.statusOverrides ?? {};
    this.configureOutcome = init.configureOutcome ?? 'opened';
    this.permission = init.permission ?? 'allowed';
    this.permissionHelp = init.permissionHelp ?? { canOpenSettings: false, canAskAgain: false };
    this.send = init.emit ?? (() => {});
  }

  /** The `mockIPC` handler. Throws a `CommandError` like a failing command. */
  handle = (cmd: string, args: unknown): unknown => {
    this.calls.push({ cmd, args });
    const override = this.overrides[cmd];
    return override ? override(args) : this.run(cmd, args);
  };

  /** Runs a command for real, past any override (which may want to call it after a delay). */
  run(cmd: string, args: unknown): unknown {
    const a = (args ?? {}) as Record<string, unknown>;
    switch (cmd) {
      case 'get_settings':
        return structuredClone(this.settings);
      case 'update_settings':
        return this.update(a.patch as Partial<Settings>);
      case 'get_status':
        return this.status();
      case 'list_apps':
        return structuredClone(this.apps);
      case 'list_output_devices':
        return structuredClone(this.devices);
      case 'select_app':
        return this.select(a.app as AppEntry);
      case 'set_paused':
        this.paused = a.paused === true;
        if (this.paused) this.wipe();
        return this.push();
      case 'replay':
        return this.startReplay();
      case 'catch_up':
        return this.toggleCatchUp();
      case 'stop':
        this.playback = 'idle';
        return this.push();
      case 'suspend_hotkeys':
        return null;
      case 'configure_shortcuts':
        return this.configureOutcome;
      case 'quit_app':
        // The real app ends; the mock page has nowhere to go.
        return null;
      case 'capture_permission_help':
        return { ...this.permissionHelp };
      case 'ask_for_capture_permission':
        if (!this.permissionHelp.canAskAgain) return fail('backend', NOTHING_TO_ALLOW);
        // The user chooses Allow at once; the real engine takes up to 2 s to notice.
        this.permission = 'allowed';
        return this.push();
      case 'open_capture_permission_settings':
        if (!this.permissionHelp.canOpenSettings) return fail('backend', NOTHING_TO_ALLOW);
        return null;
      default:
        return fail('backend', `Unknown command: ${cmd}`);
    }
  }

  /** Lets `ms` of simulated time pass: speech accumulates while capturing, a replay plays on. */
  tick(ms: number): void {
    if (this.captureState() === 'capturing') {
      this.bufferedMs = Math.min(this.bufferedMs + ms * 0.6, this.settings.bufferSeconds * 1000);
    }
    if (this.playback !== 'idle') {
      const speed = this.playback === 'catchingUp' ? this.settings.catchUpSpeed : this.settings.replaySpeed;
      this.behindMs -= ms * speed * 0.5;
      if (this.behindMs <= 0) {
        this.behindMs = 0;
        this.playback = 'idle';
      }
    }
    this.push();
  }

  /** Replaces the list of running apps, as when a call app starts or quits. */
  setApps(apps: AppEntry[]): void {
    this.apps = apps;
    this.push();
  }

  status(): Status {
    const state = this.captureState();
    const target = this.settings.target;
    const hotkeys = Object.fromEntries(
      HOTKEY_NAMES.map((name): [HotkeyName, HotkeyState] => {
        const accel = this.settings.hotkeys[name];
        return [name, accel === null ? 'unbound' : accel === TAKEN_BY_ANOTHER_APP ? 'inUse' : 'ok'];
      }),
    ) as Status['hotkeys'];
    return {
      capture: { state, message: state === 'notAllowed' ? MOCK_HOW_TO_ALLOW : null },
      targetName: target?.name ?? null,
      playback: this.playback,
      behindMs: this.playback === 'idle' ? null : Math.round(this.behindMs),
      bufferedMs: Math.round(this.bufferedMs),
      output: { deviceName: this.devices.find((d) => d.isDefault)?.name ?? null, fellBack: false, error: null },
      hotkeys,
      vadFallback: false,
      settingsProblem: null,
      hotkeyBackend: 'plugin',
      hotkeyTriggers: { replay: null, stop: null, catchUp: null },
      hotkeyCommands: null,
      trayAvailable: true,
      ...this.statusOverrides,
    };
  }

  private captureState(): Status['capture']['state'] {
    const target = this.settings.target;
    if (target === null) return 'noTarget';
    if (this.paused) return 'paused';
    if (!this.apps.some((app) => matchesTarget(target, app))) return 'waiting';
    return this.permission === 'denied' ? 'notAllowed' : 'capturing';
  }

  private wipe(): void {
    this.bufferedMs = 0;
    this.playback = 'idle';
    this.behindMs = 0;
  }

  private push(): null {
    this.send('status', this.status());
    return null;
  }

  private select(app: AppEntry): null {
    this.settings = { ...this.settings, target: { exePath: app.exePath, name: app.name } };
    this.paused = false;
    this.wipe();
    this.send('settings-changed', structuredClone(this.settings));
    return this.push();
  }

  private update(patch: Partial<Settings>): Settings {
    const next: Settings = { ...this.settings, ...patch };
    next.duck = { ...this.settings.duck, ...patch.duck };
    next.hotkeys = { ...this.settings.hotkeys, ...patch.hotkeys };

    next.bufferSeconds = clamp(next.bufferSeconds, LIMITS.bufferSeconds.min, LIMITS.bufferSeconds.max);
    next.replaySpeed = clamp(next.replaySpeed, LIMITS.replaySpeed.min, LIMITS.replaySpeed.max);
    next.catchUpSpeed = clamp(next.catchUpSpeed, LIMITS.catchUpSpeed.min, LIMITS.catchUpSpeed.max);
    const duckLevel = duckLevelLimits();
    next.duck.level = clamp(next.duck.level, duckLevel.min, duckLevel.max);

    for (const name of HOTKEY_NAMES) {
      const accel = next.hotkeys[name];
      if (accel !== null && !ACCEL_SHAPE.test(accel)) fail('invalid', `"${accel}" is not a valid hotkey.`);
    }
    // The combination that changed is the one refused; the message names the action that already has it.
    const changed = HOTKEY_NAMES.filter((name) => next.hotkeys[name] !== this.settings.hotkeys[name]);
    for (const name of changed.length > 0 ? changed : HOTKEY_NAMES) {
      const accel = next.hotkeys[name];
      const other = accel === null ? undefined : HOTKEY_NAMES.find((n) => n !== name && next.hotkeys[n] === accel);
      if (other !== undefined) fail('conflict', `${accel} is already used for ${HOTKEY_LABELS[other]}.`);
    }

    this.settings = next;
    this.bufferedMs = Math.min(this.bufferedMs, next.bufferSeconds * 1000);
    this.push();
    return structuredClone(next);
  }

  private startReplay(): null {
    if (this.bufferedMs < 500) return null; // the real app beeps
    const cap = Math.min(this.bufferedMs, 8000);
    this.behindMs = this.playback === 'idle' ? cap : Math.min(this.behindMs + 2000, this.bufferedMs);
    if (this.playback === 'idle') this.playback = 'replaying';
    return this.push();
  }

  private toggleCatchUp(): null {
    if (this.playback === 'idle') {
      if (this.bufferedMs < 500) return null;
      this.behindMs = Math.min(this.bufferedMs, 8000);
      this.playback = 'catchingUp';
    } else {
      this.playback = this.playback === 'catchingUp' ? 'replaying' : 'catchingUp';
    }
    return this.push();
  }
}

/** Reads `?scenario=` from the page URL; anything unknown is the first run. */
export function scenarioFromUrl(search: string): Scenario {
  const wanted = new URLSearchParams(search).get('scenario');
  return SCENARIOS.find((s) => s === wanted) ?? 'first-run';
}

/** Installs the mock as the page's Tauri IPC and starts the simulated clock. */
export function installMockBackend(scenario: Scenario = 'first-run'): MockBackend {
  const backend = new MockBackend({
    ...scenarioInit(scenario),
    emit: (event, payload) => void emit(event, payload),
  });
  mockIPC(backend.handle, { shouldMockEvents: true });
  window.setInterval(() => backend.tick(1000), 1000);
  // For poking at it from the browser console, e.g. `__mockBackend.setApps([])`.
  (window as unknown as { __mockBackend: MockBackend }).__mockBackend = backend;
  return backend;
}
