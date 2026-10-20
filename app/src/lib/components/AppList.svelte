<script lang="ts">
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

  function choose(row: AppRow) {
    if (row.app !== null && !row.selected) void store.selectApp(row.app);
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
  {:else}
    <div class="list" role="radiogroup" aria-label="App to capture">
      {#each rows as row (row.key)}
        <label class="row" class:selected={row.selected}>
          <input
            type="radio"
            name="target"
            checked={row.selected}
            aria-label={row.waiting ? `${row.name} · waiting` : row.name}
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

  .row.selected {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 10%, var(--surface));
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
