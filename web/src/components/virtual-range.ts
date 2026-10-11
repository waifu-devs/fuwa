/*
 * The arithmetic behind lists that draw only the lines in view (pure, so
 * it's tested on its own): where each line sits, and which lines reach into
 * a stretch of the list, found by binary search instead of a pass over
 * every line.
 */

/** Where each line starts, from their heights and the space between them, and how tall they are together. */
export function layoutLines(heights: readonly number[], gap = 0): { tops: number[]; total: number } {
  const tops = new Array<number>(heights.length);
  let y = 0;
  for (let i = 0; i < heights.length; i++) {
    tops[i] = y;
    y += heights[i]! + gap;
  }
  return { tops, total: heights.length ? y - gap : 0 };
}

/**
 * The lines that reach into [from, to]: the first whose bottom is at or past
 * `from`, up to (not including) the first that starts after `to`. `tops`
 * must be in order, as `layoutLines` gives them.
 */
export function linesIn(tops: readonly number[], heights: readonly number[], from: number, to: number): [start: number, end: number] {
  // First line whose bottom reaches `from`.
  let lo = 0;
  let hi = tops.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (tops[mid]! + heights[mid]! < from) lo = mid + 1;
    else hi = mid;
  }
  const start = lo;
  // First line that starts after `to`.
  hi = tops.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (tops[mid]! <= to) lo = mid + 1;
    else hi = mid;
  }
  return [start, Math.max(start, lo)];
}
