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
    instantBundle,
    dynamicPatch,
    strobeRates,
    type LiveSettings,
  } from './lib/settings';
  import Segmented from './lib/Segmented.svelte';
  import Fader from './lib/Fader.svelte';
  import Preview from './lib/Preview.svelte';
  import Devices from './lib/Devices.svelte';
  import InstallApp from './lib/InstallApp.svelte';
  import BlackFrameSettings from './lib/BlackFrameSettings.svelte';
  import Diagnostics from './lib/Diagnostics.svelte';
  import Streaming from './lib/Streaming.svelte';
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
  let presetHelp = $state(
    'Pick a colour style, how fast the lights react, and how much neighbouring lights blend. Each choice is independent. Tap an option to see what it does.',
  );
  let settings = $derived(model.settings);
  const responseOptions = [
    {
      value: 'low',
      label: 'Slow',
      help: 'Lights drift slowly from colour to colour, and scene cuts become soft fades. It is easy on the eyes, though the room lags visibly behind fast action.',
    },
    {
      value: 'default',
      label: 'Default',
      help: 'Lights follow the picture closely with a short glide, and hard cuts or flashes become quick fades.',
    },
    {
      value: 'fast',
      label: 'Fast',
      help: 'Lights keep up with quick camera moves and cuts, with only slight smoothing. Busy scenes can flicker a little.',
    },
    {
      value: 'instant',
      label: 'Instant',
      help: 'Lights change as soon as the TV reports new colours, with no smoothing, no speed limit and no pause on black screens. Strobe boost is on. This is the fastest and flashiest setting. Avoid it if anyone watching is sensitive to flashing light.',
    },
    {
      value: 'dynamic',
      label: 'Dynamic',
      help: 'Smooth during slow scenes. When the screen gets busy, as in concerts, action or sport, the lights react like Instant and strobe boost turns on.',
    },
  ];
  const blendOptions = [
    {
      value: 'local',
      label: 'Local',
      help: 'Every light shows only its own part of the screen, so a red car on the left lights only the left side of the room.',
    },
    {
      value: 'balanced',
      label: 'Balanced',
      help: 'Each light takes a little colour from its neighbours. Colours still move around the room, with softer edges.',
    },
    {
      value: 'soft',
      label: 'Soft',
      help: 'Lights share much more colour with their neighbours, so the room glows more evenly from left to right.',
    },
  ];
  const depthIntro =
    'Lights placed further back in the room (in the Hue app’s entertainment area) can watch a wider part of the screen than lights near the TV.';
  const depthOptions = [
    {
      value: '0',
      label: 'Off',
      help: 'Every light watches a patch a quarter of the screen wide around its position, wherever it sits in the room.',
    },
    {
      value: '0.5',
      label: 'Mild',
      help: 'Lights behind you watch a wider patch, up to about a third of the screen, so they show a steadier blend of colours. Lights by the TV stay precise.',
    },
    {
      value: '1',
      label: 'Strong',
      help: 'Lights behind you watch a much wider patch, so the back of the room follows the overall colour of the scene.',
    },
  ];
  let depthHelp = $state(depthIntro);
  let strobeHelp = $state(
    'The fastest the lights flash while strobe boost sees a strobe on screen.',
  );
  const tabs = [
    ['tuning', 'Tuning'],
    ['devices', 'Devices'],
    ['streaming', 'Streaming'],
    ['diagnostics', 'Diagnostics'],
  ];
  const demo = import.meta.env.DEV;
  function changeResponse(value: string) {
    if (value === 'dynamic') {
      model.patch(dynamicPatch(settings));
      return;
    }
    const [s, r, f, max] = responseModes[value as keyof typeof responseModes];
    model.patch({
      smoothing_factor: s,
      rise_smoothing_factor: r,
      fall_smoothing_factor: f,
      max_color_step: max,
      dynamic_response: false,
      // Instant applies the whole low-latency bundle, including strobe boost.
      ...(value === 'instant' ? instantBundle : {}),
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
      {#if settings?.dynamic_response}<div class="energy" role="status" aria-live="off">
          <span>Energy</span><meter
            min="0"
            max="1"
            low="0.4"
            high="0.75"
            optimum="0"
            value={model.status?.energy ?? 0}
          ></meter><span class="energy-value"
            >{Math.round((model.status?.energy ?? 0) * 100)}%{model.status?.strobe_active
              ? ' · strobe'
              : ''}</span
          >
        </div>{/if}
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
              : 'Connecting to the TV…'}
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
            onhelp={(text) => (depthHelp = `${depthIntro} ${text}`)}
          />
          <p class="muted">{depthHelp}</p>
          <SettingToggle
            title="Direct RGB mode"
            description="On, Hue lights are as bright and punchy as they go, though some shades, especially skin tones and oranges, may not match the screen exactly. Off, colours match the screen more closely and are a little dimmer."
            checked={settings ? !settings.use_xy_gamut : false}
            disabled={!settings || model.busy}
            onchange={(checked) => model.patch({ use_xy_gamut: !checked })}
          />

          <SettingToggle
            title="Strobe boost"
            description="When part of the picture strobes, as with concert lights or lightning, the lights covering that part flash along between the strobe's own dim and bright colours, as fast as the strobe up to the rate below. This TV reports new screen colours only about 10 times a second, so without it fast strobes look like a slow, uneven flicker. Avoid it if anyone watching is sensitive to flashing light."
            checked={settings?.strobe_boost ?? false}
            disabled={!settings || model.busy}
            onchange={(checked) => model.patch({ strobe_boost: checked })}
          />
          <Segmented
            label="Strobe rate"
            options={strobeRates.map((hz) => ({
              value: String(hz),
              label: `${hz} Hz`,
              help: `Lights flash at most ${hz} times per second; slower strobes flash at their own speed. Lower rates are easier on the eyes.`,
            }))}
            value={String(settings?.strobe_hz ?? 10)}
            disabled={!settings || model.busy || !settings.strobe_boost}
            onchange={(value) => model.patch({ strobe_hz: Number(value) })}
            onhelp={(text) => (strobeHelp = text)}
          />
          <p class="muted">{strobeHelp}</p>

          <SettingToggle
            title="Gentle room lights"
            description="Ordinary Hue bulbs and lamps change slowly and smoothly, and small bright details don’t make them jump. Gradient strips and Nanoleaf still follow the action. Turn this off to make bulbs react as quickly as everything else."
            checked={settings?.gentle_room_lights ?? true}
            disabled={!settings || model.busy}
            onchange={(checked) => model.patch({ gentle_room_lights: checked })}
          />

          <SettingToggle
            title="Instant scene cuts"
            description="When a film cuts to a completely different shot, say from a dark room to a bright beach, the lights switch to the new colours at once. Otherwise they fade over a fraction of a second. This only matters while Scene limit is on; Instant already turns that limit off. Rapid cuts will flash the room. Avoid it if anyone watching is sensitive to flashing light."
            checked={settings?.instant_scene_cuts ?? false}
            disabled={!settings || model.busy}
            onchange={(checked) => model.patch({ instant_scene_cuts: checked })}
          />

          <SettingToggle
            title="Letterbox compensation"
            description="With films that have black bars, the lights read colours from the picture between the bars, so lights near the bars don’t go dark."
            checked={settings?.letterbox_detection ?? false}
            disabled={!settings || model.busy}
            onchange={(checked) => model.patch({ letterbox_detection: checked })}
          />

          <SettingToggle
            title="Midtone lift (legacy)"
            description="Brightens dark and medium scenes and softens very bright ones, so the room stays more evenly lit. This is an older option and does not process HDR. Gamma below 1 does much the same with finer control."
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
  <div
    id="panel-streaming"
    role="tabpanel"
    aria-labelledby="tab-streaming"
    hidden={tab !== 'streaming'}
  >
    <Streaming active={tab === 'streaming'} />
  </div>
</main>
