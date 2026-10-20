import { isTauri } from '@tauri-apps/api/core';
import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';

// `npm run dev` in a plain browser has no Rust backend: stand in for it so the page can be clicked through.
// `import.meta.env.DEV` is false in production builds, which drops this branch and the mock with it.
// Add `?scenario=configured|waiting|problems|empty` to the URL to start from another state.
if (import.meta.env.DEV && !isTauri()) {
  const { installMockBackend, scenarioFromUrl } = await import('./lib/mock-ipc');
  installMockBackend(scenarioFromUrl(window.location.search));
}

const target = document.getElementById('app');
if (!target) {
  throw new Error('Missing #app element');
}

export default mount(App, { target });
