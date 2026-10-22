<script lang="ts">
  import type { Settings } from '../api';
  import { openExternal } from '../opener';
  import type { SettingsStore } from '../store.svelte';
  import { isLink } from '../updates';

  interface Props {
    store: SettingsStore;
    settings: Settings;
  }

  let { store, settings }: Props = $props();
  const uid = $props.id();

  const update = $derived(store.updates);
  const offer = $derived(update.available);

  // Only a build that checks can be asked; `null` is "not asked yet".
  const asking = $derived((update.mode === 'install' || update.mode === 'notify') && settings.checkForUpdates === null);

  /**
   * What to say about the version on offer. A download or install under way is always shown. A version the user put
   * off ("Later") stays hidden, whether it is waiting or its install failed. A failed check isn't here, whether or not
   * it still has a version: it can't be acted on, and General shows it.
   */
  const form = $derived.by(() => {
    if (offer === null) return null;
    if (update.state === 'downloading' || update.state === 'installing') return update.state;
    if (store.offerDismissed) return null;
    if (update.state === 'available') return 'available';
    if (update.state === 'error' && update.errorKind === 'install') return 'error';
    return null;
  });

  const title = $derived.by(() => {
    if (offer === null) return '';
    switch (form) {
      case 'downloading':
        return `Downloading version ${offer.version}`;
      case 'installing':
        return `Installing version ${offer.version}`;
      case 'error':
        return `Couldn't install version ${offer.version}`;
      default:
        return `Version ${offer.version} is available`;
    }
  });

  /** The version in use, and in notify mode why this build can't install the new one. */
  const hint = $derived(
    [
      update.currentVersion === '' ? '' : `You have ${update.currentVersion}.`,
      update.mode === 'notify' ? (update.modeReason ?? '') : '',
    ]
      .filter((part) => part !== '')
      .join(' '),
  );

  const percent = $derived(update.progress === null ? null : Math.round(update.progress * 100));

  // A plain link does nothing inside the app window; see openExternal.
  function openRelease(event: MouseEvent, url: string) {
    event.preventDefault();
    void openExternal(url);
  }
</script>

{#if asking}
  <aside class="banner" aria-labelledby="{uid}-question">
    <p class="title" id="{uid}-question">Check for updates automatically?</p>
    <p>
      Once a day Repeat That asks GitHub whether a new version is out. GitHub sees your IP address; nothing else is
      sent. You can change this later under General.
    </p>
    <div class="buttons">
      <button type="button" class="primary" onclick={() => void store.update({ checkForUpdates: true })}>
        Check automatically
      </button>
      <button type="button" onclick={() => void store.update({ checkForUpdates: false })}>Not now</button>
    </div>
  </aside>
{/if}

{#if offer !== null && form !== null}
  <aside class="banner" class:failed={form === 'error'} aria-labelledby="{uid}-offer">
    <p class="title" id="{uid}-offer">{title}</p>

    {#if form === 'available'}
      {#if hint !== ''}<p class="muted">{hint}</p>{/if}
      {#if offer.notes.trim() !== ''}
        <details>
          <summary>What's new</summary>
          <pre class="notes">{offer.notes}</pre>
        </details>
      {/if}
      <div class="buttons">
        {#if update.mode === 'install'}
          <button
            type="button"
            class="primary"
            disabled={store.updateCallPending}
            onclick={() => void store.installUpdate()}
          >
            Install and restart
          </button>
        {:else if isLink(offer.releaseUrl)}
          <button type="button" class="primary" onclick={() => void openExternal(offer.releaseUrl)}>
            Download page
          </button>
        {/if}
        <button type="button" onclick={() => store.dismissUpdate()}>Later</button>
        {#if update.mode === 'install' && isLink(offer.releaseUrl)}
          <a href={offer.releaseUrl} onclick={(event) => openRelease(event, offer.releaseUrl)}>
            Release notes on GitHub
          </a>
        {/if}
      </div>
    {:else if form === 'downloading'}
      <div class="download">
        <progress aria-label="Download progress" max="1" value={update.progress ?? undefined}></progress>
        {#if percent !== null}<span class="muted">{percent}%</span>{/if}
      </div>
    {:else if form === 'installing'}
      <p class="muted">Repeat That restarts when it's done.</p>
    {:else}
      <p>{update.error ?? 'Something went wrong.'}</p>
      <div class="buttons">
        {#if isLink(offer.releaseUrl)}
          <button type="button" class="primary" onclick={() => void openExternal(offer.releaseUrl)}>
            Download page
          </button>
        {/if}
        <button type="button" onclick={() => store.dismissUpdate()}>Later</button>
      </div>
    {/if}
  </aside>
{/if}

<style>
  .banner {
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
    border: 1px solid var(--accent);
    border-radius: var(--radius);
    padding: 0.75rem 1rem;
    background: color-mix(in srgb, var(--accent) 10%, var(--surface));
  }

  .banner.failed {
    border-color: var(--danger);
    background: var(--danger-bg);
  }

  .title {
    font-weight: 650;
  }

  .buttons {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem 0.75rem;
  }

  .primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-text);
  }

  summary {
    cursor: pointer;
  }

  .notes {
    margin: 0.4rem 0 0;
    padding: 0.5rem 0.65rem;
    max-height: 12rem;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    font: inherit;
    font-size: 0.9rem;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .download {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  progress {
    flex: 1;
  }
</style>
