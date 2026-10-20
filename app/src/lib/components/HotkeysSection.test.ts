import { emit } from '@tauri-apps/api/event';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
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
