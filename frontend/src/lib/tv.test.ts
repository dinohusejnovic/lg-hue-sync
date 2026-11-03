import { describe, expect, test } from 'bun:test';
import { pickNext, wheelPixels, type Box } from './tv';

const box = (left: number, top: number, width = 100, height = 40): Box => ({
  left,
  top,
  width,
  height,
});

describe('pickNext', () => {
  const from = box(200, 200);

  test('moves to the nearest box in each direction', () => {
    const boxes = [box(200, 100), box(200, 300), box(50, 200), box(350, 200)];
    expect(pickNext(from, 'up', boxes)).toBe(0);
    expect(pickNext(from, 'down', boxes)).toBe(1);
    expect(pickNext(from, 'left', boxes)).toBe(2);
    expect(pickNext(from, 'right', boxes)).toBe(3);
  });

  test('prefers the aligned box over a nearer off-axis one', () => {
    const near = box(330, 250); // down-right, close
    const aligned = box(200, 400); // straight below, further away
    expect(pickNext(from, 'down', [near, aligned])).toBe(1);
  });

  test('ignores boxes behind or level with the current one', () => {
    expect(pickNext(from, 'down', [box(200, 100), box(200, 200)])).toBe(-1);
    expect(pickNext(from, 'right', [box(0, 200)])).toBe(-1);
    expect(pickNext(from, 'up', [])).toBe(-1);
  });
});

describe('wheelPixels', () => {
  test('passes pixel deltas through and converts line deltas', () => {
    expect(wheelPixels({ deltaY: 120, deltaMode: 0 })).toBe(120);
    expect(wheelPixels({ deltaY: 3, deltaMode: 1 })).toBe(120);
    expect(wheelPixels({ deltaY: 0, deltaMode: 0 })).toBe(0);
  });
});
