import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { openExternal, openLink } from './opener';

const URL_TO_OPEN = 'https://example.com/PRIVACY.md';

let open: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  open = vi.spyOn(window, 'open').mockReturnValue(null);
});

afterEach(() => {
  open.mockRestore();
  Reflect.deleteProperty(globalThis, 'isTauri');
  clearMocks();
});

function insideTauri(handler: (cmd: string, args: unknown) => unknown) {
  Object.defineProperty(globalThis, 'isTauri', { configurable: true, value: true });
  mockIPC(handler);
}

describe('openExternal', () => {
  it('opens a new browser tab in a plain browser', async () => {
    await openExternal(URL_TO_OPEN);

    expect(open).toHaveBeenCalledWith(URL_TO_OPEN, '_blank', 'noopener,noreferrer');
  });

  it('asks the app inside Tauri', async () => {
    const calls: unknown[] = [];
    insideTauri((cmd, args) => {
      calls.push({ cmd, args });
    });

    await openExternal(URL_TO_OPEN);

    expect(calls).toEqual([{ cmd: 'open_link', args: { url: URL_TO_OPEN } }]);
    expect(open).not.toHaveBeenCalled();
  });

  it('falls back to a browser tab when the app can not open it', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    insideTauri(() => {
      throw { kind: 'backend', message: 'xdg-open: No such file or directory' };
    });

    await openExternal(URL_TO_OPEN);

    expect(open).toHaveBeenCalledWith(URL_TO_OPEN, '_blank', 'noopener,noreferrer');
  });
});

describe('openLink', () => {
  it('keeps the window on its page and opens the address outside it', async () => {
    const click = new MouseEvent('click', { cancelable: true });

    openLink(click, URL_TO_OPEN);

    expect(click.defaultPrevented).toBe(true);
    await vi.waitFor(() => expect(open).toHaveBeenCalledWith(URL_TO_OPEN, '_blank', 'noopener,noreferrer'));
  });
});
