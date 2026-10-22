<script lang="ts">
  import { quitApp, toApiError, type Settings } from '../api';
  import { openExternal } from '../opener';
  import { isWindows } from '../platform';
  import type { SettingsStore } from '../store.svelte';
  import MacosNotice from './MacosNotice.svelte';
  import Section from './Section.svelte';
  import Switch from './Switch.svelte';

  interface Props {
    store: SettingsStore;
    settings: Settings;
  }

  let { store, settings }: Props = $props();

  const PRIVACY_URL = 'https://github.com/Roboroads/repeat-that/blob/main/PRIVACY.md';
  // A plain link does nothing inside the app window; see openExternal.
  function openPrivacy(event: MouseEvent) {
    event.preventDefault();
    void openExternal(PRIVACY_URL);
  }

  const autostartLabel = isWindows() ? 'Start with Windows' : 'Start at login';

  /** Some Linux desktops have nothing to show a tray icon in. Everywhere else this is `true`. */
  const hasTray = $derived(store.status?.trayAvailable !== false);

  async function quit() {
    try {
      await quitApp();
    } catch (raw) {
      store.error = toApiError(raw).message;
    }
  }
</script>

<Section title="General" id="general">
  <MacosNotice />
  <Switch
    label={autostartLabel}
    description={hasTray ? 'Starts in the tray, without opening this window.' : 'Starts without opening this window.'}
    checked={settings.autostart}
    onchange={(autostart) => store.update({ autostart })}
  />
  <p class="privacy muted">
    <a href={PRIVACY_URL} target="_blank" rel="noreferrer" onclick={openPrivacy}>Audio stays in memory only</a>
    and is wiped when you pause, pick another app or quit. Only your settings are saved, including the name and
    location of the app you picked.
  </p>
  {#if !hasTray}
    <!-- The tray menu is where Quit lives; without a tray, this is the way out. -->
    <p class="callout">
      No tray on this desktop. Open Repeat That from your app menu. On GNOME, install the AppIndicator and
      KStatusNotifierItem Support extension for a tray icon.
    </p>
    <div>
      <button type="button" onclick={quit}>Quit Repeat That</button>
    </div>
  {/if}
</Section>

<style>
  .privacy {
    font-size: 0.9rem;
  }
</style>
