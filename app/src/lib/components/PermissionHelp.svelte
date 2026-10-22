<script lang="ts">
  import { onMount } from 'svelte';
  import {
    askForCapturePermission,
    capturePermissionHelp,
    openCapturePermissionSettings,
    toApiError,
    type PermissionHelp,
  } from '../api';

  // How to allow capture is the backend's sentence, in the status bar above. This has what it doesn't say: the buttons
  // the OS has (macOS 15 and later can open the settings, 14.x can only ask again), and what to do if a switch turned
  // on in the settings isn't picked up.
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
  {#if help?.canOpenSettings}
    <p class="note muted">If capturing doesn't resume once it's turned on, quit and reopen Repeat That.</p>
  {/if}
  {#if failure}<p class="note problem" role="alert">{failure}</p>{/if}
</div>

<style>
  .permission {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .permission:empty {
    display: none;
  }

  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
  }

  .note {
    font-size: 0.85rem;
  }

  .problem {
    color: var(--warn);
  }
</style>
