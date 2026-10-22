import { emit } from '@tauri-apps/api/event';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { argsOf, renderApp } from '../../test-utils';
import type { UpdateStatus } from '../api';
import { offUpdateStatus } from '../defaults';
import type { MockBackendInit } from '../mock-ipc';

const RELEASE_PAGE = 'https://github.com/Roboroads/repeat-that/releases/tag/v0.2.0';
const offer = { version: '0.2.0', date: null, notes: 'Fixes a crash on start.', releaseUrl: RELEASE_PAGE };

const QUESTION = 'Check for updates automatically?';
const question = () => screen.queryByRole('complementary', { name: QUESTION });

/** An answered question and an updater that installs, which is the quiet start of every test but the question's. */
const ANSWERED: MockBackendInit = { settings: { checkForUpdates: true }, updates: { mode: 'install' } };

function status(change: Partial<UpdateStatus>): UpdateStatus {
  return { ...offUpdateStatus('0.1.0'), mode: 'install', modeReason: null, ...change };
}

/** Shows the page a status, as the backend's `update-status` event does. */
async function receive(change: Partial<UpdateStatus>) {
  await emit('update-status', status(change));
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('the consent question', () => {
  it('asks what is sent, to whom, and where to change the answer', async () => {
    await renderApp({ updates: { mode: 'install' } });

    const ask = screen.getByRole('complementary', { name: QUESTION });
    expect(ask).toHaveTextContent('Once a day Repeat That asks GitHub whether a new version is out.');
    expect(ask).toHaveTextContent('GitHub sees your IP address; nothing else is sent.');
    expect(ask).toHaveTextContent('You can change this later under General.');
    expect(within(ask).getByRole('button', { name: 'Check automatically' })).toBeInTheDocument();
    expect(within(ask).getByRole('button', { name: 'Not now' })).toBeInTheDocument();
  });

  it('is asked in notify mode too, where the check only fetches the version', async () => {
    await renderApp({ updates: { mode: 'notify', modeReason: 'Update from the download page.' } });

    expect(question()).toBeInTheDocument();
  });

  it.each(['external', 'off'] as const)('is not asked in mode %s, which never checks', async (mode) => {
    await renderApp({ updates: { mode, modeReason: 'Updates come from somewhere else.' } });

    expect(question()).not.toBeInTheDocument();
  });

  it('is not asked in a build without an updater, nor when the backend has no update commands', async () => {
    await renderApp();
    expect(question()).not.toBeInTheDocument();
  });

  it('does not come back after either answer', async () => {
    await renderApp({ settings: { checkForUpdates: false }, updates: { mode: 'install' } });
    expect(question()).not.toBeInTheDocument();
  });

  it('turns automatic checks on with "Check automatically", and goes away', async () => {
    const { backend } = await renderApp({ updates: { mode: 'install' } });

    await fireEvent.click(screen.getByRole('button', { name: 'Check automatically' }));

    await waitFor(() => expect(question()).not.toBeInTheDocument());
    expect(argsOf(backend, 'update_settings')).toContainEqual({ patch: { checkForUpdates: true } });
    expect(backend.settings.checkForUpdates).toBe(true);
  });

  it('turns them off with "Not now", and goes away', async () => {
    const { backend } = await renderApp({ updates: { mode: 'install' } });

    await fireEvent.click(screen.getByRole('button', { name: 'Not now' }));

    await waitFor(() => expect(question()).not.toBeInTheDocument());
    expect(argsOf(backend, 'update_settings')).toContainEqual({ patch: { checkForUpdates: false } });
    expect(backend.settings.checkForUpdates).toBe(false);
  });

  it('comes back, with the error, when the answer could not be saved', async () => {
    const { backend } = await renderApp({ updates: { mode: 'install' } });
    backend.overrides.update_settings = () => {
      throw { kind: 'backend', message: 'The settings file is read-only' };
    };

    await fireEvent.click(screen.getByRole('button', { name: 'Not now' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('The settings file is read-only');
    await waitFor(() => expect(question()).toBeInTheDocument());
  });
});

describe('an update that was found', () => {
  it('names the version and the one in use, and has nothing else for a check that found nothing', async () => {
    await renderApp(ANSWERED);
    expect(screen.queryByRole('complementary', { name: /is available/ })).not.toBeInTheDocument();

    await receive({ state: 'checking' });
    await receive({ state: 'upToDate' });
    await receive({ state: 'error', error: "Couldn't read the update information" });

    expect(screen.queryByRole('complementary', { name: /available|Downloading|Installing|install/ })).not.toBeInTheDocument();

    await receive({ state: 'available', available: offer });
    const banner = await screen.findByRole('complementary', { name: 'Version 0.2.0 is available' });
    expect(banner).toHaveTextContent('You have 0.1.0.');
  });

  it('puts the release notes in a "What\'s new" box, as text and never as markup', async () => {
    const { container } = await renderApp(ANSWERED);
    const notes = '- Faster replays\n<script>document.title = "pwned"</script>\n<img src=x onerror="document.title = 1">';

    await receive({ state: 'available', available: { ...offer, notes } });

    const banner = await screen.findByRole('complementary', { name: 'Version 0.2.0 is available' });
    const details = banner.querySelector('details') as HTMLDetailsElement;
    expect(within(details).getByText("What's new")).toBeInTheDocument();
    expect(details.open).toBe(false);
    expect(details.querySelector('pre')?.textContent).toBe(notes);
    expect(container.querySelector('main script, main img[src="x"]')).toBeNull();
    expect(document.title).not.toBe('pwned');
  });

  it('has no "What\'s new" box when the release came without notes', async () => {
    await renderApp(ANSWERED);

    await receive({ state: 'available', available: { ...offer, notes: '  \n' } });

    const banner = await screen.findByRole('complementary', { name: 'Version 0.2.0 is available' });
    expect(within(banner).queryByText("What's new")).not.toBeInTheDocument();
  });

  it('offers to install in mode install, and starts the install', async () => {
    const { backend } = await renderApp(ANSWERED);
    await backend.handle('check_for_updates', {});

    const banner = await screen.findByRole('complementary', { name: 'Version 99.0.0 is available' });
    expect(within(banner).queryByRole('button', { name: 'Download page' })).not.toBeInTheDocument();
    await fireEvent.click(within(banner).getByRole('button', { name: 'Install and restart' }));

    await waitFor(() => expect(argsOf(backend, 'install_update')).toHaveLength(1));
    expect(await screen.findByRole('complementary', { name: 'Downloading version 99.0.0' })).toBeInTheDocument();
  });

  it('has no button to press again once the install was accepted, even before the backend says so', async () => {
    const { backend } = await renderApp(ANSWERED);
    await backend.handle('check_for_updates', {});
    backend.overrides.install_update = () => null;
    const install = await screen.findByRole('button', { name: 'Install and restart' });

    await fireEvent.click(install);

    expect(await screen.findByRole('complementary', { name: 'Downloading version 99.0.0' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Install and restart' })).not.toBeInTheDocument();
    expect(argsOf(backend, 'install_update')).toHaveLength(1);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('shows the download, then the install', async () => {
    const { backend } = await renderApp(ANSWERED);
    await backend.handle('check_for_updates', {});
    await fireEvent.click(await screen.findByRole('button', { name: 'Install and restart' }));

    const downloading = await screen.findByRole('complementary', { name: 'Downloading version 99.0.0' });
    expect(within(downloading).getByRole('progressbar', { name: 'Download progress' })).toHaveProperty('value', 0);
    expect(within(downloading).queryByRole('button')).not.toBeInTheDocument();

    backend.tick(1000);
    await waitFor(() => expect(within(downloading).getByText('50%')).toBeInTheDocument());
    expect(within(downloading).getByRole('progressbar')).toHaveProperty('value', 0.5);

    backend.tick(1000);
    const installing = await screen.findByRole('complementary', { name: 'Installing version 99.0.0' });
    expect(installing).toHaveTextContent("Repeat That restarts when it's done.");
    expect(within(installing).queryByRole('progressbar')).not.toBeInTheDocument();
  });

  it('shows an indeterminate bar while the size of the download is not known', async () => {
    await renderApp(ANSWERED);

    await receive({ state: 'downloading', available: offer, progress: null });

    const bar = await screen.findByRole('progressbar', { name: 'Download progress' });
    expect(bar).not.toHaveAttribute('value');
    expect(screen.getByRole('complementary', { name: 'Downloading version 0.2.0' })).not.toHaveTextContent('%');
  });

  it('keeps showing a download when the user had pressed "Later" on that version', async () => {
    await renderApp(ANSWERED);
    await receive({ state: 'available', available: offer });
    await fireEvent.click(await screen.findByRole('button', { name: 'Later' }));

    await receive({ state: 'downloading', available: offer, progress: 0.1 });

    expect(await screen.findByRole('complementary', { name: 'Downloading version 0.2.0' })).toBeInTheDocument();
  });

  it('hides the banner with "Later" for that version only', async () => {
    await renderApp(ANSWERED);
    await receive({ state: 'available', available: offer });

    await fireEvent.click(await screen.findByRole('button', { name: 'Later' }));

    expect(screen.queryByRole('complementary', { name: 'Version 0.2.0 is available' })).not.toBeInTheDocument();
    // The same version found again (the daily check) stays hidden.
    await receive({ state: 'checking', available: offer });
    await receive({ state: 'available', available: offer, lastCheckedAt: 1 });
    expect(screen.queryByRole('complementary', { name: 'Version 0.2.0 is available' })).not.toBeInTheDocument();
    // A newer one is offered.
    await receive({ state: 'available', available: { ...offer, version: '0.3.0' } });
    expect(await screen.findByRole('complementary', { name: 'Version 0.3.0 is available' })).toBeInTheDocument();
  });

  it('opens the release page outside the window from "Release notes on GitHub"', async () => {
    const open = vi.spyOn(window, 'open').mockReturnValue(null);
    await renderApp(ANSWERED);
    await receive({ state: 'available', available: offer });

    const link = await screen.findByRole('link', { name: 'Release notes on GitHub' });
    expect(link).toHaveAttribute('href', RELEASE_PAGE);
    await fireEvent.click(link);

    expect(open).toHaveBeenCalledWith(RELEASE_PAGE, '_blank', 'noopener,noreferrer');
  });

  it('has no link to a release page that is not a web address', async () => {
    await renderApp(ANSWERED);

    await receive({ state: 'available', available: { ...offer, releaseUrl: '' } });

    const banner = await screen.findByRole('complementary', { name: 'Version 0.2.0 is available' });
    expect(within(banner).queryByRole('link')).not.toBeInTheDocument();
    expect(within(banner).getByRole('button', { name: 'Install and restart' })).toBeInTheDocument();
  });
});

describe('an update that only builds of another kind can install', () => {
  const NOTIFY: MockBackendInit = {
    settings: { checkForUpdates: true },
    updates: { mode: 'notify', modeReason: 'Install updates from the download page or your package manager.' },
  };

  it('sends the user to the download page instead of installing', async () => {
    const open = vi.spyOn(window, 'open').mockReturnValue(null);
    const { backend } = await renderApp(NOTIFY);
    await backend.handle('check_for_updates', {});

    const banner = await screen.findByRole('complementary', { name: 'Version 99.0.0 is available' });
    expect(banner).toHaveTextContent('Install updates from the download page or your package manager.');
    expect(within(banner).queryByRole('button', { name: 'Install and restart' })).not.toBeInTheDocument();
    expect(within(banner).queryByRole('link', { name: 'Release notes on GitHub' })).not.toBeInTheDocument();
    await fireEvent.click(within(banner).getByRole('button', { name: 'Download page' }));

    expect(open).toHaveBeenCalledWith(
      'https://github.com/Roboroads/repeat-that/releases/tag/v99.0.0',
      '_blank',
      'noopener,noreferrer',
    );
    expect(argsOf(backend, 'install_update')).toHaveLength(0);
  });

  it('can still be put off with "Later"', async () => {
    await renderApp(NOTIFY);
    await receive({ mode: 'notify', modeReason: 'x', state: 'available', available: offer });

    await fireEvent.click(await screen.findByRole('button', { name: 'Later' }));

    expect(screen.queryByRole('complementary', { name: /is available/ })).not.toBeInTheDocument();
  });
});

describe('an install that failed', () => {
  it('says why, links to the download page, and can be put off', async () => {
    const open = vi.spyOn(window, 'open').mockReturnValue(null);
    await renderApp(ANSWERED);

    await receive({ state: 'error', available: offer, error: "Couldn't verify the download" });

    const banner = await screen.findByRole('complementary', { name: "Couldn't install version 0.2.0" });
    expect(banner).toHaveTextContent("Couldn't verify the download");
    expect(within(banner).queryByRole('button', { name: 'Install and restart' })).not.toBeInTheDocument();
    await fireEvent.click(within(banner).getByRole('button', { name: 'Download page' }));
    expect(open).toHaveBeenCalledWith(RELEASE_PAGE, '_blank', 'noopener,noreferrer');

    await fireEvent.click(within(banner).getByRole('button', { name: 'Later' }));
    expect(screen.queryByRole('complementary', { name: /install/ })).not.toBeInTheDocument();
  });

  it('shows a failed download from the mock the same way', async () => {
    const { backend } = await renderApp({ ...ANSWERED, updates: { mode: 'install', outcome: 'fail-download' } });
    await backend.handle('check_for_updates', {});
    await fireEvent.click(await screen.findByRole('button', { name: 'Install and restart' }));

    backend.tick(1000);

    const banner = await screen.findByRole('complementary', { name: "Couldn't install version 99.0.0" });
    expect(banner).toHaveTextContent('The connection was lost during the download');
  });
});

describe('a check that failed', () => {
  it('stays out of the banner, which is for an update to act on', async () => {
    const { backend } = await renderApp({ ...ANSWERED, updates: { mode: 'install', outcome: 'fail-check' } });

    await backend.handle('check_for_updates', {});

    await waitFor(() => expect(screen.getByRole('region', { name: 'General' })).toHaveTextContent("Couldn't check"));
    expect(screen.queryByRole('complementary', { name: /install|available/ })).not.toBeInTheDocument();
  });
});
