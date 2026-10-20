import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import App from './App.svelte';

describe('App', () => {
  it('shows the app name and that the settings are not here yet', () => {
    render(App);

    expect(screen.getByRole('heading', { level: 1, name: 'Repeat that?!' })).toBeInTheDocument();
    expect(screen.getByText(/settings are coming soon/i)).toBeInTheDocument();
  });

  it('tells the user the app lives in the tray', () => {
    render(App);

    expect(screen.getByText(/closing this window keeps the app running in the tray/i)).toBeInTheDocument();
  });
});
