import { describe, expect, it } from 'vitest';
import { pretendPlatform } from '../test-utils';
import type { AppEntry, Settings, Status } from './api';
import { MOCK_HOW_TO_ALLOW, MockBackend, SCENARIOS, scenarioInit } from './mock-ipc';

function backend(init: ConstructorParameters<typeof MockBackend>[0] = {}) {
  const events: Array<{ event: string; payload: unknown }> = [];
  const b = new MockBackend({ ...init, emit: (event, payload) => events.push({ event, payload }) });
  return { b, events };
}

const discord: AppEntry = { pid: 1, name: 'Discord', exePath: 'C:\\Discord.exe', icon: null };

describe('MockBackend', () => {
  it('starts with the contract defaults', () => {
    const { b } = backend();
    const settings = b.handle('get_settings', {}) as Settings;

    expect(settings).toMatchObject({ version: 1, target: null, bufferSeconds: 30, firstRunDone: false });
    expect((b.handle('get_status', {}) as Status).capture.state).toBe('noTarget');
  });

  it('merges a patch, clamps numbers and returns the new settings', () => {
    const { b } = backend();

    const result = b.handle('update_settings', {
      patch: { bufferSeconds: 500, replaySpeed: 0.1, duck: { enabled: false, level: 0 } },
    }) as Settings;

    expect(result.bufferSeconds).toBe(120);
    expect(result.replaySpeed).toBe(0.75);
    expect(result.duck).toEqual({ enabled: false, level: 0.05 });
    expect((b.handle('get_settings', {}) as Settings).bufferSeconds).toBe(120);
  });

  it('lets the duck level go down to muted on a Mac only', () => {
    pretendPlatform('MacIntel');
    const { b } = backend();

    const result = b.handle('update_settings', { patch: { duck: { enabled: true, level: -1 } } }) as Settings;

    expect(result.duck.level).toBe(0);
  });

  it('rejects a hotkey that another action uses, changing nothing', () => {
    const { b } = backend();

    expect(() =>
      b.handle('update_settings', {
        patch: { hotkeys: { replay: 'Ctrl+Alt+Enter', stop: 'Ctrl+Alt+Backspace', catchUp: 'Ctrl+Alt+Enter' } },
      }),
    ).toThrowError(expect.objectContaining({ kind: 'conflict' }));
    expect((b.handle('get_settings', {}) as Settings).hotkeys.replay).toBe('Ctrl+Alt+Space');
  });

  it('rejects an accelerator it cannot parse', () => {
    const { b } = backend();

    expect(() =>
      b.handle('update_settings', {
        patch: { hotkeys: { replay: 'Ctrl++', stop: null, catchUp: null } },
      }),
    ).toThrowError(expect.objectContaining({ kind: 'invalid' }));
  });

  it('reports a hotkey another app owns as inUse and an unbound one as unbound', () => {
    const { b } = backend();

    b.handle('update_settings', {
      patch: { hotkeys: { replay: 'Ctrl+Shift+Escape', stop: null, catchUp: 'Ctrl+Alt+Enter' } },
    });

    expect((b.handle('get_status', {}) as Status).hotkeys).toEqual({ replay: 'inUse', stop: 'unbound', catchUp: 'ok' });
  });

  it('selecting a running app starts capturing it with an empty buffer and tells the window', () => {
    const { b, events } = backend({ apps: [discord] });

    b.handle('select_app', { app: discord });
    b.tick(10_000);
    const status = b.handle('get_status', {}) as Status;

    expect(status.capture.state).toBe('capturing');
    expect(status.targetName).toBe('Discord');
    expect(status.bufferedMs).toBeGreaterThan(0);
    expect(events.map((e) => e.event)).toContain('settings-changed');
    expect(events.map((e) => e.event)).toContain('status');
  });

  it('waits for a target that is not running', () => {
    const { b } = backend({ apps: [], settings: { target: { exePath: 'C:\\Discord.exe', name: 'Discord' } } });

    expect((b.handle('get_status', {}) as Status).capture.state).toBe('waiting');
  });

  it('pausing wipes the buffer; resuming captures again', () => {
    const { b } = backend({ apps: [discord] });
    b.handle('select_app', { app: discord });
    b.tick(10_000);

    b.handle('set_paused', { paused: true });
    expect(b.handle('get_status', {}) as Status).toMatchObject({ capture: { state: 'paused' }, bufferedMs: 0 });

    b.handle('set_paused', { paused: false });
    expect((b.handle('get_status', {}) as Status).capture.state).toBe('capturing');
  });

  it('never buffers more than the buffer length', () => {
    const { b } = backend({ apps: [discord], settings: { bufferSeconds: 10 } });
    b.handle('select_app', { app: discord });

    b.tick(600_000);

    expect((b.handle('get_status', {}) as Status).bufferedMs).toBe(10_000);
  });

  it('replays, catches up, stops, and returns to idle by itself', () => {
    const { b } = backend({ apps: [discord] });
    b.handle('select_app', { app: discord });
    b.tick(20_000);

    b.handle('replay', {});
    expect((b.handle('get_status', {}) as Status).playback).toBe('replaying');
    b.handle('catch_up', {});
    expect((b.handle('get_status', {}) as Status).playback).toBe('catchingUp');
    b.handle('stop', {});
    expect((b.handle('get_status', {}) as Status).playback).toBe('idle');

    b.handle('replay', {});
    b.tick(60_000);
    expect((b.handle('get_status', {}) as Status).playback).toBe('idle');
  });

  it('does nothing visible when replaying an empty buffer', () => {
    const { b } = backend({ apps: [discord] });

    b.handle('replay', {});

    expect((b.handle('get_status', {}) as Status).playback).toBe('idle');
  });

  it('rejects an unknown command', () => {
    const { b } = backend();

    expect(() => b.handle('nope', {})).toThrowError(expect.objectContaining({ kind: 'backend' }));
  });

  it('records every call', () => {
    const { b } = backend();

    b.handle('suspend_hotkeys', { suspended: true });

    expect(b.calls).toEqual([{ cmd: 'suspend_hotkeys', args: { suspended: true } }]);
  });
});

describe('the desktop', () => {
  it('is an ordinary desktop by default: plugin hotkeys, a tray, nothing assigned', () => {
    const { b } = backend();
    const status = b.handle('get_status', {}) as Status;

    expect(status.hotkeyBackend).toBe('plugin');
    expect(status.hotkeyTriggers).toEqual({ replay: null, stop: null, catchUp: null });
    expect(status.hotkeyCommands).toBeNull();
    expect(status.trayAvailable).toBe(true);
  });

  it('lets a test or a scenario say otherwise', () => {
    const { b } = backend({ statusOverrides: { hotkeyBackend: 'none', trayAvailable: false } });
    const status = b.handle('get_status', {}) as Status;

    expect(status.hotkeyBackend).toBe('none');
    expect(status.trayAvailable).toBe(false);
  });

  it('configure_shortcuts opens the settings, or says it cannot', () => {
    expect(backend().b.handle('configure_shortcuts', {})).toBe('opened');
    expect(backend({ configureOutcome: 'unsupported' }).b.handle('configure_shortcuts', {})).toBe('unsupported');
  });

  it('quit_app is a command, and the mock keeps running', () => {
    const { b } = backend();

    expect(b.handle('quit_app', {})).toBeNull();
    expect(b.calls.map((call) => call.cmd)).toEqual(['quit_app']);
  });

  it('the portal scenario has the keys the desktop assigned', () => {
    const status = backend(scenarioInit('portal')).b.status();

    expect(status.hotkeyBackend).toBe('portal');
    expect(status.hotkeyTriggers.replay).not.toBeNull();
    expect(status.hotkeyCommands).toBeNull();
  });

  it('the no-hotkeys scenario has the commands to bind', () => {
    const status = backend(scenarioInit('no-hotkeys')).b.status();

    expect(status.hotkeyBackend).toBe('none');
    expect(status.hotkeyCommands).toMatchObject({ replay: expect.stringContaining('--replay') });
    expect(status.hotkeyTriggers).toEqual({ replay: null, stop: null, catchUp: null });
  });

  it('the no-tray scenario has no tray', () => {
    expect(backend(scenarioInit('no-tray')).b.status().trayAvailable).toBe(false);
  });
});

describe('the permission to capture audio', () => {
  it('has nothing to allow by default, as outside macOS', () => {
    const { b } = backend({ apps: [discord], settings: { target: { exePath: discord.exePath, name: 'Discord' } } });

    expect(b.status().capture).toEqual({ state: 'capturing', message: null });
    expect(b.handle('capture_permission_help', {})).toEqual({ canOpenSettings: false, canAskAgain: false });
    expect(() => b.handle('ask_for_capture_permission', {})).toThrowError(expect.objectContaining({ kind: 'backend' }));
    expect(() => b.handle('open_capture_permission_settings', {})).toThrowError(
      expect.objectContaining({ kind: 'backend' }),
    );
  });

  it('is not allowed to capture a running target until asked again, and buffers nothing meanwhile', () => {
    const { b, events } = backend(scenarioInit('not-allowed'));

    b.tick(10_000);
    expect(b.status().capture).toEqual({ state: 'notAllowed', message: MOCK_HOW_TO_ALLOW });
    expect(b.status().bufferedMs).toBe(0);
    expect(b.handle('capture_permission_help', {})).toEqual({ canOpenSettings: true, canAskAgain: true });
    expect(b.handle('open_capture_permission_settings', {})).toBeNull();

    expect(b.handle('ask_for_capture_permission', {})).toBeNull();
    expect(b.status().capture.state).toBe('capturing');
    expect(events.at(-1)).toMatchObject({ event: 'status', payload: { capture: { state: 'capturing' } } });
  });

  it('waits for a target that is not running, allowed or not', () => {
    const { b } = backend({ ...scenarioInit('not-allowed'), apps: [] });

    expect(b.status().capture.state).toBe('waiting');
  });
});

describe('scenarios', () => {
  it.each(SCENARIOS)('%s builds a backend that answers get_settings', (name) => {
    const { b } = backend(scenarioInit(name));

    expect((b.handle('get_settings', {}) as Settings).version).toBe(1);
  });
});
