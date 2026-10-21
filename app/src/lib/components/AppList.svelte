<script lang="ts">
  import { tick } from 'svelte';
  import type { Settings } from '../api';
  import { appRows, type AppRow } from '../apps';
  import type { SettingsStore } from '../store.svelte';
  import AppIcon from './AppIcon.svelte';
  import Section from './Section.svelte';

  interface Props {
    store: SettingsStore;
    settings: Settings;
  }

  let { store, settings }: Props = $props();

  const rows = $derived(appRows(store.apps, settings.target));
  const chosen = $derived(rows.find((row) => row.selected) ?? null);
  // Collapsed into the chosen app once there is one; open while there is none.
  let expanded = $state(false);
  const open = $derived(chosen === null || expanded);

  let list = $state<HTMLElement | null>(null);
  let chosenButton = $state<HTMLButtonElement | null>(null);

  const label = (row: AppRow) => (row.waiting ? `${row.name} · waiting` : row.name);

  function choose(row: AppRow) {
    if (row.app !== null && !row.selected) void store.selectApp(row.app);
    void collapse();
  }

  /** Clicking the app that is already chosen changes nothing, and closes the list. */
  function keep(row: AppRow) {
    if (row.selected) void collapse();
  }

  async function collapse() {
    const hadFocus = list?.contains(document.activeElement) ?? false;
    expanded = false;
    await tick();
    if (hadFocus) chosenButton?.focus();
  }

  async function expand() {
    expanded = true;
    await tick();
    list?.querySelector<HTMLInputElement>('input:checked')?.focus();
  }
</script>

<Section title="App" id="apps">
  {#snippet actions()}
    <button type="button" onclick={() => store.refreshApps()}>Refresh</button>
  {/snippet}

  {#if store.appsError}
    <p class="problem" role="status">Couldn't list the apps: {store.appsError}</p>
  {/if}

  {#if rows.length === 0}
    <!-- With an error showing, an empty list would be a wrong answer: stay quiet. -->
    {#if !store.appsError && store.appsLoaded}
      <p class="empty muted">No apps are playing audio right now. Start your call app, and it shows up here.</p>
    {:else if !store.appsError}
      <p class="empty muted">Looking for apps that play audio…</p>
    {/if}
  {:else if !open && chosen !== null}
    <button
      type="button"
      class="row chosen"
      aria-expanded="false"
      aria-label="{label(chosen)}, change app"
      bind:this={chosenButton}
      onclick={expand}
    >
      <AppIcon src={chosen.icon} />
      <span class="name">{chosen.name}{#if chosen.waiting}<span class="state">{' · waiting'}</span>{/if}</span>
      <span class="change">Change</span>
    </button>
  {:else}
    <div class="list" role="radiogroup" aria-label="App to capture" bind:this={list}>
      {#each rows as row (row.key)}
        <label class="row" class:selected={row.selected}>
          <input
            type="radio"
            name="target"
            checked={row.selected}
            aria-label={label(row)}
            onclick={() => keep(row)}
            onchange={() => choose(row)}
          />
          <AppIcon src={row.icon} />
          <span class="name">{row.name}{#if row.waiting}<span class="state">{' · waiting'}</span>{/if}</span>
        </label>
      {/each}
    </div>
  {/if}
</Section>

<style>
  .empty {
    padding: 0.5rem 0;
  }

  .problem {
    padding: 0.45rem 0.7rem;
    border-radius: 8px;
    background: var(--warn-bg);
    color: var(--warn);
    font-size: 0.9rem;
  }

  .list {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 0.65rem;
    padding: 0.4rem 0.6rem;
    border: 1px solid transparent;
    border-radius: 8px;
    cursor: pointer;
  }

  .row:hover {
    background: var(--surface-2);
  }

  .row.selected,
  .row.chosen {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 10%, var(--surface));
  }

  .row.chosen {
    width: 100%;
    font: inherit;
    color: inherit;
    text-align: left;
  }

  .row.chosen:hover {
    background: color-mix(in srgb, var(--accent) 16%, var(--surface));
  }

  .change {
    color: var(--accent);
    font-size: 0.9rem;
  }

  .change::after {
    content: ' ▾';
  }

  input {
    margin: 0;
    accent-color: var(--accent);
  }

  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .state {
    color: var(--text-muted);
  }
</style>
