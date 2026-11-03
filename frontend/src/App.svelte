<script lang="ts">
  import { onMount } from 'svelte';
  import { RefreshCw, Monitor, Moon, Sun, Circle, Save, RotateCcw } from '@lucide/svelte';
  import SettingToggle from './lib/SettingToggle.svelte';
  import { Toggle } from './lib/components/ui/toggle';
  import { Button } from './lib/components/ui/button';
  import { Dashboard } from './lib/dashboard.svelte';
  import {
    blendMode,
    blendModes,
    colourModes,
    faders,
    responseMode,
    responseModes,
    type LiveSettings,
  } from './lib/settings';
  import Segmented from './lib/Segmented.svelte';
  import Fader from './lib/Fader.svelte';
  import Preview from './lib/Preview.svelte';
  import Devices from './lib/Devices.svelte';
  import InstallApp from './lib/InstallApp.svelte';
  import BlackFrameSettings from './lib/BlackFrameSettings.svelte';
  import Diagnostics from './lib/Diagnostics.svelte';
  import { vibrate } from './lib/haptics';
  import { registerAgentTools } from './lib/agent-tools';
  import { detectTv, installTvNavigation } from './lib/tv';
  const model = new Dashboard();
  const tv = detectTv();
  let tab = $state('tuning'),
    theme = $state('system'),
    now = $state(Date.now());
  let help = $state({
    title: 'Response controls',
    text: 'Touch or focus a fader to see what it changes.',
  });
  let presetHelp = $state('Combine colour, response and spatial blending independently.');
  let settings = $derived(model.settings);
  const responseOptions = [
    { value: 'low', label: 'Slow', help: 'Gentler motion and smaller scene-change steps.' },
    {
      value: 'default',
      label: 'Default',
      help: 'Balanced responsiveness and scene-change limiting.',
    },
    { value: 'fast', label: 'Fast', help: 'Faster transitions and larger scene-change steps.' },
    {
      value: 'instant',
      label: 'Instant',
      help: 'No temporal smoothing and room bulbs at full speed; lowest delay, most flicker.',
    },
  ];
  const blendOptions = [
    {
      value: 'local',
      label: 'Local',
      help: 'Only the light’s own screen region, without neighbour blending.',
    },
    {
      value: 'balanced',
      label: 'Balanced',
      help: 'Mix 15% of nearby regions for gentle spatial transitions.',
    },
    {
      value: 'soft',
      label: 'Soft',
      help: 'Mix 30% of nearby regions for a more unified ambient wash.',
    },
  ];
  const depthOptions = [
    {
      value: '0',
      label: 'Off',
      help: 'Keep the same base sampling footprint at every room depth.',
    },
    {
      value: '0.5',
      label: 'Mild',
      help: 'Gently widen sampling toward the rear of the room, up to 31.25% before clipping.',
    },
    {
      value: '1',
      label: 'Strong',
      help: 'Widen sampling toward the rear of the room, capped at 37.5% before clipping.',
    },
  ];
  let depthHelp = $state(
    'Changes sampling size using Hue room depth; keeps channel positions unchanged.',
  );
  const tabs = [
    ['tuning', 'Tuning'],
    ['devices', 'Devices'],
    ['diagnostics', 'Diagnostics'],
  ];
  const demo = import.meta.env.DEV;
  function changeResponse(value: string) {
    const [s, r, f, max] = responseModes[value as keyof typeof responseModes];
    model.patch({
      smoothing_factor: s,
      rise_smoothing_factor: r,
      fall_smoothing_factor: f,
      max_color_step: max,
      // The gentle room response would still slow single-channel bulbs.
      ...(value === 'instant' ? { gentle_room_lights: false } : {}),
    });
  }
  function changeBlend(value: string) {
    model.patch({ spatial_blend: blendModes[value as keyof typeof blendModes] });
  }
  function changeSetting(key: keyof LiveSettings, value: number) {
    model.patch(
      key === 'smoothing_factor'
        ? { smoothing_factor: value, rise_smoothing_factor: value, fall_smoothing_factor: value }
        : { [key]: value },
    );
  }
  function cycleTheme() {
    theme = ['system', 'dark', 'light'][(['system', 'dark', 'light'].indexOf(theme) + 1) % 3];
    applyTheme();
  }
  function applyTheme() {
    if (theme === 'system') delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = theme;
    try {
      localStorage.setItem('lg-hue-sync-theme', theme);
    } catch {
      /* Storage may be unavailable. */
    }
  }
  function tabKey(event: KeyboardEvent, index: number) {
    const direction = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0;
    if (!direction && event.key !== 'Home' && event.key !== 'End') return;
    event.preventDefault();
    tab =
      tabs[
        event.key === 'Home'
          ? 0
          : event.key === 'End'
            ? tabs.length - 1
            : (index + direction + tabs.length) % tabs.length
      ][0];
    document.getElementById(`tab-${tab}`)?.focus();
  }
  onMount(() => {
    try {
      theme = localStorage.getItem('lg-hue-sync-theme') || 'system';
    } catch {
      /* Storage may be unavailable. */
    }
    applyTheme();
    let removeTvNavigation = () => {};
    if (tv) {
      document.documentElement.dataset.tv = '';
      removeTvNavigation = installTvNavigation({
        // Back steps out to the main view first, then leaves the app.
        onBack: () => {
          if (tab !== 'tuning') tab = 'tuning';
          else window.close();
        },
      });
    }
    const stop = model.start();
    const unregister = registerAgentTools(model);
    const clock = setInterval(() => (now = Date.now()), 1000);
    return () => {
      removeTvNavigation();
      stop();
      unregister();
      clearInterval(clock);
    };
  });
</script>

<svelte:document
  onclick={(event) => {
    if (event.isTrusted && (event.target as Element).closest('button:not(:disabled), summary'))
      vibrate();
  }}
/>
<main>
  {#if demo}<p class="preview-badge">
      <span class="status-dot active"></span>Local preview · simulated TV and lights
    </p>{/if}
  <header class="header-actions" aria-label="TV and screen sync controls">
    <Toggle
      variant="outline"
      class="power-button"
      bind:pressed={
        () => model.requested,
        (pressed) => model.command(pressed ? '/api/start' : '/api/stop').catch(() => {})
      }
      disabled={!model.status || model.busy}
      ><RefreshCw size={17} />Screen sync {model.requested ? 'On' : 'Off'}</Toggle
    >
    <Toggle
      variant="outline"
      class="power-button"
      bind:pressed={
        () => model.display === true, () => model.command('/api/display/toggle').catch(() => {})
      }
      disabled={model.display === null || model.busy}
      ><Monitor size={17} />Display {model.display === null
        ? '—'
        : model.display
          ? 'On'
          : 'Off'}</Toggle
    >
    {#if tv}
      <Toggle
        variant="outline"
        class="power-button"
        bind:pressed={
          () => settings?.hue_sync_enabled ?? false,
          (pressed) => model.patch({ hue_sync_enabled: pressed })
        }
        disabled={!settings || model.busy}>Hue {settings?.hue_sync_enabled ? 'On' : 'Off'}</Toggle
      >
      <Toggle
        variant="outline"
        class="power-button"
        bind:pressed={
          () => settings?.nanoleaf_sync_enabled ?? false,
          (pressed) => model.patch({ nanoleaf_sync_enabled: pressed })
        }
        disabled={!settings || model.busy}
        >Nanoleaf {settings?.nanoleaf_sync_enabled ? 'On' : 'Off'}</Toggle
      >
    {/if}
    <Button
      variant="outline"
      class="icon-button"
      aria-label={`Theme: ${theme}. Change appearance`}
      title={`Theme: ${theme}`}
      onclick={cycleTheme}
      >{#if theme === 'light'}<Sun />{:else if theme === 'dark'}<Moon />{:else}<Circle
        />{/if}</Button
    >
    {#if tv}<Button variant="outline" onclick={() => window.close()}>Exit</Button>{/if}
  </header>
  {#if model.error}<div class="notice error" role="alert">{model.error}</div>{/if}
  <div class="dashboard-tabs" role="tablist" aria-label="Dashboard views">
    {#each tabs as [value, label], index (value)}<Button
        variant="ghost"
        id={`tab-${value}`}
        role="tab"
        aria-controls={`panel-${value}`}
        aria-selected={tab === value}
        tabindex={tab === value ? 0 : -1}
        onclick={() => (tab = value)}
        onkeydown={(event) => tabKey(event, index)}>{label}</Button
      >{/each}
  </div>
  <div id="panel-tuning" role="tabpanel" aria-labelledby="tab-tuning" hidden={tab !== 'tuning'}>
    <section class="card preset-panel">
      <h2>Ambient style</h2>
      <Segmented
        label="Colour"
        options={colourModes}
        value={model.status?.active_preset ?? ''}
        disabled={!settings || model.busy}
        onchange={(value) => model.preset(value)}
        onhelp={(text) => (presetHelp = text)}
      />
      <Segmented
        label="Response"
        options={responseOptions}
        value={responseMode(settings)}
        disabled={!settings || model.busy}
        onchange={changeResponse}
        onhelp={(text) => (presetHelp = text)}
      />
      <Segmented
        label="Spatial blending"
        options={blendOptions}
        value={blendMode(settings)}
        disabled={!settings || model.busy}
        onchange={changeBlend}
        onhelp={(text) => (presetHelp = text)}
      />
      <p class="preset-help muted">{presetHelp}</p>
      <p class="save-state" role="status" aria-live="polite">
        {model.saving
          ? 'Saving…'
          : model.dirty
            ? 'Saving changes…'
            : model.online
              ? 'Changes saved automatically.'
              : 'Connecting to daemon…'}
      </p>
    </section>
    <Preview status={model.status} {settings} active={tab === 'tuning'} />
    <section class="card tuning-panel">
      <div class="panel-heading">
        <h2>Advanced tuning</h2>
        <div class="button-row">
          <Button
            variant="outline"
            class="icon-button"
            aria-label="Save settings to disk"
            title="Save now"
            disabled={!settings || model.busy || model.saving}
            onclick={() => model.save()}><Save /></Button
          ><Button
            variant="outline"
            class="icon-button"
            aria-label="Reset colour to factory defaults"
            title="Reset colour; response and blend stay unchanged"
            disabled={!model.status?.active_preset || model.busy}
            onclick={() => {
              if (model.status?.active_preset) void model.preset(model.status.active_preset, true);
            }}><RotateCcw /></Button
          >
        </div>
      </div>
      <div class="fader-bank" role="group" aria-label="Colour and response faders">
        {#each faders as fader (fader.key)}<div class="fader-card">
            <Fader
              label={fader.label}
              value={settings?.[fader.key] ?? fader.stops[0]}
              stops={fader.stops}
              format={fader.format}
              help={fader.help}
              vertical={!tv}
              disabled={!settings || model.busy}
              onchange={(value) => changeSetting(fader.key, value)}
              onhelp={() => (help = { title: fader.label, text: fader.help })}
            />{#if fader.key === 'noise_gate_threshold'}<SettingToggle
                compact
                title="Strict black"
                checked={settings?.strict_blackout ?? false}
                disabled={!settings || model.busy}
                onchange={(checked) => model.patch({ strict_blackout: checked })}
              />{/if}
          </div>{/each}
      </div>
      <aside class="tuning-help" aria-live="polite">
        <strong>{help.title}</strong>
        <p>{help.text}</p>
      </aside>
      <details class="processing">
        <summary>Processing options</summary>
        <div class="section-body">
          <BlackFrameSettings
            {settings}
            disabled={model.busy}
            onchange={(patch) => model.patch(patch)}
          />
          <Segmented
            label="Depth influence"
            options={depthOptions}
            value={String(settings?.depth_influence ?? 0.5)}
            disabled={!settings || model.busy}
            onchange={(value) => model.patch({ depth_influence: Number(value) })}
            onhelp={(text) => (depthHelp = text)}
          />
          <p class="muted">{depthHelp}</p>
          <SettingToggle
            title="Direct RGB mode"
            description="Maximum Hue brightness and punch; off uses CIE 1931 xy."
            checked={settings ? !settings.use_xy_gamut : false}
            disabled={!settings || model.busy}
            onchange={(checked) => model.patch({ use_xy_gamut: !checked })}
          />

          <SettingToggle
            title="Gentle room lights"
            description="Slower, steadier response for single-channel Hue bulbs. Off follows the screen as fast as the gradients."
            checked={settings?.gentle_room_lights ?? true}
            disabled={!settings || model.busy}
            onchange={(checked) => model.patch({ gentle_room_lights: checked })}
          />

          <SettingToggle
            title="Instant scene cuts"
            description="Hard cuts land in one frame instead of ramping. This removes the flash-safety limit: strobing content will strobe the room. Avoid with photosensitive viewers."
            checked={settings?.instant_scene_cuts ?? false}
            disabled={!settings || model.busy}
            onchange={(checked) => model.patch({ instant_scene_cuts: checked })}
          />

          <SettingToggle
            title="Letterbox compensation"
            description="Sample active video inside black bars."
            checked={settings?.letterbox_detection ?? false}
            disabled={!settings || model.busy}
            onchange={(checked) => model.patch({ letterbox_detection: checked })}
          />

          <SettingToggle
            title="Midtone lift (legacy)"
            description="Legacy brightness curve; not HDR correction."
            checked={settings?.hdr_tone_mapping ?? false}
            disabled={!settings || model.busy}
            onchange={(checked) => model.patch({ hdr_tone_mapping: checked })}
          />
        </div>
      </details>
    </section>
  </div>
  <div id="panel-devices" role="tabpanel" aria-labelledby="tab-devices" hidden={tab !== 'devices'}>
    <Devices {model} />
    <InstallApp />
  </div>
  <div
    id="panel-diagnostics"
    role="tabpanel"
    aria-labelledby="tab-diagnostics"
    hidden={tab !== 'diagnostics'}
  >
    <Diagnostics status={model.status} receivedAt={model.receivedAt} online={model.online} {now} />
  </div>
</main>
