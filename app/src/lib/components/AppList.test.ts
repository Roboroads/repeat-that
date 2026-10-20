import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AppEntry } from '../api';
import { argsOf, holdAnswers, renderApp, setVisibility } from '../../test-utils';

const discord: AppEntry = { pid: 4120, name: 'Discord', exePath: 'C:\\Apps\\Discord.exe', icon: 'data:image/png;base64,AAAA' };
const teams: AppEntry = { pid: 8844, name: 'Teams', exePath: 'C:\\Apps\\Teams.exe', icon: null };
const target = { exePath: discord.exePath, name: discord.name };

afterEach(() => {
  vi.useRealTimers();
});

describe('app list', () => {
  it('explains an empty list', async () => {
    await renderApp({ apps: [] });

    expect(
      await screen.findByText('No apps are playing audio right now. Start your call app, and it shows up here.'),
    ).toBeInTheDocument();
    expect(screen.queryByRole('radio', { name: 'Discord' })).not.toBeInTheDocument();
  });

  it('lists the apps with their icons', async () => {
    await renderApp({ apps: [discord, teams] });

    const group = await screen.findByRole('radiogroup', { name: 'App to capture' });
    const radios = within(group).getAllByRole('radio');
    expect(radios.map((r) => r.closest('label')?.textContent?.trim())).toEqual(['Discord', 'Teams']);
    expect(group.querySelector('img')).toHaveAttribute('src', discord.icon);
  });

  it('uses a generic icon when an app has none', async () => {
    await renderApp({ apps: [teams] });

    const row = (await screen.findByRole('radio', { name: 'Teams' })).closest('label');
    expect(within(row as HTMLElement).getByTestId('generic-icon')).toBeInTheDocument();
  });

  it('falls back to the generic icon when the image cannot be shown', async () => {
    await renderApp({ apps: [discord] });
    const row = (await screen.findByRole('radio', { name: 'Discord' })).closest('label') as HTMLElement;
    expect(within(row).queryByTestId('generic-icon')).not.toBeInTheDocument();

    await fireEvent.error(row.querySelector('img') as HTMLImageElement);

    expect(within(row).getByTestId('generic-icon')).toBeInTheDocument();
  });

  it('selects an app through select_app, without sending its icon', async () => {
    const { backend } = await renderApp({ apps: [discord, teams] });

    await fireEvent.click(await screen.findByRole('radio', { name: 'Teams' }));

    await waitFor(() => expect(screen.getByRole('radio', { name: 'Teams' })).toBeChecked());
    expect(argsOf(backend, 'select_app')).toEqual([{ app: { ...teams, icon: null } }]);
    expect(backend.settings.target).toEqual({ exePath: teams.exePath, name: 'Teams' });
  });

  it('waits for a settings save that is still out before switching the app', async () => {
    const { backend } = await renderApp({ apps: [discord, teams], settings: { target, firstRunDone: true } });
    await screen.findByRole('radio', { name: 'Teams' });
    const release = holdAnswers(backend, 'update_settings');

    await fireEvent.change(screen.getByRole('slider', { name: 'Buffer length' }), { target: { value: '45' } });
    await fireEvent.click(screen.getByRole('radio', { name: 'Teams' }));

    await waitFor(() => expect(argsOf(backend, 'update_settings')).toHaveLength(1));
    expect(argsOf(backend, 'select_app')).toEqual([]);
    expect(screen.getByRole('radio', { name: 'Teams' })).toBeChecked();

    // The save's answer was written before the switch and still names the old app. It must not undo the switch.
    release();
    await waitFor(() => expect(argsOf(backend, 'select_app')).toHaveLength(1));
    await waitFor(() => expect(backend.settings.target?.name).toBe('Teams'));
    await waitFor(() => expect(screen.getByRole('slider', { name: 'Buffer length' })).toHaveValue('45'));
    expect(screen.getByRole('radio', { name: 'Teams' })).toBeChecked();
    expect(screen.getByRole('radio', { name: 'Discord' })).not.toBeChecked();
  });

  it('shows the current target selected', async () => {
    await renderApp({ apps: [discord, teams], settings: { target } });

    expect(await screen.findByRole('radio', { name: 'Discord' })).toBeChecked();
    expect(screen.getByRole('radio', { name: 'Teams' })).not.toBeChecked();
  });

  it('pins a remembered target that is not running on top, as waiting', async () => {
    await renderApp({ apps: [teams], settings: { target, firstRunDone: true } });

    const group = await screen.findByRole('radiogroup', { name: 'App to capture' });
    const radios = within(group).getAllByRole('radio');
    expect(radios[0]).toHaveAccessibleName('Discord · waiting');
    expect(radios[0]).toBeChecked();
    expect(radios[1]).toHaveAccessibleName('Teams');
  });

  it('moves the target into place when the app starts', async () => {
    const { backend } = await renderApp({ apps: [teams], settings: { target, firstRunDone: true } });
    await screen.findByRole('radio', { name: 'Discord · waiting' });

    backend.apps = [teams, discord];
    await fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));

    expect(await screen.findByRole('radio', { name: 'Discord' })).toBeChecked();
    expect(screen.queryByRole('radio', { name: /waiting/ })).not.toBeInTheDocument();
  });

  it('refreshes with the Refresh button', async () => {
    const { backend } = await renderApp({ apps: [teams] });
    await screen.findByRole('radio', { name: 'Teams' });
    const before = argsOf(backend, 'list_apps').length;

    backend.apps = [teams, discord];
    await fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));

    expect(await screen.findByRole('radio', { name: 'Discord' })).toBeInTheDocument();
    expect(argsOf(backend, 'list_apps').length).toBe(before + 1);
  });

  it('refreshes every 2 s while the window is visible, and not while it is hidden', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    const { backend } = await renderApp({ apps: [teams] });
    await screen.findByRole('radio', { name: 'Teams' });
    const listed = () => argsOf(backend, 'list_apps').length;
    const first = listed();

    await vi.advanceTimersByTimeAsync(1900);
    expect(listed()).toBe(first);
    await vi.advanceTimersByTimeAsync(100);
    expect(listed()).toBe(first + 1);
    await vi.advanceTimersByTimeAsync(4000);
    expect(listed()).toBe(first + 3);

    setVisibility('hidden');
    await vi.advanceTimersByTimeAsync(20_000);
    expect(listed()).toBe(first + 3);

    setVisibility('visible');
    await vi.advanceTimersByTimeAsync(0);
    expect(listed()).toBe(first + 4); // right away on coming back
    await vi.advanceTimersByTimeAsync(2000);
    expect(listed()).toBe(first + 5);
  });

  it('stops refreshing once the page is gone', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    const { backend, unmount } = await renderApp({ apps: [teams] });
    await screen.findByRole('radio', { name: 'Teams' });

    unmount();
    const before = argsOf(backend, 'list_apps').length;
    await vi.advanceTimersByTimeAsync(10_000);

    expect(argsOf(backend, 'list_apps').length).toBe(before);
  });

  it('keeps the old list when a refresh fails', async () => {
    const { backend } = await renderApp({ apps: [teams] });
    await screen.findByRole('radio', { name: 'Teams' });
    backend.overrides.list_apps = () => {
      throw 'enumeration failed';
    };

    await fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));

    await waitFor(() => expect(argsOf(backend, 'list_apps').length).toBeGreaterThan(1));
    expect(screen.getByRole('radio', { name: 'Teams' })).toBeInTheDocument();
    expect(await screen.findByText("Couldn't list the apps: enumeration failed")).toBeInTheDocument();
  });

  it('shows the error, not the empty state, when the list cannot be read', async () => {
    const { backend } = await renderApp({ apps: [] }, (b) => {
      b.overrides.list_apps = () => {
        throw 'enumeration failed';
      };
    });

    expect(await screen.findByText("Couldn't list the apps: enumeration failed")).toBeInTheDocument();
    expect(screen.queryByText(/No apps are playing audio/)).not.toBeInTheDocument();

    delete backend.overrides.list_apps;
    await fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));

    expect(await screen.findByText(/No apps are playing audio/)).toBeInTheDocument();
    expect(screen.queryByText(/Couldn't list the apps/)).not.toBeInTheDocument();
  });
});
