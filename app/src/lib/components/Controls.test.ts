import { fireEvent, screen, waitFor } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import { argsOf, pretendPlatform, renderApp } from '../../test-utils';

async function patches(backend: Awaited<ReturnType<typeof renderApp>>['backend'], expected: unknown[]) {
  await waitFor(() => expect(argsOf(backend, 'update_settings')).toEqual(expected.map((patch) => ({ patch }))));
}

describe('replay section', () => {
  it('buffer slider: the label follows the drag, the value is saved on release', async () => {
    const { backend } = await renderApp();
    const slider = screen.getByRole('slider', { name: 'Buffer length' });
    expect(slider).toHaveAttribute('min', '10');
    expect(slider).toHaveAttribute('max', '120');

    await fireEvent.input(slider, { target: { value: '45' } });

    expect(screen.getByText('45 s')).toBeInTheDocument();
    expect(argsOf(backend, 'update_settings')).toEqual([]);

    await fireEvent.change(slider);
    await patches(backend, [{ bufferSeconds: 45 }]);
    expect(screen.getByText('45 s')).toBeInTheDocument();
  });

  it('replay speed slider saves the speed', async () => {
    const { backend } = await renderApp();
    const slider = screen.getByRole('slider', { name: 'Replay speed' });
    expect(slider).toHaveAttribute('min', '0.75');
    expect(slider).toHaveAttribute('max', '1.5');
    expect(screen.getByText('1.00×')).toBeInTheDocument();

    await fireEvent.change(slider, { target: { value: '1.25' } });

    await patches(backend, [{ replaySpeed: 1.25 }]);
    expect(screen.getByText('1.25×')).toBeInTheDocument();
  });

  it('catch-up speed slider saves the speed', async () => {
    const { backend } = await renderApp();
    const slider = screen.getByRole('slider', { name: 'Catch-up speed' });
    expect(slider).toHaveAttribute('min', '1.25');
    expect(slider).toHaveAttribute('max', '2');
    expect(screen.getByText('1.50×')).toBeInTheDocument();

    await fireEvent.change(slider, { target: { value: '1.75' } });

    await patches(backend, [{ catchUpSpeed: 1.75 }]);
  });

  it('output device: lists the system default and the devices, saves the choice, null for the default', async () => {
    const { backend } = await renderApp();
    const select = await screen.findByRole('combobox', { name: 'Output device' });
    await waitFor(() => expect(screen.getByRole('option', { name: 'Headphones (USB Audio)' })).toBeInTheDocument());
    expect(screen.getByRole('option', { name: 'System default' })).toBeInTheDocument();
    expect(select).toHaveValue('');

    await fireEvent.change(select, { target: { value: 'headphones' } });
    await patches(backend, [{ outputDevice: 'headphones' }]);

    await fireEvent.change(select, { target: { value: '' } });
    await patches(backend, [{ outputDevice: 'headphones' }, { outputDevice: null }]);
  });

  it('marks a stored device that is not connected', async () => {
    await renderApp({ settings: { outputDevice: 'usb-dac-1234' } });

    const option = await screen.findByRole('option', { name: 'usb-dac-1234 (not connected)' });
    expect(option).toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: 'Output device' })).toHaveValue('usb-dac-1234');
  });

  it('shows the name of a device that was connected earlier and is gone now', async () => {
    const { backend } = await renderApp({ settings: { outputDevice: 'headphones' } });
    await screen.findByRole('option', { name: 'Headphones (USB Audio)' });

    backend.devices = backend.devices.filter((d) => d.id !== 'headphones');
    await fireEvent.focus(screen.getByRole('combobox', { name: 'Output device' }));

    expect(await screen.findByRole('option', { name: 'Headphones (USB Audio) (not connected)' })).toBeInTheDocument();
  });

  it('does not flag a stored device that is connected', async () => {
    await renderApp({ settings: { outputDevice: 'headphones' } });
    await screen.findByRole('option', { name: 'Headphones (USB Audio)' });

    expect(screen.queryByRole('option', { name: /not connected/ })).not.toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: 'Output device' })).toHaveValue('headphones');
  });
});

describe('ducking section', () => {
  it('toggle saves the whole duck object', async () => {
    const { backend } = await renderApp();

    await fireEvent.click(screen.getByRole('switch', { name: /Lower the app's volume/ }));

    await patches(backend, [{ duck: { enabled: false, level: 0.2 } }]);
    expect(screen.getByRole('slider', { name: 'Volume during replay' })).toBeDisabled();
  });

  it('level slider is in percent and saves a fraction', async () => {
    const { backend } = await renderApp();
    const slider = screen.getByRole('slider', { name: 'Volume during replay' });
    expect(slider).toHaveAttribute('min', '5');
    expect(slider).toHaveAttribute('max', '100');
    expect(slider).toHaveAttribute('step', '5');
    expect(screen.getByText('20%')).toBeInTheDocument();

    await fireEvent.change(slider, { target: { value: '55' } });

    await patches(backend, [{ duck: { enabled: true, level: 0.55 } }]);
    expect(screen.getByText('55%')).toBeInTheDocument();
  });

  it.each(['Win32', 'Linux x86_64'])('level slider stops at 5%% on %s', async (platform) => {
    pretendPlatform(platform);
    await renderApp();

    expect(screen.getByRole('slider', { name: 'Volume during replay' })).toHaveAttribute('min', '5');
  });

  it('level slider goes down to Muted on a Mac', async () => {
    pretendPlatform('MacIntel');
    const { backend } = await renderApp();
    const slider = screen.getByRole('slider', { name: 'Volume during replay' });
    expect(slider).toHaveAttribute('min', '0');
    expect(slider).toHaveAttribute('max', '100');

    await fireEvent.input(slider, { target: { value: '0' } });
    expect(screen.getByText('Muted')).toBeInTheDocument();
    expect(slider).toHaveAttribute('aria-valuetext', 'Muted');
    await fireEvent.change(slider);

    await patches(backend, [{ duck: { enabled: true, level: 0 } }]);
    expect(screen.getByText('Muted')).toBeInTheDocument();
    await fireEvent.change(slider, { target: { value: '5' } });
    await waitFor(() => expect(screen.getByText('5%')).toBeInTheDocument());
  });
});

describe('detection section', () => {
  it('saves the chosen sensitivity and explains the current one', async () => {
    const { backend } = await renderApp();
    expect(screen.getByRole('radio', { name: 'Normal' })).toBeChecked();
    const normalHint = screen.getByTestId('sensitivity-hint').textContent;

    await fireEvent.click(screen.getByRole('radio', { name: 'High' }));

    await patches(backend, [{ sensitivity: 'high' }]);
    expect(screen.getByRole('radio', { name: 'High' })).toBeChecked();
    expect(screen.getByTestId('sensitivity-hint').textContent).not.toBe(normalHint);

    await fireEvent.click(screen.getByRole('radio', { name: 'Low' }));
    await patches(backend, [{ sensitivity: 'high' }, { sensitivity: 'low' }]);
  });
});

describe('general section', () => {
  it('saves autostart', async () => {
    const { backend } = await renderApp();

    await fireEvent.click(screen.getByRole('switch', { name: /Start at login|Start with Windows/ }));

    await patches(backend, [{ autostart: true }]);
  });

  it('calls autostart "Start with Windows" on Windows and "Start at login" elsewhere', async () => {
    const platform = vi.spyOn(window.navigator, 'platform', 'get');

    platform.mockReturnValue('Win32');
    const first = await renderApp();
    expect(screen.getByRole('switch', { name: 'Start with Windows' })).toBeInTheDocument();
    first.unmount();

    platform.mockReturnValue('Linux x86_64');
    await renderApp();
    expect(screen.getByRole('switch', { name: 'Start at login' })).toBeInTheDocument();
    platform.mockRestore();
  });

  it('links to the privacy statement and opens it outside the window', async () => {
    const open = vi.spyOn(window, 'open').mockReturnValue(null);
    await renderApp();

    const link = screen.getByRole('link', { name: /Audio stays in memory only/ });
    expect(link.getAttribute('href')).toMatch(/PRIVACY\.md$/);
    await fireEvent.click(link);

    expect(open).toHaveBeenCalledWith(link.getAttribute('href'), '_blank', 'noopener,noreferrer');
    open.mockRestore();
  });
});
