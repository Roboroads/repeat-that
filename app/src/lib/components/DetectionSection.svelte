<script lang="ts">
  import type { Sensitivity, Settings } from '../api';
  import type { SettingsStore } from '../store.svelte';
  import Section from './Section.svelte';

  interface Props {
    store: SettingsStore;
    settings: Settings;
  }

  let { store, settings }: Props = $props();

  const OPTIONS: Array<{ value: Sensitivity; label: string; hint: string }> = [
    { value: 'low', label: 'Low', hint: 'Ignores background noise, but may miss quiet speech.' },
    { value: 'normal', label: 'Normal', hint: 'Balanced for most calls.' },
    { value: 'high', label: 'High', hint: 'Catches quiet speech, but may also pick up background noise.' },
  ];

  const hint = $derived(OPTIONS.find((o) => o.value === settings.sensitivity)?.hint ?? '');
</script>

<Section title="Detection" id="detection">
  <div class="segmented" role="radiogroup" aria-label="Speech detection sensitivity" aria-describedby="sensitivity-hint">
    {#each OPTIONS as option (option.value)}
      <label class:active={settings.sensitivity === option.value}>
        <input
          type="radio"
          name="sensitivity"
          value={option.value}
          checked={settings.sensitivity === option.value}
          onchange={() => store.update({ sensitivity: option.value })}
        />
        {option.label}
      </label>
    {/each}
  </div>
  <p id="sensitivity-hint" class="muted" data-testid="sensitivity-hint">{hint}</p>
</Section>

<style>
  .segmented {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    border: 1px solid var(--border);
    border-radius: 8px;
    overflow: hidden;
  }

  label {
    position: relative;
    padding: 0.35rem 0.5rem;
    text-align: center;
    cursor: pointer;
    background: var(--surface-2);
  }

  label + label {
    border-left: 1px solid var(--border);
  }

  label.active {
    background: var(--accent);
    color: var(--accent-text);
    font-weight: 600;
  }

  /* The radio itself stays in the layout for keyboard and screen reader users. */
  input {
    position: absolute;
    inset: 0;
    opacity: 0;
    margin: 0;
    cursor: pointer;
  }

  label:has(input:focus-visible) {
    outline: 2px solid var(--accent);
    outline-offset: -3px;
  }

  p {
    font-size: 0.9rem;
  }
</style>
