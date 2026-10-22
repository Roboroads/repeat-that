<script lang="ts">
  import { onMount } from 'svelte';
  import {
    askForCapturePermission,
    capturePermissionHelp,
    openCapturePermissionSettings,
    toApiError,
    type PermissionHelp,
  } from '../api';

  interface Props {
    /** What the backend says about allowing capture; shown until the steps for this OS are known, or without them. */
    message: string | null;
  }

  let { message }: Props = $props();

  // What the OS lets the user do from here (macOS: 15 and later can open the settings, 14.x can only ask again).
  let help = $state<PermissionHelp | null>(null);
  let busy = $state(false);
  let failure = $state<string | null>(null);

  onMount(async () => {
    try {
      help = await capturePermissionHelp();
    } catch (raw) {
      failure = toApiError(raw).message;
    }
  });

  const steps = $derived(
    help?.canOpenSettings
      ? "Open System Settings, Privacy & Security, Screen & System Audio Recording, turn on Repeat That, then quit and reopen Repeat That if capturing doesn't resume."
      : help?.canAskAgain
        ? 'Click Ask again and choose Allow.'
        : message,
  );

  async function run(command: () => Promise<void>) {
    busy = true;
    failure = null;
    try {
      await command();
    } catch (raw) {
      failure = toApiError(raw).message;
    } finally {
      busy = false;
    }
  }
</script>

<!-- Capture is tried again every 2 s, so it starts by itself once the user has allowed it; this goes away then. -->
<div class="permission" role="group" aria-label="Allow audio capture">
  {#if steps}<p class="steps">{steps}</p>{/if}
  {#if help?.canOpenSettings || help?.canAskAgain}
    <div class="buttons">
      {#if help.canOpenSettings}
        <button type="button" disabled={busy} onclick={() => run(openCapturePermissionSettings)}>
          Open System Settings
        </button>
      {/if}
      {#if help.canAskAgain}
        <button type="button" disabled={busy} onclick={() => run(askForCapturePermission)}>Ask again</button>
      {/if}
    </div>
  {/if}
  {#if failure}<p class="problem" role="alert">{failure}</p>{/if}
</div>

<style>
  .permission {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .steps {
    line-height: 1.35;
  }

  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
  }

  .problem {
    font-size: 0.85rem;
    color: var(--warn);
  }
</style>
