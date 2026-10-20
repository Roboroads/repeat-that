<script lang="ts">
  import type { Settings } from '../api';
  import type { SettingsStore } from '../store.svelte';
  import Section from './Section.svelte';
  import Slider from './Slider.svelte';
  import Switch from './Switch.svelte';

  interface Props {
    store: SettingsStore;
    settings: Settings;
  }

  let { store, settings }: Props = $props();
</script>

<Section title="Ducking" id="ducking">
  <Switch
    label="Lower the app's volume during a replay"
    description="So the replay is easy to hear over the call. The volume comes back when you are caught up."
    checked={settings.duck.enabled}
    onchange={(enabled) => store.update({ duck: { ...settings.duck, enabled } })}
  />
  <!-- The slider works in whole percent; the backend stores a fraction. -->
  <Slider
    id="duck-level"
    label="Volume during replay"
    value={Math.round(settings.duck.level * 100)}
    min={5}
    max={100}
    step={5}
    format={(v) => `${v}%`}
    disabled={!settings.duck.enabled}
    oncommit={(percent) => store.update({ duck: { ...settings.duck, level: percent / 100 } })}
  />
</Section>
