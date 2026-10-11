import { useEffect, useMemo, useState } from "react";

/** setTimeout's longest wait (about 24.8 days); a later time is waited for in steps. */
const LONGEST = 2 ** 31 - 1;

/**
 * A `now` that moves only when one of `times` (unix milliseconds) passes,
 * instead of on a ticking clock: a list of time-outs redraws when one ends,
 * not every second. Times already past are ignored.
 */
export function useNowAtExpiry(times: Iterable<number>): number {
  const [now, setNow] = useState(Date.now);
  const next = useMemo(() => {
    let soonest = Infinity;
    for (const t of times) if (t > now && t < soonest) soonest = t;
    return soonest;
  }, [times, now]);
  useEffect(() => {
    if (next === Infinity) return;
    const id = setTimeout(() => setNow(Date.now()), Math.min(LONGEST, Math.max(0, next - Date.now() + 20)));
    return () => clearTimeout(id);
  }, [next, now]);
  return now;
}
