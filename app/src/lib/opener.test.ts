import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { openExternal } from './opener';

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

  it('asks the opener plugin inside Tauri', async () => {
    const calls: unknown[] = [];
    insideTauri((cmd, args) => {
      calls.push({ cmd, args });
    });

    await openExternal(URL_TO_OPEN);

    expect(calls).toEqual([{ cmd: 'plugin:opener|open_url', args: { url: URL_TO_OPEN } }]);
    expect(open).not.toHaveBeenCalled();
  });

  it('falls back to a browser tab when the plugin is not there', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    insideTauri(() => {
      throw 'Command plugin:opener|open_url not found';
    });

    await openExternal(URL_TO_OPEN);

    expect(open).toHaveBeenCalledWith(URL_TO_OPEN, '_blank', 'noopener,noreferrer');
  });
});
