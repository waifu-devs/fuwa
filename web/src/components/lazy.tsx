import { lazy, Suspense, useState, type ComponentType } from "react";

/*
 * Big screens that only open on request (settings) live in their own files,
 * so the app starts with less to download and run. Each is fetched once the
 * app is idle after starting, so opening one is still instant; if it's
 * opened before that, it shows as soon as its file arrives.
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

/**
 * A component loaded from its own file. With `when`, nothing is fetched or
 * drawn until it's first wanted (a dialog's `open`); from then on it stays
 * mounted, so closing animations still play.
 */
export function lazyComponent<P extends object>(
  load: () => Promise<ComponentType<P>>,
  when?: (props: P) => boolean,
): ComponentType<P> {
  let promise: Promise<ComponentType<P>> | null = null;
  const fetch = () => (promise ??= load().catch((err: unknown) => {
    promise = null;
    throw err;
  }));
  const Loaded = lazy(() => fetch().then((component) => ({ default: component })));
  preloads.push(fetch);
  schedulePreload();
  return function Lazy(props: P) {
    const wanted = !when || when(props);
    const [seen, setSeen] = useState(wanted);
    if (wanted && !seen) setSeen(true);
    if (!seen) return null;
    return (
      <Suspense fallback={null}>
        <Loaded {...props} />
      </Suspense>
    );
  };
}
