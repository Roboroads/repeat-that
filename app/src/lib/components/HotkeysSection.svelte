<script lang="ts">
  import { onDestroy } from 'svelte';
  import { suspendHotkeys, type HotkeyName, type Settings } from '../api';
  import { DEFAULT_HOTKEYS } from '../defaults';
  import { formatAccel, heldModifiers, interpretKey } from '../hotkey';
  import type { SettingsStore } from '../store.svelte';
  import Section from './Section.svelte';

  interface Props {
    store: SettingsStore;
    settings: Settings;
  }

  let { store, settings }: Props = $props();

  const FIELDS: Array<{ name: HotkeyName; label: string }> = [
    { name: 'replay', label: 'Replay' },
    { name: 'stop', label: 'Stop' },
    { name: 'catchUp', label: 'Catch up' },
  ];

  /** The field that is waiting for a combination, if any. */
  let recording = $state<HotkeyName | null>(null);
  /** Modifiers held right now while recording, shown as "Ctrl+Alt+…". */
  let held = $state<string[]>([]);
  /** Why the last key press was not accepted. */
  let hint = $state<string | null>(null);
  /** The backend's refusal of the last combination, per field. */
  let errors = $state<Partial<Record<HotkeyName, string>>>({});
  /** The code of a key whose press was handled; its release must not click the button. */
  let swallowed: string | null = null;

  // The global hotkeys are switched off while recording, so pressing a combination does not also run its action.
  // The calls go one after the other: the backend must see "on" and "off" in the order they happened.
  let suspendChain: Promise<unknown> = Promise.resolve();
  function suspend(suspended: boolean) {
    suspendChain = suspendChain
      .then(() => suspendHotkeys(suspended))
      .catch((error: unknown) => console.warn('suspend_hotkeys failed', error));
  }

  const isDefault = $derived(FIELDS.every(({ name }) => settings.hotkeys[name] === DEFAULT_HOTKEYS[name]));

  function begin(name: HotkeyName) {
    if (recording === name) return;
    const idle = recording === null;
    recording = name;
    held = [];
    hint = null;
    errors = { ...errors, [name]: undefined };
    if (idle) suspend(true);
  }

  /** Leaves recording and gives the hotkeys back. */
  function end() {
    if (recording === null) return;
    recording = null;
    held = [];
    hint = null;
    suspend(false);
  }

  async function commit(name: HotkeyName, accel: string | null) {
    recording = null;
    held = [];
    hint = null;
    if (accel !== settings.hotkeys[name]) {
      const error = await store.update(
        { hotkeys: { ...settings.hotkeys, [name]: accel } },
        { optimistic: false, report: false },
      );
      if (error) errors = { ...errors, [name]: error.message };
    }
    // Another field may have started recording while the backend was busy.
    if (recording === null) suspend(false);
  }

  function onkeydown(event: KeyboardEvent, name: HotkeyName) {
    if (recording !== name) return;
    const outcome = interpretKey(event);
    if (outcome.kind === 'ignore') return;
    event.preventDefault();
    swallowed = event.code;
    switch (outcome.kind) {
      case 'pending':
        held = outcome.modifiers;
        hint = null;
        break;
      case 'reject':
        held = [];
        hint = outcome.reason;
        break;
      case 'cancel':
        end();
        break;
      case 'clear':
        void commit(name, null);
        break;
      case 'bind':
        void commit(name, outcome.accel);
        break;
    }
  }

  function onkeyup(event: KeyboardEvent, name: HotkeyName) {
    // Keep the "Ctrl+Alt+…" preview in step with what is still held.
    if (recording === name) held = heldModifiers(event);
    if (swallowed === event.code) {
      swallowed = null;
      event.preventDefault();
    }
  }

  function reset() {
    errors = {};
    void store.update({ hotkeys: { ...DEFAULT_HOTKEYS } }, { optimistic: false });
  }

  onDestroy(() => {
    if (recording !== null) suspend(false);
  });

  const STATE_TEXT = { inUse: 'In use by another app', invalid: 'Invalid', unbound: 'Not set' } as const;

  function fieldState(name: HotkeyName) {
    if (settings.hotkeys[name] === null) return 'unbound';
    return store.status?.hotkeys[name] ?? 'ok';
  }

  function describe(name: HotkeyName, label: string): string {
    if (recording === name) return `${label} hotkey: press a combination`;
    const keys = formatAccel(settings.hotkeys[name]);
    return `${label} hotkey: ${keys.length > 0 ? keys.join('+') : 'not set'}`;
  }
</script>

<Section title="Hotkeys" id="hotkeys">
  {#snippet actions()}
    <button type="button" disabled={isDefault} onclick={reset}>Reset to defaults</button>
  {/snippet}

  {#each FIELDS as { name, label } (name)}
    {@const state = fieldState(name)}
    <div class="hotkey">
      <span class="label" aria-hidden="true">{label}</span>
      <button
        type="button"
        class="field"
        class:recording={recording === name}
        aria-label={describe(name, label)}
        onclick={(event) => {
          // Safari and Firefox on macOS do not focus a button on click; the key presses must reach this one.
          event.currentTarget.focus();
          begin(name);
        }}
        onkeydown={(event) => onkeydown(event, name)}
        onkeyup={(event) => onkeyup(event, name)}
        onblur={() => {
          if (recording === name) end();
        }}
      >
        {#if recording === name}
          <span class="placeholder">{held.length > 0 ? `${held.join('+')}+…` : 'Press a combination'}</span>
        {:else if settings.hotkeys[name] !== null}
          {#each formatAccel(settings.hotkeys[name]) as key, index (index)}
            {#if index > 0}<span class="plus">+</span>{/if}<kbd>{key}</kbd>
          {/each}
        {:else}
          <span class="placeholder">Click to set</span>
        {/if}
      </button>
      {#if recording === name}
        <p class="note">{hint ?? 'Esc cancels, Backspace clears.'}</p>
      {:else if errors[name]}
        <p class="note problem" role="alert">{errors[name]}</p>
      {:else if state !== 'ok'}
        <p class="note" class:problem={state !== 'unbound'}>{STATE_TEXT[state]}</p>
      {/if}
    </div>
  {/each}
</Section>

<style>
  .hotkey {
    display: grid;
    grid-template-columns: 5.5rem 1fr;
    grid-template-areas:
      'label field'
      '. note';
    align-items: center;
    column-gap: 0.75rem;
    row-gap: 0.2rem;
  }

  .label {
    grid-area: label;
  }

  .field {
    grid-area: field;
    display: flex;
    align-items: center;
    gap: 0.2rem;
    min-height: 2.2rem;
    padding: 0.2rem 0.6rem;
    text-align: left;
    background: var(--surface);
  }

  .field.recording {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent) 35%, transparent);
  }

  .plus,
  .placeholder {
    color: var(--text-muted);
  }

  .note {
    grid-area: note;
    font-size: 0.85rem;
    color: var(--text-muted);
  }

  .note.problem {
    color: var(--warn);
  }
</style>
