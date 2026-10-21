<script lang="ts">
  import type { Settings } from '../api';
  import { hotkeyHint } from '../hotkey';
  import { describeStatus, outputWarnings } from '../status';
  import type { SettingsStore } from '../store.svelte';

  interface Props {
    store: SettingsStore;
    settings: Settings;
  }

  let { store, settings }: Props = $props();

  const text = $derived(describeStatus(store.status, settings.target));
  const warnings = $derived(store.status ? outputWarnings(store.status) : []);
  const state = $derived(store.status?.capture.state ?? 'noTarget');
  const nothingToCapture = $derived(state === 'noTarget' || state === 'unsupported');
  const playing = $derived((store.status?.playback ?? 'idle') !== 'idle');

  // Where the desktop owns the keys, the hint is the key it assigned, not the one that was asked for.
  const hint = (action: 'replay' | 'stop') => hotkeyHint(action, settings, store.status);
</script>

<section class="status" aria-label="Status">
  <!-- The chosen app, with its icon, is in the app selector below. -->
  <div class="lines">
    <p class="headline {text.tone}" aria-live="polite">{text.headline}</p>
    {#if text.detail}<p class="muted">{text.detail}</p>{/if}
    {#if text.playback}<p class="playback">{text.playback}</p>{/if}
  </div>

  <div class="buttons">
    <button type="button" class="primary" disabled={nothingToCapture} title={hint('replay')} onclick={() => store.replay()}>
      Replay
    </button>
    <button type="button" disabled={!playing} title={hint('stop')} onclick={() => store.stopPlayback()}>Stop</button>
    <button
      type="button"
      disabled={nothingToCapture}
      aria-pressed={state === 'paused'}
      onclick={() => store.setPaused(state !== 'paused')}
    >
      Pause capture
    </button>
  </div>

  {#if warnings.length > 0}
    <ul class="warnings">
      {#each warnings as warning (warning)}
        <li role="status">{warning}</li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .status {
    position: sticky;
    top: 0;
    z-index: 1;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 0.85rem 1rem;
    box-shadow: 0 4px 10px -8px rgb(0 0 0 / 0.5);
  }

  .lines p {
    line-height: 1.35;
  }

  .headline {
    font-weight: 600;
  }

  .headline.ok::before,
  .headline.warn::before {
    content: '';
    display: inline-block;
    width: 0.55rem;
    height: 0.55rem;
    margin-right: 0.45rem;
    border-radius: 50%;
    background: var(--ok);
  }

  .headline.warn::before {
    background: var(--warn);
  }

  .playback {
    color: var(--accent);
    font-weight: 600;
  }

  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
  }

  button.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-text);
  }

  .warnings {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }

  .warnings li {
    padding: 0.45rem 0.7rem;
    border-radius: 8px;
    background: var(--warn-bg);
    color: var(--warn);
    font-size: 0.9rem;
  }
</style>
