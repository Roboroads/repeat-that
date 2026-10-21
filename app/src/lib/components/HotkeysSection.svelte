<script lang="ts">
  import { onDestroy } from 'svelte';
  import { configureShortcuts, suspendHotkeys, toApiError, type HotkeyName, type Settings } from '../api';
  import { copyText } from '../clipboard';
  import { DEFAULT_HOTKEYS } from '../defaults';
  import { formatAccel, heldModifiers, interpretKey } from '../hotkey';
  import type { SettingsStore } from '../store.svelte';
  import Section from './Section.svelte';

  interface Props {
    store: SettingsStore;
    settings: Settings;
  }

  let { store, settings }: Props = $props();

  /** How the hotkeys work here. The recorder is for `plugin`; `portal` shows what the desktop assigned; `none` has commands. */
  const backend = $derived(store.status?.hotkeyBackend ?? 'plugin');

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
    const error = await store.update((current) => ({ hotkeys: { ...current.hotkeys, [name]: accel } }), {
      optimistic: false,
      report: false,
    });
    if (error) errors = { ...errors, [name]: error.message };
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

  // Where the desktop owns the keys (the portal).
  /** The desktop can't be asked to show its settings, so the window says where they are. */
  let settingsElsewhere = $state(false);
  /** Why asking failed. */
  let configureError = $state<string | null>(null);

  async function openSystemSettings() {
    settingsElsewhere = false;
    configureError = null;
    try {
      settingsElsewhere = (await configureShortcuts()) === 'unsupported';
    } catch (raw) {
      configureError = toApiError(raw).message;
    }
  }

  // Where there are no hotkeys: the commands to bind by hand.
  /** The action whose command was copied a moment ago. */
  let copied = $state<HotkeyName | null>(null);
  let copiedTimer: ReturnType<typeof setTimeout> | null = null;

  async function copy(name: HotkeyName, command: string) {
    if (!(await copyText(command))) return;
    copied = name;
    if (copiedTimer !== null) clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => (copied = null), 2000);
  }

  onDestroy(() => {
    if (recording !== null) suspend(false);
    if (copiedTimer !== null) clearTimeout(copiedTimer);
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
    {#if backend === 'plugin'}
      <button type="button" disabled={isDefault} onclick={reset}>Reset to defaults</button>
    {/if}
  {/snippet}

  {#if backend === 'portal'}
    <p class="note">Your desktop decides which keys these use.</p>
    {#each FIELDS as { name, label } (name)}
      {@const trigger = store.status?.hotkeyTriggers[name] ?? null}
      <div class="hotkey">
        <span class="label">{label}</span>
        <span class="field assigned">
          {#if trigger !== null}<kbd>{trigger}</kbd>{:else}<span class="placeholder">Not assigned</span>{/if}
        </span>
      </div>
    {/each}
    <div class="system">
      <button type="button" onclick={openSystemSettings}>Change in system settings</button>
    </div>
    {#if settingsElsewhere}
      <p class="note">
        This desktop can't open them from here. In KDE, look under System Settings → Shortcuts → Repeat That. In GNOME,
        under Settings → Keyboard → View and Customize Shortcuts → Apps.
      </p>
    {/if}
    {#if configureError}
      <p class="note problem" role="alert">{configureError}</p>
    {/if}
  {:else if backend === 'none'}
    <p class="warning">Global hotkeys aren't available on this desktop.</p>
    <p class="note">
      Bind each command below as a custom keyboard shortcut in your desktop's settings (in GNOME: Settings → Keyboard →
      View and Customize Shortcuts → Custom Shortcuts). Each one acts on the running app.
    </p>
    {#each FIELDS as { name, label } (name)}
      {@const command = store.status?.hotkeyCommands?.[name]}
      {#if command}
        <div class="command">
          <span class="label">{label}</span>
          <code>{command}</code>
          <button
            type="button"
            aria-label="{copied === name ? 'Copied' : 'Copy'} the {label} command"
            onclick={() => copy(name, command)}
          >
            {copied === name ? 'Copied' : 'Copy'}
          </button>
        </div>
      {/if}
    {/each}
  {:else}
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
  {/if}
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

  /* What the desktop assigned is shown like a key field but is not a button. */
  .field.assigned {
    border: 1px solid var(--border);
    border-radius: var(--radius);
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

  .system {
    display: flex;
    justify-content: flex-start;
  }

  .warning {
    padding: 0.45rem 0.7rem;
    border-radius: 8px;
    background: var(--warn-bg);
    color: var(--warn);
    font-size: 0.9rem;
  }

  .command {
    display: grid;
    grid-template-columns: 5.5rem 1fr auto;
    align-items: center;
    column-gap: 0.75rem;
  }

  .command code {
    min-width: 0;
    overflow-wrap: anywhere;
    padding: 0.25rem 0.5rem;
    border-radius: 6px;
    background: var(--surface);
    border: 1px solid var(--border);
    font-size: 0.85rem;
    user-select: all;
  }
</style>
