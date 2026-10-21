import { emit } from '@tauri-apps/api/event';
import { screen, waitFor } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import { renderApp } from '../../test-utils';

const replay = () => screen.getByRole('button', { name: 'Replay' });
const stop = () => screen.getByRole('button', { name: 'Stop' });

describe('status bar key hints', () => {
  it('name the stored combinations with the plugin', async () => {
    await renderApp();

    expect(replay()).toHaveAttribute('title', 'Ctrl+Alt+Space');
    expect(stop()).toHaveAttribute('title', 'Ctrl+Alt+Backspace');
  });

  it('name no key for an action that has none', async () => {
    await renderApp({ settings: { hotkeys: { replay: null, stop: 'Ctrl+Alt+Backspace', catchUp: null } } });

    expect(replay()).not.toHaveAttribute('title');
    expect(stop()).toHaveAttribute('title', 'Ctrl+Alt+Backspace');
  });

  it('name the keys the desktop assigned under the portal', async () => {
    await renderApp({
      statusOverrides: {
        hotkeyBackend: 'portal',
        hotkeyTriggers: { replay: 'Meta+Shift+R', stop: null, catchUp: null },
      },
    });

    expect(replay()).toHaveAttribute('title', 'Meta+Shift+R');
    expect(stop()).not.toHaveAttribute('title');
  });

  it('name nothing without hotkeys, whatever is stored', async () => {
    await renderApp({ statusOverrides: { hotkeyBackend: 'none' } });

    expect(replay()).not.toHaveAttribute('title');
    expect(stop()).not.toHaveAttribute('title');
  });

  it('follow the backend when the desktop changes a key', async () => {
    const view = await renderApp({
      statusOverrides: {
        hotkeyBackend: 'portal',
        hotkeyTriggers: { replay: 'Meta+Shift+R', stop: null, catchUp: null },
      },
    });

    await emit('status', {
      ...view.backend.status(),
      hotkeyTriggers: { replay: 'Meta+R', stop: 'Meta+S', catchUp: null },
    });

    await waitFor(() => expect(replay()).toHaveAttribute('title', 'Meta+R'));
    expect(stop()).toHaveAttribute('title', 'Meta+S');
  });
});
