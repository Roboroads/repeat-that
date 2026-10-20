import { describe, expect, it } from 'vitest';
import { formatAccel, interpretKey } from './hotkey';

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
