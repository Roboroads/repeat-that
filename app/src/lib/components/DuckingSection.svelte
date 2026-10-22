<script lang="ts">
  import type { Settings } from '../api';
  import { duckLevelLimits } from '../defaults';
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
  // Down to 0 on macOS, where that mutes the app during a replay; 5% elsewhere.
  const limits = duckLevelLimits();
</script>

<Section title="Ducking" id="ducking">
  <Switch
    label="Lower the app's volume during a replay"
    description="So the replay is easy to hear over the call. The volume comes back when you are caught up."
    checked={settings.duck.enabled}
    onchange={(enabled) => store.update((current) => ({ duck: { ...current.duck, enabled } }))}
  />
  <Slider
    id="duck-level"
    label="Volume during replay"
    value={percent(settings.duck.level)}
    min={percent(limits.min)}
    max={percent(limits.max)}
    step={percent(limits.step)}
    format={(v) => (v === 0 ? 'Muted' : `${v}%`)}
    disabled={!settings.duck.enabled}
    oncommit={(value) => store.update((current) => ({ duck: { ...current.duck, level: value / 100 } }))}
  />
</Section>
