import { lazy, Suspense, useState, type ComponentType } from "react";

/*
 * Big screens that only open on request (settings) live in their own files,
 * so the app starts with less to download, parse and keep in memory. Each is
 * fetched when someone shows they're about to open it (the pointer over its
 * button, focus on it, a press: `intent`), so opening one is still quick;
 * if it's opened before that, it shows as soon as its file arrives. Only a
 * few small, likely-next ones (pages you reach from the sidebar) are also
 * fetched once the app is idle after starting (`idle: true`).
 */

const preloads: (() => Promise<unknown>)[] = [];
let scheduled = false;

function schedulePreload() {
  if (scheduled) return;
  scheduled = true;
  const idle = (fn: () => void) =>
    "requestIdleCallback" in window ? window.requestIdleCallback(fn, { timeout: 4000 }) : setTimeout(fn, 1500);
  // After the first screen is up, one file per idle moment.
  setTimeout(() => {
    const next = () => {
      const load = preloads.shift();
      if (load) load().finally(() => idle(next));
    };
    idle(next);
  }, 1000);
}

/** A component loaded from its own file, which `preload` fetches ahead of time. */
export type LazyComponent<P> = ComponentType<P> & { preload: () => void };

/**
 * A component loaded from its own file. With `when`, nothing is fetched or
 * drawn until it's first wanted (a dialog's `open`); from then on it stays
 * mounted, so closing animations still play. With `idle`, its file is
 * fetched once the app is idle after starting; otherwise only on `preload`
 * (see `intent`) or when it's first drawn.
 */
export function lazyComponent<P extends object>(
  load: () => Promise<ComponentType<P>>,
  when?: (props: P) => boolean,
  { idle = false }: { idle?: boolean } = {},
): LazyComponent<P> {
  let promise: Promise<ComponentType<P>> | null = null;
  const fetch = () => (promise ??= load().catch((err: unknown) => {
    promise = null;
    throw err;
  }));
  const Loaded = lazy(() => fetch().then((component) => ({ default: component })));
  if (idle) {
    preloads.push(fetch);
    schedulePreload();
  }
  function Lazy(props: P) {
    const wanted = !when || when(props);
    const [seen, setSeen] = useState(wanted);
    if (wanted && !seen) setSeen(true);
    if (!seen) return null;
    return (
      <Suspense fallback={null}>
        <Loaded {...props} />
      </Suspense>
    );
  }
  return Object.assign(Lazy, { preload: () => void fetch().catch(() => {}) });
}

/**
 * Props for a button that opens lazy screens: their files start loading as
 * soon as the pointer comes over it, it gets focus, or it's pressed.
 */
export function intent(...targets: { preload: () => void }[]) {
  const go = () => {
    for (const t of targets) t.preload();
  };
  return { onPointerEnter: go, onFocus: go, onPointerDown: go };
}

/**
 * True from the first time `wanted` is: a dialog that's mounted only once
 * it's been opened, and then stays, so its closing animation plays.
 */
export function useSeen(wanted: boolean): boolean {
  const [seen, setSeen] = useState(wanted);
  if (wanted && !seen) setSeen(true);
  return seen || wanted;
}
