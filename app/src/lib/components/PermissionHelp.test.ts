import { emit } from '@tauri-apps/api/event';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import { argsOf, renderApp } from '../../test-utils';
import type { PermissionHelp } from '../api';
import { MOCK_HOW_TO_ALLOW, type MockBackendInit, scenarioInit } from '../mock-ipc';

const MACOS_15: PermissionHelp = { canOpenSettings: true, canAskAgain: true };
const MACOS_14: PermissionHelp = { canOpenSettings: false, canAskAgain: true };
const REOPEN = "If capturing doesn't resume once it's turned on, quit and reopen Repeat That.";

const help = () => screen.queryByRole('group', { name: 'Allow audio capture' });
const button = (name: string) => screen.queryByRole('button', { name });

/** The page while the pretend macOS doesn't let the app capture the chosen app. */
async function notAllowed(permissionHelp: PermissionHelp = MACOS_15, init: MockBackendInit = {}) {
  const view = await renderApp({ ...scenarioInit('not-allowed'), permissionHelp, ...init });
  await screen.findByText("Repeat That isn't allowed to capture audio");
  return view;
}

describe('help for a capture that is not allowed', () => {
  it('is not there while capturing', async () => {
    await renderApp(scenarioInit('configured'));

    await screen.findByText('Capturing Discord');
    expect(help()).not.toBeInTheDocument();
  });

  it("says how to allow it in the backend's words, with both buttons on macOS 15 and later", async () => {
    const { backend } = await notAllowed(MACOS_15);
    const bar = screen.getByRole('region', { name: 'Status' });

    expect(within(bar).getByText(MOCK_HOW_TO_ALLOW)).toBeInTheDocument();
    const group = await screen.findByRole('group', { name: 'Allow audio capture' });
    expect(await within(group).findByRole('button', { name: 'Open System Settings' })).toBeEnabled();
    expect(within(group).getByRole('button', { name: 'Ask again' })).toBeEnabled();
    expect(within(group).getByText(REOPEN)).toBeInTheDocument();
    expect(argsOf(backend, 'capture_permission_help')).toHaveLength(1);
  });

  it('has only Ask again on macOS 14', async () => {
    await notAllowed(MACOS_14);

    expect(await screen.findByRole('button', { name: 'Ask again' })).toBeInTheDocument();
    expect(button('Open System Settings')).not.toBeInTheDocument();
    expect(screen.queryByText(REOPEN)).not.toBeInTheDocument();
  });

  it("has no buttons where there is nothing to do from here, only the backend's sentence", async () => {
    const { backend } = await notAllowed({ canOpenSettings: false, canAskAgain: false });

    await waitFor(() => expect(argsOf(backend, 'capture_permission_help')).toHaveLength(1));
    expect(within(help()!).queryByRole('button')).not.toBeInTheDocument();
    expect(screen.getByText(MOCK_HOW_TO_ALLOW)).toBeInTheDocument();
  });

  it('asks again, and goes away once capturing resumes', async () => {
    const { backend } = await notAllowed();

    await fireEvent.click(await screen.findByRole('button', { name: 'Ask again' }));

    await waitFor(() => expect(argsOf(backend, 'ask_for_capture_permission')).toHaveLength(1));
    expect(await screen.findByText('Capturing Discord')).toBeInTheDocument();
    expect(help()).not.toBeInTheDocument();
  });

  it('opens System Settings', async () => {
    const { backend } = await notAllowed();

    await fireEvent.click(await screen.findByRole('button', { name: 'Open System Settings' }));

    await waitFor(() => expect(argsOf(backend, 'open_capture_permission_settings')).toHaveLength(1));
    expect(help()).toBeInTheDocument();
  });

  it('shows why a command failed, next to its buttons', async () => {
    const { backend } = await notAllowed();
    backend.overrides.ask_for_capture_permission = () => {
      throw { kind: 'backend', message: 'Asking again works only in the installed app, not when it runs from a terminal' };
    };

    await fireEvent.click(await screen.findByRole('button', { name: 'Ask again' }));

    const alert = await within(help()!).findByRole('alert');
    expect(alert).toHaveTextContent('Asking again works only in the installed app');
    expect(button('Ask again')).toBeEnabled();
  });

  it("says why it can't tell which buttons there are, under the backend's sentence", async () => {
    await renderApp(scenarioInit('not-allowed'), (backend) => {
      backend.overrides.capture_permission_help = () => {
        throw { kind: 'backend', message: 'The audio service is not running' };
      };
    });

    const group = await screen.findByRole('group', { name: 'Allow audio capture' });
    expect(await within(group).findByRole('alert')).toHaveTextContent('The audio service is not running');
    expect(within(group).queryByRole('button')).not.toBeInTheDocument();
    expect(screen.getByText(MOCK_HOW_TO_ALLOW)).toBeInTheDocument();
  });

  it('comes back when the permission is taken back while capturing, and asks for the help again', async () => {
    const { backend } = await notAllowed();
    await fireEvent.click(await screen.findByRole('button', { name: 'Ask again' }));
    await screen.findByText('Capturing Discord');

    await emit('status', { ...backend.status(), capture: { state: 'notAllowed', message: MOCK_HOW_TO_ALLOW } });

    expect(await screen.findByRole('group', { name: 'Allow audio capture' })).toBeInTheDocument();
    await waitFor(() => expect(argsOf(backend, 'capture_permission_help')).toHaveLength(2));
  });

  it('keeps Replay and Pause, since the buffer is kept', async () => {
    await notAllowed();
    const bar = screen.getByRole('region', { name: 'Status' });

    expect(within(bar).getByRole('button', { name: 'Replay' })).toBeEnabled();
    expect(within(bar).getByRole('button', { name: 'Pause capture' })).toBeEnabled();
  });
});
