import { describe, expect, it } from 'vitest';
import { ApiError } from './api';
import { SettingsStore } from './store.svelte';

describe('SettingsStore.update', () => {
  it('fails, rather than pretending to save, before the settings are loaded', async () => {
    const store = new SettingsStore();

    const result = await store.update({ autostart: true });

    expect(result).toBeInstanceOf(ApiError);
    expect(result?.message).toMatch(/not loaded/i);
    expect(store.error).toBe(result?.message);
  });

  it('leaves the banner alone when the caller reports the error itself', async () => {
    const store = new SettingsStore();

    const result = await store.update({ autostart: true }, { report: false });

    expect(result).toBeInstanceOf(ApiError);
    expect(store.error).toBeNull();
  });
});
