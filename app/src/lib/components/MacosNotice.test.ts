import { fireEvent, screen, within } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import { pretendPlatform, renderApp } from '../../test-utils';

const NOTICE = /macOS support is new and hasn't been tested on a real Mac yet/;
const REPORT_URL = 'https://github.com/Roboroads/repeat-that/issues/new?template=macos.yml';

describe('macOS notice', () => {
  it('says on a Mac that macOS is untested, and links to the macOS issue form', async () => {
    pretendPlatform('MacIntel');
    await renderApp();

    const general = screen.getByRole('region', { name: 'General' });
    const notice = within(general).getByText(NOTICE);
    expect(notice).toHaveTextContent('If something doesn\'t work, please report it.');
    expect(within(notice).getByRole('link', { name: 'report it' })).toHaveAttribute('href', REPORT_URL);
  });

  it('opens the issue form outside the window', async () => {
    pretendPlatform('MacIntel');
    const open = vi.spyOn(window, 'open').mockReturnValue(null);
    await renderApp();

    await fireEvent.click(screen.getByRole('link', { name: 'report it' }));

    expect(open).toHaveBeenCalledWith(REPORT_URL, '_blank', 'noopener,noreferrer');
    open.mockRestore();
  });

  it.each(['Win32', 'Linux x86_64'])('is not there on %s', async (platform) => {
    pretendPlatform(platform);
    await renderApp();

    expect(screen.queryByText(NOTICE)).not.toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'report it' })).not.toBeInTheDocument();
  });
});
