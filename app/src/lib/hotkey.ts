// Turns what the hotkey recorder sees into the accelerator strings of tauri-plugin-global-shortcut ("Ctrl+Alt+Space").
import type { Accel, HotkeyName, Settings, Status } from './api';
import { currentPlatform, isMac, isWindows } from './platform';

type KeyInput = Pick<KeyboardEvent, 'code' | 'ctrlKey' | 'altKey' | 'shiftKey' | 'metaKey'>;

export type KeyOutcome =
  /** A full combination: save it. */
  | { kind: 'bind'; accel: string }
  /** Only modifiers so far; `modifiers` is what is held. */
  | { kind: 'pending'; modifiers: string[] }
  /** Bare Escape: stop recording. */
  | { kind: 'cancel' }
  /** Bare Backspace: unbind. */
  | { kind: 'clear' }
  /** Not usable as a global hotkey; `reason` says why. */
  | { kind: 'reject'; reason: string }
  /** Not ours (bare Tab): let the browser move focus. */
  | { kind: 'ignore' };

const MODIFIER_CODES = new Set([
  'ControlLeft',
  'ControlRight',
  'AltLeft',
  'AltRight',
  'ShiftLeft',
  'ShiftRight',
  'MetaLeft',
  'MetaRight',
  'OSLeft',
  'OSRight',
  'AltGraph',
]);

/** Key codes whose `KeyboardEvent.code` is also the name the shortcut parser knows. */
const NAMED_KEYS = new Set([
  'Space',
  'Enter',
  'Backspace',
  'Escape',
  'Delete',
  'Insert',
  'Home',
  'End',
  'PageUp',
  'PageDown',
  'ArrowUp',
  'ArrowDown',
  'ArrowLeft',
  'ArrowRight',
  'Backquote',
  'Minus',
  'Equal',
  'Comma',
  'Period',
  'Slash',
  'Backslash',
  'Semicolon',
  'Quote',
  'BracketLeft',
  'BracketRight',
]);

/** The shortcut parser's name for a key, or `undefined` when it has none we trust. */
function keyName(code: string): string | undefined {
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter) return letter[1];
  const digit = /^Digit([0-9])$/.exec(code);
  if (digit) return digit[1];
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  if (/^Numpad([0-9]|Add|Subtract|Multiply|Divide|Decimal|Enter)$/.test(code)) return code;
  return NAMED_KEYS.has(code) ? code : undefined;
}

/** The modifiers held in `e`, as accelerator names in the order they are written. */
export function heldModifiers(e: Pick<KeyboardEvent, 'ctrlKey' | 'altKey' | 'shiftKey' | 'metaKey'>): string[] {
  const held: string[] = [];
  if (e.ctrlKey) held.push('Ctrl');
  if (e.altKey) held.push('Alt');
  if (e.shiftKey) held.push('Shift');
  if (e.metaKey) held.push('Super');
  return held;
}

export function interpretKey(e: KeyInput): KeyOutcome {
  const modifiers = heldModifiers(e);

  if (MODIFIER_CODES.has(e.code)) return { kind: 'pending', modifiers };

  if (modifiers.length === 0) {
    if (e.code === 'Escape') return { kind: 'cancel' };
    if (e.code === 'Backspace') return { kind: 'clear' };
  }
  if (e.code === 'Tab' && !e.ctrlKey && !e.altKey && !e.metaKey) return { kind: 'ignore' };

  const name = keyName(e.code);
  if (name === undefined) return { kind: 'reject', reason: "That key can't be used for a hotkey." };

  // A bare letter, digit or space would fire while typing anywhere. Function keys are the exception.
  const isFunctionKey = /^F\d+$/.test(name);
  if (!isFunctionKey && !(e.ctrlKey || e.altKey || e.metaKey)) {
    return { kind: 'reject', reason: 'Hold Ctrl or Alt together with the key.' };
  }
  return { kind: 'bind', accel: [...modifiers, name].join('+') };
}

const FRIENDLY: Record<string, string> = {
  ArrowUp: 'Up',
  ArrowDown: 'Down',
  ArrowLeft: 'Left',
  ArrowRight: 'Right',
  Backquote: '`',
  Minus: '-',
  Equal: '=',
  Comma: ',',
  Period: '.',
  Slash: '/',
  Backslash: '\\',
  Semicolon: ';',
  Quote: "'",
  BracketLeft: '[',
  BracketRight: ']',
};

/** The Super key is called Win on Windows and Cmd on macOS. `platform` is a `navigator.platform` style string. */
function superName(platform: string): string {
  if (isMac(platform)) return 'Cmd';
  if (isWindows(platform)) return 'Win';
  return 'Super';
}

/** The keys of an accelerator as shown to the user, one string per key cap. */
export function formatAccel(accel: Accel, platform: string = currentPlatform()): string[] {
  if (accel === null || accel === '') return [];
  return accel.split('+').map((part) => (part === 'Super' ? superName(platform) : (FRIENDLY[part] ?? part)));
}

/**
 * The key to name next to an action (on its button, say), or `undefined` when it has none. With the plugin that is the
 * stored combination; under the portal it is what the desktop assigned, which the stored one only asked for; without
 * hotkeys there is nothing to name.
 */
export function hotkeyHint(name: HotkeyName, settings: Settings, status: Status | null): string | undefined {
  switch (status?.hotkeyBackend ?? 'plugin') {
    case 'portal':
      return status?.hotkeyTriggers[name] ?? undefined;
    case 'none':
      return undefined;
    case 'plugin': {
      const keys = formatAccel(settings.hotkeys[name]);
      return keys.length > 0 ? keys.join('+') : undefined;
    }
  }
}
