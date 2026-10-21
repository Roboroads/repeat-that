import { describe, expect, it } from 'vitest';
import type { Settings, Status } from './api';
import { defaultSettings } from './defaults';
import { formatAccel, hotkeyHint, interpretKey } from './hotkey';
import { MockBackend } from './mock-ipc';

type Mods = { ctrlKey?: boolean; altKey?: boolean; shiftKey?: boolean; metaKey?: boolean };

function key(code: string, mods: Mods = {}) {
  return { code, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...mods };
}

describe('interpretKey', () => {
  it('binds the default replay combination', () => {
    expect(interpretKey(key('Space', { ctrlKey: true, altKey: true }))).toEqual({
      kind: 'bind',
      accel: 'Ctrl+Alt+Space',
    });
  });

  it('orders modifiers Ctrl, Alt, Shift, Super whatever the press order', () => {
    expect(interpretKey(key('KeyR', { metaKey: true, shiftKey: true, altKey: true, ctrlKey: true }))).toEqual({
      kind: 'bind',
      accel: 'Ctrl+Alt+Shift+Super+R',
    });
  });

  it.each([
    ['KeyA', 'A'],
    ['Digit7', '7'],
    ['F9', 'F9'],
    ['Numpad3', 'Numpad3'],
    ['Enter', 'Enter'],
    ['ArrowUp', 'ArrowUp'],
    ['PageDown', 'PageDown'],
    ['Backquote', 'Backquote'],
    ['BracketLeft', 'BracketLeft'],
  ])('names %s as %s', (code, name) => {
    expect(interpretKey(key(code, { ctrlKey: true }))).toEqual({ kind: 'bind', accel: `Ctrl+${name}` });
  });

  it('binds Backspace and Escape when a modifier is held', () => {
    expect(interpretKey(key('Backspace', { ctrlKey: true, altKey: true }))).toEqual({
      kind: 'bind',
      accel: 'Ctrl+Alt+Backspace',
    });
    expect(interpretKey(key('Escape', { ctrlKey: true }))).toEqual({ kind: 'bind', accel: 'Ctrl+Escape' });
  });

  it('cancels on a bare Escape and clears on a bare Backspace', () => {
    expect(interpretKey(key('Escape'))).toEqual({ kind: 'cancel' });
    expect(interpretKey(key('Backspace'))).toEqual({ kind: 'clear' });
  });

  it('allows a bare function key, with or without Shift', () => {
    expect(interpretKey(key('F9'))).toEqual({ kind: 'bind', accel: 'F9' });
    expect(interpretKey(key('F9', { shiftKey: true }))).toEqual({ kind: 'bind', accel: 'Shift+F9' });
  });

  it('rejects a key that would hijack typing', () => {
    expect(interpretKey(key('KeyA'))).toMatchObject({ kind: 'reject' });
    expect(interpretKey(key('KeyA', { shiftKey: true }))).toMatchObject({ kind: 'reject' });
    expect(interpretKey(key('Space'))).toMatchObject({ kind: 'reject' });
  });

  it('rejects a key a global shortcut cannot use', () => {
    expect(interpretKey(key('MediaPlayPause', { ctrlKey: true }))).toMatchObject({ kind: 'reject' });
    expect(interpretKey(key('CapsLock', { ctrlKey: true }))).toMatchObject({ kind: 'reject' });
  });

  it('waits on a modifier alone and reports what is held', () => {
    expect(interpretKey(key('ControlLeft', { ctrlKey: true }))).toEqual({ kind: 'pending', modifiers: ['Ctrl'] });
    expect(interpretKey(key('AltRight', { ctrlKey: true, altKey: true }))).toEqual({
      kind: 'pending',
      modifiers: ['Ctrl', 'Alt'],
    });
    expect(interpretKey(key('ShiftLeft', { shiftKey: true }))).toEqual({ kind: 'pending', modifiers: ['Shift'] });
    expect(interpretKey(key('MetaLeft', { metaKey: true }))).toEqual({ kind: 'pending', modifiers: ['Super'] });
  });

  it('leaves a bare Tab to the browser so focus can move away', () => {
    expect(interpretKey(key('Tab'))).toEqual({ kind: 'ignore' });
    expect(interpretKey(key('Tab', { shiftKey: true }))).toEqual({ kind: 'ignore' });
  });
});

describe('formatAccel', () => {
  it('splits an accelerator into the keys to show', () => {
    expect(formatAccel('Ctrl+Alt+Space', 'Linux')).toEqual(['Ctrl', 'Alt', 'Space']);
  });

  it('shows Super as Win on Windows and Cmd on macOS', () => {
    expect(formatAccel('Ctrl+Super+R', 'Windows')).toEqual(['Ctrl', 'Win', 'R']);
    expect(formatAccel('Ctrl+Super+R', 'macOS')).toEqual(['Ctrl', 'Cmd', 'R']);
  });

  it('shows friendlier names for arrows, digits and punctuation', () => {
    expect(formatAccel('Ctrl+ArrowUp', 'Linux')).toEqual(['Ctrl', 'Up']);
    expect(formatAccel('Ctrl+Backquote', 'Linux')).toEqual(['Ctrl', '`']);
  });

  it('gives nothing for an unbound hotkey', () => {
    expect(formatAccel(null, 'Linux')).toEqual([]);
  });
});

describe('hotkeyHint', () => {
  const settings: Settings = {
    ...defaultSettings(),
    hotkeys: { replay: 'Ctrl+Alt+Space', stop: null, catchUp: 'Ctrl+Alt+Enter' },
  };
  const status = (overrides: Partial<Status>) => new MockBackend({ statusOverrides: overrides }).status();
  const triggers = { replay: 'Meta+Shift+R', stop: null, catchUp: 'Meta+Escape' };

  it('is the stored combination with the plugin, and nothing for an unbound action', () => {
    const plugin = status({ hotkeyBackend: 'plugin' });
    expect(hotkeyHint('replay', settings, plugin)).toBe('Ctrl+Alt+Space');
    expect(hotkeyHint('catchUp', settings, plugin)).toBe('Ctrl+Alt+Enter');
    expect(hotkeyHint('stop', settings, plugin)).toBeUndefined();
  });

  it('is the stored combination before the first status', () => {
    expect(hotkeyHint('replay', settings, null)).toBe('Ctrl+Alt+Space');
  });

  it('is the key the desktop assigned under the portal, whatever is stored', () => {
    const portal = status({ hotkeyBackend: 'portal', hotkeyTriggers: triggers });
    expect(hotkeyHint('replay', settings, portal)).toBe('Meta+Shift+R');
    expect(hotkeyHint('catchUp', settings, portal)).toBe('Meta+Escape');
    expect(hotkeyHint('stop', settings, portal)).toBeUndefined();
  });

  it('is nothing without hotkeys', () => {
    const none = status({ hotkeyBackend: 'none', hotkeyTriggers: triggers });
    expect(hotkeyHint('replay', settings, none)).toBeUndefined();
    expect(hotkeyHint('catchUp', settings, none)).toBeUndefined();
  });
});
