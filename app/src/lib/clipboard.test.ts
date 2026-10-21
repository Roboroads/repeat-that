import { afterEach, describe, expect, it, vi } from 'vitest';
import { copyText } from './clipboard';

afterEach(() => {
  Reflect.deleteProperty(navigator, 'clipboard');
  Reflect.deleteProperty(document, 'execCommand');
});

describe('copyText', () => {
  it('writes to the clipboard', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });

    await expect(copyText('repeat-that --replay')).resolves.toBe(true);

    expect(writeText).toHaveBeenCalledWith('repeat-that --replay');
  });

  it('falls back to selecting the text and copying it when the clipboard is not there', async () => {
    let copied: string | null = null;
    Object.defineProperty(document, 'execCommand', {
      configurable: true,
      value: vi.fn((command: string) => {
        copied = (document.activeElement as HTMLTextAreaElement).value;
        return command === 'copy';
      }),
    });

    await expect(copyText('repeat-that --stop')).resolves.toBe(true);

    expect(copied).toBe('repeat-that --stop');
    expect(document.querySelector('textarea')).toBeNull();
  });

  it('falls back when the clipboard refuses', async () => {
    const writeText = vi.fn().mockRejectedValue(new Error('not allowed'));
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    const execCommand = vi.fn(() => true);
    Object.defineProperty(document, 'execCommand', { configurable: true, value: execCommand });

    await expect(copyText('repeat-that --catch-up')).resolves.toBe(true);

    expect(execCommand).toHaveBeenCalledWith('copy');
  });

  it('says so when nothing works', async () => {
    Object.defineProperty(document, 'execCommand', { configurable: true, value: vi.fn(() => false) });

    await expect(copyText('repeat-that --replay')).resolves.toBe(false);
    expect(document.querySelector('textarea')).toBeNull();
  });
});
