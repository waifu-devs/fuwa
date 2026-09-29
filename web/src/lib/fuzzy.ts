/**
 * Loose matching for the quick switcher: the letters typed must appear in
 * order, and matches score higher when they run together, start words, or
 * start the name. Returns where the letters matched, for highlighting.
 */
export function fuzzy(query: string, text: string): { score: number; hits: number[] } | null {
  const q = query.toLowerCase().replace(/\s+/g, "");
  const t = text.toLowerCase();
  if (!q) return { score: 0, hits: [] };
  const hits: number[] = [];
  let score = 0;
  let from = 0;
  let previous = -2;
  for (const ch of q) {
    const at = t.indexOf(ch, from);
    if (at === -1) return null;
    const wordStart = at === 0 || /[\s\-_./·]/.test(t[at - 1]!);
    score += 1 + (at === previous + 1 ? 3 : 0) + (wordStart ? 2 : 0);
    hits.push(at);
    previous = at;
    from = at + 1;
  }
  if (t.startsWith(q)) score += 6;
  else if (t.includes(q)) score += 3;
  return { score: score - t.length * 0.02, hits };
}
