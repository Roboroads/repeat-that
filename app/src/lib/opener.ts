import { invoke, isTauri } from '@tauri-apps/api/core';

/**
 * Opens `url`, one of this project's pages, in the user's browser. Inside the app a plain link does nothing (the
 * webview refuses to open new windows), so this asks the app's `open_link` command, which opens this project's pages
 * only and, from an AppImage, starts the browser with the host's environment. In a plain browser, and when the command
 * fails, it falls back to `window.open`.
 */
export async function openExternal(url: string): Promise<void> {
  if (isTauri()) {
    try {
      await invoke('open_link', { url });
      return;
    } catch (error) {
      console.warn('The app could not open the link', error);
    }
  }
  window.open(url, '_blank', 'noopener,noreferrer');
}
