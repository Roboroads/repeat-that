import { invoke, isTauri } from '@tauri-apps/api/core';

/**
 * Opens `url` in the user's browser. Inside the app a plain link does nothing (the webview refuses to open new
 * windows), so this asks the Tauri opener plugin. That needs the plugin registered on the Rust side and the
 * `opener:allow-open-url` permission in the window's capability; until then, and in a plain browser, it falls back to
 * `window.open`.
 */
export async function openExternal(url: string): Promise<void> {
  if (isTauri()) {
    try {
      await invoke('plugin:opener|open_url', { url });
      return;
    } catch (error) {
      console.warn('The opener plugin could not open the link', error);
    }
  }
  window.open(url, '_blank', 'noopener,noreferrer');
}
