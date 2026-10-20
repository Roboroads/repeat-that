<script lang="ts">
  import type { Settings } from '../api';
  import { openExternal } from '../opener';
  import { isWindows } from '../platform';
  import type { SettingsStore } from '../store.svelte';
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
</script>

<Section title="General" id="general">
  <Switch
    label={autostartLabel}
    description="Starts in the tray, without opening this window."
    checked={settings.autostart}
    onchange={(autostart) => store.update({ autostart })}
  />
  <p class="privacy muted">
    <a href={PRIVACY_URL} target="_blank" rel="noreferrer" onclick={openPrivacy}>Audio stays in memory only</a>
    and is wiped when you pause, pick another app or quit. Only your settings are saved, including the name and
    location of the app you picked.
  </p>
</Section>

<style>
  .privacy {
    font-size: 0.9rem;
  }
</style>
