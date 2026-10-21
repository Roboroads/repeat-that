import { emit } from '@tauri-apps/api/event';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Status } from '../api';
import { argsOf, holdAnswers, renderApp } from '../../test-utils';

const replayField = () => screen.getByRole('button', { name: /^Replay hotkey/ });
const stopField = () => screen.getByRole('button', { name: /^Stop hotkey/ });
const catchUpField = () => screen.getByRole('button', { name: /^Catch up hotkey/ });

type Backend = Awaited<ReturnType<typeof renderApp>>['backend'];
const suspends = (backend: Backend) => argsOf(backend, 'suspend_hotkeys');

function press(el: HTMLElement, code: string, mods: { ctrl?: boolean; alt?: boolean; shift?: boolean } = {}) {
  return fireEvent.keyDown(el, { code, key: code, ctrlKey: mods.ctrl, altKey: mods.alt, shiftKey: mods.shift });
}

describe('hotkey recorder', () => {
  it('shows the current combinations', async () => {
    await renderApp();

    expect(replayField()).toHaveAccessibleName('Replay hotkey: Ctrl+Alt+Space');
    expect(stopField()).toHaveAccessibleName('Stop hotkey: Ctrl+Alt+Backspace');
    expect(catchUpField()).toHaveAccessibleName('Catch up hotkey: Ctrl+Alt+Enter');
  });

  it('suspends the hotkeys while recording and saves the pressed combination', async () => {
    const { backend } = await renderApp();

    await fireEvent.click(replayField());
    expect(suspends(backend)).toEqual([{ suspended: true }]);
    expect(replayField()).toHaveAccessibleName('Replay hotkey: press a combination');

    await press(replayField(), 'KeyR', { ctrl: true, alt: true });

    await waitFor(() => expect(suspends(backend)).toEqual([{ suspended: true }, { suspended: false }]));
    expect(argsOf(backend, 'update_settings')).toEqual([
      { patch: { hotkeys: { replay: 'Ctrl+Alt+R', stop: 'Ctrl+Alt+Backspace', catchUp: 'Ctrl+Alt+Enter' } } },
    ]);
    await waitFor(() => expect(replayField()).toHaveAccessibleName('Replay hotkey: Ctrl+Alt+R'));
  });

  it('cancels on Escape without saving', async () => {
    const { backend } = await renderApp();
    await fireEvent.click(stopField());

    await press(stopField(), 'Escape');

    await waitFor(() => expect(suspends(backend)).toEqual([{ suspended: true }, { suspended: false }]));
    expect(argsOf(backend, 'update_settings')).toEqual([]);
    expect(stopField()).toHaveAccessibleName('Stop hotkey: Ctrl+Alt+Backspace');
  });

  it('unbinds on Backspace', async () => {
    const { backend } = await renderApp();
    await fireEvent.click(catchUpField());

    await press(catchUpField(), 'Backspace');

    await waitFor(() =>
      expect(argsOf(backend, 'update_settings')).toEqual([
        { patch: { hotkeys: { replay: 'Ctrl+Alt+Space', stop: 'Ctrl+Alt+Backspace', catchUp: null } } },
      ]),
    );
    await waitFor(() => expect(suspends(backend)).toEqual([{ suspended: true }, { suspended: false }]));
    await waitFor(() => expect(catchUpField()).toHaveAccessibleName('Catch up hotkey: not set'));
    expect(await screen.findByText('Not set')).toBeInTheDocument();
  });

  it('treats Backspace with modifiers as a key to bind, not as unbind', async () => {
    const { backend } = await renderApp();
    await fireEvent.click(replayField());

    await press(replayField(), 'Backspace', { ctrl: true, alt: true });

    await waitFor(() =>
      expect(argsOf(backend, 'update_settings')).toEqual([
        { patch: { hotkeys: { replay: 'Ctrl+Alt+Backspace', stop: 'Ctrl+Alt+Backspace', catchUp: 'Ctrl+Alt+Enter' } } },
      ]),
    );
  });

  it('keeps recording and says what to do when the key is not usable alone', async () => {
    const { backend } = await renderApp();
    await fireEvent.click(replayField());

    await press(replayField(), 'KeyA');

    expect(await screen.findByText('Hold Ctrl or Alt together with the key.')).toBeInTheDocument();
    expect(argsOf(backend, 'update_settings')).toEqual([]);
    expect(suspends(backend)).toEqual([{ suspended: true }]);
  });

  it('shows the held modifiers while waiting for the key', async () => {
    await renderApp();
    await fireEvent.click(replayField());

    await press(replayField(), 'ControlLeft', { ctrl: true });

    expect(replayField()).toHaveTextContent('Ctrl');
  });

  it('drops a modifier from the preview when it is released', async () => {
    await renderApp();
    await fireEvent.click(replayField());
    const release = (code: string, mods: { ctrlKey?: boolean; altKey?: boolean }) =>
      fireEvent.keyUp(replayField(), { code, key: code, ...mods });

    await press(replayField(), 'ControlLeft', { ctrl: true });
    await press(replayField(), 'AltLeft', { ctrl: true, alt: true });
    expect(replayField()).toHaveTextContent('Ctrl+Alt+…');

    await release('AltLeft', { ctrlKey: true });
    expect(replayField()).toHaveTextContent('Ctrl+…');
    expect(replayField()).not.toHaveTextContent('Alt');

    await release('ControlLeft', {});
    expect(replayField()).toHaveTextContent('Press a combination');
  });

  it('shows the backend error for a conflicting combination and leaves the field as it was', async () => {
    const { backend } = await renderApp();
    await fireEvent.click(replayField());

    await press(replayField(), 'Enter', { ctrl: true, alt: true });

    const alert = await screen.findByText('Ctrl+Alt+Enter is already used for Catch up.');
    expect(alert).toBeInTheDocument();
    expect(replayField()).toHaveAccessibleName('Replay hotkey: Ctrl+Alt+Space');
    expect(backend.settings.hotkeys.replay).toBe('Ctrl+Alt+Space');
    await waitFor(() => expect(suspends(backend)).toEqual([{ suspended: true }, { suspended: false }]));
  });

  it('keeps the first binding when a second one is saved before the first was answered', async () => {
    const { backend } = await renderApp();
    const release = holdAnswers(backend, 'update_settings');

    await fireEvent.click(replayField());
    await press(replayField(), 'KeyR', { ctrl: true, alt: true });
    await fireEvent.click(stopField());
    await press(stopField(), 'KeyS', { ctrl: true, alt: true });
    await waitFor(() => expect(argsOf(backend, 'update_settings')).toHaveLength(1));
    release();

    await waitFor(() => expect(argsOf(backend, 'update_settings')).toHaveLength(2));
    expect(argsOf(backend, 'update_settings')[1]).toEqual({
      patch: { hotkeys: { replay: 'Ctrl+Alt+R', stop: 'Ctrl+Alt+S', catchUp: 'Ctrl+Alt+Enter' } },
    });
    await waitFor(() => expect(stopField()).toHaveAccessibleName('Stop hotkey: Ctrl+Alt+S'));
    expect(replayField()).toHaveAccessibleName('Replay hotkey: Ctrl+Alt+R');
    expect(backend.settings.hotkeys).toEqual({ replay: 'Ctrl+Alt+R', stop: 'Ctrl+Alt+S', catchUp: 'Ctrl+Alt+Enter' });
  });

  it('forgets the error when recording starts again', async () => {
    await renderApp();
    await fireEvent.click(replayField());
    await press(replayField(), 'Enter', { ctrl: true, alt: true });
    await screen.findByText('Ctrl+Alt+Enter is already used for Catch up.');

    await fireEvent.click(replayField());

    expect(screen.queryByText('Ctrl+Alt+Enter is already used for Catch up.')).not.toBeInTheDocument();
  });

  it('stops recording and resumes the hotkeys when the field loses focus', async () => {
    const { backend } = await renderApp();
    await fireEvent.click(replayField());

    await fireEvent.blur(replayField());

    await waitFor(() => expect(suspends(backend)).toEqual([{ suspended: true }, { suspended: false }]));
    expect(replayField()).toHaveAccessibleName('Replay hotkey: Ctrl+Alt+Space');
  });

  it('moving to another field gives the hotkeys back and takes them again, in that order', async () => {
    const { backend } = await renderApp();
    await fireEvent.click(replayField());

    await fireEvent.blur(replayField());
    await fireEvent.click(stopField());

    await waitFor(() => expect(suspends(backend)).toEqual([{ suspended: true }, { suspended: false }, { suspended: true }]));
    expect(stopField()).toHaveAccessibleName('Stop hotkey: press a combination');
    expect(replayField()).toHaveAccessibleName('Replay hotkey: Ctrl+Alt+Space');
  });

  it('resumes the hotkeys when the page is closed mid-recording', async () => {
    const { backend, unmount } = await renderApp();
    await fireEvent.click(replayField());

    unmount();

    await waitFor(() => expect(suspends(backend)).toEqual([{ suspended: true }, { suspended: false }]));
  });

  it('still records when suspending fails', async () => {
    const { backend } = await renderApp();
    backend.overrides.suspend_hotkeys = () => {
      throw 'cannot suspend';
    };

    await fireEvent.click(replayField());

    expect(replayField()).toHaveAccessibleName('Replay hotkey: press a combination');
  });

  it('ignores a second click on the field that is recording', async () => {
    const { backend } = await renderApp();

    await fireEvent.click(replayField());
    await fireEvent.click(replayField());

    expect(suspends(backend)).toEqual([{ suspended: true }]);
  });
});

describe('hotkey states', () => {
  async function withStatus(hotkeys: Status['hotkeys']) {
    const view = await renderApp();
    await emit('status', { ...view.backend.status(), hotkeys });
    return view;
  }

  it('says when another app owns a hotkey', async () => {
    await withStatus({ replay: 'inUse', stop: 'ok', catchUp: 'ok' });

    expect(await screen.findByText('In use by another app')).toBeInTheDocument();
  });

  it('says when a hotkey is invalid', async () => {
    await withStatus({ replay: 'ok', stop: 'invalid', catchUp: 'ok' });

    expect(await screen.findByText('Invalid')).toBeInTheDocument();
  });

  it('says when a hotkey is not set', async () => {
    await renderApp({ settings: { hotkeys: { replay: 'Ctrl+Alt+Space', stop: null, catchUp: 'Ctrl+Alt+Enter' } } });

    const row = stopField().closest('.hotkey') as HTMLElement;
    expect(await within(row).findByText('Not set')).toBeInTheDocument();
  });

  it('says nothing when the hotkeys work', async () => {
    await renderApp();

    expect(screen.queryByText('In use by another app')).not.toBeInTheDocument();
    expect(screen.queryByText('Invalid')).not.toBeInTheDocument();
    expect(screen.queryByText('Not set')).not.toBeInTheDocument();
  });
});

describe('reset to defaults', () => {
  it('puts the three default combinations back', async () => {
    const { backend } = await renderApp({
      settings: { hotkeys: { replay: 'Ctrl+Alt+R', stop: null, catchUp: 'Ctrl+Alt+C' } },
    });

    await fireEvent.click(screen.getByRole('button', { name: 'Reset to defaults' }));

    await waitFor(() =>
      expect(argsOf(backend, 'update_settings')).toEqual([
        { patch: { hotkeys: { replay: 'Ctrl+Alt+Space', stop: 'Ctrl+Alt+Backspace', catchUp: 'Ctrl+Alt+Enter' } } },
      ]),
    );
    await waitFor(() => expect(replayField()).toHaveAccessibleName('Replay hotkey: Ctrl+Alt+Space'));
  });

  it('is disabled when the defaults are already in place', async () => {
    await renderApp();

    expect(screen.getByRole('button', { name: 'Reset to defaults' })).toBeDisabled();
  });
});

const KEYS_FROM_THE_DESKTOP = { replay: 'Ctrl+Alt+Space', stop: null, catchUp: 'Meta+Escape' };

describe('hotkeys the desktop assigns (the portal)', () => {
  const portal = (extra: Partial<Status> = {}) => ({
    statusOverrides: { hotkeyBackend: 'portal' as const, hotkeyTriggers: KEYS_FROM_THE_DESKTOP, ...extra },
  });

  it('shows the key each action got, and "Not assigned" for one that got none', async () => {
    await renderApp(portal());

    const rows = document.querySelectorAll<HTMLElement>('.hotkey');
    expect(rows).toHaveLength(3);
    expect(within(rows[0]).getByText('Replay')).toBeInTheDocument();
    expect(within(rows[0]).getByText('Ctrl+Alt+Space')).toBeInTheDocument();
    expect(within(rows[1]).getByText('Stop')).toBeInTheDocument();
    expect(within(rows[1]).getByText('Not assigned')).toBeInTheDocument();
    expect(within(rows[2]).getByText('Meta+Escape')).toBeInTheDocument();
  });

  it('has no recorder and no reset', async () => {
    await renderApp(portal());

    expect(screen.queryByRole('button', { name: /hotkey:/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Reset to defaults' })).not.toBeInTheDocument();
    expect(screen.queryByText('Click to set')).not.toBeInTheDocument();
  });

  it('never touches the hotkeys by itself', async () => {
    const { backend } = await renderApp(portal());

    await fireEvent.click(screen.getByRole('button', { name: 'Change in system settings' }));

    expect(suspends(backend)).toEqual([]);
    expect(argsOf(backend, 'update_settings')).toEqual([]);
  });

  it('asks the desktop to show its shortcut settings', async () => {
    const { backend } = await renderApp(portal());

    await fireEvent.click(screen.getByRole('button', { name: 'Change in system settings' }));

    await waitFor(() => expect(argsOf(backend, 'configure_shortcuts')).toHaveLength(1));
    expect(screen.queryByText(/Shortcuts → Repeat That/)).not.toBeInTheDocument();
  });

  it('says where to look when the desktop cannot be asked to', async () => {
    await renderApp({ ...portal(), configureOutcome: 'unsupported' });

    await fireEvent.click(screen.getByRole('button', { name: 'Change in system settings' }));

    const where = await screen.findByText(/System Settings → Shortcuts → Repeat That/);
    expect(where).toHaveTextContent('Settings → Keyboard → View and Customize Shortcuts → Apps');
  });

  it('offers to set the shortcuts up when no action has a key, which is how a closed dialog is asked for again', async () => {
    const { backend } = await renderApp(
      portal({ hotkeyTriggers: { replay: null, stop: null, catchUp: null } }),
    );

    expect(screen.queryByRole('button', { name: 'Change in system settings' })).not.toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Set up shortcuts' }));

    await waitFor(() => expect(argsOf(backend, 'configure_shortcuts')).toHaveLength(1));
    expect(screen.queryByText(/waiting for you/)).not.toBeInTheDocument();
  });

  it('goes back to the system settings once any action has a key', async () => {
    const view = await renderApp(portal({ hotkeyTriggers: { replay: null, stop: null, catchUp: null } }));
    expect(screen.getByRole('button', { name: 'Set up shortcuts' })).toBeInTheDocument();

    await emit('status', {
      ...view.backend.status(),
      hotkeyTriggers: { replay: 'Meta+R', stop: null, catchUp: null },
    });

    expect(await screen.findByRole('button', { name: 'Change in system settings' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Set up shortcuts' })).not.toBeInTheDocument();
  });

  it('says the desktop is waiting for its dialog to be answered when it is', async () => {
    await renderApp({ ...portal(), configureOutcome: 'busy' });

    await fireEvent.click(screen.getByRole('button', { name: 'Change in system settings' }));

    expect(await screen.findByText(/waiting for you to answer its shortcuts dialog/)).toBeInTheDocument();
  });

  it('forgets that note when asking again works', async () => {
    const { backend } = await renderApp({ ...portal(), configureOutcome: 'busy' });
    await fireEvent.click(screen.getByRole('button', { name: 'Change in system settings' }));
    await screen.findByText(/waiting for you to answer/);

    backend.configureOutcome = 'opened';
    await fireEvent.click(screen.getByRole('button', { name: 'Change in system settings' }));

    await waitFor(() => expect(screen.queryByText(/waiting for you to answer/)).not.toBeInTheDocument());
  });

  it('says what went wrong when asking fails', async () => {
    const { backend } = await renderApp(portal());
    backend.overrides.configure_shortcuts = () => {
      throw { kind: 'backend', message: 'Could not open the shortcut settings: the desktop did not answer' };
    };

    await fireEvent.click(screen.getByRole('button', { name: 'Change in system settings' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('the desktop did not answer');
  });

  it('follows a key the user changes in the system settings', async () => {
    const view = await renderApp(portal());
    expect(screen.getByText('Ctrl+Alt+Space')).toBeInTheDocument();

    await emit('status', {
      ...view.backend.status(),
      hotkeyTriggers: { replay: 'Meta+R', stop: 'Meta+S', catchUp: null },
    });

    expect(await screen.findByText('Meta+R')).toBeInTheDocument();
    expect(screen.getByText('Meta+S')).toBeInTheDocument();
    expect(screen.queryByText('Ctrl+Alt+Space')).not.toBeInTheDocument();
  });

  it('does not repeat the in-use and invalid notes of the recorder', async () => {
    await renderApp(portal({ hotkeys: { replay: 'inUse', stop: 'invalid', catchUp: 'ok' } }));

    expect(screen.queryByText('In use by another app')).not.toBeInTheDocument();
    expect(screen.queryByText('Invalid')).not.toBeInTheDocument();
  });
});

describe('no hotkeys on this desktop', () => {
  const COMMANDS = {
    replay: 'repeat-that --replay',
    stop: 'repeat-that --stop',
    catchUp: 'repeat-that --catch-up',
  };
  const none = (hotkeyCommands: Status['hotkeyCommands'] = COMMANDS) => ({
    statusOverrides: { hotkeyBackend: 'none' as const, hotkeyCommands },
  });

  afterEach(() => {
    Reflect.deleteProperty(navigator, 'clipboard');
  });

  it('warns that global hotkeys are not available', async () => {
    await renderApp(none());

    expect(screen.getByText(/Global hotkeys aren't available on this desktop/)).toBeInTheDocument();
  });

  it('shows the command of each action, and how to bind it', async () => {
    await renderApp(none());

    for (const command of Object.values(COMMANDS)) expect(screen.getByText(command)).toBeInTheDocument();
    expect(screen.getByText(/custom keyboard shortcut/)).toBeInTheDocument();
    expect(screen.getByText(/Custom Shortcuts/)).toBeInTheDocument();
  });

  it('has no recorder and no reset', async () => {
    await renderApp(none());

    expect(screen.queryByRole('button', { name: /hotkey:/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Reset to defaults' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Change in system settings' })).not.toBeInTheDocument();
  });

  it('copies a command and says so', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    await renderApp(none());

    await fireEvent.click(screen.getByRole('button', { name: 'Copy the Catch up command' }));

    await waitFor(() => expect(writeText).toHaveBeenCalledWith('repeat-that --catch-up'));
    expect(await screen.findByRole('button', { name: 'Copied the Catch up command' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Copy the Replay command' })).toBeInTheDocument();
  });

  it('copies each of the three commands', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    await renderApp(none());

    for (const label of ['Replay', 'Stop', 'Catch up']) {
      await fireEvent.click(screen.getByRole('button', { name: `Copy the ${label} command` }));
    }

    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(3));
    expect(writeText.mock.calls.map(([text]) => text)).toEqual(Object.values(COMMANDS));
  });

  it('shows the commands as the backend worded them for a flatpak', async () => {
    await renderApp(
      none({
        replay: 'flatpak run me.roboroads.RepeatThat --replay',
        stop: 'flatpak run me.roboroads.RepeatThat --stop',
        catchUp: 'flatpak run me.roboroads.RepeatThat --catch-up',
      }),
    );

    expect(screen.getByText('flatpak run me.roboroads.RepeatThat --stop')).toBeInTheDocument();
  });

  it('still explains itself before the backend has sent the commands', async () => {
    await renderApp(none(null));

    expect(screen.getByText(/Global hotkeys aren't available on this desktop/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^Copy the/ })).not.toBeInTheDocument();
  });
});

describe('hotkeys with the plugin', () => {
  it('show nothing of the other modes', async () => {
    await renderApp();

    expect(screen.queryByRole('button', { name: 'Change in system settings' })).not.toBeInTheDocument();
    expect(screen.queryByText(/Global hotkeys aren't available/)).not.toBeInTheDocument();
    expect(screen.queryByText('Not assigned')).not.toBeInTheDocument();
  });
});
