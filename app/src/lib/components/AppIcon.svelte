<script lang="ts">
  interface Props {
    /** A `data:` URL; `null` shows the generic icon. */
    src?: string | null;
    size?: number;
  }

  let { src = null, size = 28 }: Props = $props();

  // An icon that fails to load is replaced by the generic one, until a different one arrives.
  let failedSrc = $state<string | null>(null);
  const showImage = $derived(src !== null && src !== failedSrc);
</script>

{#if showImage}
  <img {src} alt="" width={size} height={size} onerror={() => (failedSrc = src)} />
{:else}
  <svg data-testid="generic-icon" width={size} height={size} viewBox="0 0 32 32" aria-hidden="true">
    <rect width="32" height="32" rx="7" fill="currentColor" opacity="0.14" />
    <path d="M8 13v6h4l5 4V9l-5 4H8z" fill="currentColor" opacity="0.7" />
    <path
      d="M20.5 12.5a5 5 0 0 1 0 7"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      opacity="0.7"
    />
  </svg>
{/if}

<style>
  img,
  svg {
    flex: none;
    border-radius: 6px;
    color: var(--text-muted);
  }
</style>
