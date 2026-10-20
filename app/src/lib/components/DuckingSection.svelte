<script lang="ts">
  import type { Settings } from '../api';
  import { LIMITS } from '../defaults';
  import type { SettingsStore } from '../store.svelte';
  import Section from './Section.svelte';
  import Slider from './Slider.svelte';
  import Switch from './Switch.svelte';

  interface Props {
    store: SettingsStore;
    settings: Settings;
  }

  let { store, settings }: Props = $props();

  // The slider works in whole percent; the backend stores a fraction.
  const percent = (fraction: number) => Math.round(fraction * 100);
</script>

<Section title="Ducking" id="ducking">
  <Switch
    label="Lower the app's volume during a replay"
    description="So the replay is easy to hear over the call. The volume comes back when you are caught up."
    checked={settings.duck.enabled}
    onchange={(enabled) => store.update({ duck: { ...settings.duck, enabled } })}
  />
  <Slider
    id="duck-level"
    label="Volume during replay"
    value={percent(settings.duck.level)}
    min={percent(LIMITS.duckLevel.min)}
    max={percent(LIMITS.duckLevel.max)}
    step={percent(LIMITS.duckLevel.step)}
    format={(v) => `${v}%`}
    disabled={!settings.duck.enabled}
    oncommit={(value) => store.update({ duck: { ...settings.duck, level: value / 100 } })}
  />
</Section>
