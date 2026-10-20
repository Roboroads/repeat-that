import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import App from './App.svelte';

describe('App', () => {
  it('shows the app name and that the settings are not here yet', () => {
    render(App);

    expect(screen.getByRole('heading', { level: 1, name: 'Repeat That' })).toBeInTheDocument();
    expect(screen.getByText(/settings are coming soon/i)).toBeInTheDocument();
  });

  it('shows the app icon from its source SVG', () => {
    const { container } = render(App);

    // Vite turns the imported file into a URL (a data URI in tests, a hashed file in the build), not the old public path.
    const src = container.querySelector('img.logo')?.getAttribute('src');
    expect(src).toMatch(/svg/);
    expect(src).not.toBe('/icon.svg');
  });

  it('tells the user the app lives in the tray', () => {
    render(App);

    expect(screen.getByText(/closing this window keeps the app running in the tray/i)).toBeInTheDocument();
  });
});
