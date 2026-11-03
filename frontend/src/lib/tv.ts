// TV mode: drive the dashboard with the Magic Remote D-pad.
//
// The Magic Remote pointer already works as a mouse, but the D-pad only produces
// arrow-key events and webOS web apps get no default focus movement from them.
// This module adds geometric focus navigation and the Back key, so the same
// dashboard can be operated from the couch. Faders keep their native behaviour:
// Left/Right changes the value, Up/Down moves to the next control.

const WEBOS_BACK = 461;

/** `?tv=1` / `?tv=0` force the mode; otherwise webOS browsers opt in automatically. */
export function detectTv(): boolean {
  try {
    const forced = new URLSearchParams(location.search).get('tv');
    if (forced === '1') return true;
    if (forced === '0') return false;
  } catch {
    /* Fall through to user-agent detection. */
  }
  return /Web0S|webOS|NetCast/i.test(navigator.userAgent);
}

export type Direction = 'left' | 'right' | 'up' | 'down';

const BY_KEY: Record<string, Direction | undefined> = {
  ArrowLeft: 'left',
  ArrowRight: 'right',
  ArrowUp: 'up',
  ArrowDown: 'down',
};
// Older webOS browsers only report numeric key codes for the D-pad.
const BY_CODE: Record<number, Direction | undefined> = {
  37: 'left',
  38: 'up',
  39: 'right',
  40: 'down',
};

const FOCUSABLE = [
  'button:not(:disabled)',
  'a[href]',
  'input:not(:disabled):not([type="hidden"])',
  'select:not(:disabled)',
  'summary',
  '[role="switch"]:not(:disabled)',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

function visible(element: HTMLElement) {
  if (element.closest('[hidden], [inert], [aria-hidden="true"]')) return false;
  const rect = element.getBoundingClientRect();
  if (rect.width === 0 || rect.height === 0) return false;
  // Content of a closed <details> is not rendered, but guard against odd layouts anyway.
  const details = element.closest('details');
  return !details || details.open || element.tagName === 'SUMMARY';
}

export interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

function centre(box: Box) {
  return { x: box.left + box.width / 2, y: box.top + box.height / 2 };
}

/**
 * Index of the nearest box in `direction` from `from`, or -1. Boxes on the same
 * row/column win over closer ones that are off-axis, which matches how a grid of
 * controls is read from the couch.
 */
export function pickNext(from: Box, direction: Direction, boxes: Box[]): number {
  const origin = centre(from);
  let best = -1;
  let bestScore = Infinity;
  boxes.forEach((box, index) => {
    const point = centre(box);
    const dx = point.x - origin.x;
    const dy = point.y - origin.y;
    const primary =
      direction === 'right' ? dx : direction === 'left' ? -dx : direction === 'down' ? dy : -dy;
    if (primary <= 1) return;
    const cross = direction === 'left' || direction === 'right' ? Math.abs(dy) : Math.abs(dx);
    const score = primary + cross * 2.5;
    if (score < bestScore) {
      bestScore = score;
      best = index;
    }
  });
  return best;
}

function nextFocus(
  from: HTMLElement,
  direction: Direction,
  candidates: HTMLElement[],
): HTMLElement | null {
  const usable = candidates.filter((candidate) => candidate !== from && visible(candidate));
  const index = pickNext(
    from.getBoundingClientRect(),
    direction,
    usable.map((candidate) => candidate.getBoundingClientRect()),
  );
  return index < 0 ? null : usable[index];
}

function isRange(element: Element | null): element is HTMLInputElement {
  return element instanceof HTMLInputElement && element.type === 'range';
}

/** Text-like fields keep Left/Right for the caret. */
function isTextField(element: Element | null) {
  return (
    (element instanceof HTMLInputElement &&
      !['range', 'checkbox', 'radio', 'button', 'submit'].includes(element.type)) ||
    element instanceof HTMLTextAreaElement
  );
}

export function installTvNavigation(options: { onBack: () => void }): () => void {
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey) return;

    if (event.keyCode === WEBOS_BACK || event.key === 'GoBack' || event.key === 'Escape') {
      event.preventDefault();
      options.onBack();
      return;
    }

    const direction = BY_KEY[event.key] ?? BY_CODE[event.keyCode];
    if (!direction) return;

    const active = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const horizontal = direction === 'left' || direction === 'right';
    // Let the control consume the key: slider value changes and caret movement.
    if (horizontal && (isRange(active) || isTextField(active))) return;

    const candidates = Array.from(document.querySelectorAll<HTMLElement>(FOCUSABLE));
    let target: HTMLElement | null = null;
    if (active && active !== document.body && visible(active)) {
      target = nextFocus(active, direction, candidates);
    } else {
      target = candidates.find(visible) ?? null;
    }
    // Stay put at an edge rather than letting the page scroll away from the focus.
    event.preventDefault();
    if (!target) return;
    target.focus({ preventScroll: true });
    target.scrollIntoView({ block: 'center', inline: 'nearest' });
  };

  // Magic Remote wheel: webOS only scrolls web apps natively with
  // `useNativeScroll`. If a wheel event leaves the page where it was, scroll it here.
  const onWheel = (event: WheelEvent) => {
    if (event.defaultPrevented || event.ctrlKey) return;
    const before = window.scrollY;
    const delta = wheelPixels(event);
    if (delta === 0) return;
    requestAnimationFrame(() => {
      if (window.scrollY === before) window.scrollBy({ top: delta, behavior: 'auto' });
    });
  };

  document.addEventListener('keydown', onKeyDown);
  window.addEventListener('wheel', onWheel, { passive: true });
  return () => {
    document.removeEventListener('keydown', onKeyDown);
    window.removeEventListener('wheel', onWheel);
  };
}

/** Wheel delta in pixels; line/page modes (some remotes) are converted. */
export function wheelPixels(event: Pick<WheelEvent, 'deltaY' | 'deltaMode'>): number {
  if (event.deltaMode === 1) return event.deltaY * 40;
  if (event.deltaMode === 2) return event.deltaY * window.innerHeight * 0.9;
  return event.deltaY;
}
