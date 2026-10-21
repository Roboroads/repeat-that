import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { emit } from '@tauri-apps/api/event';
import { afterEach, describe, expect, it } from 'vitest';
import * as api from './api';
import { defaultSettings } from './defaults';

type Call = { cmd: string; args: unknown };

function mockBackend(handler: (cmd: string, args: unknown) => unknown = () => null) {
  const calls: Call[] = [];
  mockIPC(
    (cmd, args) => {
      calls.push({ cmd, args });
      return handler(cmd, args);
    },
    { shouldMockEvents: true },
  );
  return calls;
}

afterEach(() => {
  clearMocks();
});

describe('commands', () => {
  it('get_settings returns what the backend sent', async () => {
    mockBackend(() => defaultSettings());

    await expect(api.getSettings()).resolves.toEqual(defaultSettings());
  });

  it('update_settings wraps the patch', async () => {
    const calls = mockBackend(() => defaultSettings());

    await api.updateSettings({ bufferSeconds: 45 });

    expect(calls).toEqual([{ cmd: 'update_settings', args: { patch: { bufferSeconds: 45 } } }]);
  });

  it('select_app sends the app without its icon', async () => {
    const calls = mockBackend();

    await api.selectApp({ pid: 7, name: 'Discord', exePath: 'C:\\Discord.exe', icon: 'data:image/png;base64,AAAA' });

    expect(calls[0]).toEqual({
      cmd: 'select_app',
      args: { app: { pid: 7, name: 'Discord', exePath: 'C:\\Discord.exe', icon: null } },
    });
  });

  it.each([
    ['setPaused', () => api.setPaused(true), 'set_paused', { paused: true }],
    ['suspendHotkeys', () => api.suspendHotkeys(false), 'suspend_hotkeys', { suspended: false }],
    ['replay', () => api.replay(), 'replay', {}],
    ['stop', () => api.stop(), 'stop', {}],
    ['catchUp', () => api.catchUp(), 'catch_up', {}],
    ['configureShortcuts', () => api.configureShortcuts(), 'configure_shortcuts', {}],
    ['quitApp', () => api.quitApp(), 'quit_app', {}],
    ['getStatus', () => api.getStatus(), 'get_status', {}],
    ['listApps', () => api.listApps(), 'list_apps', {}],
    ['listOutputDevices', () => api.listOutputDevices(), 'list_output_devices', {}],
  ])('%s calls its command', async (_name, run, cmd, args) => {
    const calls = mockBackend();

    await run();

    expect(calls).toHaveLength(1);
    expect(calls[0].cmd).toBe(cmd);
    expect(calls[0].args).toEqual(args);
  });
});

describe('desktop commands', () => {
  it('configureShortcuts resolves with what the desktop did', async () => {
    mockBackend(() => 'opened');
    await expect(api.configureShortcuts()).resolves.toBe('opened');

    mockBackend(() => 'unsupported');
    await expect(api.configureShortcuts()).resolves.toBe('unsupported');
  });

  it('configureShortcuts rejects with the backend error', async () => {
    mockBackend(() => {
      throw { kind: 'backend', message: 'Could not open the shortcut settings: gone' };
    });

    await expect(api.configureShortcuts()).rejects.toMatchObject({
      kind: 'backend',
      message: 'Could not open the shortcut settings: gone',
    });
  });

  it('a status carries the desktop fields next to the old ones', async () => {
    mockBackend();
    const seen: api.Status[] = [];
    await api.onStatus((status) => seen.push(status));
    const status = {
      hotkeyBackend: 'portal',
      hotkeyTriggers: { replay: 'Ctrl+Alt+Space', stop: null, catchUp: null },
      hotkeyCommands: null,
      trayAvailable: false,
    };

    await emit('status', status);

    expect(seen).toEqual([status]);
  });
});

describe('errors', () => {
  it('keeps the kind and message of a CommandError', async () => {
    mockBackend(() => {
      throw { kind: 'conflict', message: 'Ctrl+Alt+Space is already used for Replay' };
    });

    const error = await api.updateSettings({}).catch((e: unknown) => e);

    expect(error).toBeInstanceOf(api.ApiError);
    expect(error).toMatchObject({ kind: 'conflict', message: 'Ctrl+Alt+Space is already used for Replay' });
  });

  it('turns a plain string into a backend error', async () => {
    mockBackend(() => {
      throw 'boom';
    });

    const error = await api.getStatus().catch((e: unknown) => e);

    expect(error).toMatchObject({ kind: 'backend', message: 'boom' });
  });

  it('turns an Error into a backend error', async () => {
    mockBackend(() => {
      throw new Error('no ipc');
    });

    const error = await api.getStatus().catch((e: unknown) => e);

    expect(error).toMatchObject({ kind: 'backend', message: 'no ipc' });
  });

  it('falls back to a generic message for an unknown shape', async () => {
    mockBackend(() => {
      throw { weird: true };
    });

    const error = await api.getStatus().catch((e: unknown) => e);

    expect(error).toMatchObject({ kind: 'backend' });
    expect((error as Error).message).not.toBe('');
  });
});

describe('events', () => {
  it('delivers status payloads until unlistened', async () => {
    mockBackend();
    const seen: unknown[] = [];
    const unlisten = await api.onStatus((status) => seen.push(status));

    await emit('status', { playback: 'replaying' });
    unlisten();
    await emit('status', { playback: 'idle' });

    expect(seen).toEqual([{ playback: 'replaying' }]);
  });

  it('delivers settings-changed payloads', async () => {
    mockBackend();
    const seen: unknown[] = [];
    await api.onSettingsChanged((settings) => seen.push(settings));

    await emit('settings-changed', defaultSettings());

    expect(seen).toEqual([defaultSettings()]);
  });
});
