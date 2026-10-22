import { emit } from '@tauri-apps/api/event';
import { fireEvent, screen, waitFor } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import { argsOf, renderApp } from '../../test-utils';

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
