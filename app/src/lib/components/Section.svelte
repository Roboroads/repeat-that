<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    title: string;
    /** Prefix for the heading id; unique on the page. */
    id: string;
    /** Buttons at the right of the heading. */
    actions?: Snippet;
    children: Snippet;
  }

  let { title, id, actions, children }: Props = $props();
</script>

<section class="card" aria-labelledby="{id}-title">
  <header>
    <h2 id="{id}-title">{title}</h2>
    {#if actions}<div class="actions">{@render actions()}</div>{/if}
  </header>
  <div class="body">{@render children()}</div>
</section>

<style>
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 0.85rem 1rem 1rem;
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    min-height: 1.8rem;
  }

  .body {
    display: flex;
    flex-direction: column;
    gap: 0.9rem;
    margin-top: 0.5rem;
  }
</style>
