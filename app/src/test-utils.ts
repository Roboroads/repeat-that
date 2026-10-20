import { emit } from '@tauri-apps/api/event';
import { mockIPC } from '@tauri-apps/api/mocks';
import { render, screen } from '@testing-library/svelte';
import App from './App.svelte';
import { MockBackend, type MockBackendInit } from './lib/mock-ipc';

/** Installs a fake backend as the Tauri IPC. Events it emits reach the page like the real ones. */
export function installBackend(init: MockBackendInit = {}): MockBackend {
  const backend = new MockBackend({ ...init, emit: (event, payload) => void emit(event, payload) });
  mockIPC(backend.handle, { shouldMockEvents: true });
  return backend;
}

/**
 * Renders the whole settings page against a fake backend and waits until it has loaded. `prepare` runs before the page
 * starts, to make a command fail from the first call.
 */
export async function renderApp(init: MockBackendInit = {}, prepare?: (backend: MockBackend) => void) {
  const backend = installBackend(init);
  prepare?.(backend);
  const view = render(App);
  await screen.findByRole('region', { name: 'Status' });
  return { backend, ...view };
}

/** The arguments of every call of one command, oldest first. */
export function argsOf(backend: MockBackend, cmd: string): unknown[] {
  return backend.calls.filter((call) => call.cmd === cmd).map((call) => call.args);
}

/** Makes `document.visibilityState` report `state` and tells the page. */
export function setVisibility(state: 'visible' | 'hidden'): void {
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => state });
  document.dispatchEvent(new Event('visibilitychange'));
}

/**
 * Makes the backend slow to answer `cmd`: it does the work at once but the answer only arrives when the returned
 * function is called. The answer is what the backend knew when it did the work, so it can be out of date by then.
 */
export function holdAnswers(backend: MockBackend, cmd: string): () => void {
  let release!: () => void;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  backend.overrides[cmd] = async (args) => {
    const answer = backend.run(cmd, args);
    await gate;
    return answer;
  };
  return release;
}
