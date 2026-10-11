/**
 * Timers that know whether anyone is looking. A tab in the background
 * shouldn't poll every minute or redraw clocks nobody sees, so these pause
 * (or slow down) while `document.hidden` and catch up when the tab is shown
 * again. Outside a browser (tests, workers) the page always counts as shown.
 */

type Listener = (visible: boolean) => void;

const listeners = new Set<Listener>();
let watching = false;

function doc(): Document | undefined {
  return (globalThis as { document?: Document }).document;
}

/** Whether the page is shown right now. */
export function isVisible(): boolean {
  return !doc()?.hidden;
}

function watch() {
  if (watching) return;
  const d = doc();
  if (!d?.addEventListener) return;
  watching = true;
  d.addEventListener("visibilitychange", () => {
    const visible = isVisible();
    for (const l of [...listeners]) l(visible);
  });
}

/** Calls `fn` each time the page is shown or hidden. Returns the way to stop. */
export function onVisibility(fn: Listener): () => void {
  watch();
  listeners.add(fn);
  return () => void listeners.delete(fn);
}

export type EveryOptions = {
  /**
   * When the page is shown again and a run came due while it was hidden,
   * run at once instead of waiting for the next tick. Default true.
   */
  onVisible?: boolean;
  /**
   * While hidden, run every `hiddenMs` instead of not at all. For work that
   * must still happen in the background (a lease renewal), slower.
   */
  hiddenMs?: number;
  /** Run once right away too. Default false. */
  immediate?: boolean;
};

/**
 * Runs `fn` every `ms` while the page is shown, pausing (or slowing to
 * `hiddenMs`) while it's hidden. Each run is timed from the last one, so
 * showing the page again doesn't double up. Returns the way to stop.
 */
export function everyWhileVisible(ms: number, fn: () => void, options: EveryOptions = {}): () => void {
  const { onVisible = true, hiddenMs, immediate = false } = options;
  let last = Date.now();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let stopped = false;

  const period = () => (isVisible() ? ms : hiddenMs);
  const run = () => {
    if (stopped) return;
    last = Date.now();
    try {
      fn();
    } finally {
      arm();
    }
  };
  const arm = () => {
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
    if (stopped) return;
    const p = period();
    if (p === undefined) return; // hidden, paused
    timer = setTimeout(run, Math.max(0, last + p - Date.now()));
  };

  const off = onVisibility((visible) => {
    if (stopped) return;
    if (visible && onVisible && Date.now() - last >= ms) run();
    else arm();
  });
  if (immediate) run();
  else arm();

  return () => {
    stopped = true;
    off();
    if (timer !== undefined) clearTimeout(timer);
  };
}

/**
 * Like `setTimeout`, but a delay that ends while the page is hidden waits for
 * it to be shown. Returns the way to cancel.
 */
export function afterWhileVisible(ms: number, fn: () => void): () => void {
  let done = false;
  let off = () => {};
  const timer = setTimeout(() => {
    if (isVisible()) return finish();
    off = onVisibility((visible) => visible && finish());
  }, ms);
  function finish() {
    if (done) return;
    done = true;
    off();
    fn();
  }
  return () => {
    done = true;
    clearTimeout(timer);
    off();
  };
}

/* One shared clock per period ------------------------------------------- */

type Clock = { now: number; subscribers: Set<() => void>; stop: () => void };
const clocks = new Map<number, Clock>();

/**
 * A clock that ticks every `ms`, shared by everyone asking for that period:
 * one timer however many subscribe, stopped when the last one leaves,
 * paused while the page is hidden (and ticking at once when shown again).
 * Returns the way to unsubscribe.
 */
export function subscribeClock(ms: number, fn: () => void): () => void {
  let clock = clocks.get(ms);
  if (!clock) {
    // Starts from the time callers were already shown, so subscribing doesn't redraw them.
    const c: Clock = { now: clockNow(ms), subscribers: new Set(), stop: () => {} };
    c.stop = everyWhileVisible(ms, () => {
      c.now = Date.now();
      idle.set(ms, c.now);
      for (const s of [...c.subscribers]) s();
    });
    clocks.set(ms, c);
    clock = c;
  }
  clock.subscribers.add(fn);
  const mine = clock;
  return () => {
    mine.subscribers.delete(fn);
    if (!mine.subscribers.size && clocks.get(ms) === mine) {
      mine.stop();
      clocks.delete(ms);
    }
  };
}

/** What `clockNow` says for periods nobody is subscribed to: the same value until a period has passed. */
const idle = new Map<number, number>();

/**
 * The time on the shared clock for `ms`: when it last ticked. With nobody
 * subscribed, it's the time it was first asked, for up to one period, so
 * asking twice in a row gives the same answer (as React's external stores
 * need).
 */
export function clockNow(ms: number): number {
  const clock = clocks.get(ms);
  if (clock) return clock.now;
  const now = Date.now();
  const at = idle.get(ms);
  if (at !== undefined && now - at < ms) return at;
  idle.set(ms, now);
  return now;
}

/** How many shared clocks are running (for tests). */
export function runningClocks(): number {
  return clocks.size;
}
