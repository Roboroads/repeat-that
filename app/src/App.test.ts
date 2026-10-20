import { emit } from '@tauri-apps/api/event';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import App from './App.svelte';
import css from './app.css?raw';
import type { AppEntry, Status } from './lib/api';
import { defaultSettings } from './lib/defaults';
import { mockApps, scenarioInit } from './lib/mock-ipc';
import { argsOf, installBackend, renderApp } from './test-utils';

const discord: AppEntry = { pid: 4120, name: 'Discord', exePath: 'C:\\Apps\\Discord.exe', icon: null };
const target = { exePath: discord.exePath, name: discord.name };

describe('loading', () => {
  it('shows the app name and the settings once they arrived', async () => {
    await renderApp();

    expect(screen.getByRole('heading', { level: 1, name: 'Repeat that?!' })).toBeInTheDocument();
    expect(screen.getByRole('slider', { name: 'Buffer length' })).toHaveValue('30');
    expect(screen.getByRole('radio', { name: 'Normal' })).toBeChecked();
    expect(screen.getByRole('switch', { name: /Start with|Start at login/ })).not.toBeChecked();
  });

  it('shows what the backend stored, not the defaults', async () => {
    await renderApp({
      settings: {
        bufferSeconds: 90,
        sensitivity: 'high',
        replaySpeed: 1.25,
        catchUpSpeed: 2,
        duck: { enabled: false, level: 0.5 },
        autostart: true,
      },
    });

    expect(screen.getByRole('slider', { name: 'Buffer length' })).toHaveValue('90');
    expect(screen.getByRole('radio', { name: 'High' })).toBeChecked();
    expect(screen.getByRole('slider', { name: 'Replay speed' })).toHaveValue('1.25');
    expect(screen.getByRole('slider', { name: 'Catch-up speed' })).toHaveValue('2');
    expect(screen.getByRole('switch', { name: /Lower the app's volume/ })).not.toBeChecked();
    expect(screen.getByRole('slider', { name: 'Volume during replay' })).toHaveValue('50');
    expect(screen.getByRole('switch', { name: /Start with|Start at login/ })).toBeChecked();
  });

  it('says so while loading and when the backend does not answer, and retries', async () => {
    const backend = installBackend();
    let healthy = false;
    backend.overrides.get_settings = () => {
      if (!healthy) throw 'backend is down';
      return structuredClone(backend.settings);
    };

    render(App);
    expect(screen.getByRole('status')).toHaveTextContent('Loading');

    expect(await screen.findByRole('alert')).toHaveTextContent('backend is down');

    healthy = true;
    await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(screen.getByRole('slider', { name: 'Buffer length' })).toBeInTheDocument());
  });

  it('shows the app icon from its source SVG', async () => {
    const { container } = await renderApp();

    // Vite turns the imported file into a URL (a data URI in tests, a hashed file in the build), not a public path.
    const src = container.querySelector('img.logo')?.getAttribute('src');
    expect(src).toMatch(/svg/);
    expect(src).not.toBe('/icon.svg');
  });

  it('applies a settings-changed event from the backend', async () => {
    await renderApp();

    await emit('settings-changed', { ...defaultSettings(), bufferSeconds: 60 });

    await waitFor(() => expect(screen.getByRole('slider', { name: 'Buffer length' })).toHaveValue('60'));
  });
});

describe('status bar', () => {
  async function statusBar(init: Parameters<typeof renderApp>[0]) {
    const view = await renderApp(init);
    return { ...view, bar: screen.getByRole('region', { name: 'Status' }) };
  }

  it('says no app is selected and disables the controls that need one', async () => {
    const { bar } = await statusBar({});

    expect(await within(bar).findByText('No app selected')).toBeInTheDocument();
    expect(within(bar).getByRole('button', { name: 'Replay' })).toBeDisabled();
    expect(within(bar).getByRole('button', { name: 'Pause capture' })).toBeDisabled();
    expect(within(bar).getByRole('button', { name: 'Stop' })).toBeDisabled();
  });

  it('shows what is captured with its icon and the speech buffered', async () => {
    const icon = 'data:image/png;base64,AAAA';
    const { backend, bar } = await statusBar({ apps: [{ ...discord, icon }], settings: { target } });

    backend.tick(40_000);

    expect(await within(bar).findByText('Capturing Discord')).toBeInTheDocument();
    expect(await within(bar).findByText('24 s of speech buffered')).toBeInTheDocument();
    await waitFor(() => expect(bar.querySelector('img')).toHaveAttribute('src', icon));
  });

  it('shows waiting for a target that is not running', async () => {
    const { bar } = await statusBar({ apps: [], settings: { target } });

    expect(await within(bar).findByText('Waiting for Discord')).toBeInTheDocument();
    expect(within(bar).getByRole('button', { name: 'Replay' })).toBeEnabled();
  });

  it('shows an error and why capturing is unsupported', async () => {
    const { bar } = await statusBar({
      apps: [discord],
      settings: { target },
      statusOverrides: { capture: { state: 'error', message: 'Access denied' } },
    });

    expect(await within(bar).findByText("Can't capture Discord")).toBeInTheDocument();
    expect(within(bar).getByText('Access denied')).toBeInTheDocument();
  });

  it('shows unsupported', async () => {
    const { bar } = await statusBar({
      statusOverrides: { capture: { state: 'unsupported', message: "Capturing apps isn't supported on this OS yet" } },
    });

    expect(await within(bar).findByText("Capturing apps isn't supported on this OS yet")).toBeInTheDocument();
    expect(within(bar).getByRole('button', { name: 'Replay' })).toBeDisabled();
  });

  it('replays, stops and pauses through the backend', async () => {
    const { backend, bar } = await statusBar({ apps: [discord], settings: { target } });
    backend.tick(40_000);
    await within(bar).findByText('24 s of speech buffered');

    await fireEvent.click(within(bar).getByRole('button', { name: 'Replay' }));
    expect(await within(bar).findByText(/^Replaying/)).toBeInTheDocument();
    await fireEvent.click(within(bar).getByRole('button', { name: 'Stop' }));
    await waitFor(() => expect(within(bar).queryByText(/^Replaying/)).not.toBeInTheDocument());

    await fireEvent.click(within(bar).getByRole('button', { name: 'Pause capture' }));
    expect(await within(bar).findByText('Paused')).toBeInTheDocument();
    expect(within(bar).getByRole('button', { name: 'Pause capture' })).toHaveAttribute('aria-pressed', 'true');
    await fireEvent.click(within(bar).getByRole('button', { name: 'Pause capture' }));
    expect(await within(bar).findByText('Capturing Discord')).toBeInTheDocument();

    expect(argsOf(backend, 'replay')).toHaveLength(1);
    expect(argsOf(backend, 'stop')).toHaveLength(1);
    expect(argsOf(backend, 'set_paused')).toEqual([{ paused: true }, { paused: false }]);
  });

  it('shows how far behind live a replay is', async () => {
    const { backend, bar } = await statusBar({ apps: [discord], settings: { target } });
    backend.tick(40_000);
    backend.handle('replay', {});

    expect(await within(bar).findByText(/^Replaying · \d+ s behind live$/)).toBeInTheDocument();
  });

  it('warns about an output that fell back, no output at all, and the detector fallback', async () => {
    const { backend } = await statusBar({ apps: [discord], settings: { target } });

    const status = backend.status();
    await emit('status', {
      ...status,
      output: { deviceName: 'Speakers', fellBack: true, error: null },
      vadFallback: true,
    } satisfies Status);
    expect(await screen.findByText(/replays use the system default \(Speakers\)/)).toBeInTheDocument();
    expect(screen.getByText(/speech detection model did not load/)).toBeInTheDocument();

    await emit('status', { ...status, output: { deviceName: null, fellBack: false, error: 'No output device' } });
    expect(await screen.findByText('Replays have nowhere to play: No output device')).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByText(/speech detection model did not load/)).not.toBeInTheDocument());
  });

  it('shows the warnings of the problems scenario', async () => {
    await renderApp(scenarioInit('problems'));

    expect(await screen.findByText(/replays use the system default/)).toBeInTheDocument();
    expect(screen.getByText(/speech detection model did not load/)).toBeInTheDocument();
  });
});

describe('first run', () => {
  it('shows the banner until an app is picked, then hides it and ends the first run', async () => {
    const { backend } = await renderApp({ apps: mockApps() });

    expect(screen.getByText('Pick the app you want to catch up on')).toBeInTheDocument();

    await fireEvent.click(await screen.findByRole('radio', { name: 'Discord' }));

    await waitFor(() => expect(screen.queryByText('Pick the app you want to catch up on')).not.toBeInTheDocument());
    expect(backend.settings.firstRunDone).toBe(true);
    expect(argsOf(backend, 'update_settings')).toContainEqual({ patch: { firstRunDone: true } });
  });

  it('is not shown once the first run is done', async () => {
    await renderApp({ apps: mockApps(), settings: { firstRunDone: true } });

    expect(screen.queryByText('Pick the app you want to catch up on')).not.toBeInTheDocument();
  });

  it('is not shown when a target exists even if the first run is not marked done', async () => {
    await renderApp({ apps: mockApps(), settings: { target, firstRunDone: false } });

    expect(screen.queryByText('Pick the app you want to catch up on')).not.toBeInTheDocument();
  });
});

describe('errors', () => {
  it('shows a failed command and lets it be dismissed', async () => {
    const { backend } = await renderApp({ apps: [discord], settings: { target } });
    backend.overrides.replay = () => {
      throw { kind: 'backend', message: 'The audio engine is not running' };
    };

    await fireEvent.click(screen.getByRole('button', { name: 'Replay' }));

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('The audio engine is not running');
    await fireEvent.click(within(alert).getByRole('button', { name: 'Dismiss' }));
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('puts back the previous value when a setting is rejected', async () => {
    const { backend } = await renderApp();
    backend.overrides.update_settings = () => {
      throw { kind: 'invalid', message: 'Nope' };
    };

    await fireEvent.click(screen.getByRole('radio', { name: 'High' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('Nope');
    await waitFor(() => expect(screen.getByRole('radio', { name: 'Normal' })).toBeChecked());
  });
});

describe('theme', () => {
  it('follows the system theme through prefers-color-scheme', () => {
    expect(css).toMatch(/color-scheme:\s*light dark/);
    const dark = /@media \(prefers-color-scheme: dark\)\s*{([\s\S]*?)\n}\n/.exec(css);
    expect(dark?.[1]).toContain('--bg:');
    expect(dark?.[1]).toContain('--text:');
    expect(dark?.[1]).toContain('--surface:');
  });

  it('uses the system font and no framework', () => {
    expect(css).toMatch(/font-family:\s*system-ui/);
    expect(css).not.toMatch(/tailwind|bootstrap/i);
  });
});
