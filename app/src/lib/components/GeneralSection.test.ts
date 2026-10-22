import { emit } from '@tauri-apps/api/event';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import { argsOf, renderApp } from '../../test-utils';
import type { UpdateStatus } from '../api';
import { offUpdateStatus } from '../defaults';
import type { MockBackendInit } from '../mock-ipc';

const NOTICE = /No tray on this desktop/;
const quitButton = () => screen.queryByRole('button', { name: 'Quit Repeat That' });

describe('general section without a tray', () => {
  it('shows nothing about a tray when there is one', async () => {
    await renderApp();

    expect(screen.queryByText(NOTICE)).not.toBeInTheDocument();
    expect(quitButton()).not.toBeInTheDocument();
  });

  it('says there is no tray and how to get one', async () => {
    await renderApp({ statusOverrides: { trayAvailable: false } });

    const notice = screen.getByText(NOTICE);
    expect(notice).toHaveTextContent('Open Repeat That from your app menu.');
    expect(notice).toHaveTextContent('AppIndicator and KStatusNotifierItem Support extension');
  });

  it('has a button that quits the app, which the tray menu would have', async () => {
    const { backend } = await renderApp({ statusOverrides: { trayAvailable: false } });

    await fireEvent.click(screen.getByRole('button', { name: 'Quit Repeat That' }));

    await waitFor(() => expect(argsOf(backend, 'quit_app')).toHaveLength(1));
  });

  it('says so when quitting does not work', async () => {
    const { backend } = await renderApp({ statusOverrides: { trayAvailable: false } });
    backend.overrides.quit_app = () => {
      throw { kind: 'backend', message: 'The app could not quit' };
    };

    await fireEvent.click(screen.getByRole('button', { name: 'Quit Repeat That' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('The app could not quit');
  });

  it('shows the notice when the tray host goes away and drops it when one shows up', async () => {
    const view = await renderApp();
    expect(screen.queryByText(NOTICE)).not.toBeInTheDocument();

    await emit('status', { ...view.backend.status(), trayAvailable: false });
    expect(await screen.findByText(NOTICE)).toBeInTheDocument();
    expect(quitButton()).toBeInTheDocument();

    // The AppIndicator extension was turned on.
    await emit('status', { ...view.backend.status(), trayAvailable: true });
    await waitFor(() => expect(screen.queryByText(NOTICE)).not.toBeInTheDocument());
    expect(quitButton()).not.toBeInTheDocument();
  });

  it('says the autostart entry starts the app without a window, with or without a tray', async () => {
    await renderApp();
    expect(screen.getByText('Starts in the tray, without opening this window.')).toBeInTheDocument();
  });

  it('does not promise a tray to start in when there is none', async () => {
    await renderApp({ statusOverrides: { trayAvailable: false } });

    expect(screen.getByText('Starts without opening this window.')).toBeInTheDocument();
    expect(screen.queryByText('Starts in the tray, without opening this window.')).not.toBeInTheDocument();
  });
});

describe('general section updates', () => {
  const INSTALL: MockBackendInit = { settings: { checkForUpdates: true }, updates: { mode: 'install' } };
  const updates = () => screen.getByRole('group', { name: 'Updates' });
  const autoSwitch = () => within(updates()).getByRole('switch', { name: 'Check for updates automatically' });
  const checkNow = () => within(updates()).getByRole('button', { name: 'Check now' });

  it('has a switch that shows the setting, off while the question is not answered', async () => {
    await renderApp({ updates: { mode: 'install' } });
    expect(autoSwitch()).not.toBeChecked();
  });

  it.each([
    [true, true],
    [false, false],
  ])('shows the answer %s as a switch that is %s', async (answer, checked) => {
    await renderApp({ settings: { checkForUpdates: answer }, updates: { mode: 'install' } });
    expect(autoSwitch().matches(':checked')).toBe(checked);
  });

  it('turns automatic checks on and off', async () => {
    const { backend } = await renderApp(INSTALL);

    await fireEvent.click(autoSwitch());
    await waitFor(() => expect(backend.settings.checkForUpdates).toBe(false));
    await fireEvent.click(autoSwitch());
    await waitFor(() => expect(backend.settings.checkForUpdates).toBe(true));

    expect(argsOf(backend, 'update_settings')).toEqual([
      { patch: { checkForUpdates: false } },
      { patch: { checkForUpdates: true } },
    ]);
  });

  it('answers the consent question too when the switch is the first answer', async () => {
    await renderApp({ updates: { mode: 'install' } });
    const asked = () => screen.queryByRole('complementary', { name: 'Check for updates automatically?' });
    expect(asked()).toBeInTheDocument();

    await fireEvent.click(autoSwitch());

    await waitFor(() => expect(asked()).not.toBeInTheDocument());
  });

  it('names the version in use and says it was never checked', async () => {
    await renderApp(INSTALL);

    expect(updates()).toHaveTextContent('Version 0.1.0 · last checked never');
  });

  it('checks now, whatever the setting says, and then shows when', async () => {
    const { backend } = await renderApp({ settings: { checkForUpdates: false }, updates: { mode: 'install', outcome: 'none' } });

    await fireEvent.click(checkNow());

    expect(await within(updates()).findByText('You have the latest version.')).toBeInTheDocument();
    expect(argsOf(backend, 'check_for_updates')).toHaveLength(1);
    expect(updates()).not.toHaveTextContent('last checked never');
    expect(updates()).toHaveTextContent(/Version 0\.1\.0 · last checked \d/);
  });

  it('is busy while it checks', async () => {
    await renderApp({ ...INSTALL, updates: { mode: 'install', outcome: 'none', checkMs: 30 } });

    await fireEvent.click(checkNow());

    expect(await within(updates()).findByText('Checking…')).toBeInTheDocument();
    expect(checkNow()).toBeDisabled();
    expect(await within(updates()).findByText('You have the latest version.')).toBeInTheDocument();
    await waitFor(() => expect(checkNow()).toBeEnabled());
  });

  it('says what went wrong with a check the user asked for, next to the button', async () => {
    await renderApp({ ...INSTALL, updates: { mode: 'install', outcome: 'fail-check' } });

    await fireEvent.click(checkNow());

    const result = await within(updates()).findByRole('status');
    await waitFor(() => expect(result).toHaveTextContent("Couldn't check for updates."));
    expect(result).toHaveTextContent("Couldn't reach GitHub to look for a new version");
  });

  it('says that an automatic check failed, without calling it a failure of something the user did', async () => {
    const { backend } = await renderApp({ ...INSTALL, updates: { mode: 'install', outcome: 'fail-check' } });

    backend.tick(5000);

    const result = await within(updates()).findByRole('status');
    await waitFor(() => expect(result).toHaveTextContent('The last automatic check failed.'));
    expect(result).toHaveTextContent("Couldn't reach GitHub to look for a new version");
  });

  it('says that a version is out, and brings back the banner that "Later" closed', async () => {
    const { backend } = await renderApp(INSTALL);
    await backend.handle('check_for_updates', {});
    const banner = () => screen.queryByRole('complementary', { name: 'Version 99.0.0 is available' });
    const result = await within(updates()).findByText('Version 99.0.0 is available.');
    expect(within(result.parentElement as HTMLElement).queryByRole('button', { name: 'Show' })).not.toBeInTheDocument();

    await fireEvent.click(within(banner() as HTMLElement).getByRole('button', { name: 'Later' }));
    expect(banner()).not.toBeInTheDocument();
    await fireEvent.click(within(updates()).getByRole('button', { name: 'Show' }));

    expect(banner()).toBeInTheDocument();
    expect(within(updates()).queryByRole('button', { name: 'Show' })).not.toBeInTheDocument();
  });

  it('says that an install failed, and where to look', async () => {
    await renderApp(INSTALL);
    const failed: UpdateStatus = {
      ...offUpdateStatus('0.1.0'),
      mode: 'install',
      modeReason: null,
      state: 'error',
      errorKind: 'install',
      error: "Couldn't verify the download",
      available: { version: '0.2.0', date: null, notes: '', releaseUrl: 'https://github.com/Roboroads/repeat-that/releases/tag/v0.2.0' },
    };

    await emit('update-status', failed);

    expect(await within(updates()).findByText("Couldn't install version 0.2.0.")).toBeInTheDocument();
  });

  it('can check, but not install, in notify mode', async () => {
    const { backend } = await renderApp({
      settings: { checkForUpdates: true },
      updates: { mode: 'notify', modeReason: 'Install updates from the download page or your package manager.' },
    });

    expect(autoSwitch()).toBeChecked();
    await fireEvent.click(checkNow());

    expect(await within(updates()).findByText('Version 99.0.0 is available.')).toBeInTheDocument();
    expect(argsOf(backend, 'check_for_updates')).toHaveLength(1);
  });

  it.each([
    ['external', 'Updates come through Flathub.'],
    ['off', 'Updates are turned off in this build.'],
  ] as const)('says why in mode %s, and offers nothing', async (mode, modeReason) => {
    await renderApp({ updates: { mode, modeReason } });

    expect(updates()).toHaveTextContent(modeReason);
    expect(within(updates()).queryByRole('switch')).not.toBeInTheDocument();
    expect(within(updates()).queryByRole('button')).not.toBeInTheDocument();
  });

  it('says the build does not check when the backend has no update commands', async () => {
    await renderApp({}, (backend) => {
      backend.overrides.get_update_status = () => {
        throw 'Command get_update_status not found';
      };
    });

    expect(updates()).toHaveTextContent("This build doesn't check for updates.");
    expect(within(updates()).queryByRole('button')).not.toBeInTheDocument();
  });

  it('still says something when a build without a reason is not checking', async () => {
    await renderApp({ updates: { mode: 'external', modeReason: null } });

    expect(updates()).toHaveTextContent("This build doesn't check for updates.");
  });
});
