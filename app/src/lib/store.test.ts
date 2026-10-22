import { emit } from '@tauri-apps/api/event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { argsOf, holdAnswers, installBackend } from '../test-utils';
import { ApiError, type UpdateStatus } from './api';
import { offUpdateStatus } from './defaults';
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

describe('SettingsStore updates', () => {
  const stores: SettingsStore[] = [];
  afterEach(() => stores.splice(0).forEach((store) => store.dispose()));

  async function open(init: Parameters<typeof installBackend>[0] = {}) {
    const backend = installBackend({ updates: { mode: 'install' }, ...init });
    const store = new SettingsStore();
    stores.push(store);
    await store.start();
    return { backend, store };
  }

  it('loads the update status with the settings', async () => {
    const { store } = await open({ updates: { mode: 'notify', modeReason: 'Use the download page.' } });

    expect(store.updates).toMatchObject({ mode: 'notify', modeReason: 'Use the download page.', state: 'idle' });
    expect(store.settings?.checkForUpdates).toBeNull();
  });

  it('treats a backend without the update commands as a build without an updater, and loads the page as before', async () => {
    const backend = installBackend();
    backend.overrides.get_update_status = () => {
      throw 'Command get_update_status not found';
    };
    const store = new SettingsStore();
    stores.push(store);

    await store.start();

    expect(store.loadError).toBeNull();
    expect(store.settings).not.toBeNull();
    expect(store.status).not.toBeNull();
    expect(store.updates).toEqual(offUpdateStatus());
  });

  it('says nothing about a backend that has no update commands', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const backend = installBackend();
    backend.overrides.get_update_status = () => {
      throw 'Command get_update_status not found';
    };
    const store = new SettingsStore();
    stores.push(store);

    await store.start();

    expect(store.updates.mode).toBe('off');
    expect(warn).not.toHaveBeenCalled();
    warn.mockRestore();
  });

  it('logs why the update status could not be read when the backend does have the command', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const backend = installBackend();
    backend.overrides.get_update_status = () => {
      throw { kind: 'backend', message: 'The updater crashed' };
    };
    const store = new SettingsStore();
    stores.push(store);

    await store.start();

    // The page still loads, without an updater.
    expect(store.loadError).toBeNull();
    expect(store.settings).not.toBeNull();
    expect(store.updates.mode).toBe('off');
    expect(warn).toHaveBeenCalledWith(
      expect.stringContaining('update status'),
      expect.objectContaining({ message: 'The updater crashed' }),
    );
    warn.mockRestore();
  });

  it('replaces the update status with every update-status event', async () => {
    const { backend, store } = await open();
    const found = {
      ...offUpdateStatus('0.1.0'),
      mode: 'install' as const,
      modeReason: null,
      state: 'available' as const,
      available: {
        version: '0.2.0',
        date: null,
        notes: 'Fixes',
        releaseUrl: 'https://github.com/Roboroads/repeat-that/releases/tag/v0.2.0',
      },
    };

    await emit('update-status', found);
    await vi.waitFor(() => expect(store.updates).toEqual(found));
    await emit('update-status', { ...found, state: 'downloading', progress: 0.25 });

    await vi.waitFor(() => expect(store.updates.progress).toBe(0.25));
    expect(argsOf(backend, 'get_update_status')).toHaveLength(1);
  });

  it('shows the result of a check, and is busy until it is back', async () => {
    const { backend, store } = await open({ updates: { mode: 'install', checkMs: 10 } });

    const checking = store.checkForUpdates();
    expect(store.updateCallPending).toBe(true);
    await checking;

    expect(store.updateCallPending).toBe(false);
    expect(store.updates).toMatchObject({ state: 'available', manual: true, available: { version: '99.0.0' } });
    expect(argsOf(backend, 'check_for_updates')).toHaveLength(1);
  });

  it('puts a check that could not run in the error banner', async () => {
    const { backend, store } = await open();
    backend.overrides.check_for_updates = () => {
      throw { kind: 'backend', message: 'The updater is not running' };
    };

    await store.checkForUpdates();

    expect(store.error).toBe('The updater is not running');
    expect(store.updateCallPending).toBe(false);
  });

  it('puts a refused install in the error banner', async () => {
    const { store } = await open();

    await store.installUpdate();

    expect(store.error).toBe('There is no update to install');
    expect(store.updateCallPending).toBe(false);
  });

  it('installs what a check found', async () => {
    const { backend, store } = await open();
    await store.checkForUpdates();

    await store.installUpdate();

    expect(store.error).toBeNull();
    expect(argsOf(backend, 'install_update')).toHaveLength(1);
    await vi.waitFor(() => expect(store.updates.state).toBe('downloading'));
  });

  it('is downloading as soon as the backend took the install, before its event comes in', async () => {
    const { backend, store } = await open();
    await store.checkForUpdates();
    // Accepted, and the event that says so is still on its way.
    backend.overrides.install_update = () => null;

    await store.installUpdate();

    expect(store.updates.state).toBe('downloading');
  });

  it('sends one install for two presses, and one check for two presses', async () => {
    const { backend, store } = await open();
    await store.checkForUpdates();
    backend.overrides.install_update = () => null;

    await Promise.all([store.installUpdate(), store.installUpdate()]);
    expect(argsOf(backend, 'install_update')).toHaveLength(1);

    const again = await open({ updates: { mode: 'install', checkMs: 10 } });
    await Promise.all([again.store.checkForUpdates(), again.store.checkForUpdates()]);
    expect(argsOf(again.backend, 'check_for_updates')).toHaveLength(1);
  });

  it('hides the offer for the version dismissed, shows it again for a newer one or when asked to', async () => {
    const { store } = await open();
    await store.checkForUpdates();
    expect(store.offerDismissed).toBe(false);

    store.dismissUpdate();
    expect(store.offerDismissed).toBe(true);

    const newer = { ...store.updates.available!, version: '99.1.0' };
    await emit('update-status', { ...store.updates, available: newer });
    await vi.waitFor(() => expect(store.updates.available?.version).toBe('99.1.0'));
    expect(store.offerDismissed).toBe(false);

    store.dismissUpdate();
    store.showUpdate();
    expect(store.offerDismissed).toBe(false);
  });

  it('shows a dismissed offer again when the user checks by hand', async () => {
    const { store } = await open();
    await store.checkForUpdates();
    store.dismissUpdate();

    await store.checkForUpdates();

    expect(store.offerDismissed).toBe(false);
  });

  describe('when an event and an answer cross', () => {
    const offer = {
      version: '0.2.0',
      date: null,
      notes: '',
      releaseUrl: 'https://github.com/Roboroads/repeat-that/releases/tag/v0.2.0',
    };
    const at = (change: Partial<UpdateStatus>): UpdateStatus => ({
      ...offUpdateStatus('0.1.0'),
      mode: 'install',
      modeReason: null,
      ...change,
    });

    it('keeps an event that came in while the first snapshot was on its way', async () => {
      const backend = installBackend({ updates: { mode: 'install' } });
      // The snapshot is taken now, in state idle, but only arrives after the event.
      const release = holdAnswers(backend, 'get_update_status');
      const store = new SettingsStore();
      stores.push(store);

      const started = store.start();
      await vi.waitFor(() => expect(argsOf(backend, 'get_update_status')).toHaveLength(1));
      await emit('update-status', at({ state: 'error', error: 'The download failed', available: offer }));
      release();
      await started;

      expect(store.updates).toMatchObject({ state: 'error', error: 'The download failed' });
    });

    it('keeps an event that arrived before the answer of a check', async () => {
      const { backend, store } = await open();
      backend.overrides.check_for_updates = async () => {
        await emit('update-status', at({ state: 'available', available: offer }));
        // What the backend knew when the user asked, which is older than the event.
        return at({ state: 'checking', manual: true });
      };

      await store.checkForUpdates();

      expect(store.updates.state).toBe('available');
      expect(store.updateCallPending).toBe(false);
    });

    it('still takes the answer of a check when no event came in', async () => {
      const { backend, store } = await open();
      backend.overrides.check_for_updates = () => at({ state: 'upToDate', manual: true });

      await store.checkForUpdates();

      expect(store.updates.state).toBe('upToDate');
    });
  });

  it('has nothing to dismiss when nothing was found', async () => {
    const { store } = await open();

    store.dismissUpdate();

    expect(store.offerDismissed).toBe(false);
  });
});
