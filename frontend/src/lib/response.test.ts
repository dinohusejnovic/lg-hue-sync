import { describe, expect, test } from 'bun:test';
import { dynamicPatch, initialSettings, instantBundle, responseMode } from './settings';

describe('response modes', () => {
  test('Dynamic wins over the smoothing tuple it shares with Default', () => {
    expect(responseMode(initialSettings)).toBe('default');
    expect(responseMode({ ...initialSettings, dynamic_response: true })).toBe('dynamic');
  });

  test('Dynamic restores the black-frame hold that Instant cleared', () => {
    const afterInstant = { ...initialSettings, ...instantBundle };
    const patch = dynamicPatch(afterInstant);
    expect(patch.dynamic_response).toBe(true);
    expect(patch.strobe_boost).toBe(true);
    expect(patch.black_frame_hold_ms).toBe(750);
    expect(patch.black_frame_fade_ms).toBe(2000);
    // A user-chosen hold is left alone.
    expect(dynamicPatch(initialSettings).black_frame_hold_ms).toBeUndefined();
  });
});
