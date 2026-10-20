<script lang="ts">
  import type { Settings } from '../api';
  import { LIMITS } from '../defaults';
  import type { SettingsStore } from '../store.svelte';
  import Section from './Section.svelte';
  import Slider from './Slider.svelte';

  interface Props {
    store: SettingsStore;
    settings: Settings;
  }

  let { store, settings }: Props = $props();

  const speed = (value: number) => `${value.toFixed(2)}×`;
  // Steps of 0.05 do not survive floating point arithmetic in every browser; the backend gets clean numbers.
  const clean = (value: number) => Math.round(value * 100) / 100;

  const stored = $derived(settings.outputDevice);
  const connected = $derived(stored === null || store.devices.some((device) => device.id === stored));
</script>

<Section title="Replay" id="replay">
  <Slider
    id="buffer"
    label="Buffer length"
    value={settings.bufferSeconds}
    {...LIMITS.bufferSeconds}
    format={(v) => `${v} s`}
    oncommit={(v) => store.update({ bufferSeconds: v })}
    hint="How much recent speech is kept. Longer uses more memory."
  />
  <Slider
    id="replay-speed"
    label="Replay speed"
    value={settings.replaySpeed}
    {...LIMITS.replaySpeed}
    format={speed}
    oncommit={(v) => store.update({ replaySpeed: clean(v) })}
  />
  <Slider
    id="catch-up-speed"
    label="Catch-up speed"
    value={settings.catchUpSpeed}
    {...LIMITS.catchUpSpeed}
    format={speed}
    oncommit={(v) => store.update({ catchUpSpeed: clean(v) })}
    hint="How fast a replay plays after the catch-up hotkey, to get back to live."
  />

  <div class="field">
    <label for="output-device">Output device</label>
    <select
      id="output-device"
      value={stored ?? ''}
      onfocus={() => store.refreshDevices()}
      onchange={(e) => store.update({ outputDevice: e.currentTarget.value === '' ? null : e.currentTarget.value })}
    >
      <option value="">System default</option>
      {#each store.devices as device (device.id)}
        <option value={device.id}>{device.name}</option>
      {/each}
      {#if stored !== null && store.devicesLoaded && !connected}
        <option value={stored}>{store.deviceName(stored)} (not connected)</option>
      {/if}
    </select>
  </div>
</Section>

<style>
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
</style>
