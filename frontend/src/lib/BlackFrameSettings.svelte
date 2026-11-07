<script lang="ts">
  import Fader from './Fader.svelte';
  import type { LiveSettings } from './settings';

  let {
    settings,
    disabled = false,
    onchange,
  }: {
    settings: LiveSettings | null;
    disabled?: boolean;
    onchange: (patch: Partial<LiveSettings>) => void;
  } = $props();

  const holdHelp =
    'Keep the last colours briefly when the whole screen goes black. Zero skips the hold.';
  const fadeHelp =
    'After the hold, fade the last colours to off. Zero switches off when the hold ends.';
  const seconds = (value: number) => `${Number((value / 1000).toFixed(2))} s`;
</script>

<fieldset class="black-frame-settings">
  <legend>Black-screen transitions</legend>
  <p class="muted">
    When the whole screen goes nearly black, the lights keep their last colours for a moment, then
    fade out. They come back as soon as the picture does. Strict black still switches off lights
    over dark parts of the screen. Set both times to zero to turn this off.
  </p>
  <Fader
    label="Black-screen hold"
    value={settings?.black_frame_hold_ms ?? 750}
    stops={[0, 250, 500, 750, 1000, 1500, 2000, 3000, 5000]}
    format={seconds}
    help={holdHelp}
    disabled={!settings || disabled}
    onchange={(value) => onchange({ black_frame_hold_ms: value })}
    onhelp={() => {}}
    vertical={false}
  />
  <Fader
    label="Black-screen fade"
    value={settings?.black_frame_fade_ms ?? 2000}
    stops={[0, 250, 500, 1000, 1500, 2000, 3000, 5000, 10000]}
    format={seconds}
    help={fadeHelp}
    disabled={!settings || disabled}
    onchange={(value) => onchange({ black_frame_fade_ms: value })}
    onhelp={() => {}}
    vertical={false}
  />
</fieldset>

<style>
  .black-frame-settings {
    display: grid;
    gap: 1rem;
    min-inline-size: 0;
    padding: 0;
    border: 0;
  }

  legend {
    font-weight: 600;
    margin-block-end: 0.5rem;
  }
</style>
