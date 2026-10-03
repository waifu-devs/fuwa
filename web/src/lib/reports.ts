import { Code, ConnectError, type Interceptor } from "@connectrpc/connect";
import { useSyncExternalStore } from "react";
import { getPrefs, subscribePrefs } from "@/lib/prefs";

/**
 * Anonymous reports: what went wrong and what was slow in this app, as
 * counts. Errors are counted by kind and where in the app they happened,
 * durations go into fixed buckets, and a few features count how often
 * they're used. Never message text, names, file names, ids or addresses.
 *
 * Reports go only to an instance you're signed in to, never anywhere else:
 * the instance adds them to its own hourly report to Waifu Devs, and takes
 * none while its telemetry is off. "Help fix bugs" in Advanced turns this
 * off, and while it's off nothing is counted at all.
 */

declare const __FUWA_VERSION__: string;

/** Upper bounds of the timing buckets in milliseconds, the server's (`ReportTiming`); one more bucket holds anything longer. */
export const BOUNDS_MS = [5, 10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000, 30000] as const;
const BUCKETS = BOUNDS_MS.length + 1;
/** Distinct entries of each kind a report holds; the server takes no more. */
const MAX_ENTRIES = 64;
/** How often a report goes out, when there is one. The server takes one a minute per account. */
const EVERY_MS = 10 * 60 * 1000;
const FIRST_MS = 60 * 1000;
/** Frames that take longer than this count as long. */
const LONG_FRAME_MS = 50;

type Timing = { buckets: number[]; sumMs: number };
export type Pending = {
  errors: Map<string, number>;
  timings: Map<string, Timing>;
  usage: Map<string, number>;
};

const empty = (): Pending => ({ errors: new Map(), timings: new Map(), usage: new Map() });
let pending = empty();
let version = 0;
const listeners = new Set<() => void>();
const changed = () => {
  version++;
  for (const l of listeners) l();
};

const on = () => getPrefs().shareReports;

/** Letters, digits and `_.:/#@-`, as the server allows, and short. */
export function clean(value: string, max: number): string {
  return value.replace(/[^A-Za-z0-9_.:/#@-]/g, "_").slice(0, max) || "unknown";
}

function bump(map: Map<string, number>, key: string, by = 1) {
  const now = map.get(key);
  if (now !== undefined) map.set(key, now + by);
  else if (map.size < MAX_ENTRIES) map.set(key, by);
  else return;
  changed();
}

/** Counts a failure of `kind` (such as "TypeError") at `place` (such as "settings/roles"). */
export function reportError(kind: string, place: string) {
  if (!on()) return;
  bump(pending.errors, `${clean(kind, 48)}\n${clean(place, 120)}`);
}

/** Counts how long something took, in milliseconds. */
export function reportTiming(metric: string, ms: number) {
  if (!on() || !Number.isFinite(ms) || ms < 0) return;
  const key = clean(metric, 96);
  let timing = pending.timings.get(key);
  if (!timing) {
    if (pending.timings.size >= MAX_ENTRIES) return;
    timing = { buckets: Array<number>(BUCKETS).fill(0), sumMs: 0 };
    pending.timings.set(key, timing);
  }
  const at = BOUNDS_MS.findIndex((bound) => ms <= bound);
  timing.buckets[at === -1 ? BOUNDS_MS.length : at]!++;
  timing.sumMs += Math.round(ms);
  changed();
}

/** Counts one use of a feature, such as "message.send". */
export function reportUsage(feature: string) {
  if (!on()) return;
  bump(pending.usage, clean(feature, 96));
}

/** How many entries are waiting, for the settings page. */
export function usePendingReport(): { errors: number; timings: number; usage: number } {
  useSyncExternalStore(
    (l) => (listeners.add(l), () => listeners.delete(l)),
    () => version,
  );
  return { errors: pending.errors.size, timings: pending.timings.size, usage: pending.usage.size };
}

// ---------------------------------------------------------------------------
// Where in the app: the route's pattern (never its values), and for errors
// thrown by the app's own code, the file and line in this build.

let route: () => string = () => "app";
/** The router tells reports which page is showing, as its pattern ("/$instance/$server"). */
export function setRouteSource(source: () => string | undefined) {
  route = () => clean((source() ?? "app").replace(/\$/g, ":").replace(/^\/+/, "") || "home", 60);
}

/** The first frame in the stack from this app's own files, as "assets/index-abc.js:1:2345". */
function ownFrame(stack: string | undefined): string | null {
  if (!stack) return null;
  const origin = location.origin;
  for (const line of stack.split("\n")) {
    const at = line.indexOf(origin);
    if (at === -1) continue;
    const match = /^(\/[^\s?#)]*?\.(?:js|mjs|ts|tsx)):(\d+):(\d+)/.exec(line.slice(at + origin.length));
    if (match) return `${match[1]!.replace(/^\/+/, "")}:${match[2]}:${match[3]}`;
  }
  return null;
}

/** The kind of a thrown value: its class name when it has a plain one. */
function kindOf(error: unknown): string {
  if (error instanceof Error) return /^[A-Za-z][A-Za-z0-9_]{0,47}$/.test(error.name) ? error.name : "Error";
  return error === null ? "null" : typeof error;
}

/** Counts something thrown and not caught, if it came from the app's own code. */
export function reportThrown(error: unknown, where = "uncaught") {
  if (!on()) return;
  const frame = ownFrame(error instanceof Error ? error.stack : undefined);
  // Browser extensions and other pages' scripts fail in here too; they aren't fuwa's bugs.
  if (!frame && error instanceof Error) return;
  reportError(kindOf(error), `${where}:${route()}${frame ? `@${frame}` : ""}`);
}

/** Times every call to the instance and counts the ones that failed on the server's side. */
export const timeCalls: Interceptor = (next) => async (req) => {
  if (!on()) return next(req);
  const method = `${req.service.typeName.replace(/^fuwa\.v1\./, "")}/${req.method.name}`;
  const started = performance.now();
  try {
    const res = await next(req);
    reportTiming(`rpc:${method}`, performance.now() - started);
    return res;
  } catch (err) {
    const code = err instanceof ConnectError ? err.code : null;
    if (code === Code.Internal || code === Code.Unknown || code === Code.DataLoss) reportError("rpc_internal", method);
    throw err;
  }
};

// ---------------------------------------------------------------------------

/** Which browser family and OS family this is: nothing finer. */
function platform(): { platform: string; os: string } {
  const ua = navigator.userAgent;
  const os = /Windows/.test(ua)
    ? "windows"
    : /Android/.test(ua)
      ? "android"
      : /iPhone|iPad|iPod/.test(ua)
        ? "ios"
        : /CrOS/.test(ua)
          ? "chromeos"
          : /Mac OS X|Macintosh/.test(ua)
            ? "macos"
            : /Linux/.test(ua)
              ? "linux"
              : "other";
  const browser = /Firefox\//.test(ua) ? "firefox" : /Chrome\/|Chromium\//.test(ua) ? "chromium" : /Safari\//.test(ua) ? "safari" : "other";
  return { platform: browser, os };
}

/** Somewhere to send a report: an instance you're signed in to that takes them. */
export type ReportTarget = {
  sendReport: (report: {
    report: {
      app: string;
      version: string;
      platform: string;
      os: string;
      errors: { kind: string; place: string; count: number }[];
      timings: { metric: string; buckets: number[]; sumMs: bigint }[];
      usage: { feature: string; count: number }[];
    };
  }) => Promise<unknown>;
};

/** Puts a report that didn't go out back with whatever was counted since. */
function putBack(taken: Pending) {
  for (const [key, n] of taken.errors) bump(pending.errors, key, n);
  for (const [key, n] of taken.usage) bump(pending.usage, key, n);
  for (const [key, t] of taken.timings) {
    const now = pending.timings.get(key);
    if (now) {
      now.buckets = now.buckets.map((n, i) => n + (t.buckets[i] ?? 0));
      now.sumMs += t.sumMs;
    } else if (pending.timings.size < MAX_ENTRIES) pending.timings.set(key, t);
  }
  changed();
}

/** Sends what's waiting to the first instance `target` finds, if any. */
export async function sendReport(target: () => ReportTarget | null) {
  if (!on()) {
    pending = empty();
    changed();
    return;
  }
  const to = target();
  const taken = pending;
  if (!to || (taken.errors.size === 0 && taken.timings.size === 0 && taken.usage.size === 0)) return;
  pending = empty();
  changed();
  const report = {
    app: "web",
    version: typeof __FUWA_VERSION__ === "string" ? __FUWA_VERSION__ : "dev",
    ...platform(),
    errors: [...taken.errors].map(([key, count]) => {
      const [kind, place] = key.split("\n");
      return { kind: kind!, place: place!, count };
    }),
    timings: [...taken.timings].map(([metric, t]) => ({ metric, buckets: t.buckets, sumMs: BigInt(t.sumMs) })),
    usage: [...taken.usage].map(([feature, count]) => ({ feature, count })),
  };
  try {
    await to.sendReport({ report });
  } catch {
    putBack(taken);
  }
}

let started = false;

/**
 * Starts counting uncaught errors and long frames, and sending every few
 * minutes to whichever instance `target` picks. Call once at startup.
 */
export function startReports(target: () => ReportTarget | null) {
  if (started) return;
  started = true;
  window.addEventListener("error", (e) => reportThrown(e.error ?? new Error(e.message)));
  window.addEventListener("unhandledrejection", (e) => reportThrown(e.reason, "rejection"));

  // Long frames, where the browser can say (Chromium): free, since the browser measures them anyway.
  try {
    const observer = new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) if (entry.duration >= LONG_FRAME_MS) reportTiming("frame.long", entry.duration);
    });
    observer.observe({ type: "long-animation-frame", buffered: false });
  } catch {
    // Not in this browser.
  }

  // Turning it off throws away what was waiting.
  subscribePrefs(() => {
    if (!on() && (pending.errors.size || pending.timings.size || pending.usage.size)) {
      pending = empty();
      changed();
    }
  });

  setTimeout(() => {
    void sendReport(target);
    setInterval(() => void sendReport(target), EVERY_MS);
  }, FIRST_MS);
}

/** Startup: from opening the page until the first instance is live. Counted once. */
let startupCounted = false;
export function reportStartup() {
  if (startupCounted) return;
  startupCounted = true;
  reportTiming("startup", performance.now());
}
