<script lang="ts">
  interface Props {
    id: string;
    label: string;
    value: number;
    min: number;
    max: number;
    step: number;
    /** The text for a value, shown live while dragging. */
    format: (value: number) => string;
    /** Called once when the thumb is released (or a key press ends). */
    oncommit: (value: number) => void;
    disabled?: boolean;
    hint?: string;
  }

  let { id, label, value, min, max, step, format, oncommit, disabled = false, hint }: Props = $props();

  // While dragging the slider shows its own value; otherwise the saved one.
  let dragging = $state<number | null>(null);
  const shown = $derived(dragging ?? value);
</script>

<div class="slider">
  <div class="head">
    <label for={id}>{label}</label>
    <output for={id}>{format(shown)}</output>
  </div>
  <input
    {id}
    type="range"
    {min}
    {max}
    {step}
    {disabled}
    value={shown}
    aria-valuetext={format(shown)}
    aria-describedby={hint ? `${id}-hint` : undefined}
    oninput={(e) => (dragging = e.currentTarget.valueAsNumber)}
    onchange={(e) => {
      dragging = null;
      oncommit(e.currentTarget.valueAsNumber);
    }}
  />
  {#if hint}<p id="{id}-hint" class="hint muted">{hint}</p>{/if}
</div>

<style>
  .head {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
  }

  output {
    font-variant-numeric: tabular-nums;
    color: var(--text-muted);
  }

  input {
    width: 100%;
    margin: 0.25rem 0 0;
    accent-color: var(--accent);
  }

  .hint {
    font-size: 0.85rem;
  }
</style>
