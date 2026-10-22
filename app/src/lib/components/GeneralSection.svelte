<script lang="ts">
  import { quitApp, toApiError, type Settings } from '../api';
  import { NO_UPDATER_REASON } from '../defaults';
  import { openLink } from '../opener';
  import { isWindows } from '../platform';
  import type { SettingsStore } from '../store.svelte';
  import { checksForUpdates, versionLine } from '../updates';
  import MacosNotice from './MacosNotice.svelte';
  import Section from './Section.svelte';
  import Switch from './Switch.svelte';

  interface Props {
    store: SettingsStore;
    settings: Settings;
  }

  let { store, settings }: Props = $props();

  const PRIVACY_URL = 'https://github.com/Roboroads/repeat-that/blob/main/PRIVACY.md';

  const autostartLabel = isWindows() ? 'Start with Windows' : 'Start at login';

  /** Some Linux desktops have nothing to show a tray icon in. Everywhere else this is `true`. */
  const hasTray = $derived(store.status?.trayAvailable !== false);

  const uid = $props.id();
  const update = $derived(store.updates);
  /** Only a build that looks for updates itself has a switch and a button; the others just say why not. */
  const checks = $derived(checksForUpdates(update.mode));
  const busy = $derived(
    store.updateCallPending ||
      update.state === 'checking' ||
      update.state === 'downloading' ||
      update.state === 'installing',
  );

  /** What the last check came to, next to the button the user pressed. */
  const result = $derived.by((): { text: string; detail?: string; offer?: boolean } | null => {
    const found = update.available;
    switch (update.state) {
      case 'checking':
        return { text: 'Checking…' };
      case 'upToDate':
        return { text: 'You have the latest version.' };
      case 'available':
        return found && { text: `Version ${found.version} is available.`, offer: true };
      case 'error':
        if (update.errorKind === 'install' && found) {
          return { text: `Couldn't install version ${found.version}.`, detail: update.error ?? undefined, offer: true };
        }
        return {
          text: update.manual ? "Couldn't check for updates." : 'The last automatic check failed.',
          detail: update.error ?? undefined,
        };
      default:
        return null;
    }
  });

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
  <div class="updates" role="group" aria-labelledby="{uid}-updates">
    <h3 id="{uid}-updates">Updates</h3>
    {#if checks}
      <Switch
        label="Check for updates automatically"
        description="Asks GitHub once a day. GitHub sees your IP address."
        checked={settings.checkForUpdates === true}
        onchange={(checkForUpdates) => store.update({ checkForUpdates })}
      />
      <div class="check">
        <p class="muted">{versionLine(update.currentVersion, update.lastCheckedAt)}</p>
        <button type="button" disabled={busy} onclick={() => void store.checkForUpdates()}>Check now</button>
      </div>
      <div class="result" role="status">
        {#if result}
          <p>{result.text}</p>
          {#if result.detail}<p class="muted">{result.detail}</p>{/if}
          {#if result.offer && store.offerDismissed}
            <button type="button" onclick={() => store.showUpdate()}>Show</button>
          {/if}
        {/if}
      </div>
    {:else}
      <p class="muted">{update.modeReason || NO_UPDATER_REASON}</p>
    {/if}
  </div>
  <p class="privacy muted">
    <a href={PRIVACY_URL} target="_blank" rel="noreferrer" onclick={(event) => openLink(event, PRIVACY_URL)}>Audio stays in memory only</a>
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

  .updates {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    padding-top: 0.9rem;
    border-top: 1px solid var(--border);
  }

  h3 {
    margin: 0;
    font-size: 0.95rem;
    font-weight: 650;
  }

  .check {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    font-size: 0.9rem;
  }

  /* Always there, so a screen reader announces what lands in it; an empty one takes no room. */
  .result:empty {
    margin-top: -0.6rem;
  }

  .result {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.25rem 0.75rem;
    font-size: 0.9rem;
  }
</style>
