<script lang="ts">
  import type { Settings } from '../api';

  let { settings }: { settings: Settings } = $props();
</script>

<!-- First run only; gone as soon as an app is picked (which also marks the first run done). -->
{#if !settings.firstRunDone && settings.target === null}
  <aside class="banner">
    <p class="title">Pick the app you want to catch up on</p>
    <p>
      Repeat that?! keeps the last few seconds of that app's speech in memory, so you can replay what you just
      missed.
    </p>
  </aside>
{/if}

<style>
  .banner {
    border: 1px solid var(--accent);
    border-radius: var(--radius);
    padding: 0.75rem 1rem;
    background: color-mix(in srgb, var(--accent) 10%, var(--surface));
  }

  .title {
    font-weight: 650;
  }
</style>
