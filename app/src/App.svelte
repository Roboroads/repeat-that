<script lang="ts">
  import { onMount } from 'svelte';
  // The app icon's source file: the same SVG the PNG and ICO app icons are rendered from.
  import icon from '../src-tauri/icons/svg/app.svg';
  import AppList from './lib/components/AppList.svelte';
  import DetectionSection from './lib/components/DetectionSection.svelte';
  import DuckingSection from './lib/components/DuckingSection.svelte';
  import FirstRunBanner from './lib/components/FirstRunBanner.svelte';
  import GeneralSection from './lib/components/GeneralSection.svelte';
  import HotkeysSection from './lib/components/HotkeysSection.svelte';
  import ReplaySection from './lib/components/ReplaySection.svelte';
  import StatusBar from './lib/components/StatusBar.svelte';
  import { SettingsStore } from './lib/store.svelte';

  const store = new SettingsStore();

  onMount(() => {
    void store.start();
    return () => store.dispose();
  });
</script>

<main>
  <header class="brand">
    <img class="logo" src={icon} alt="" width="28" height="28" />
    <h1>Repeat that?!</h1>
  </header>

  {#if store.settings}
    {@const settings = store.settings}
    <StatusBar {store} {settings} />

    {#if store.error}
      <div class="error" role="alert">
        <p>{store.error}</p>
        <button type="button" onclick={() => store.dismissError()}>Dismiss</button>
      </div>
    {/if}

    <FirstRunBanner {settings} />
    <AppList {store} {settings} />
    <ReplaySection {store} {settings} />
    <DuckingSection {store} {settings} />
    <DetectionSection {store} {settings} />
    <HotkeysSection {store} {settings} />
    <GeneralSection {store} {settings} />
  {:else if store.loadError}
    <div class="error" role="alert">
      <p>Can't reach the app: {store.loadError}</p>
      <button type="button" onclick={() => store.load()}>Try again</button>
    </div>
  {:else}
    <p role="status" class="muted">Loading…</p>
  {/if}
</main>

<style>
  main {
    display: flex;
    flex-direction: column;
    gap: 0.85rem;
    max-width: 34rem;
    margin: 0 auto;
    padding: 0.85rem 1rem 2rem;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }

  h1 {
    font-size: 1.15rem;
    font-weight: 650;
  }

  .error {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.6rem 0.9rem;
    border-radius: var(--radius);
    background: var(--danger-bg);
    color: var(--danger);
  }
</style>
