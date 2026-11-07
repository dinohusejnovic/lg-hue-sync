export interface LiveSettings {
  brightness_multiplier: number;
  hue_output_brightness: number;
  nanoleaf_output_brightness: number;
  saturation_boost: number;
  peak_weight: number;
  spatial_blend: number;
  depth_influence: number;
  gamma: number;
  noise_gate_threshold: number;
  smoothing_factor: number;
  rise_smoothing_factor: number;
  fall_smoothing_factor: number;
  strict_blackout: boolean;
  black_frame_hold_ms: number;
  black_frame_fade_ms: number;
  instant_scene_cuts: boolean;
  gentle_room_lights: boolean;
  strobe_boost: boolean;
  dynamic_response: boolean;
  strobe_hz: number;
  use_xy_gamut: boolean;
  letterbox_detection: boolean;
  hdr_tone_mapping: boolean;
  hue_sync_enabled: boolean;
  nanoleaf_sync_enabled: boolean;
  auto_tv_power: boolean;
  nanoleaf_alignment: {
    start_corner: string;
    reverse_direction: boolean;
    perimeter_offset: number;
  };
  max_color_step: number;
}
export const initialSettings: LiveSettings = {
  brightness_multiplier: 1,
  hue_output_brightness: 1,
  nanoleaf_output_brightness: 1,
  saturation_boost: 1,
  peak_weight: 0.15,
  spatial_blend: 0.15,
  depth_influence: 0.5,
  gamma: 1,
  noise_gate_threshold: 0.02,
  smoothing_factor: 0.35,
  rise_smoothing_factor: 0.35,
  fall_smoothing_factor: 0.35,
  strict_blackout: false,
  black_frame_hold_ms: 750,
  black_frame_fade_ms: 2000,
  instant_scene_cuts: false,
  gentle_room_lights: true,
  strobe_boost: false,
  dynamic_response: false,
  strobe_hz: 10,
  use_xy_gamut: false,
  letterbox_detection: true,
  hdr_tone_mapping: false,
  hue_sync_enabled: true,
  nanoleaf_sync_enabled: true,
  auto_tv_power: true,
  nanoleaf_alignment: {
    start_corner: 'bottom-center',
    reverse_direction: false,
    perimeter_offset: 0,
  },
  max_color_step: 12,
};
export type NumericKey =
  | 'brightness_multiplier'
  | 'saturation_boost'
  | 'peak_weight'
  | 'spatial_blend'
  | 'gamma'
  | 'smoothing_factor'
  | 'max_color_step'
  | 'noise_gate_threshold'
  | 'hue_output_brightness'
  | 'nanoleaf_output_brightness';
export const faders: {
  key: NumericKey;
  label: string;
  stops: number[];
  format: (v: number) => string;
  help: string;
}[] = [
  {
    key: 'brightness_multiplier',
    label: 'Brightness',
    stops: [0.5, 1, 1.5, 2, 2.5],
    format: (v) => `${Number(v.toFixed(2))}×`,
    help: 'How bright the lights get for a given picture. 1× matches the screen; higher makes the room glow brighter in dim scenes, lower keeps it subtle. The Hue and Nanoleaf trims on the Devices tab adjust each set of lights on top of this.',
  },
  {
    key: 'saturation_boost',
    label: 'Saturation',
    stops: [0.5, 1, 1.5, 2, 2.5],
    format: (v) => `${Number(v.toFixed(2))}×`,
    help: 'How rich the colours are. Below 1× they drift towards white for a softer wash; 1× matches the screen; above 1× pastel scenes turn into bold, deep colours.',
  },
  {
    key: 'peak_weight',
    label: 'Highlights',
    stops: [0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1],
    format: (v) => `${Math.round(v * 100)}%`,
    help: 'How much small bright details pull on a light. At 0% a light shows the average colour of its part of the screen, so a candle in a dark room barely registers. Higher values let explosions, lamps and sparks light the room even when they fill only a small area. (The frozen preview does not show this.)',
  },
  {
    key: 'spatial_blend',
    label: 'Blend',
    stops: [0, 0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.35, 0.4, 0.45, 0.5],
    format: (v) => `${Math.round(v * 100)}%`,
    help: 'How much each light borrows from the lights next to it. 0% keeps every light true to its own part of the screen, which is sharper and more dramatic. Higher values make neighbouring lights look alike, for a calmer, more unified glow. Lights over black areas stay off either way.',
  },
  {
    key: 'gamma',
    label: 'Gamma',
    stops: [0.5, 0.6, 0.7, 0.85, 1, 1.2, 1.4, 1.7, 2],
    format: (v) => v.toFixed(2),
    help: 'Contrast of the lighting. Above 1 dims medium-bright scenes while keeping bright ones bright, so the room gets moodier and only really lights up for bright moments. Below 1 lifts dim scenes so the room stays lit more of the time. 1 is neutral.',
  },
  {
    key: 'smoothing_factor',
    label: 'Response',
    stops: [0.05, 0.08, 0.12, 0.18, 0.25, 0.35, 0.5, 0.65, 0.8, 1],
    format: (v) => `${Math.round(v * 100)}%`,
    help: 'How quickly the lights catch up with the picture. Low values glide slowly between colours: relaxing, but they lag behind fast action. High values react almost immediately; 100% has no smoothing at all, which can look flickery on busy scenes.',
  },
  {
    key: 'max_color_step',
    label: 'Scene limit',
    stops: [4, 6, 8, 12, 16, 24, 32, 48, 64, 255],
    format: (v) => (v >= 255 ? 'Off' : `${v} steps`),
    help: 'A speed limit on how far a light may change colour or brightness in a split second. Low values turn hard cuts and flashes into short fades, which is easier on the eyes and safer for photosensitive viewers. Off lets the lights jump and flash with the picture.',
  },
  {
    key: 'noise_gate_threshold',
    label: 'Black gate',
    stops: [0, 0.005, 0.01, 0.015, 0.02, 0.03, 0.04, 0.06],
    format: (v) => `${Number((v * 100).toFixed(1))}%`,
    help: 'How dark a part of the screen must be before its light switches off. Higher values keep lights off during dark scenes, credits and black bars instead of glowing a dim muddy colour; set it too high and dim scenes lose their lighting. With Strict black on, those lights snap off at once instead of fading out.',
  },
];
export const colourModes = [
  {
    value: 'softColor',
    label: 'Soft',
    help: 'Calmer, paler, dimmer colours with little reaction to bright highlights. Good for background ambience.',
  },
  {
    value: 'neutral',
    label: 'Default',
    help: 'Colours and brightness that match the screen as closely as possible.',
  },
  {
    value: 'vividColor',
    label: 'Vivid',
    help: 'Deeper colours, punchier highlights and more contrast between dark and bright scenes. Good for games and animation.',
  },
];
export const responseModes = {
  low: [0.2, 0.18, 0.22, 6],
  default: [0.35, 0.35, 0.35, 12],
  fast: [0.65, 0.65, 0.7, 64],
  instant: [1, 1, 1, 255],
} as const;
/** Everything Instant sets besides the response curve. Keep in sync with INSTANT_BUNDLE in src/mqtt.rs. */
export const instantBundle = {
  gentle_room_lights: false,
  black_frame_hold_ms: 0,
  black_frame_fade_ms: 0,
  instant_scene_cuts: true,
  strobe_boost: true,
} satisfies Partial<LiveSettings>;
export const strobeRates = [6, 8, 10, 12];
export const blendModes = { local: 0, balanced: 0.15, soft: 0.3 } as const;
export function nearestStop(value: number, stops: readonly number[]) {
  return stops.reduce(
    (best, stop, i) => (Math.abs(stop - value) < Math.abs(stops[best] - value) ? i : best),
    0,
  );
}
/**
 * Dynamic response: Default as the calm base; the daemon scales towards Instant
 * with on-screen energy. Restores the black-frame hold if Instant cleared it.
 * Keep in sync with the "Dynamic" branch in src/mqtt.rs.
 */
export function dynamicPatch(settings: LiveSettings | null): Partial<LiveSettings> {
  const [s, r, f, max] = responseModes.default;
  return {
    dynamic_response: true,
    strobe_boost: true,
    smoothing_factor: s,
    rise_smoothing_factor: r,
    fall_smoothing_factor: f,
    max_color_step: max,
    ...(settings?.black_frame_hold_ms === 0 && settings.black_frame_fade_ms === 0
      ? { black_frame_hold_ms: 750, black_frame_fade_ms: 2000 }
      : {}),
  };
}
export function responseMode(settings: LiveSettings | null) {
  if (settings?.dynamic_response) return 'dynamic';
  return (
    Object.entries(responseModes).find(
      ([, v]) =>
        settings &&
        [
          settings.smoothing_factor,
          settings.rise_smoothing_factor,
          settings.fall_smoothing_factor,
          settings.max_color_step,
        ].every((n, i) => Math.abs(n - v[i]) < 0.00001),
    )?.[0] ?? ''
  );
}
export function blendMode(settings: LiveSettings | null) {
  return (
    Object.entries(blendModes).find(
      ([, v]) => settings && Math.abs(settings.spatial_blend - v) < 0.00001,
    )?.[0] ?? ''
  );
}
